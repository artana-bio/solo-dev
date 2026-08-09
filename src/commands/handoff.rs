//! Handoff commands: create, inspect, and revoke.

use std::{collections::BTreeSet, fmt::Write as _, fs, path::PathBuf};

use clap::{Args, Subcommand};

use crate::{
    cli::output::CommandOutcome,
    commands::CONTROL_ENV,
    commands::{
        card::{load_card, require_convergence_budget, store_card_state},
        gate::{load_gate, receipts_for, require_before_handoff},
        lesson::{all_lessons, validate_manifest_registry},
        review::dependency_standings,
        transaction::with_transaction,
        work::held_lease,
    },
    config::ConvergencePolicy,
    control::{
        event_store::{EventDraft, EventStore},
        repository::ControlRepository,
    },
    domain::{
        card::CardState,
        clock::Clock,
        cycle::CycleRecord,
        digest::{CANONICAL_ALGORITHM, Digest},
        handoff::{
            ActorDeclaration, DeclaredGateFailure, DependencyBinding, EvidenceEntry, HANDOFF_DIR,
            HANDOFF_SCHEMA, HandoffRecord, HandoffStatus, check_delivered_sha,
        },
        ids::CardId,
        lesson::LessonManifest,
    },
    error::{ErrorCode, HarnessError},
    git::{command::GitScope, diff::DiffSummary, inspect},
    policy::convergence::{ATTEMPT_RECORDED_EVENT, AttemptKind, ReasonCategory},
    policy::lessons::build_manifest,
    policy::verification::{CandidateFacts, VerificationReport, verify},
    runner::receipt::evidence_is_acceptable,
};

/// Subcommands under `handoff`.
#[derive(Debug, Subcommand)]
pub enum HandoffCommand {
    /// Bind the current candidate to a reviewable record.
    Create(CreateArgs),
    /// Show a card's handoff and whether it still applies.
    Inspect(CardArgs),
    /// Withdraw a handoff, returning the card to work.
    Revoke(RevokeArgs),
    /// Print a complete, valid handoff declaration example.
    ///
    /// Built by constructing a real `ActorDeclaration` and serializing it,
    /// so this can never disagree with what `handoff create` accepts — see
    /// #108. #180 added this alongside `card example`, closing the second
    /// of the two document kinds #142 §11 had found with no generated
    /// example at all.
    Example(ExampleArgs),
}

impl HandoffCommand {
    /// Its dotted command path, as the result envelope reports it.
    ///
    /// The error envelope used to carry only the group — `handoff` — while a
    /// success carried the full path, so a consumer matching on `command` got a
    /// different granularity depending on whether the command worked.
    #[must_use]
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Create(..) => "handoff.create",
            Self::Inspect(..) => "handoff.inspect",
            Self::Revoke(..) => "handoff.revoke",
            Self::Example(..) => "handoff.example",
        }
    }
}

/// Arguments shared by handoff subcommands.
#[derive(Debug, Args)]
pub struct CommonArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
    pub control: PathBuf,
    /// Identifies the acting party. Declared, not proven; see D-013.
    #[arg(long, default_value = "operator")]
    pub actor: String,
}

/// Arguments accepted by `handoff create`.
#[derive(Debug, Args)]
pub struct CreateArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The card to hand off.
    #[arg(long)]
    pub card_id: String,
    /// Path to the actor's declaration, in YAML or JSON.
    #[arg(long)]
    pub declaration: PathBuf,
    /// Exact lesson-manifest digest emitted by `work packet`; required once
    /// the project has governed lesson history.
    #[arg(long)]
    pub lesson_manifest_digest: Option<String>,
    /// Report planned mutations without performing them.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments naming only a card.
#[derive(Debug, Args)]
pub struct CardArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The card to act on.
    #[arg(long)]
    pub card_id: String,
}

/// Arguments accepted by `handoff revoke`.
#[derive(Debug, Args)]
pub struct RevokeArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The card whose handoff is withdrawn.
    #[arg(long)]
    pub card_id: String,
    /// Why it is being withdrawn.
    #[arg(long)]
    pub reason: String,
    /// Report planned mutations without performing them.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `handoff example`.
///
/// Deliberately empty, and deliberately not [`CommonArgs`]: #108 constraint 1
/// requires this to be reachable before an operator has a control repository
/// or a card to point it at, so it names neither — the same reasoning
/// `review::ExampleArgs`, `project::ExampleArgs`, and `card::ExampleArgs`
/// already give.
#[derive(Debug, Args)]
pub struct ExampleArgs {}

/// Executes a `handoff` subcommand.
///
/// # Errors
///
/// Returns a policy, precondition, or gate error as appropriate.
pub fn execute(
    command: &HandoffCommand,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    match command {
        HandoffCommand::Create(args) => run_create(args, clock),
        HandoffCommand::Inspect(args) => run_inspect(args),
        HandoffCommand::Revoke(args) => run_revoke(args, clock),
        HandoffCommand::Example(..) => run_example(),
    }
}

/// Allocates the next handoff identifier.
///
/// Monotonic rather than derived from the candidate SHA. A derived identifier
/// would be deterministic but unordered, and "the latest handoff" is a question
/// this code asks constantly: sorting SHA-prefixed names returns whichever
/// candidate happened to hash lower, not the most recent one.
fn next_handoff_id(control: &ControlRepository) -> Result<String, HarnessError> {
    let directory = control.path(HANDOFF_DIR);
    let highest = if directory.exists() {
        fs::read_dir(&directory)
            .map_err(|source| HarnessError::ControlIo {
                path: directory.clone(),
                source,
            })?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                entry
                    .path()
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .and_then(|stem| stem.strip_prefix("H-"))
                    .and_then(|digits| digits.parse::<u64>().ok())
            })
            .max()
            .unwrap_or(0)
    } else {
        0
    };
    Ok(format!("H-{:06}", highest + 1))
}

/// The most recently written handoff for a card, if any.
///
/// # Errors
///
/// Returns an error when the store cannot be read.
pub fn latest_handoff(
    control: &ControlRepository,
    card_id: &CardId,
) -> Result<Option<HandoffRecord>, HarnessError> {
    let directory = control.path(HANDOFF_DIR);
    if !directory.exists() {
        return Ok(None);
    }
    let mut names: Vec<String> = fs::read_dir(&directory)
        .map_err(|source| HarnessError::ControlIo {
            path: directory,
            source,
        })?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension()? == "json")
                .then(|| path.file_stem()?.to_str().map(ToOwned::to_owned))?
        })
        .collect();
    // Identifiers are zero-padded and monotonic, so lexical order is issue
    // order.
    names.sort();

    for name in names.iter().rev() {
        let raw = control.read(&HandoffRecord::relative_path(name))?;
        let record: HandoffRecord =
            serde_json::from_str(&raw).map_err(|source| HarnessError::Control {
                reason: format!("handoff {name} is malformed: {source}"),
                code: ErrorCode::InternalControlCorrupt,
            })?;
        if record.card_id == *card_id {
            return Ok(Some(record));
        }
    }
    Ok(None)
}

