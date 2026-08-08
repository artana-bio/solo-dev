//! Card lifecycle commands.

use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

use crate::{
    cli::output::CommandOutcome,
    commands::CONTROL_ENV,
    commands::{gate::require_registered, transaction::with_transaction},
    config::{ProjectConfig, ValidationPolicy},
    control::{
        event_store::{EventDraft, EventStore},
        repository::ControlRepository,
    },
    domain::{
        artifact::{ArtifactClass, GeneratedArtifact},
        card::{
            Acceptance, CARD_DIR, CardDraft, CardRecord, CardState, NamedGates, PROOF_MAP_SCHEMA,
            ProofMap, ProofMapEntry, Risk, WriteScope,
        },
        clock::Clock,
        cycle::{CYCLE_DIR, CycleRecord, CycleStatus},
        digest::Digest,
        ids::CardId,
    },
    error::{ErrorCode, HarnessError},
    policy::allocation::{Claim, check_admissible, check_dependencies},
    policy::convergence::{
        ATTEMPT_RECORDED_EVENT, AttemptKind, CardConvergence, CardDimension, NextPermittedAction,
        ReasonCategory, ScopeBreadth, assess_card, project,
    },
};

/// Schema identifier for a card's mutable state file.
pub const CARD_STATE_SCHEMA: &str = "harness.card-state/v1";

/// The mutable pointer to a card's current immutable revision.
///
/// Kept separate from the revision records so the records themselves are never
/// rewritten. Section 7.3.3 makes an activated card immutable; a state field
/// living inside it would break that.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CardStateRecord {
    /// Always [`CARD_STATE_SCHEMA`].
    pub schema: String,
    /// Identifies the card.
    pub card_id: CardId,
    /// Where the card sits in its lifecycle.
    pub state: CardState,
    /// The revision currently in force.
    pub current_revision: u32,
    /// Digest of the revision currently in force.
    pub current_digest: Digest,
    /// The canonicalization algorithm the digest was computed under.
    pub canonical_algorithm: String,
}

impl CardStateRecord {
    /// Relative path of a card's state file.
    #[must_use]
    pub fn relative_path(card_id: &CardId) -> String {
        format!("{CARD_DIR}/{card_id}/state.json")
    }
}

/// Subcommands under `card`.
#[derive(Debug, Subcommand)]
pub enum CardCommand {
    /// Validate a draft without storing it.
    Validate(DraftArgs),
    /// Store a draft against a cycle.
    Create(CreateArgs),
    /// Freeze a draft into an immutable revision.
    Activate(ActivateArgs),
    /// Supersede the current revision with a new one.
    Revise(ReviseArgs),
    /// Abandon a card that will not be landed.
    Abandon(AbandonArgs),
    /// Report a card's state, revision, and digest.
    Status(StatusArgs),
    /// Print a complete, valid card draft example.
    ///
    /// Built by constructing a real `CardDraft` and serializing it, so this
    /// can never disagree with what `card create` accepts — see #108. #180
    /// added this: the one document kind #142 §11 had found with no
    /// generated example at all, and the exact dead end its own §1 traces —
    /// `acceptance: invalid type: sequence, expected struct Acceptance`,
    /// with a recovery that used to say outright that no example existed.
    Example(ExampleArgs),
}

impl CardCommand {
    /// Its dotted command path, as the result envelope reports it.
    ///
    /// The error envelope used to carry only the group — `card` — while a
    /// success carried the full path, so a consumer matching on `command` got a
    /// different granularity depending on whether the command worked.
    #[must_use]
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Validate(..) => "card.validate",
            Self::Create(..) => "card.create",
            Self::Activate(..) => "card.activate",
            Self::Revise(..) => "card.revise",
            Self::Abandon(..) => "card.abandon",
            Self::Status(..) => "card.status",
            Self::Example(..) => "card.example",
        }
    }
}

/// Arguments shared by card subcommands that touch control state.
#[derive(Debug, Args)]
pub struct CommonArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
    pub control: PathBuf,
    /// Identifies the acting party. Declared, not proven; see D-013.
    #[arg(long, default_value = "operator")]
    pub actor: String,
}

/// Arguments accepted by `card validate`.
#[derive(Debug, Args)]
pub struct DraftArgs {
    /// Path to the draft, in YAML or JSON.
    #[arg(long)]
    pub draft: PathBuf,
}

/// Arguments accepted by `card create`.
#[derive(Debug, Args)]
pub struct CreateArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// Path to the draft, in YAML or JSON.
    #[arg(long)]
    pub draft: PathBuf,
    /// Report planned mutations without performing them.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `card activate`.
#[derive(Debug, Args)]
pub struct ActivateArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The card to activate.
    #[arg(long)]
    pub card_id: String,
    /// Report planned mutations without performing them.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `card revise`.
#[derive(Debug, Args)]
pub struct ReviseArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The card to revise.
    #[arg(long)]
    pub card_id: String,
    /// Path to the replacement draft.
    #[arg(long)]
    pub draft: PathBuf,
    /// Why the card is being revised.
    #[arg(long)]
    pub reason: String,
    /// Report planned mutations without performing them.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `card abandon`.
#[derive(Debug, Args)]
pub struct AbandonArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The card to abandon.
    #[arg(long)]
    pub card_id: String,
    /// Why it is being abandoned.
    #[arg(long)]
    pub reason: String,
    /// Report planned mutations without performing them.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `card status`.
#[derive(Debug, Args)]
pub struct StatusArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The card to report on.
    #[arg(long)]
    pub card_id: String,
}

/// Arguments accepted by `card example`.
///
/// Deliberately empty, and deliberately not [`CommonArgs`]: #108 constraint 1
/// requires this to be reachable before an operator has a control repository,
/// a project, or a cycle to point it at, so it names neither — the same
/// reasoning `review::ExampleArgs` and `project::ExampleArgs` already give.
#[derive(Debug, Args)]
pub struct ExampleArgs {}

/// Executes a `card` subcommand.
///
/// # Errors
///
/// Returns a policy, precondition, or configuration error as appropriate.
pub fn execute(command: &CardCommand, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    match command {
        CardCommand::Validate(args) => run_validate(args),
        CardCommand::Create(args) => run_create(args, clock),
        CardCommand::Activate(args) => run_activate(args, clock),
        CardCommand::Revise(args) => run_revise(args, clock),
        CardCommand::Abandon(args) => run_abandon(args, clock),
        CardCommand::Status(args) => run_status(args),
        CardCommand::Example(..) => run_example(),
    }
}

/// #142: B1's second reproduction (#142 §1) is this exact site — `card
/// create --draft <missing path>` used to answer "Correct the JSON or YAML
/// syntax of the document" for a file that did not exist at all. Distinct
/// call site from [`CardDraft::parse`]'s own parse failure, so it needs no
/// introspection to tell apart from a schema failure.
const CARD_DRAFT_READ_RECOVERY: &str = "This is a read failure, not a syntax problem: the draft file above could not be opened. Confirm the path exists, is spelled correctly, and is readable by this process.";