/// Every handoff written for a card, oldest first.
///
/// # Errors
///
/// Returns an error when the store cannot be read.
pub fn handoffs_for(
    control: &ControlRepository,
    card_id: &CardId,
) -> Result<Vec<HandoffRecord>, HarnessError> {
    let directory = control.path(HANDOFF_DIR);
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut names: Vec<String> = fs::read_dir(&directory)
        .map_err(|source| HarnessError::ControlIo {
            path: directory,
            source,
        })?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension()? == "json")
                .then(|| path.file_stem()?.to_str().map(ToOwned::to_owned))?
        })
        .collect();
    names.sort();

    let mut records = Vec::new();
    for name in &names {
        let raw = control.read(&HandoffRecord::relative_path(name))?;
        let record: HandoffRecord =
            serde_json::from_str(&raw).map_err(|source| HarnessError::Control {
                reason: format!("handoff {name} is malformed: {source}"),
                code: ErrorCode::InternalControlCorrupt,
            })?;
        if record.card_id == *card_id {
            records.push(record);
        }
    }
    Ok(records)
}

/// Whether `ancestor` is in `descendant`'s history, or `None` if unanswerable.
///
/// An object Git no longer has is not an error here, and it is not an answer
/// either. A card's candidate can become unreachable once its branch is deleted
/// and its objects collected, and the two callers of this want opposite
/// defaults in that case — binding a dependency treats it as not incorporated,
/// checking one treats it as not superseded — so the ambiguity is returned
/// rather than resolved here.
///
/// # Errors
///
/// Returns an error when `is_ancestor` itself fails. Note that a missing object
/// cannot reach that path: `object_type` reports a Git execution failure the
/// same way it reports an absent object, so both collapse into `None` above.
/// That is the conservative direction for both callers — neither invalidates on
/// an unanswerable question — but it means a genuinely broken Git is reported as
/// "cannot say" rather than as a failure. A reviewer identified this; it is
/// recorded rather than papered over, because tightening it would mean
/// distinguishing the two inside `object_type`, which is a wider change than
/// this card owns.
pub(crate) fn ancestry(
    scope: &GitScope,
    ancestor: &str,
    descendant: &str,
) -> Result<Option<bool>, HarnessError> {
    if inspect::object_type(scope, ancestor).is_err()
        || inspect::object_type(scope, descendant).is_err()
    {
        return Ok(None);
    }
    inspect::is_ancestor(scope, ancestor, descendant).map(Some)
}

/// Binds each declared dependency to the newest handed-off commit its candidate
/// history contains.
///
/// Section 10.7's `dependency SHAs`. The question is deliberately about an
/// ancestor in this candidate, not the dependency's current approval. A
/// dependent branched from the cycle baseline incorporates no handed-off
/// dependency commit and binds `None`, and stays valid however often its
/// dependency is re-reviewed; a dependent branched from — or merged with — a
/// handed-off dependency candidate binds that commit, and goes stale when the
/// dependency is re-approved somewhere else, because the candidate then carries
/// a superseded version of code that is about to land twice.
///
/// Handoffs are searched newest first, so the binding is the most recent
/// *handed-off* dependency commit the candidate has. That is not necessarily
/// the dependency commit the candidate actually contains: unhanded work on top
/// of an older handoff binds the older commit, and an approval that still
/// contains it passes containment with no sign of the gap. `base_sha` does not
/// close it: the Section 10.2 precondition that it names an accepted dependency
/// SHA is not enforced beyond validating 40 hexadecimal characters.
fn resolve_dependency_bindings(
    control: &ControlRepository,
    scope: &GitScope,
    depends_on: &[CardId],
    candidate_sha: &str,
) -> Result<Vec<DependencyBinding>, HarnessError> {
    let mut ordered: Vec<CardId> = depends_on.to_vec();
    ordered.sort();
    ordered.dedup();

    let mut bindings = Vec::with_capacity(ordered.len());
    for card_id in ordered {
        let mut incorporated_sha = None;
        for handoff in handoffs_for(control, &card_id)?.iter().rev() {
            if ancestry(scope, &handoff.candidate_sha, candidate_sha)?.unwrap_or(false) {
                incorporated_sha = Some(handoff.candidate_sha.clone());
                break;
            }
        }
        bindings.push(DependencyBinding {
            card_id,
            incorporated_sha,
        });
    }
    Ok(bindings)
}

/// #142: distinct call site from the parse below, so the read failure needs
/// no introspection to tell apart from a schema failure.
const HANDOFF_DECLARATION_READ_RECOVERY: &str = "This is a read failure, not a syntax problem: the declaration file above could not be opened. Confirm the path exists, is spelled correctly, and is readable by this process.";

/// #180 added `handoff example` (below), which prints a complete, valid
/// handoff declaration by constructing a real `ActorDeclaration` and
/// serializing it — so this recovery now names that command instead of
/// #142's original "there is no generated example" wording, which #180 §2
/// made false the moment the command existed:
/// `tests/config_malformed_example_claims.rs` fails on this exact constant
/// the moment `handoff example` is real, and would keep failing on the old
/// wording forever after (`src/error.rs`'s own
/// `the_card_recovery_names_no_specification_section` test pins a related
/// mistake shut for a different code, after it shipped once as "Section
/// 10.3" — naming a command, not a document section, is the same discipline
/// applied here). `serde_yaml_ng` still offers no syntax/schema split — see
/// `GATE_DEFINITION_PARSE_RECOVERY` in `src/commands/gate.rs` — so this
/// remains one message, honest for both failure shapes.
const HANDOFF_DECLARATION_PARSE_RECOVERY: &str = "This handoff declaration could not be parsed as YAML, or it parsed but does not match the schema; the message above names the position or the field. Compare it against `handoff example`'s output, a complete, valid handoff declaration.";

/// Reads and parses an actor declaration.
fn read_declaration(path: &PathBuf) -> Result<ActorDeclaration, HarnessError> {
    let raw = fs::read_to_string(path).map_err(|source| HarnessError::ControlWithRecovery {
        reason: format!("cannot read declaration {}: {source}", path.display()),
        code: ErrorCode::ConfigMalformed,
        recovery: HANDOFF_DECLARATION_READ_RECOVERY,
    })?;
    serde_yaml_ng::from_str(&raw).map_err(|source| HarnessError::ControlWithRecovery {
        reason: format!("handoff declaration is malformed: {source}"),
        code: ErrorCode::ConfigMalformed,
        recovery: HANDOFF_DECLARATION_PARSE_RECOVERY,
    })
}