/// Reads and parses a draft file.
fn read_draft(path: &PathBuf) -> Result<CardDraft, HarnessError> {
    let raw = fs::read_to_string(path).map_err(|source| HarnessError::ControlWithRecovery {
        reason: format!("cannot read draft {}: {source}", path.display()),
        code: ErrorCode::ConfigMalformed,
        recovery: CARD_DRAFT_READ_RECOVERY,
    })?;
    CardDraft::parse(&raw)
}

/// `deadbeef`, the classic hexspeak placeholder, repeated to fill the 40 hex
/// characters `base_sha` requires (`CardDraft::validate`). A real object id
/// is the SHA-1 of actual Git object content; producing one that happens to
/// equal a constructed string is not a thing an operator, or this function,
/// could do by accident. `deadbeef` is also recognizable on sight as
/// constructed, unlike an arbitrary-looking hex string that could pass for a
/// real prefix and tempt an operator into wondering whether it means
/// something. [`run_example`]'s warning says outright that it must be
/// replaced with the target cycle's real frozen baseline before `card
/// create` is run for anything but this example.
const EXAMPLE_BASE_SHA: &str = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";

/// A complete, valid card draft, for `card example` to emit.
///
/// #180: `CardDraft` is the first example document with genuinely optional
/// fields — `non_goals`, `contract_reads`, `contract_changes`, `depends_on`,
/// `exclusive_resources`, and `generated_artifacts` all carry
/// `#[serde(default)]`, and `proof_map` carries
/// `#[serde(default, skip_serializing_if = "Option::is_none")]` besides.
/// Every one of them is populated here, for the same reason
/// [`crate::commands::review::example_verdict`] populates
/// [`crate::commands::review::Verdict`]'s own optional fields: an example
/// that omits a field teaches a reader nothing about its shape. This matters
/// more for `proof_map` than for the rest — left at `None`, it would not
/// merely look empty, `skip_serializing_if` would drop the key from the
/// document entirely, hiding that the field exists at all. Populating it
/// also means [`optional_fields`] (below) reports it correctly: a key that
/// never appears in the serialized document can never be discovered by
/// removing it and re-parsing.
///
/// The one field this must get right above every other is `acceptance`: the
/// operator whose dead end #180 exists to close had written a sequence
/// where `Acceptance` — a struct with `behaviors` and `regressions` — was
/// expected. Constructing a real `Acceptance` here means that shape is
/// exactly what a reader sees; see [`run_example`]'s doc comment for why
/// that guarantee is structural rather than a promise this function could
/// silently stop keeping.
///
/// `depends_on` names one illustrative card id, `F-099`, distinct from this
/// document's own `F-100`, so the field's shape — a list of card ids, not a
/// single one — is visible without needing a second entry. Nothing about
/// `card create` checks that a declared dependency exists (`card activate`
/// is where that would matter, and this document never reaches it in
/// `card`'s own test), so the id needs only to be well-formed, not real;
/// [`run_example`]'s warning says to remove it entirely for a card with no
/// real dependency, which is the common case this example is not.
///
/// `generated_artifacts` carries one per-card entry whose `sources` is
/// contained in `write_scope.include`, so [`CardDraft::validate`]'s
/// ownership check (`validate_generated_artifacts`, `src/domain/card.rs`)
/// accepts it — the artifact's own `path` is not checked against scope by
/// that function, but this example still places it inside the declared
/// scope, matching what a card would need in order to actually commit it.
///
/// `risk` is `Low`, under which `card activate` never requires a
/// `proof_map` (`ValidationPolicy::requires_proof_map`'s default names only
/// `medium`, `high`, and `critical`) — the point being made here is that a
/// proof map is always *accepted*, at any risk, not that this risk demands
/// one.
fn example_card_draft() -> CardDraft {
    CardDraft {
        card_id: "F-100".parse().unwrap_or_else(|error| {
            unreachable!("`F-100` is a well-formed card id literal: {error}")
        }),
        cycle_id: "C-100".parse().unwrap_or_else(|error| {
            unreachable!("`C-100` is a well-formed cycle id literal: {error}")
        }),
        title: "One-line title".to_owned(),
        goal: "What will be true when this card lands.".to_owned(),
        non_goals: vec!["What this card deliberately does not do.".to_owned()],
        risk: Risk::Low,
        change_kind: "feature".to_owned(),
        base_sha: EXAMPLE_BASE_SHA.to_owned(),
        write_scope: WriteScope {
            include: vec![
                "src/example_feature.rs".to_owned(),
                "src/example_feature_generated.rs".to_owned(),
                "tests/example_feature.rs".to_owned(),
            ],
            exclude: vec![],
        },
        contract_reads: vec!["auth.session".to_owned()],
        contract_changes: vec!["api.v1".to_owned()],
        depends_on: vec!["F-099".parse().unwrap_or_else(|error| {
            unreachable!("`F-099` is a well-formed card id literal: {error}")
        })],
        exclusive_resources: vec!["shared-database-migration-lock".to_owned()],
        named_gates: NamedGates {
            feature: vec!["gate.unit".to_owned()],
            review: vec![],
            integration: vec!["gate.all".to_owned()],
        },
        acceptance: Acceptance {
            behaviors: vec!["Observable behavior a test can fail.".to_owned()],
            regressions: vec!["Behavior that must keep working.".to_owned()],
        },
        generated_artifacts: vec![GeneratedArtifact {
            path: "src/example_feature_generated.rs".to_owned(),
            class: ArtifactClass::PerCard,
            generator: Some("gate.codegen".to_owned()),
            sources: vec!["src/example_feature.rs".to_owned()],
            identifier: None,
        }],
        review_policy: "independent".to_owned(),
        rollback_strategy: "revert the landing commit on the protected branch".to_owned(),
        proof_map: Some(ProofMap {
            schema: PROOF_MAP_SCHEMA.to_owned(),
            entries: vec![ProofMapEntry {
                id: Some("proof-1".to_owned()),
                invariant: "The declared behavior stays true.".to_owned(),
                precondition: "A focused fixture exercises exactly this behavior.".to_owned(),
                assertion: "The check observes the behavior directly.".to_owned(),
                mutation: "A discriminating change that must make the assertion fail.".to_owned(),
                gate_oracle: Some("gate.example".to_owned()),
            }],
            claim_boundary: "Only the behavior named above; nothing wider is claimed.".to_owned(),
        }),
    }
}