/// `cafebabe`, another classic hexspeak placeholder — deliberately not
/// `deadbeef`, which `card example`'s `base_sha` already uses, so the two
/// generated documents never show the same 40 characters and invite reading
/// them as the same commit — repeated to fill the 40 hex characters
/// `delivered_sha` requires (`ActorDeclaration::validate`). See
/// `EXAMPLE_BASE_SHA` in `src/commands/card.rs` for the full argument that a
/// constructed hexspeak string cannot be mistaken for a real object id.
/// [`run_example`]'s warning below says outright that it must be replaced
/// with the exact commit `git rev-parse HEAD` reports in the allocated
/// worktree before `handoff create` is run for anything but this example —
/// unlike `base_sha`, which `card create` never checks against reality (see
/// `EXAMPLE_BASE_SHA`'s own doc comment), `delivered_sha` is checked the
/// moment `handoff create` resolves a candidate (`check_delivered_sha`,
/// `candidate_of`), so no fixed placeholder could ever be accepted as-is by
/// the real command; a generator cannot know a caller's future commit
/// ahead of time.
const EXAMPLE_DELIVERED_SHA: &str = "cafebabecafebabecafebabecafebabecafebabe";

/// A complete, valid actor declaration, for `handoff example` to emit.
///
/// #180: unlike `CardDraft`, `ActorDeclaration` (`src/domain/handoff.rs`)
/// has exactly one field carrying `#[serde(default)]` —
/// `gate_failures`, added by 71-R3 after every other field already existed,
/// with `skip_serializing_if = "Vec::is_empty"` besides. Every other field
/// here (`assumptions`, `known_limitations`, `residual_risks` included) has
/// no default: the key must be present even where an empty list is a
/// legitimate value, exactly as
/// `domain::handoff::tests::an_absent_key_fails_deserialization_and_never_reaches_validate`
/// pins for `implementation_decisions`. Every field is still populated
/// here, non-empty, for the reason `example_card_draft`'s own doc comment
/// gives: an example that omits a field, or leaves a list looking
/// structurally empty, teaches a reader less about its shape than one that
/// does not. `gate_failures` needs this most of all — left at its default
/// `vec![]`, `skip_serializing_if` would drop the key entirely, and
/// [`optional_fields`] (below) can only discover a key that is actually
/// present to remove.
///
/// The narrative fields reuse `SKILL.md`'s own "Verify and hand off"
/// template phrasing (`behavior_delivered`, `implementation_decisions`,
/// `assumptions`, `known_limitations`, `residual_risks`, `rollback_notes`)
/// deliberately: this document and that section describe the same shape,
/// and matching wording means a reader who has seen one recognizes the
/// other rather than wondering whether they disagree.
///
/// `gate_failures` declares one entry naming `gate.unit` — the feature-gate
/// name every fixture in this crate registers
/// (`tests/support::Workspace::initialized`) — with `reason_category:
/// regression`, one of the two reasons `AttemptKind::GateFailure` admits
/// (`policy::convergence::AttemptKind::admits`). [`run_example`]'s warning
/// says to omit it, or leave it `[]`, for a delivery that hit no admitted
/// gate failure — the common case this example is not.
fn example_declaration() -> ActorDeclaration {
    ActorDeclaration {
        delivered_sha: EXAMPLE_DELIVERED_SHA.to_owned(),
        behavior_delivered: "What the candidate actually does.".to_owned(),
        implementation_decisions: vec!["A choice you made and why.".to_owned()],
        assumptions: vec!["Something inferred rather than specified.".to_owned()],
        known_limitations: vec!["Something deliberately not done.".to_owned()],
        residual_risks: vec!["Something that could still be wrong.".to_owned()],
        rollback_notes: "revert the landing commit on the protected branch".to_owned(),
        gate_failures: vec![DeclaredGateFailure {
            gate_id: "gate.unit".to_owned(),
            reason_category: ReasonCategory::Regression,
        }],
    }
}

/// Which of `declaration`'s top-level fields `ActorDeclaration`'s
/// deserializer accepts as absent.
///
/// Mirrors [`crate::commands::review::optional_fields`] and
/// `crate::commands::card::optional_fields` exactly — same technique, same
/// reasoning, kept as its own copy rather than shared for the same reason
/// `card.rs`'s copy is: the function each mirrors is private to its own
/// file, and #180's frozen file scope does not add a shared home for it.
/// `ActorDeclaration` has only one `#[serde(default)]` field today, so this
/// is expected to report a single-element list — but that expectation is
/// exactly the kind of hand-maintained claim #108 exists to stop from
/// silently drifting from what the parser actually accepts, so it is
/// computed here rather than written down as a constant.
///
/// # Errors
///
/// Returns an error when `declaration` cannot be serialized to inspect.
fn optional_fields(declaration: &ActorDeclaration) -> Result<Vec<String>, HarnessError> {
    let value = serde_json::to_value(declaration)?;
    let object = value.as_object().ok_or_else(|| HarnessError::Control {
        reason: "the example declaration did not serialize to a document with fields".to_owned(),
        code: ErrorCode::InternalEncoding,
    })?;
    let mut optional: Vec<String> = object
        .keys()
        .filter(|key| {
            let mut reduced = object.clone();
            reduced.remove(key.as_str());
            serde_json::from_value::<ActorDeclaration>(serde_json::Value::Object(reduced)).is_ok()
        })
        .cloned()
        .collect();
    optional.sort();
    Ok(optional)
}

/// Emits a complete, valid handoff-declaration example.
///
/// #108 constraint 1: reachable without a control repository or a card —
/// [`ExampleArgs`] carries nothing to open one with. #108 constraint 2:
/// listed under `handoff --help` because it is an ordinary [`HandoffCommand`]
/// variant like any other.
///
/// The document is YAML: [`read_declaration`], above, parses with
/// `serde_yaml_ng` directly — unlike [`crate::commands::card::read_draft`],
/// which goes through `CardDraft::parse`, this reads straight into
/// `ActorDeclaration` with no intermediate — and `serde_yaml_ng` accepts
/// JSON as the YAML it syntactically is, so YAML is the strictly more
/// general of the two accepted shapes. This mirrors `review example` and
/// `card example`; see `run_example` in `src/commands/review.rs` for the
/// full argument.
///
/// Built by constructing an [`ActorDeclaration`] and serializing it, never
/// by writing the document out by hand, so this and [`read_declaration`] can
/// never disagree about the shape: they are the same `serde` implementation.
///
/// # Errors
///
/// Returns an error when the example cannot be rendered.
fn run_example() -> Result<CommandOutcome, HarnessError> {
    let declaration = example_declaration();
    let example =
        serde_yaml_ng::to_string(&declaration).map_err(|source| HarnessError::Control {
            reason: format!("failed to render the example handoff declaration: {source}"),
            code: ErrorCode::InternalEncoding,
        })?;
    let optional = optional_fields(&declaration)?;
    Ok(CommandOutcome::new(
        "handoff.example",
        example.clone(),
        serde_json::json!({
            "format": "yaml",
            "example": example,
            "optional_fields": optional,
        }),
    )
    .with_warning(format!(
        "every value above is illustrative, not a template to copy verbatim: `delivered_sha` \
         is the hexspeak placeholder `cafebabe` repeated to fill 40 hex characters — not a \
         real commit in any repository — which must be replaced with the exact commit `git \
         rev-parse HEAD` reports in the allocated worktree before `handoff create` is run for \
         real, since that command checks this field against the branch it finds \
         (`check_delivered_sha`) and refuses any mismatch; `gate_failures` is shown populated \
         only so its shape is visible; omit it, or leave it `[]`, for a delivery that hit no \
         admitted gate failure. Every field above is shown so its shape is visible, including \
         these, which default when the key is absent: {}",
        optional.join(", ")
    )))
}

/// Collects the gate evidence in force for a candidate, refusing if it is not.
///
/// Section 10.7: handoff creation fails when required gates are stale or
/// missing. The check is here rather than at review time because a handoff is
/// supposed to be the package a reviewer can trust without re-deriving it.
fn collect_evidence(
    control: &ControlRepository,
    card_id: &CardId,
    card_digest: &crate::domain::digest::Digest,
    gates: &[String],
    candidate_sha: &str,
) -> Result<Vec<EvidenceEntry>, HarnessError> {
    require_before_handoff(control, card_id, candidate_sha)?;
    let receipts = receipts_for(control, card_id)?;
    let mut evidence = Vec::new();

    for gate_id in gates {
        let gate = load_gate(control, gate_id)?;
        let gate_digest = gate.digest()?;
        let current: Vec<_> = receipts
            .iter()
            .filter(|receipt| {
                receipt.card_digest.as_ref() == Some(card_digest)
                    && receipt.gate_id == *gate_id
                    && receipt.is_current_for(candidate_sha, &gate_digest)
            })
            .cloned()
            .collect();

        if !evidence_is_acceptable(&current, gate.retry_policy.max_attempts) {
            let reason = receipts
                .iter()
                .rfind(|receipt| receipt.gate_id == *gate_id)
                .and_then(|receipt| receipt.staleness(candidate_sha, &gate_digest))
                .unwrap_or_else(|| format!("no passing run of `{gate_id}` for this candidate"));
            return Err(HarnessError::Control {
                reason: format!("required gate `{gate_id}` is not satisfied: {reason}"),
                code: ErrorCode::GateEvidenceStale,
            });
        }

        for receipt in current.iter().filter(|receipt| receipt.passed) {
            evidence.push(EvidenceEntry {
                gate_id: receipt.gate_id.clone(),
                gate_digest: receipt.gate_digest.clone(),
                receipt_id: receipt.receipt_id.to_string(),
                passed: receipt.passed,
                evaluated_sha: receipt.evaluated_sha.clone(),
            });
        }
    }
    Ok(evidence)
}

// 71-R3: a successful `handoff create`, under a configured convergence
// policy, records a `gate_failure` fact for each gate failure the actor
// declares and a `repair_attempt` fact when the delivery answers a prior
// review return. See `ActorDeclaration::gate_failures` for why the first is
// declared here rather than counted from `gate run`, and
// `repair_attempt_reason` for why the second is inherited rather than
// declared or derived from card state.

/// Every reason category that exists, so an error message can render the set
/// [`AttemptKind::GateFailure`] actually admits instead of a second,
/// hand-typed list that could silently drift from [`AttemptKind::admits`].
/// Mirrors `review.rs`'s constant of the same purpose; kept as its own copy
/// here rather than shared, because there is no third caller yet to justify
/// factoring it out.
const ALL_REASON_CATEGORIES: [ReasonCategory; 6] = [
    ReasonCategory::AcceptanceDefect,
    ReasonCategory::Regression,
    ReasonCategory::SecurityConcern,
    ReasonCategory::ScopeChange,
    ReasonCategory::IntegrationConflict,
    ReasonCategory::NonBlockingImprovement,
];

/// Renders a reason category using the same spelling it would serialize to,
/// so a diagnostic never hand-spells a name `serde` already owns.
fn reason_wire_name(reason: ReasonCategory) -> String {
    match serde_json::to_value(reason) {
        Ok(serde_json::Value::String(name)) => name,
        _ => format!("{reason:?}"),
    }
}