/// Which of `draft`'s top-level fields `CardDraft::parse` accepts as absent.
///
/// Mirrors [`crate::commands::review::optional_fields`] exactly — same
/// technique, same reasoning — rather than importing it: that function is
/// private to `review.rs` and #180's frozen file scope does not include it,
/// but the argument for existing at all is unchanged. Established by asking
/// the real deserializer, field by field, rather than reading a hand-typed
/// list off `CardDraft`'s `#[serde(default)]` attributes: a hand-maintained
/// claim about the schema is exactly what #108 exists to stop from silently
/// drifting from what the parser actually accepts, and `CardDraft` — unlike
/// [`crate::commands::review::Verdict`] before it — has six such attributes
/// to keep in step, not one.
///
/// # Errors
///
/// Returns an error when `draft` cannot be serialized to inspect.
fn optional_fields(draft: &CardDraft) -> Result<Vec<String>, HarnessError> {
    let value = serde_json::to_value(draft)?;
    let object = value.as_object().ok_or_else(|| HarnessError::Control {
        reason: "the example card draft did not serialize to a document with fields".to_owned(),
        code: ErrorCode::InternalEncoding,
    })?;
    let mut optional: Vec<String> = object
        .keys()
        .filter(|key| {
            let mut reduced = object.clone();
            reduced.remove(key.as_str());
            serde_json::from_value::<CardDraft>(serde_json::Value::Object(reduced)).is_ok()
        })
        .cloned()
        .collect();
    optional.sort();
    Ok(optional)
}

/// Emits a complete, valid card-draft example.
///
/// #108 constraint 1: reachable without a control repository, a project, or
/// a cycle — [`ExampleArgs`] carries nothing to open one with. #108
/// constraint 2: listed under `card --help` because it is an ordinary
/// [`CardCommand`] variant like any other.
///
/// The document is YAML: `read_draft` and `stored_draft` both parse with
/// [`CardDraft::parse`], which reads with `serde_yaml_ng` — the same parser
/// `review example`'s document is read back with, and `serde_yaml_ng`
/// accepts JSON as the YAML it syntactically is, so YAML is the strictly
/// more general of the two accepted shapes. This mirrors `review example`
/// exactly; see `run_example` in `src/commands/review.rs` for the full
/// argument.
///
/// Built by constructing a [`CardDraft`] and serializing it, never by
/// writing the document out by hand, so this and [`CardDraft::parse`] can
/// never disagree about the shape: they are the same `serde`
/// implementation. That equivalence is what makes `acceptance` land as a
/// struct here rather than the sequence the operator #180 exists for wrote:
/// Rust's type system does not let this function build a `CardDraft` whose
/// `acceptance` field is anything but a real `Acceptance`, so there is no
/// path through this code that could regress the exact shape that mattered.
///
/// # Errors
///
/// Returns an error when the example cannot be rendered.
fn run_example() -> Result<CommandOutcome, HarnessError> {
    let draft = example_card_draft();
    let example = serde_yaml_ng::to_string(&draft).map_err(|source| HarnessError::Control {
        reason: format!("failed to render the example card draft: {source}"),
        code: ErrorCode::InternalEncoding,
    })?;
    let optional = optional_fields(&draft)?;
    Ok(CommandOutcome::new(
        "card.example",
        example.clone(),
        serde_json::json!({
            "format": "yaml",
            "example": example,
            "optional_fields": optional,
        }),
    )
    .with_warning(format!(
        "every value above is illustrative, not a template to copy verbatim: `card_id`, \
         `cycle_id`, and the single `depends_on` entry are placeholder identifiers and must be \
         replaced with this project's real ones (or `depends_on` cleared entirely, for a card \
         with no real dependency), and `base_sha` is the hexspeak placeholder `deadbeef` \
         repeated to fill 40 hex characters — not a real commit in any repository — which must \
         be replaced with the exact 40-character object id the target cycle is frozen at \
         before `card create` is run for real. Every field above is shown so its shape is \
         visible, including these, which default when the key is absent: {}",
        optional.join(", ")
    )))
}

/// Relative path of a stored draft.
fn draft_path(card_id: &CardId) -> String {
    format!("{CARD_DIR}/{card_id}/draft.json")
}

/// Reads the stored draft for a card.
fn stored_draft(control: &ControlRepository, card_id: &CardId) -> Result<CardDraft, HarnessError> {
    let relative = draft_path(card_id);
    if !control.path(&relative).exists() {
        return Err(HarnessError::Control {
            reason: format!("card {card_id} has no stored draft; run `card create` first"),
            code: ErrorCode::PreconditionNotFound,
        });
    }
    CardDraft::parse(&control.read(&relative)?)
}

/// Reads a card's activated revision and current state together.
///
/// Shared with the `work` commands so both read allocation state through one
/// path rather than each re-deriving it.
///
/// # Errors
///
/// Returns a precondition error when the card is not activated.
pub fn load_card(
    control: &ControlRepository,
    card_id: &CardId,
) -> Result<(CardRecord, CardStateRecord), HarnessError> {
    let state = state_of(control, card_id)?.ok_or_else(|| HarnessError::Control {
        reason: format!("card {card_id} is not activated"),
        code: ErrorCode::PreconditionNotFound,
    })?;
    let record: CardRecord = serde_json::from_str(
        &control.read(&CardRecord::relative_path(card_id, state.current_revision))?,
    )
    .map_err(|source| HarnessError::Control {
        reason: format!("card {card_id} revision record is malformed: {source}"),
        code: ErrorCode::InternalControlCorrupt,
    })?;
    Ok((record, state))
}

/// Renders a [`CardDimension`] using the same spelling it would serialize
/// to, so a refusal message never hand-spells a name `serde` already owns.
/// Mirrors `reason_wire_name` in `handoff.rs` and `review.rs`.
fn dimension_wire_name(dimension: CardDimension) -> String {
    match serde_json::to_value(dimension) {
        Ok(serde_json::Value::String(name)) => name,
        _ => format!("{dimension:?}"),
    }
}

/// Renders a [`NextPermittedAction`] using the same spelling it would
/// serialize to, so `card status`'s human text never hand-spells a name
/// `serde` already owns. Mirrors `dimension_wire_name`, just above.
fn next_permitted_action_wire_name(action: NextPermittedAction) -> String {
    match serde_json::to_value(action) {
        Ok(serde_json::Value::String(name)) => name,
        _ => format!("{action:?}"),
    }
}