/// The reasons a declared gate failure may name, rendered for an error
/// message.
fn admissible_gate_failure_reasons() -> String {
    ALL_REASON_CATEGORIES
        .into_iter()
        .filter(|reason| AttemptKind::GateFailure.admits(*reason))
        .map(reason_wire_name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Refuses a declaration whose `gate_failures` cannot be recorded: a `gate_id`
/// the card does not name among its feature gates, a `reason_category`
/// [`AttemptKind::GateFailure`] does not admit, or the same gate declared
/// twice.
///
/// Checked only under a configured convergence policy — with none
/// configured, `gate_failures` is accepted and ignored exactly as
/// [`ActorDeclaration::gate_failures`] documents, so an unconfigured project
/// gains no refusal it did not already have. Called from both `run_create`
/// and `preview_create`, in the same place relative to their other checks,
/// so a dry run can never promise a write the real command would refuse —
/// the same discipline `review.rs`'s `require_review_return_reason`
/// established for 71-R2.
///
/// # Errors
///
/// Returns [`ErrorCode::PolicyIncompleteHandoff`] for an unregistered gate,
/// an inadmissible reason, or a repeated gate.
fn validate_declared_gate_failures(
    policy: Option<&ConvergencePolicy>,
    feature_gates: &[String],
    gate_failures: &[DeclaredGateFailure],
) -> Result<(), HarnessError> {
    if policy.is_none() {
        return Ok(());
    }
    let mut declared_gates = BTreeSet::new();
    for failure in gate_failures {
        if !feature_gates.contains(&failure.gate_id) {
            return Err(HarnessError::Control {
                reason: format!(
                    "declared gate failure names `{}`, which is not one of this card's feature gates: {}",
                    failure.gate_id,
                    feature_gates.join(", ")
                ),
                code: ErrorCode::PolicyIncompleteHandoff,
            });
        }
        if !AttemptKind::GateFailure.admits(failure.reason_category) {
            return Err(HarnessError::Control {
                reason: format!(
                    "declared gate failure for `{}` names `reason_category: {}`, which a gate failure cannot declare; admissible reasons are: {}",
                    failure.gate_id,
                    reason_wire_name(failure.reason_category),
                    admissible_gate_failure_reasons()
                ),
                code: ErrorCode::PolicyIncompleteHandoff,
            });
        }
        if !declared_gates.insert(failure.gate_id.as_str()) {
            return Err(HarnessError::Control {
                reason: format!(
                    "declared gate failure names `{}` more than once; declare each failing gate once",
                    failure.gate_id
                ),
                code: ErrorCode::PolicyIncompleteHandoff,
            });
        }
    }
    Ok(())
}

/// The reason category a repair-attempt fact must inherit, if one may be
/// recorded at all.
///
/// A `handoff create` answering a `changes_requested` return is a repair
/// attempt, but nothing about *why* the work is being redelivered is
/// declared on this command's own input, or derived from the card's state:
/// the reviewer already declared it, as the `review_return` fact 71-R2
/// records. Asking the actor to declare it again would let the same return
/// be filed under two different reasons by two different people, so it is
/// inherited instead — contrast [`ActorDeclaration::gate_failures`], where
/// nothing else ever records why a gate went red, which is exactly why that
/// one *is* declared here.
///
/// Returns `None` — meaning no repair-attempt fact is recorded — in exactly
/// two cases, both deliberate:
///
/// - No `review_return` fact exists for this card at all, for instance
///   because the policy was configured after the return happened. There is
///   nothing to inherit, and defaulting to some category would attribute a
///   reason the reviewer never declared.
/// - The inherited reason is `non_blocking_improvement`: the one reason
///   [`AttemptKind::ReviewReturn`] admits that [`AttemptKind::RepairAttempt`]
///   does not. Polishing on request is not the convergence failure this
///   budget exists to detect, so a return filed for that reason produces no
///   repair attempt however many times the actor redelivers.
///
/// "The" `review_return` fact is the one with the greatest `event_id`, which
/// is monotonic across the control repository's whole history; `occurred_at`
/// is a wall clock and is never consulted.
///
/// # Errors
///
/// Returns an error when the event store cannot be read, or when the latest
/// `review_return` fact's `reason_category` cannot be parsed — a fact this
/// command itself wrote and cannot read back is control corruption, not an
/// absent one.
fn repair_attempt_reason(
    control: &ControlRepository,
    card_id: &CardId,
) -> Result<Option<ReasonCategory>, HarnessError> {
    let events = EventStore::new(control).for_card(card_id)?;
    let latest_return = events
        .iter()
        .filter(|event| {
            event.event_type == ATTEMPT_RECORDED_EVENT
                && event
                    .metadata
                    .get("attempt_kind")
                    .and_then(|value| serde_json::from_value::<AttemptKind>(value.clone()).ok())
                    == Some(AttemptKind::ReviewReturn)
        })
        .max_by_key(|event| &event.event_id);

    let Some(event) = latest_return else {
        return Ok(None);
    };
    let reason: ReasonCategory = event
        .metadata
        .get("reason_category")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .ok_or_else(|| HarnessError::Control {
            reason: format!(
                "event {} is a review_return attempt fact with no readable reason_category",
                event.event_id
            ),
            code: ErrorCode::InternalControlCorrupt,
        })?;

    Ok(AttemptKind::RepairAttempt.admits(reason).then_some(reason))
}

/// Renders the "would record" / "recorded" summary lines shared by
/// `preview_create` and `run_create`'s outcome messages.
fn fact_summary(gate_failure_facts: usize, repair_attempt_recorded: bool, tense: &str) -> String {
    let mut summary = String::new();
    if gate_failure_facts > 0 {
        let _ = write!(
            summary,
            "\n{tense} {gate_failure_facts} gate_failure convergence fact(s)"
        );
    }
    if repair_attempt_recorded {
        let _ = write!(summary, "\n{tense} one repair_attempt convergence fact");
    }
    summary
}

/// Reports what `handoff create` would bind, without binding it.
fn preview_create(
    args: &CreateArgs,
    card_id: &CardId,
    declaration: &ActorDeclaration,
) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.common.control)?;
    let config = control.project()?;
    let (record, state) = load_card(&control, card_id)?;
    let lesson_manifest =
        bound_packet_manifest(&control, &record, args.lesson_manifest_digest.as_deref())?;
    // 72-2: the first check that can refuse, before anything else — a
    // preview must never promise a handoff the real command would refuse
    // for an escalated card. See `require_convergence_budget`.
    require_convergence_budget(&control, &config, &record)?;
    state.state.check_transition(CardState::HandedOff)?;
    // 71-R3: the same validation `run_create` performs, in the same place
    // relative to its other checks — see `validate_declared_gate_failures`.
    validate_declared_gate_failures(
        config.convergence_policy.as_ref(),
        &record.named_gates.feature,
        &declaration.gate_failures,
    )?;
    let (_lease, scope, head) = candidate_of(&control, card_id, &declaration.delivered_sha)?;
    let (baseline, _commits, diff) = derive_facts(&scope, &record.base_sha, &head)?;
    verify_handoff_candidate(
        &record,
        state.current_digest.as_str(),
        &scope,
        &baseline,
        &head,
        diff,
    )?;
    // Preview must validate the same feature-gate evidence the real handoff
    // binds. Otherwise it can report ready immediately before the real command
    // refuses for a missing or stale receipt.
    let receipts = collect_evidence(
        &control,
        card_id,
        &state.current_digest,
        &record.named_gates.feature,
        &head,
    )?;
    require_lesson_evidence(&control, &record, &lesson_manifest, &receipts)?;

    // 71-R3: the same two counts `run_create` would actually record — see the
    // matching fields on its own success outcome.
    let (gate_failure_facts, repair_attempt_recorded) = if config.convergence_policy.is_some() {
        (
            declaration.gate_failures.len(),
            repair_attempt_reason(&control, card_id)?.is_some(),
        )
    } else {
        (0, false)
    };

    let lesson_manifest_digest = lesson_manifest.digest()?;
    Ok(CommandOutcome::new(
        "handoff.create",
        format!(
            "Dry run: would hand off card {card_id} at {head}{}; nothing was changed",
            fact_summary(gate_failure_facts, repair_attempt_recorded, "would record")
        ),
        serde_json::json!({
            "dry_run": true,
            "card_id": card_id.to_string(),
            "candidate_sha": head,
            "gate_failure_facts": gate_failure_facts,
            "repair_attempt_recorded": repair_attempt_recorded,
            "lesson_manifest": lesson_manifest,
            "lesson_manifest_digest": lesson_manifest_digest.as_str(),
        }),
    ))
}

/// Reconstructs the live manifest and proves it is the exact one emitted to
/// the implementer by `work packet`.
///
/// A live recomputation alone is unsafe: retiring a lesson after packet
/// generation would make the handoff silently omit an obligation the
/// implementer received. Once a project has any lesson history, the caller
/// must therefore return the packet digest and it must match exactly. Projects
/// created before governed lessons remain compatible while their registry is
/// empty; supplying a digest there still opts into the exact comparison.
fn bound_packet_manifest(
    control: &ControlRepository,
    card: &crate::domain::card::CardRecord,
    expected_digest: Option<&str>,
) -> Result<LessonManifest, HarnessError> {
    let lessons = all_lessons(control)?;
    let manifest = build_manifest(card, &lessons)?;
    let actual_digest = manifest.digest()?;

    let expected_digest = match expected_digest {
        Some(value) => Some(value.parse::<Digest>()?),
        None if lessons.is_empty() => None,
        None => {
            return Err(HarnessError::Control {
                reason: format!(
                    "handoff for card {} is missing the lesson manifest digest from `work packet`; the current manifest is {actual_digest}",
                    card.card_id
                ),
                code: ErrorCode::PolicyLessonManifestStale,
            });
        }
    };

    if let Some(expected_digest) = expected_digest
        && expected_digest != actual_digest
    {
        return Err(HarnessError::Control {
            reason: format!(
                "handoff for card {} expected lesson manifest {expected_digest}, but the current manifest is {actual_digest}; the implementation packet is stale",
                card.card_id
            ),
            code: ErrorCode::PolicyLessonManifestStale,
        });
    }

    Ok(manifest)
}

/// Enforces the machine-checkable part of every required lesson at handoff.
///
/// A lesson cannot silently become a note an agent may skip: required feature
/// gates must be named by the card, registered in the control repository, and
/// represented by a passing receipt for the exact candidate. Review checks are
/// enforced when the independent verdict is recorded.
fn require_lesson_evidence(
    control: &ControlRepository,
    card: &crate::domain::card::CardRecord,
    manifest: &LessonManifest,
    receipts: &[EvidenceEntry],
) -> Result<(), HarnessError> {
    for lesson in manifest.required() {
        for gate_id in lesson.obligations.gate_ids() {
            load_gate(control, &gate_id)?;
        }
        for gate_id in &lesson.obligations.feature_gates {
            if !card
                .named_gates
                .feature
                .iter()
                .any(|named| named == gate_id)
            {
                return Err(HarnessError::Control {
                    reason: format!(
                        "required lesson `{}` requires feature gate `{gate_id}`, but card {} does not name it",
                        lesson.lesson_id, card.card_id
                    ),
                    code: ErrorCode::PolicyLessonEvidenceMissing,
                });
            }
            let Some(receipt) = receipts.iter().find(|receipt| receipt.gate_id == *gate_id) else {
                return Err(HarnessError::Control {
                    reason: format!(
                        "required lesson `{}` has no evidence for feature gate `{gate_id}`",
                        lesson.lesson_id
                    ),
                    code: ErrorCode::PolicyLessonEvidenceMissing,
                });
            };
            if !receipt.passed {
                return Err(HarnessError::Control {
                    reason: format!(
                        "required lesson `{}` is not satisfied: feature gate `{gate_id}` did not pass",
                        lesson.lesson_id
                    ),
                    code: ErrorCode::PolicyLessonEvidenceMissing,
                });
            }
        }
    }
    Ok(())
}

/// Gathers the machine-computed half of a handoff from Git objects.
///
/// Separate from the actor's declaration on purpose: this half is derived and
/// trustworthy, that half is a claim. `SPIKE-001` finding F-5 showed a
/// declaration asserting behavior the code did not have.
fn derive_facts(
    scope: &GitScope,
    base_sha: &str,
    candidate_sha: &str,
) -> Result<(String, Vec<String>, DiffSummary), HarnessError> {
    let baseline = inspect::resolve_commit(scope, base_sha)?;
    let diff = crate::git::diff::diff_commits(scope, &baseline, candidate_sha)?;
    let commits: Vec<String> = inspect::raw(
        scope,
        [
            "log",
            "--format=%H",
            "--reverse",
            &format!("{baseline}..{candidate_sha}"),
        ],
    )?
    .trimmed_stdout()
    .lines()
    .map(ToOwned::to_owned)
    .collect();
    Ok((baseline, commits, diff))
}

/// Builds and enforces the verification report that a handoff is bound to.
///
/// `work verify` remains the separately observable verification command. A
/// handoff repeats its policy evaluation because it is the final control point
/// before an out-of-scope candidate becomes reviewable evidence.
fn verify_handoff_candidate(
    record: &crate::domain::card::CardRecord,
    card_digest: &str,
    scope: &GitScope,
    declared_base: &str,
    candidate_sha: &str,
    diff: DiffSummary,
) -> Result<(), HarnessError> {
    let actual_base = inspect::merge_base(scope, declared_base, candidate_sha)?;
    let subjects = inspect::raw(
        scope,
        [
            "log",
            "--format=%s",
            &format!("{declared_base}..{candidate_sha}"),
        ],
    )?;
    let commit_subjects = subjects
        .trimmed_stdout()
        .lines()
        .map(ToOwned::to_owned)
        .collect();
    let facts = CandidateFacts {
        declared_base: declared_base.to_owned(),
        actual_base,
        candidate_sha: candidate_sha.to_owned(),
        diff,
        commit_subjects,
        // `candidate_of` has already refused a dirty worktree. Keeping this
        // fact explicit makes the handoff's policy input complete without a
        // second inspection of the worktree.
        worktree_clean: true,
        dirty_paths: Vec::new(),
    };
    let report = verify(record, card_digest, &facts);
    if report.passed {
        Ok(())
    } else {
        Err(verification_refusal(&report, &record.card_id))
    }
}

/// Turns a failed verification report into the policy refusal a handoff needs.
fn verification_refusal(report: &VerificationReport, card_id: &CardId) -> HarnessError {
    HarnessError::Control {
        reason: format!(
            "candidate {} is outside card {card_id}: {}",
            report.candidate_sha,
            report
                .blocking()
                .iter()
                .map(|finding| finding.detail.clone())
                .collect::<Vec<_>>()
                .join("; ")
        ),
        code: ErrorCode::PolicyCandidateOutOfScope,
    }
}

/// Loads the cycle whose baseline the handoff must record.
fn load_handoff_cycle(
    control: &ControlRepository,
    record: &crate::domain::card::CardRecord,
) -> Result<CycleRecord, HarnessError> {
    serde_json::from_str(&control.read(&CycleRecord::relative_path(&record.cycle_id))?).map_err(
        |source| HarnessError::Control {
            reason: format!("cycle {} is malformed: {source}", record.cycle_id),
            code: ErrorCode::InternalControlCorrupt,
        },
    )
}