/// Refuses a controlled action on a card whose convergence budget is spent.
///
/// #72-2: once `assess_card` reports a card `Escalated`, the delivery and
/// review loop for *that* card stops. `handoff create`, `review begin`, and
/// `review record` — and the previews that must never promise a write the
/// real command would refuse — all call this before writing anything, so a
/// raw retry or a fresh review cannot dodge the escalation through the
/// ordinary CLI surface.
///
/// #72-3 closed the three routes left around that loop, bringing the callers
/// to six. `work start` calls this from `preflight_start`, the one place
/// `preview_start` and `run_start`'s transaction share, so a new assignment
/// is refused in a single spot rather than two. `card revise` calls it from
/// `preview_revise` and `run_revise`: a revision returns the card to `ready`,
/// and 71-R1 already made the counters span revisions, so revising neither
/// resets the budget nor may itself proceed on a spent one. `work resume`
/// calls it from `run_resume`'s dry run and from `resume_to_active`'s
/// transaction, but only when the source state is `Ready` — the one state
/// `resumes_to_active` admits that is functionally a fresh `work start`
/// rather than picking already-owned work back up. Resuming from
/// `changes_requested`, `blocked`, or `review_pending` is left open on
/// purpose, or a card could never accumulate the attempts this budget exists
/// to count; so are `work checkpoint` and `work block`, which park or
/// annotate a card without advancing it, and `card status`, which reports
/// the same assessment as data instead of a refusal.
///
/// Other cards in the same cycle are unaffected: this only ever assesses the
/// one `record` names.
///
/// `config.convergence_policy` is read exactly once, into `policy`, and that
/// same value feeds both [`project`] and [`assess_card`] below. `assess_card`
/// never checks that the projection it is handed was built under the policy
/// supplying its limits, so this function — the one place that calls both —
/// is the one place that has to guarantee they agree; reading the policy
/// once and passing that same reference to both calls is how.
///
/// A malformed, duplicate, foreign, or unbound convergence fact makes
/// `project` refuse the whole projection. That refusal is propagated as-is,
/// never treated as an unspent budget: a card whose recorded history cannot
/// be trusted does not get to proceed as though it were `Within`.
///
/// `LegacyUnassessed` and `Within` both mean the action may proceed.
/// `Escalated` refuses it, naming every exhausted dimension with its count,
/// limit, and evidence references in the returned error, so an operator can
/// see what spent the budget without opening the control repository.
///
/// # Errors
///
/// Returns [`ErrorCode::PolicyConvergenceEscalated`] when the card's
/// declared risk has at least one convergence dimension at or over its
/// configured limit. Propagates a control-read or projection failure
/// unchanged otherwise.
pub fn require_convergence_budget(
    control: &ControlRepository,
    config: &ProjectConfig,
    record: &CardRecord,
) -> Result<(), HarnessError> {
    let policy = config.convergence_policy.as_ref();
    let cycle_events = EventStore::new(control).for_cycle(&record.cycle_id)?;
    let view =
        project(policy, &config.project_id, &record.cycle_id, &cycle_events).map_err(|error| {
            HarnessError::Control {
                reason: format!(
                    "convergence projection for cycle {} is unusable: {error}",
                    record.cycle_id
                ),
                code: ErrorCode::InternalControlCorrupt,
            }
        })?;

    let CardConvergence::Escalated { exhausted, .. } =
        assess_card(policy, &view, &record.card_id, record.risk)
    else {
        return Ok(());
    };

    let detail = exhausted
        .iter()
        .map(|dimension| {
            format!(
                "{}: {}/{} (evidence: {})",
                dimension_wire_name(dimension.dimension),
                dimension.count,
                dimension.limit,
                dimension
                    .evidence
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
        .collect::<Vec<_>>()
        .join("; ");

    Err(HarnessError::Control {
        reason: format!(
            "card {} is escalated; convergence budget exhausted: {detail}",
            record.card_id
        ),
        code: ErrorCode::PolicyConvergenceEscalated,
    })
}

/// Assesses a card's convergence budget for `card status`, without refusing.
///
/// 72-3: `card status` is the operator's window into why a card stopped, so
/// unlike [`require_convergence_budget`] this never turns an `Escalated`
/// budget into a refusal — it hands the [`CardConvergence`] back as data.
/// Mirrors that function's own projection and assessment exactly: same
/// policy, same cycle events, same [`assess_card`] call, so the JSON `card
/// status` publishes and the JSON the block above decides from can never
/// disagree about whether a card is escalated. Kept as its own small function
/// rather than a shared call site because `require_convergence_budget` is
/// frozen by #72-2's contract; this only reads what it already reads.
///
/// A malformed, duplicate, foreign, or unbound convergence fact still fails
/// here exactly as it does there: that is control-repository corruption, a
/// different problem than an exhausted budget, and there is no projection
/// left to assess.
///
/// # Errors
///
/// Propagates a control-read or projection failure unchanged.
fn card_convergence(
    control: &ControlRepository,
    config: &ProjectConfig,
    record: &CardRecord,
) -> Result<CardConvergence, HarnessError> {
    let policy = config.convergence_policy.as_ref();
    let cycle_events = EventStore::new(control).for_cycle(&record.cycle_id)?;
    let view =
        project(policy, &config.project_id, &record.cycle_id, &cycle_events).map_err(|error| {
            HarnessError::Control {
                reason: format!(
                    "convergence projection for cycle {} is unusable: {error}",
                    record.cycle_id
                ),
                code: ErrorCode::InternalControlCorrupt,
            }
        })?;
    Ok(assess_card(policy, &view, &record.card_id, record.risk))
}

/// Moves a card to a new state, keeping its revision and digest.
///
/// # Errors
///
/// Returns an error when the state file cannot be written.
pub fn store_card_state(
    control: &ControlRepository,
    record: &CardRecord,
    current: &CardStateRecord,
    next: CardState,
) -> Result<(), HarnessError> {
    control.write_atomic(
        &CardStateRecord::relative_path(&record.card_id),
        &format!(
            "{}\n",
            serde_json::to_string_pretty(&CardStateRecord {
                state: next,
                ..current.clone()
            })?
        ),
    )
}

/// Reads a card's state file.
fn state_of(
    control: &ControlRepository,
    card_id: &CardId,
) -> Result<Option<CardStateRecord>, HarnessError> {
    let relative = CardStateRecord::relative_path(card_id);
    if !control.path(&relative).exists() {
        return Ok(None);
    }
    let raw = control.read(&relative)?;
    serde_json::from_str(&raw)
        .map(Some)
        .map_err(|source| HarnessError::Control {
            reason: format!("card {card_id} state is malformed: {source}"),
            code: ErrorCode::InternalControlCorrupt,
        })
}

/// Loads the cycle a draft names, requiring it to accept cards.
fn cycle_accepting_cards(
    control: &ControlRepository,
    draft: &CardDraft,
) -> Result<CycleRecord, HarnessError> {
    let relative = CycleRecord::relative_path(&draft.cycle_id);
    if !control.path(&relative).exists() {
        return Err(HarnessError::Control {
            reason: format!("cycle {} does not exist", draft.cycle_id),
            code: ErrorCode::PreconditionNotFound,
        });
    }
    let cycle: CycleRecord = serde_json::from_str(&control.read(&relative)?).map_err(|source| {
        HarnessError::Control {
            reason: format!("cycle {} is malformed: {source}", draft.cycle_id),
            code: ErrorCode::InternalControlCorrupt,
        }
    })?;
    if !cycle.status.accepts_cards() {
        return Err(HarnessError::Control {
            reason: format!(
                "cycle {} is `{}` and cannot accept new cards",
                cycle.cycle_id, cycle.status
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    }
    Ok(cycle)
}

/// Requires a card to reason from the exact baseline frozen for its cycle.
fn require_cycle_baseline(cycle: &CycleRecord, draft: &CardDraft) -> Result<(), HarnessError> {
    let baseline = cycle
        .baseline_sha
        .as_deref()
        .ok_or_else(|| HarnessError::Control {
            reason: format!(
                "cycle {} accepts cards but has no frozen baseline",
                cycle.cycle_id
            ),
            code: ErrorCode::PolicyInvalidCycle,
        })?;
    if draft.base_sha != baseline {
        return Err(HarnessError::Control {
            reason: format!(
                "card {} declares base {}, but cycle {} is frozen at {baseline}",
                draft.card_id, draft.base_sha, cycle.cycle_id
            ),
            code: ErrorCode::PolicyCycleBaselineMismatch,
        });
    }
    Ok(())
}

/// Enforces the project's declared proof requirement before an immutable card
/// definition can enter control state. Receipt freshness and gate ordering are
/// deliberately later concerns owned by progressive-validation children.
fn require_declared_proof(
    policy: &ValidationPolicy,
    record: &CardRecord,
) -> Result<(), HarnessError> {
    if policy.requires_proof_map(record.risk) && record.proof_map.is_none() {
        return Err(HarnessError::Control {
            reason: format!(
                "card {} has `{}` risk, and validation policy {} requires a proof_map with invariant, precondition, assertion, mutation, and claim boundary",
                record.card_id,
                record.risk.name(),
                policy.version
            ),
            code: ErrorCode::PolicyInvalidCard,
        });
    }
    if let Some(proof_map) = &record.proof_map {
        proof_map.validate_strict()?;
    }
    Ok(())
}

/// Collects the claims of every card already declared in a cycle.
///
/// Only activated cards appear: a draft has claimed nothing yet, and a card
/// whose state has released its claims is filtered by the allocator itself.
fn existing_claims(
    control: &ControlRepository,
    cycle: &CycleRecord,
    skip: &CardId,
) -> Result<Vec<Claim>, HarnessError> {
    let mut claims = Vec::new();
    for card_id in &cycle.card_ids {
        if card_id == skip {
            continue;
        }
        let Some(state) = state_of(control, card_id)? else {
            continue;
        };
        let record: CardRecord = serde_json::from_str(
            &control.read(&CardRecord::relative_path(card_id, state.current_revision))?,
        )
        .map_err(|source| HarnessError::Control {
            reason: format!("card {card_id} revision record is malformed: {source}"),
            code: ErrorCode::InternalControlCorrupt,
        })?;
        claims.push(Claim::from_record(&record, state.state));
    }
    Ok(claims)
}

/// Collects claims held by cards in every active cycle.
///
/// Cycle membership is the authoritative allocation index. It must be read
/// across the control repository, rather than only from the cycle named by the
/// candidate draft: concurrent cycles otherwise receive overlapping ownership
/// leases without either activation seeing the other.
fn active_cycle_claims(
    control: &ControlRepository,
    skip: &CardId,
) -> Result<Vec<Claim>, HarnessError> {
    let directory = control.path(CYCLE_DIR);
    let entries = fs::read_dir(&directory).map_err(|source| HarnessError::ControlIo {
        path: directory.clone(),
        source,
    })?;
    let mut names: Vec<String> = entries
        .map(|entry| {
            entry
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .map_err(|source| HarnessError::ControlIo {
                    path: directory.clone(),
                    source,
                })
        })
        .collect::<Result<_, _>>()?;
    names.sort();

    let mut claims = Vec::new();
    for name in names {
        if !Path::new(&name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        {
            continue;
        }
        let relative = format!("{CYCLE_DIR}/{name}");
        let cycle: CycleRecord =
            serde_json::from_str(&control.read(&relative)?).map_err(|source| {
                HarnessError::Control {
                    reason: format!("cycle record {relative} is malformed: {source}"),
                    code: ErrorCode::InternalControlCorrupt,
                }
            })?;
        if cycle.status == CycleStatus::Active {
            claims.extend(existing_claims(control, &cycle, skip)?);
        }
    }
    Ok(claims)
}

/// Refuses a card that would contend with anything already active.
fn check_allocation(
    control: &ControlRepository,
    cycle: &CycleRecord,
    record: &CardRecord,
) -> Result<(), HarnessError> {
    let existing = existing_claims(control, cycle, &record.card_id)?;
    let active = active_cycle_claims(control, &record.card_id)?;
    let candidate = Claim::from_record(record, CardState::Ready);
    check_admissible(&candidate, &active)?;

    let mut all = existing;
    all.push(candidate);
    check_dependencies(&all)
}

fn run_validate(args: &DraftArgs) -> Result<CommandOutcome, HarnessError> {
    let draft = read_draft(&args.draft)?;
    draft.validate()?;
    Ok(CommandOutcome::new(
        "card.validate",
        format!(
            "Draft for card {} is valid\ncycle: {}\nrisk: {}\nwrite scope: {}",
            draft.card_id,
            draft.cycle_id,
            draft.risk.name(),
            draft.write_scope.include.join(", ")
        ),
        serde_json::json!({
            "card_id": draft.card_id.to_string(),
            "cycle_id": draft.cycle_id.to_string(),
            "risk": draft.risk.name(),
            "valid": true,
        }),
    ))
}

fn run_create(args: &CreateArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let draft = read_draft(&args.draft)?;
    draft.validate()?;

    if args.dry_run {
        let control = ControlRepository::open(&args.common.control)?;
        cycle_accepting_cards(&control, &draft)?;
        return Ok(CommandOutcome::new(
            "card.create",
            format!(
                "Dry run: would store a draft for card {} in cycle {}; nothing was changed",
                draft.card_id, draft.cycle_id
            ),
            serde_json::json!({ "dry_run": true, "card_id": draft.card_id.to_string() }),
        ));
    }

    with_transaction(
        &args.common.control,
        "card.create",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let cycle = cycle_accepting_cards(control, &draft)?;
            if let Some(state) = state_of(control, &draft.card_id)? {
                return Err(HarnessError::Control {
                    reason: format!(
                        "card {} already exists at revision {} in state `{}`; use `card revise`",
                        draft.card_id, state.current_revision, state.state
                    ),
                    code: ErrorCode::PolicyInvalidCard,
                });
            }

            let config = control.project()?;
            control.write_atomic(
                &draft_path(&draft.card_id),
                &format!("{}\n", serde_json::to_string_pretty(&draft)?),
            )?;
            events.append(
                &config.project_id,
                EventDraft::new("card.created", &args.common.actor)
                    .cycle(cycle.cycle_id.clone())
                    .transition(None::<String>, CardState::Draft.name()),
                clock,
            )?;
            control.commit(expected, &format!("card: create {}", draft.card_id))?;

            Ok(CommandOutcome::new(
                "card.create",
                format!(
                    "Created draft for card {} in cycle {}",
                    draft.card_id, draft.cycle_id
                ),
                serde_json::json!({
                    "card_id": draft.card_id.to_string(),
                    "cycle_id": draft.cycle_id.to_string(),
                    "state": CardState::Draft.name(),
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}

/// Reports what `card activate` would do, without doing it.
///
/// Runs every check the real activation makes, in the same order. This preview
/// used to validate the draft and stop, so it reported that a card would
/// activate when its write scope overlapped an active card's — the one refusal
/// `card activate` exists to make.
fn preview_activate(
    args: &ActivateArgs,
    card_id: &CardId,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.common.control)?;
    // Every check the real activation makes, in the same order. This
    // preview validated the draft and stopped, so it reported that a card
    // would activate when its write scope overlapped an active card's —
    // the one refusal `card activate` exists to make.
    if let Some(state) = state_of(&control, card_id)? {
        return Err(HarnessError::Control {
            reason: format!(
                "card {card_id} is already activated at revision {}; an activated card is immutable, use `card revise`",
                state.current_revision
            ),
            code: ErrorCode::PolicyInvalidCard,
        });
    }
    let draft = stored_draft(&control, card_id)?;
    draft.validate()?;
    let cycle = cycle_accepting_cards(&control, &draft)?;
    require_cycle_baseline(&cycle, &draft)?;
    let config = control.project()?;
    let preview = CardRecord::activate(&draft, 1, &args.common.actor, clock.now())?;
    require_declared_proof(&config.validation_policy, &preview)?;
    require_registered(
        &control,
        preview
            .named_gates
            .feature
            .iter()
            .chain(&preview.named_gates.review)
            .chain(&preview.named_gates.integration),
    )?;
    check_allocation(&control, &cycle, &preview)?;
    Ok(CommandOutcome::new(
        "card.activate",
        format!(
            "Dry run: would activate card {card_id} at revision 1 with digest {}; nothing was changed",
            preview.digest()?
        ),
        serde_json::json!({
            "dry_run": true,
            "card_id": card_id.to_string(),
            "revision": 1,
            "digest": preview.digest()?.as_str(),
        }),
    ))
}

fn run_activate(args: &ActivateArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;

    if args.dry_run {
        return preview_activate(args, &card_id, clock);
    }

    with_transaction(
        &args.common.control,
        "card.activate",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            if let Some(state) = state_of(control, &card_id)? {
                return Err(HarnessError::Control {
                    reason: format!(
                        "card {card_id} is already activated at revision {}; an activated card is immutable, use `card revise`",
                        state.current_revision
                    ),
                    code: ErrorCode::PolicyInvalidCard,
                });
            }
            let draft = stored_draft(control, &card_id)?;
            let cycle = cycle_accepting_cards(control, &draft)?;
            require_cycle_baseline(&cycle, &draft)?;
            let config = control.project()?;

            let record = CardRecord::activate(&draft, 1, &args.common.actor, clock.now())?;
            require_declared_proof(&config.validation_policy, &record)?;
            // A card may only name gates the registry defines (D-008). Checked
            // at activation so an undefined check fails now rather than at the
            // point its evidence was supposed to exist.
            require_registered(
                control,
                record
                    .named_gates
                    .feature
                    .iter()
                    .chain(&record.named_gates.review)
                    .chain(&record.named_gates.integration),
            )?;
            // Ownership, contract, resource, and dependency checks run before
            // anything is written, so a refused card leaves no trace.
            check_allocation(control, &cycle, &record)?;
            let digest = record.digest()?;
            write_revision(control, &record, &digest, CardState::Ready)?;

            // The cycle records its membership, which is how overlap checks in
            // WP-220 find the cards to compare against.
            let mut updated = cycle;
            updated.declare_card(card_id.clone())?;
            control.write_atomic(
                &CycleRecord::relative_path(&updated.cycle_id),
                &format!("{}\n", serde_json::to_string_pretty(&updated)?),
            )?;

            events.append(
                &config.project_id,
                EventDraft::new("card.activated", &args.common.actor)
                    .cycle(updated.cycle_id.clone())
                    .card(card_id.clone(), 1, digest.clone())
                    .transition(Some(CardState::Draft.name()), CardState::Ready.name()),
                clock,
            )?;
            control.commit(expected, &format!("card: activate {card_id} r1"))?;

            // Leading signal, asked while the answer is still cheap. The card
            // is already written and activation has already succeeded; this
            // only decides whether to say something about its breadth.
            let breadth = ScopeBreadth::measure(&record.write_scope.include);
            let mut outcome = CommandOutcome::new(
                "card.activate",
                format!(
                    "Activated card {card_id} at revision 1\ndigest: {digest}\nstate: {}",
                    CardState::Ready
                ),
                serde_json::json!({
                    "card_id": card_id.to_string(),
                    "revision": 1,
                    "digest": digest.as_str(),
                    "state": CardState::Ready.name(),
                    "scope_paths": breadth.paths,
                    "scope_areas": breadth.areas,
                }),
            )
            .with_project(config.project_id.clone());
            if let Some(advisory) = breadth.advisory() {
                outcome = outcome.with_warning(advisory);
            }
            Ok(outcome)
        },
    )
}

/// Writes an immutable revision and updates the mutable state pointer.
fn write_revision(
    control: &ControlRepository,
    record: &CardRecord,
    digest: &Digest,
    state: CardState,
) -> Result<(), HarnessError> {
    let relative = CardRecord::relative_path(&record.card_id, record.revision);
    if control.path(&relative).exists() {
        return Err(HarnessError::Control {
            reason: format!(
                "revision {} of card {} already exists; an activated revision is never rewritten",
                record.revision, record.card_id
            ),
            code: ErrorCode::PolicyInvalidCard,
        });
    }
    control.write_atomic(
        &relative,
        &format!("{}\n", serde_json::to_string_pretty(record)?),
    )?;
    control.write_atomic(
        &CardStateRecord::relative_path(&record.card_id),
        &format!(
            "{}\n",
            serde_json::to_string_pretty(&CardStateRecord {
                schema: CARD_STATE_SCHEMA.to_owned(),
                card_id: record.card_id.clone(),
                state,
                current_revision: record.revision,
                current_digest: digest.clone(),
                canonical_algorithm: CardRecord::canonical_algorithm().to_owned(),
            })?
        ),
    )
}

/// Whether a revision's canonical fields differ enough from the one it
/// supersedes to force a reviewer to look again.
///
/// 71-R5: `material_scope_revisions` is not "every revision". Correcting a
/// typo in the title is not a scope revision, and counting it would exhaust
/// the budget on administrative work. Material is exactly what changes:
///
/// - `write_scope` (`include` or `exclude`);
/// - `acceptance.behaviors` or `acceptance.regressions`;
/// - `depends_on`;
/// - `base_sha`.
///
/// Title, goal, non-goals, `review_policy`, `rollback_strategy`,
/// `named_gates`, `risk`, `change_kind`, and every other field neither line
/// names are deliberately not compared here. Each revision still gets its own
/// new `card_digest`, and comparing that instead would make every edit
/// material — the same defect under a different name: a budget that always
/// empties on the first look is not a budget.
fn is_material_scope_revision(previous: &CardRecord, next: &CardRecord) -> bool {
    previous.write_scope != next.write_scope
        || previous.acceptance != next.acceptance
        || previous.depends_on != next.depends_on
        || previous.base_sha != next.base_sha
}

/// Reports a policy-equivalent revision without writing a new record.
fn preview_revise(
    args: &ReviseArgs,
    card_id: &CardId,
    draft: &CardDraft,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.common.control)?;
    let state = state_of(&control, card_id)?.ok_or_else(|| HarnessError::Control {
        reason: format!("card {card_id} is not activated"),
        code: ErrorCode::PreconditionNotFound,
    })?;
    // Loaded for the same reason `run_revise` loads it: materiality, below,
    // compares the previous revision's canonical fields against the new
    // one's. A preview that skipped this and always reported "not material"
    // would be exactly the preview/reality disagreement `preview_record`
    // already documents elsewhere in this codebase — see the "Paridad de
    // preview" note on 71-R5.
    let previous: CardRecord = serde_json::from_str(
        &control.read(&CardRecord::relative_path(card_id, state.current_revision))?,
    )
    .map_err(|source| HarnessError::Control {
        reason: format!("card {card_id} revision record is malformed: {source}"),
        code: ErrorCode::InternalControlCorrupt,
    })?;
    let config = control.project()?;
    // 72-3: same placement as `run_revise` — right after the current
    // revision's record and the project config are both available, before
    // anything else, so a preview can never promise a revision the real
    // command would refuse for an escalated card. See
    // `require_convergence_budget`.
    require_convergence_budget(&control, &config, &previous)?;
    let preview = CardRecord::activate(
        draft,
        state.current_revision + 1,
        &args.common.actor,
        clock.now(),
    )?;
    require_declared_proof(&config.validation_policy, &preview)?;
    // The same verdict the real command reaches: a fact is recorded only when
    // a policy is configured *and* the revision is material. Reported below
    // as `material_scope_revision_recorded`, not `material_scope_revision`:
    // the field states whether a fact would be *written*, not whether the
    // revision is material on its own. Under no configured policy it reads
    // `false` even for a revision that widens the write scope, because
    // nothing would be recorded — a consumer must not read `false` here as
    // "this revision was not material".
    let material_scope_revision =
        config.convergence_policy.is_some() && is_material_scope_revision(&previous, &preview);
    Ok(CommandOutcome::new(
        "card.revise",
        format!(
            "Dry run: would supersede card {card_id} revision {} with revision {}{}; nothing was changed",
            state.current_revision,
            state.current_revision + 1,
            if material_scope_revision {
                "\nwould record one material_scope_revision convergence fact"
            } else {
                ""
            }
        ),
        serde_json::json!({
            "dry_run": true,
            "card_id": card_id.to_string(),
            "superseded_revision": state.current_revision,
            "material_scope_revision_recorded": material_scope_revision,
        }),
    ))
}

// 71-R5's materiality check and its fact emission both have to run inside
// this one transaction, after `card.revised` is appended and before the
// commit — splitting either into a helper the length limit would otherwise
// invite risks losing that ordering at a call site instead of at a glance.
#[allow(clippy::too_many_lines)]
fn run_revise(args: &ReviseArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;
    let draft = read_draft(&args.draft)?;
    draft.validate()?;

    if args.dry_run {
        return preview_revise(args, &card_id, &draft, clock);
    }

    with_transaction(
        &args.common.control,
        "card.revise",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let state = state_of(control, &card_id)?.ok_or_else(|| HarnessError::Control {
                reason: format!("card {card_id} is not activated; use `card activate`"),
                code: ErrorCode::PreconditionNotFound,
            })?;
            if !state.state.is_revisable() {
                return Err(HarnessError::Control {
                    reason: format!(
                        "card {card_id} is `{}` and cannot be revised; a revision returns a card to `ready`, which from here would strand it",
                        state.state
                    ),
                    code: ErrorCode::PolicyInvalidTransition,
                });
            }

            let previous: CardRecord = serde_json::from_str(
                &control.read(&CardRecord::relative_path(&card_id, state.current_revision))?,
            )
            .map_err(|source| HarnessError::Control {
                reason: format!("card {card_id} revision record is malformed: {source}"),
                code: ErrorCode::InternalControlCorrupt,
            })?;

            let config = control.project()?;
            // 72-3: the first check able to refuse once the card record and
            // project config both exist — a revision returns the card to
            // `ready`, which would otherwise be the one route left able to
            // dodge the escalation. 71-R1 already made the convergence
            // counters span revisions, so revising does not reset the
            // budget; what was still missing is that revising itself must
            // not be permitted at all. See `require_convergence_budget`.
            require_convergence_budget(control, &config, &previous)?;
            let record = previous.revise(&draft, &args.common.actor, clock.now())?;
            require_declared_proof(&config.validation_policy, &record)?;
            // A revision may widen the write scope, so allocation is re-checked
            // rather than assumed to still hold from activation.
            let cycle: CycleRecord =
                serde_json::from_str(&control.read(&CycleRecord::relative_path(&record.cycle_id))?)
                    .map_err(|source| HarnessError::Control {
                        reason: format!("cycle {} is malformed: {source}", record.cycle_id),
                        code: ErrorCode::InternalControlCorrupt,
                    })?;
            check_allocation(control, &cycle, &record)?;
            let digest = record.digest()?;
            // A revision returns the card to `ready`: its definition changed, so
            // any work, handoff, or review bound to the old digest no longer
            // describes it. Invariant 7.3.4.
            write_revision(control, &record, &digest, CardState::Ready)?;

            events.append(
                &config.project_id,
                EventDraft::new("card.revised", &args.common.actor)
                    .cycle(record.cycle_id.clone())
                    .card(card_id.clone(), record.revision, digest.clone())
                    .transition(Some(state.state.name()), CardState::Ready.name())
                    .meta("reason", serde_json::json!(args.reason))
                    .meta(
                        "invalidated_revision",
                        serde_json::json!(state.current_revision),
                    )
                    .meta(
                        "invalidated_digest",
                        serde_json::json!(state.current_digest.as_str()),
                    ),
                clock,
            )?;

            // 71-R5: a material scope revision, under a configured convergence
            // policy, records exactly one bound `convergence.attempt_recorded`
            // fact in the same transaction as `card.revised` — #9's ledger,
            // written where the revision itself is written rather than
            // reconstructed later from card history. `AttemptKind::
            // MaterialScopeRevision` admits only `ReasonCategory::ScopeChange`
            // (see `AttemptKind::admits`), so there is nothing for the harness
            // to infer or the operator to declare here. `head_sha` binds to
            // `record.base_sha`, the new revision's own declared base: `draft`
            // was already validated — before this transaction, and before the
            // dry-run branch too, see the top of `run_revise` — which refuses a
            // `base_sha` that is not a full 40-character object id, so this
            // binding is already exact by the time either path reaches here.
            let material_scope_revision = is_material_scope_revision(&previous, &record);
            let mut recorded_material_scope_fact = false;
            if material_scope_revision && let Some(policy) = config.convergence_policy.as_ref() {
                let policy_digest = policy.digest()?;
                events.append(
                    &config.project_id,
                    EventDraft::new(ATTEMPT_RECORDED_EVENT, &args.common.actor)
                        .cycle(record.cycle_id.clone())
                        .card(card_id.clone(), record.revision, digest.clone())
                        .head(record.base_sha.clone())
                        .meta(
                            "attempt_kind",
                            serde_json::to_value(AttemptKind::MaterialScopeRevision)?,
                        )
                        .meta(
                            "reason_category",
                            serde_json::to_value(ReasonCategory::ScopeChange)?,
                        )
                        .meta(
                            "evidence_ref",
                            serde_json::json!(format!(
                                "card-revision:{card_id}@{}",
                                record.revision
                            )),
                        )
                        .meta("policy_digest", serde_json::json!(policy_digest.as_str())),
                    clock,
                )?;
                recorded_material_scope_fact = true;
            }

            control.commit(
                expected,
                &format!("card: revise {card_id} to r{}", record.revision),
            )?;

            // `material_scope_revision_recorded`, not `material_scope_revision`:
            // this reports whether the fact above was actually written, not
            // whether the revision was material on its own. It reads `false`
            // whenever no policy is configured, however material the revision
            // was, because there was nothing to record.
            Ok(CommandOutcome::new(
                "card.revise",
                format!(
                    "Revised card {card_id} to revision {}\ndigest: {digest}\nsuperseded revision {} with digest {}\nany handoff, review, or receipt bound to the superseded digest is now stale{}",
                    record.revision,
                    state.current_revision,
                    state.current_digest,
                    if recorded_material_scope_fact {
                        "\nrecorded one material_scope_revision convergence fact"
                    } else {
                        ""
                    }
                ),
                serde_json::json!({
                    "card_id": card_id.to_string(),
                    "revision": record.revision,
                    "digest": digest.as_str(),
                    "superseded_revision": state.current_revision,
                    "superseded_digest": state.current_digest.as_str(),
                    "state": CardState::Ready.name(),
                    "material_scope_revision_recorded": recorded_material_scope_fact,
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}

fn run_abandon(args: &AbandonArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;

    if args.dry_run {
        let control = ControlRepository::open(&args.common.control)?;
        let (_, state) = load_card(&control, &card_id)?;
        state.state.check_transition(CardState::Abandoned)?;
        return Ok(CommandOutcome::new(
            "card.abandon",
            format!("Dry run: would abandon card {card_id}; nothing was changed"),
            serde_json::json!({ "dry_run": true, "card_id": card_id.to_string() }),
        ));
    }

    with_transaction(
        &args.common.control,
        "card.abandon",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let (record, state) = load_card(control, &card_id)?;
            let previous = state.state;
            previous.check_transition(CardState::Abandoned)?;

            let config = control.project()?;
            store_card_state(control, &record, &state, CardState::Abandoned)?;
            events.append(
                &config.project_id,
                EventDraft::new("card.abandoned", &args.common.actor)
                    .cycle(record.cycle_id.clone())
                    .card(
                        card_id.clone(),
                        state.current_revision,
                        state.current_digest.clone(),
                    )
                    .transition(Some(previous.name()), CardState::Abandoned.name())
                    .meta("reason", serde_json::json!(args.reason)),
                clock,
            )?;
            control.commit(expected, &format!("card: abandon {card_id}"))?;

            Ok(CommandOutcome::new(
                "card.abandon",
                format!("Abandoned card {card_id}\nreason: {}", args.reason),
                serde_json::json!({
                    "card_id": card_id.to_string(),
                    "state": CardState::Abandoned.name(),
                    "reason": args.reason,
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}

// 72-3: the convergence report below has to sit inside this one function,
// alongside the digest-integrity report already here — both are read-only
// facts about the same loaded `record`, and splitting either into a helper
// the length limit would otherwise invite risks losing the "this command
// never refuses for convergence" property at a call site instead of at a
// glance.
#[allow(clippy::too_many_lines)]
fn run_status(args: &StatusArgs) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;
    let control = ControlRepository::open(&args.common.control)?;
    let config = control.project()?;

    let Some(state) = state_of(&control, &card_id)? else {
        // A stored draft with no state is a legitimate pre-activation card.
        let draft = stored_draft(&control, &card_id)?;
        return Ok(CommandOutcome::new(
            "card.status",
            format!(
                "Card {card_id} is a draft in cycle {}\nnot yet activated, so it has no digest",
                draft.cycle_id
            ),
            serde_json::json!({
                "card_id": card_id.to_string(),
                "state": CardState::Draft.name(),
                "cycle_id": draft.cycle_id.to_string(),
                "revision": serde_json::Value::Null,
                "digest": serde_json::Value::Null,
            }),
        )
        .with_project(config.project_id.clone()));
    };

    let record: CardRecord = serde_json::from_str(
        &control.read(&CardRecord::relative_path(&card_id, state.current_revision))?,
    )
    .map_err(|source| HarnessError::Control {
        reason: format!("card {card_id} revision record is malformed: {source}"),
        code: ErrorCode::InternalControlCorrupt,
    })?;

    // Recomputing rather than trusting the stored value: the digest is what
    // every downstream record binds to, so a mismatch means the immutable
    // record was edited and must not pass silently.
    let recomputed = record.digest()?;
    let intact = recomputed == state.current_digest;

    // 72-3: `card status` is the operator's window into why a card stopped,
    // so unlike every blocked command above it never turns an escalated
    // budget into a refusal — see `card_convergence`. `data.convergence` is
    // exactly `CardConvergence`'s own serialization, the same shape
    // `require_convergence_budget` decides from, so the JSON this command
    // publishes and the JSON that decides a block elsewhere can never
    // disagree.
    let convergence = card_convergence(&control, &config, &record)?;

    let mut text = format!(
        "Card {card_id}\ntitle: {}\ncycle: {}\nstate: {}\nrevision: {}\ndigest: {recomputed}\nrisk: {}",
        record.title,
        record.cycle_id,
        state.state,
        state.current_revision,
        record.risk.name()
    );
    // The human text says the same thing the JSON does, not less: an
    // operator reading text output must not have to ask for JSON to learn
    // which dimension is exhausted or what they may do next.
    if let CardConvergence::Escalated {
        exhausted,
        next_permitted_action,
    } = &convergence
    {
        let _ = write!(text, "\nconvergence: escalated");
        for dimension in exhausted {
            let _ = write!(
                text,
                "\n  {}: {}/{} (evidence: {})",
                dimension_wire_name(dimension.dimension),
                dimension.count,
                dimension.limit,
                dimension
                    .evidence
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        let _ = write!(
            text,
            "\n  next permitted action: {}",
            next_permitted_action_wire_name(*next_permitted_action)
        );
    }

    let mut outcome = CommandOutcome::new(
        "card.status",
        text,
        serde_json::json!({
            "card_id": card_id.to_string(),
            "cycle_id": record.cycle_id.to_string(),
            "state": state.state.name(),
            "revision": state.current_revision,
            "digest": recomputed.as_str(),
            "recorded_digest": state.current_digest.as_str(),
            "digest_intact": intact,
            "canonical_algorithm": state.canonical_algorithm,
            "risk": record.risk.name(),
            "title": record.title,
            "convergence": convergence,
        }),
    )
    .with_project(config.project_id.clone());

    if !intact {
        outcome = outcome.with_warning(format!(
            "recomputed digest {recomputed} does not match the recorded {}; the immutable revision was altered outside the harness",
            state.current_digest
        ));
    }
    Ok(outcome)
}