/// Resolves the candidate a handoff would describe, refusing if it cannot stand.
///
/// The `delivered_sha` check runs before every other precondition, because a
/// branch that moved after delivery invalidates the whole exercise: everything
/// downstream would describe code the actor did not produce.
fn candidate_of(
    control: &ControlRepository,
    card_id: &CardId,
    delivered_sha: &str,
) -> Result<(crate::domain::lease::LeaseRecord, GitScope, String), HarnessError> {
    let lease = held_lease(control, card_id)?.ok_or_else(|| HarnessError::Control {
        reason: format!("card {card_id} holds no lease; run `work start` first"),
        code: ErrorCode::PreconditionNotFound,
    })?;
    let scope = GitScope::work_tree(&lease.worktree_path);
    let candidate_sha = inspect::resolve_commit(&scope, "HEAD")?;

    // SPIKE-001 F-1: before anything else, confirm the branch still holds what
    // the actor says they delivered.
    check_delivered_sha(delivered_sha, &candidate_sha)?;

    let worktree_state = inspect::worktree_state(&scope)?;
    if !worktree_state.clean {
        return Err(HarnessError::Control {
            reason: format!(
                "worktree {} has uncommitted or untracked content: {}",
                lease.worktree_path.display(),
                worktree_state.dirty_paths.join(", ")
            ),
            code: ErrorCode::PreconditionWorktreeDirty,
        });
    }
    Ok((lease, scope, candidate_sha))
}

// 71-R3's fact emission has to run inside this one transaction, after the
// handoff record and its own `handoff.created` event are written and before
// the commit — splitting it into a helper the length limit would otherwise
// invite risks losing that ordering at a call site instead of at a glance.
#[allow(clippy::too_many_lines)]
fn run_create(args: &CreateArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;
    let declaration = read_declaration(&args.declaration)?;
    declaration.validate()?;

    if args.dry_run {
        return preview_create(args, &card_id, &declaration);
    }

    with_transaction(
        &args.common.control,
        "handoff.create",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let config = control.project()?;
            let (record, state) = load_card(control, &card_id)?;
            let lesson_manifest =
                bound_packet_manifest(control, &record, args.lesson_manifest_digest.as_deref())?;
            // 72-2: the first check that can refuse, before anything is
            // written — see `require_convergence_budget`.
            require_convergence_budget(control, &config, &record)?;
            state.state.check_transition(CardState::HandedOff)?;
            // 71-R3: refused before any candidate resolution or write, in the
            // same place relative to this transaction's other checks that
            // `preview_create` uses — see `validate_declared_gate_failures`.
            validate_declared_gate_failures(
                config.convergence_policy.as_ref(),
                &record.named_gates.feature,
                &declaration.gate_failures,
            )?;

            let (lease, scope, candidate_sha) =
                candidate_of(control, &card_id, &declaration.delivered_sha)?;

            let cycle = load_handoff_cycle(control, &record)?;

            let (baseline, commits, diff) = derive_facts(&scope, &record.base_sha, &candidate_sha)?;
            verify_handoff_candidate(
                &record,
                state.current_digest.as_str(),
                &scope,
                &baseline,
                &candidate_sha,
                diff.clone(),
            )?;

            let receipts = collect_evidence(
                control,
                &card_id,
                &state.current_digest,
                &record.named_gates.feature,
                &candidate_sha,
            )?;
            require_lesson_evidence(control, &record, &lesson_manifest, &receipts)?;
            let dependency_bindings =
                resolve_dependency_bindings(control, &scope, &record.depends_on, &candidate_sha)?;

            let id = next_handoff_id(control)?;
            let handoff = HandoffRecord {
                schema: HANDOFF_SCHEMA.to_owned(),
                handoff_id: id.clone(),
                card_id: card_id.clone(),
                card_revision: state.current_revision,
                card_digest: state.current_digest.clone(),
                cycle_id: record.cycle_id.clone(),
                baseline_sha: cycle.baseline_sha.clone().unwrap_or(baseline),
                branch: lease.branch.clone(),
                candidate_sha: candidate_sha.clone(),
                commits,
                dependency_bindings,
                changed_paths: diff.paths,
                receipts,
                lesson_manifest: Some(lesson_manifest.clone()),
                worktree_clean: true,
                declaration: declaration.clone(),
                actor_id: args.common.actor.clone(),
                created_at: clock.now(),
                status: HandoffStatus::Active,
                canonical_algorithm: CANONICAL_ALGORITHM.to_owned(),
            };
            let digest = handoff.digest()?;
            let lesson_manifest_digest = lesson_manifest.digest()?;

            control.write_atomic(
                &HandoffRecord::relative_path(&id),
                &format!("{}\n", serde_json::to_string_pretty(&handoff)?),
            )?;
            store_card_state(control, &record, &state, CardState::HandedOff)?;

            events.append(
                &config.project_id,
                EventDraft::new("handoff.created", &args.common.actor)
                    .cycle(record.cycle_id.clone())
                    .card(
                        card_id.clone(),
                        state.current_revision,
                        state.current_digest.clone(),
                    )
                    .transition(Some(state.state.name()), CardState::HandedOff.name())
                    .head(candidate_sha.clone())
                    .meta("handoff_id", serde_json::json!(id))
                    .meta("handoff_digest", serde_json::json!(digest.as_str()))
                    .meta(
                        "delivered_sha",
                        serde_json::json!(declaration.delivered_sha),
                    )
                    .meta(
                        "lesson_manifest_digest",
                        serde_json::json!(lesson_manifest_digest.as_str()),
                    ),
                clock,
            )?;

            // 71-R3: gate_failure and repair_attempt convergence facts,
            // recorded in this same transaction, after the handoff record and
            // its own `handoff.created` event and before the commit — the
            // same placement 069ef4c and 69c1655 established for their own
            // dimensions. Declared gate failures land in declaration order;
            // the repair attempt, if any, lands after all of them — see the
            // contract's determinism requirement, and `repair_attempt_reason`
            // for why it may legitimately be none at all.
            let mut gate_failure_facts = 0usize;
            let mut repair_attempt_recorded = false;
            if let Some(policy) = config.convergence_policy.as_ref() {
                let policy_digest = policy.digest()?;
                for failure in &declaration.gate_failures {
                    events.append(
                        &config.project_id,
                        EventDraft::new(ATTEMPT_RECORDED_EVENT, &args.common.actor)
                            .cycle(record.cycle_id.clone())
                            .card(
                                card_id.clone(),
                                state.current_revision,
                                state.current_digest.clone(),
                            )
                            .head(candidate_sha.clone())
                            .meta(
                                "attempt_kind",
                                serde_json::to_value(AttemptKind::GateFailure)?,
                            )
                            .meta(
                                "reason_category",
                                serde_json::to_value(failure.reason_category)?,
                            )
                            .meta(
                                "evidence_ref",
                                serde_json::json!(format!("handoff:{id}#gate:{}", failure.gate_id)),
                            )
                            .meta("policy_digest", serde_json::json!(policy_digest.as_str())),
                        clock,
                    )?;
                    gate_failure_facts += 1;
                }

                if let Some(reason) = repair_attempt_reason(control, &card_id)? {
                    events.append(
                        &config.project_id,
                        EventDraft::new(ATTEMPT_RECORDED_EVENT, &args.common.actor)
                            .cycle(record.cycle_id.clone())
                            .card(
                                card_id.clone(),
                                state.current_revision,
                                state.current_digest.clone(),
                            )
                            .head(candidate_sha.clone())
                            .meta(
                                "attempt_kind",
                                serde_json::to_value(AttemptKind::RepairAttempt)?,
                            )
                            .meta("reason_category", serde_json::to_value(reason)?)
                            .meta("evidence_ref", serde_json::json!(format!("handoff:{id}")))
                            .meta("policy_digest", serde_json::json!(policy_digest.as_str())),
                        clock,
                    )?;
                    repair_attempt_recorded = true;
                }
            }

            control.commit(expected, &format!("handoff: create {id}"))?;

            Ok(CommandOutcome::new(
                "handoff.create",
                format!(
                    "Handed off card {card_id}\nhandoff: {id}\ncandidate: {candidate_sha}\ndigest: {digest}\nchanged paths: {}\nevidence: {} receipt(s){}",
                    handoff.changed_paths.len(),
                    handoff.receipts.len(),
                    fact_summary(gate_failure_facts, repair_attempt_recorded, "recorded"),
                ),
                serde_json::json!({
                    "handoff": handoff,
                    "handoff_digest": digest.as_str(),
                    "lesson_manifest_digest": lesson_manifest_digest.as_str(),
                    "gate_failure_facts": gate_failure_facts,
                    "repair_attempt_recorded": repair_attempt_recorded,
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}

fn run_inspect(args: &CardArgs) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;
    let control = ControlRepository::open(&args.common.control)?;
    let config = control.project()?;
    let (record, state) = load_card(&control, &card_id)?;

    let handoff = latest_handoff(&control, &card_id)?.ok_or_else(|| HarnessError::Control {
        reason: format!("card {card_id} has no handoff"),
        code: ErrorCode::PreconditionNotFound,
    })?;

    let current_candidate = held_lease(&control, &card_id)?.and_then(|lease| {
        inspect::resolve_commit(&GitScope::work_tree(&lease.worktree_path), "HEAD").ok()
    });
    let standings = dependency_standings(
        &control,
        &GitScope::work_tree(&config.repository),
        &record.depends_on,
        &handoff.dependency_bindings,
    )?;
    let lesson_staleness = if let Some(manifest) = handoff.lesson_manifest.as_ref() {
        validate_manifest_registry(&control, manifest)?;
        let current = build_manifest(&record, &all_lessons(&control)?)?;
        (manifest.lessons != current.lessons)
            .then(|| "the active lesson set changed since this handoff was created".to_owned())
    } else {
        None
    };
    let staleness = lesson_staleness.or_else(|| {
        current_candidate
            .as_ref()
            .and_then(|sha| handoff.staleness(sha, &state.current_digest, &standings))
    });

    let mut text = format!(
        "Handoff {} for card {card_id}\ncandidate: {}\nbranch: {}\ndigest: {}\ncommits: {}\nchanged paths: {}\nevidence: {}\nstatus: {}",
        handoff.handoff_id,
        handoff.candidate_sha,
        handoff.branch,
        handoff.digest()?,
        handoff.commits.len(),
        handoff.changed_paths.len(),
        handoff.receipts.len(),
        if staleness.is_none() {
            "current"
        } else {
            "stale"
        }
    );
    if let Some(reason) = &staleness {
        let _ = write!(text, "\nstale because: {reason}");
    }

    let mut outcome = CommandOutcome::new(
        "handoff.inspect",
        text,
        serde_json::json!({
            "handoff": handoff,
            "handoff_digest": handoff.digest()?.as_str(),
            "current_candidate_sha": current_candidate,
            "is_current": staleness.is_none(),
            "stale_reason": staleness,
        }),
    )
    .with_project(config.project_id.clone());

    if staleness.is_some() {
        outcome = outcome.with_warning(
            "this handoff no longer describes the branch; a review recorded against it would be reviewing different code".to_owned(),
        );
    }
    Ok(outcome)
}

fn run_revoke(args: &RevokeArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;

    if args.dry_run {
        let control = ControlRepository::open(&args.common.control)?;
        let handoff = latest_handoff(&control, &card_id)?.ok_or_else(|| HarnessError::Control {
            reason: format!("card {card_id} has no handoff"),
            code: ErrorCode::PreconditionNotFound,
        })?;
        return Ok(CommandOutcome::new(
            "handoff.revoke",
            format!(
                "Dry run: would revoke handoff {}; nothing was changed",
                handoff.handoff_id
            ),
            serde_json::json!({ "dry_run": true, "handoff_id": handoff.handoff_id }),
        ));
    }

    with_transaction(
        &args.common.control,
        "handoff.revoke",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let config = control.project()?;
            let (record, state) = load_card(control, &card_id)?;
            let mut handoff =
                latest_handoff(control, &card_id)?.ok_or_else(|| HarnessError::Control {
                    reason: format!("card {card_id} has no handoff to revoke"),
                    code: ErrorCode::PreconditionNotFound,
                })?;

            if handoff.status == HandoffStatus::Revoked {
                return Err(HarnessError::Control {
                    reason: format!("handoff {} is already revoked", handoff.handoff_id),
                    code: ErrorCode::PolicyInvalidTransition,
                });
            }
            state.state.check_transition(CardState::Active)?;

            handoff.status = HandoffStatus::Revoked;
            control.write_atomic(
                &HandoffRecord::relative_path(&handoff.handoff_id),
                &format!("{}\n", serde_json::to_string_pretty(&handoff)?),
            )?;
            store_card_state(control, &record, &state, CardState::Active)?;

            events.append(
                &config.project_id,
                EventDraft::new("handoff.revoked", &args.common.actor)
                    .cycle(record.cycle_id.clone())
                    .card(
                        card_id.clone(),
                        state.current_revision,
                        state.current_digest.clone(),
                    )
                    .transition(Some(state.state.name()), CardState::Active.name())
                    .meta("handoff_id", serde_json::json!(handoff.handoff_id))
                    .meta("reason", serde_json::json!(args.reason)),
                clock,
            )?;
            control.commit(expected, &format!("handoff: revoke {}", handoff.handoff_id))?;

            Ok(CommandOutcome::new(
                "handoff.revoke",
                format!(
                    "Revoked handoff {}\nreason: {}\ncard {card_id} returns to active work",
                    handoff.handoff_id, args.reason
                ),
                serde_json::json!({
                    "handoff_id": handoff.handoff_id,
                    "card_id": card_id.to_string(),
                    "state": CardState::Active.name(),
                    "reason": args.reason,
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}
