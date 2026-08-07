//! Independent review commands.

use std::{fmt::Write as _, fs, path::PathBuf};

use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

use crate::{
    cli::output::CommandOutcome,
    commands::CONTROL_ENV,
    commands::{
        card::{load_card, require_convergence_budget, store_card_state},
        handoff::{ancestry, latest_handoff},
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
        digest::CANONICAL_ALGORITHM,
        handoff::{DEPENDENCIES_NOT_CHECKED, DependencyBinding, DependencyStanding, HandoffStatus},
        ids::{CardId, ReviewId},
        review::{
            Decision, Disposition, Finding, FindingSeverity, GateAdequacy, MutationEvidence,
            REVIEW_DIR, REVIEW_SCHEMA, ReviewRecord, check_independence,
        },
    },
    error::{ErrorCode, HarnessError},
    git::{command::GitScope, inspect},
    policy::convergence::{ATTEMPT_RECORDED_EVENT, AttemptKind, ReasonCategory, Round, Trend},
};

/// Subcommands under `review`.
#[derive(Debug, Subcommand)]
pub enum ReviewCommand {
    /// Assign a card for review and emit the reviewer's packet.
    Begin(BeginArgs),
    /// Record a reviewer's decision.
    Record(RecordArgs),
    /// Show a card's review history and whether the latest still applies.
    Inspect(CardArgs),
    /// Print a complete, valid verdict document.
    ///
    /// Built by constructing a real verdict and serializing it, so this can
    /// never disagree with what `review record` accepts — see #108.
    Example(ExampleArgs),
}

impl ReviewCommand {
    /// Its dotted command path, as the result envelope reports it.
    ///
    /// The error envelope used to carry only the group — `review` — while a
    /// success carried the full path, so a consumer matching on `command` got a
    /// different granularity depending on whether the command worked.
    #[must_use]
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Begin(..) => "review.begin",
            Self::Record(..) => "review.record",
            Self::Inspect(..) => "review.inspect",
            Self::Example(..) => "review.example",
        }
    }
}

/// Arguments shared by review subcommands.
#[derive(Debug, Args)]
pub struct CommonArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
    pub control: PathBuf,
    /// Identifies the acting party. Declared, not proven; see D-013.
    #[arg(long, default_value = "operator")]
    pub actor: String,
}

/// Arguments accepted by `review begin`.
#[derive(Debug, Args)]
pub struct BeginArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The card to review.
    #[arg(long)]
    pub card_id: String,
    /// Report planned mutations without performing them.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `review record`.
#[derive(Debug, Args)]
pub struct RecordArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The card being reviewed.
    #[arg(long)]
    pub card_id: String,
    /// Path to the reviewer's verdict, in YAML or JSON.
    #[arg(long)]
    pub verdict: PathBuf,
    /// Report planned mutations without performing them.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments naming only a card.
#[derive(Debug, Args)]
pub struct CardArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The card to report on.
    #[arg(long)]
    pub card_id: String,
}

/// Arguments accepted by `review example`.
///
/// Deliberately empty, and deliberately not [`CommonArgs`]: #108 constraint 1
/// requires this to be reachable before an operator has a control repository,
/// a project, or a card to point it at, so it names neither.
#[derive(Debug, Args)]
pub struct ExampleArgs {}

/// The reviewer-authored half of a review.
///
/// Deliberately separate from [`ReviewRecord`]: the reviewer supplies judgment,
/// and the harness supplies the bindings. A reviewer cannot state which
/// candidate they reviewed, because that is exactly the field that must not be
/// taken on trust.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Verdict {
    /// Who reviewed.
    pub reviewer_actor_id: String,
    /// The conclusion.
    pub decision: Decision,
    /// Why the work is being returned.
    ///
    /// Declared, not derived. A [`Finding`] carries `severity`, `location`,
    /// `detail`, and `disposition` — none of which distinguishes an
    /// acceptance defect from a regression from a security concern, so there
    /// is no field here a harness could read that fact off of. This mirrors a
    /// rule this codebase already keeps for [`ExceptionTrigger`](crate::config::ExceptionTrigger):
    /// "the harness never infers that an exception exists from a failed check
    /// or a conversation; a configured actor must raise one of these declared
    /// reasons with evidence." A review return is the same shape of claim —
    /// so the reviewer states why, and the harness only checks that the
    /// stated reason is one [`AttemptKind::ReviewReturn`](crate::policy::convergence::AttemptKind::ReviewReturn)
    /// admits.
    ///
    /// Required, and validated against that admission rule, only when
    /// `decision` is `changes_requested` under a project with a configured
    /// convergence policy; see `require_review_return_reason`. Omitted here
    /// is fine for every other verdict shape, which is what keeps every
    /// existing fixture, written before this field existed, still valid.
    #[serde(default)]
    pub reason_category: Option<ReasonCategory>,
    /// What the reviewer found.
    #[serde(default)]
    pub findings: Vec<Finding>,
    /// Whether the gates observe the acceptance list.
    pub gate_adequacy: GateAdequacy,
    /// Risks accepted if this is an approval.
    #[serde(default)]
    pub residual_risks: Vec<String>,
    /// Whether a human performed this review.
    ///
    /// Section 15.3 requires one for `high` and `critical` cards. Declared,
    /// never proven — D-013 makes every identity here a claim — so this is
    /// recorded on the review and refusable, in the same way and with the same
    /// limits as `check_independence`. It catches the omission, not the liar.
    #[serde(default)]
    pub human_reviewer: bool,
}

/// Executes a `review` subcommand.
///
/// # Errors
///
/// Returns a policy or precondition error as appropriate.
pub fn execute(command: &ReviewCommand, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    match command {
        ReviewCommand::Begin(args) => run_begin(args, clock),
        ReviewCommand::Record(args) => run_record(args, clock),
        ReviewCommand::Inspect(args) => run_inspect(args),
        ReviewCommand::Example(..) => run_example(),
    }
}

/// Allocates the next review identifier.
fn next_review_id(control: &ControlRepository) -> Result<ReviewId, HarnessError> {
    let directory = control.path(REVIEW_DIR);
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
                    .and_then(|stem| stem.strip_prefix("RV-"))
                    .and_then(|digits| digits.parse::<u64>().ok())
            })
            .max()
            .unwrap_or(0)
    } else {
        0
    };
    format!("RV-{:06}", highest + 1).parse()
}

/// Orders a record identifier by its trailing number rather than as text.
///
/// Identifiers are allocated `RV-000001`, `RV-000002`, and so on, and are
/// zero-padded to six digits — so sorting them as text is chronological right
/// up to `RV-999999`, and wrong immediately after it, because `RV-1000000`
/// sorts before `RV-999999`. Round 4 of `F-029`'s review reached that boundary
/// with the real allocator and watched a chronological `3 → 2 → 1` arrive as
/// `1 → 3 → 2`, which reads as a card that is not converging.
///
/// Returns `None` for a name with no trailing digits, which sorts first and
/// then by name — there is no such identifier today, and inventing an order
/// for one is not this function's business.
fn ordinal(name: &str) -> Option<u128> {
    let start = name
        .rfind(|c: char| !c.is_ascii_digit())
        .map_or(0, |i| i + 1);
    name.get(start..)
        .filter(|rest| !rest.is_empty())?
        .parse()
        .ok()
}

/// Orders record identifiers oldest first.
///
/// A named function rather than a closure at the call site so that a test can
/// bind to the ordering the reader actually uses. The first version of that
/// test repeated the comparator inline and would have passed against a plain
/// lexical sort — it certified a copy of the code instead of the code.
fn sort_oldest_first(names: &mut [String]) {
    names.sort_by(|left, right| {
        ordinal(left)
            .cmp(&ordinal(right))
            .then_with(|| left.cmp(right))
    });
}

/// Every review recorded for one card, oldest first.
///
/// "Oldest first" is load-bearing: `Trend` reads these rounds in order and
/// reports whether the card is settling, so an ordering that is merely usually
/// chronological would make the signal quietly wrong rather than absent.
///
/// # Errors
///
/// Returns an error when the store cannot be read.
pub fn reviews_for(
    control: &ControlRepository,
    card_id: &CardId,
) -> Result<Vec<ReviewRecord>, HarnessError> {
    let directory = control.path(REVIEW_DIR);
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
    sort_oldest_first(&mut names);

    let mut reviews = Vec::new();
    for name in names {
        let raw = control.read(&format!("{REVIEW_DIR}/{name}.json"))?;
        let review: ReviewRecord =
            serde_json::from_str(&raw).map_err(|source| HarnessError::Control {
                reason: format!("review {name} is malformed: {source}"),
                code: ErrorCode::InternalControlCorrupt,
            })?;
        if review.card_id == *card_id {
            reviews.push(review);
        }
    }
    Ok(reviews)
}

/// The most recent approval that still describes the current candidate.
///
/// Each candidate approval is judged against its own dependency bindings, so
/// the standings are resolved per review rather than once for the card.
///
/// # Errors
///
/// Returns an error when the store cannot be read.
pub fn current_approval(
    control: &ControlRepository,
    scope: &GitScope,
    card_id: &CardId,
    candidate_sha: &str,
    card_digest: &crate::domain::digest::Digest,
    depends_on: &[CardId],
) -> Result<Option<ReviewRecord>, HarnessError> {
    for review in reviews_for(control, card_id)?.into_iter().rev() {
        if review.decision != Decision::Approved {
            continue;
        }
        let standings =
            dependency_standings(control, scope, depends_on, &review.dependency_bindings)?;
        if review.is_current_for(candidate_sha, card_digest, &standings) {
            return Ok(Some(review));
        }
    }
    Ok(None)
}

/// The card's standing verdict, read from the store.
///
/// The rule lives on [`ReviewRecord::standing_approval`], which is where its
/// unit test can reach it. It deliberately does not ask whether the approval
/// still describes the card's current candidate: it answers "which commit of
/// this dependency was blessed", and an approval whose own candidate has since
/// moved still answers that correctly. The dependency's own staleness is what
/// stops it being integrated, and applying it here would refuse a dependent for
/// a fact about a card it is claiming nothing about.
///
/// # Errors
///
/// Returns an error when the store cannot be read.
pub fn standing_approval(
    control: &ControlRepository,
    card_id: &CardId,
) -> Result<Option<ReviewRecord>, HarnessError> {
    Ok(ReviewRecord::standing_approval(&reviews_for(control, card_id)?).cloned())
}

/// Where each declared dependency stands against one record's bindings.
///
/// The containment question — does the dependency's standing approval contain
/// the commit this record bound — is asked of Git here, because a record cannot
/// carry an ancestry fact about a commit that did not exist when it was
/// written.
///
/// An object Git does not have is reported as *contained*. A dependency's
/// candidate can become unreachable once its branch is deleted and its objects
/// collected, and voiding a dependent's approval because an unrelated card's
/// history aged out would refuse for a reason that is not about supersession.
/// The direction of that choice is deliberate: this check exists to catch a
/// rewrite it can see, not to demand proof of innocence.
///
/// # Errors
///
/// Returns an error when the store cannot be read or Git cannot be run.
pub fn dependency_standings(
    control: &ControlRepository,
    scope: &GitScope,
    depends_on: &[CardId],
    bindings: &[DependencyBinding],
) -> Result<Vec<DependencyStanding>, HarnessError> {
    let mut standings = Vec::with_capacity(depends_on.len());
    for card_id in depends_on {
        let approved_candidate_sha =
            standing_approval(control, card_id)?.map(|review| review.candidate_sha);
        let bound = bindings
            .iter()
            .find(|binding| binding.card_id == *card_id)
            .and_then(|binding| binding.incorporated_sha.as_deref());
        let approval_contains_binding = match (bound, approved_candidate_sha.as_deref()) {
            (Some(bound), Some(approved)) => ancestry(scope, bound, approved)?.unwrap_or(true),
            _ => true,
        };
        standings.push(DependencyStanding {
            card_id: card_id.clone(),
            approved_candidate_sha,
            approval_contains_binding,
        });
    }
    Ok(standings)
}

fn run_begin(args: &BeginArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;

    if args.dry_run {
        let control = ControlRepository::open(&args.common.control)?;
        let config = control.project()?;
        let (record, state) = load_card(&control, &card_id)?;
        // The dry run must ask this too: a preview that skips a check the
        // real command enforces is worse than no preview, per
        // `preview_record`'s "Tier 3 defect 24" note.
        require_convergence_budget(&control, &config, &record)?;
        state.state.check_transition(CardState::ReviewPending)?;
        return Ok(CommandOutcome::new(
            "review.begin",
            format!("Dry run: would open review for card {card_id}; nothing was changed"),
            serde_json::json!({ "dry_run": true, "card_id": card_id.to_string() }),
        ));
    }

    with_transaction(
        &args.common.control,
        "review.begin",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let config = control.project()?;
            let (record, state) = load_card(control, &card_id)?;
            // 72-2: the first check that can refuse, before anything is
            // written — see `require_convergence_budget`.
            require_convergence_budget(control, &config, &record)?;
            state.state.check_transition(CardState::ReviewPending)?;

            let handoff =
                latest_handoff(control, &card_id)?.ok_or_else(|| HarnessError::Control {
                    reason: format!("card {card_id} has no handoff to review"),
                    code: ErrorCode::PreconditionNotFound,
                })?;
            if handoff.status == HandoffStatus::Revoked {
                return Err(HarnessError::Control {
                    reason: format!("handoff {} was revoked", handoff.handoff_id),
                    code: ErrorCode::PolicyInvalidTransition,
                });
            }

            store_card_state(control, &record, &state, CardState::ReviewPending)?;
            events.append(
                &config.project_id,
                EventDraft::new("review.begun", &args.common.actor)
                    .cycle(record.cycle_id.clone())
                    .card(
                        card_id.clone(),
                        state.current_revision,
                        state.current_digest.clone(),
                    )
                    .transition(Some(state.state.name()), CardState::ReviewPending.name())
                    .head(handoff.candidate_sha.clone())
                    .meta("handoff_id", serde_json::json!(handoff.handoff_id)),
                clock,
            )?;
            control.commit(expected, &format!("review: begin {card_id}"))?;

            // The packet is the handoff plus the card. Section 15.1 lists what a
            // reviewer must receive; emitting it here means the reviewer never
            // has to go looking, which is what keeps their context bounded.
            Ok(CommandOutcome::new(
                "review.begin",
                format!(
                    "Review open for card {card_id}\ncandidate: {}\nhandoff: {}\nfeature actor: {}\nthe reviewer must be a different actor in a fresh context",
                    handoff.candidate_sha, handoff.handoff_id, handoff.actor_id
                ),
                serde_json::json!({
                    "card_id": card_id.to_string(),
                    "state": CardState::ReviewPending.name(),
                    "packet": {
                        "card": record,
                        "card_digest": state.current_digest.as_str(),
                        "handoff": handoff,
                        "evaluation_criteria": EVALUATION_CRITERIA,
                    },
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}

/// The Section 15.1 evaluation list, emitted with every review packet.
///
/// Included in the packet rather than assumed, because `SPIKE-001` showed the
/// reviewer working straight down this list, and criterion 6 in particular is
/// what found the seeded defect.
pub const EVALUATION_CRITERIA: [&str; 10] = [
    "requirement fidelity",
    "architecture and responsibility boundaries",
    "public API, schema, and persistence compatibility",
    "error, timeout, concurrency, and partial-state paths",
    "negative and boundary cases",
    "whether the tests could pass while the behavior remains wrong",
    "unnecessary dependencies or complexity",
    "security, privacy, logging, and audit implications",
    "deterministic generated changes",
    "maintainability by another human or agent",
];

/// #142: distinct call site from the parse below, so the read failure needs
/// no introspection to tell apart from a schema failure.
const VERDICT_READ_RECOVERY: &str = "This is a read failure, not a syntax problem: the verdict file above could not be opened. Confirm the path exists, is spelled correctly, and is readable by this process.";

/// `serde_yaml_ng` (v0.10.0) exposes no public equivalent of
/// `serde_json::Error::classify()` — see `GATE_DEFINITION_PARSE_RECOVERY` in
/// `src/commands/gate.rs` for the full finding. Same choice here: one
/// recovery, honest for both a syntax and a schema failure, naming `review
/// example` either way.
const VERDICT_PARSE_RECOVERY: &str = "This verdict could not be parsed as YAML, or it parsed but does not match the schema; the message above names the position or the field. Compare it against `review example`'s output, a complete, valid verdict.";

/// Reads and parses a reviewer's verdict.
fn read_verdict(path: &PathBuf) -> Result<Verdict, HarnessError> {
    let raw = fs::read_to_string(path).map_err(|source| HarnessError::ControlWithRecovery {
        reason: format!("cannot read verdict {}: {source}", path.display()),
        code: ErrorCode::ConfigMalformed,
        recovery: VERDICT_READ_RECOVERY,
    })?;
    serde_yaml_ng::from_str(&raw).map_err(|source| HarnessError::ControlWithRecovery {
        reason: format!("verdict is malformed: {source}"),
        code: ErrorCode::ConfigMalformed,
        recovery: VERDICT_PARSE_RECOVERY,
    })
}

/// A complete, valid review verdict, for `review example` to emit.
///
/// Every field is populated, including the ones [`Verdict::reason_category`],
/// [`Verdict::findings`], [`Verdict::residual_risks`], and
/// [`Verdict::human_reviewer`] make optional — an example that omits a field
/// teaches a reader nothing about its shape, which is the exact gap #108
/// exists to close: a finding's prose field being `detail` and not `summary`
/// is invisible in an example that carries no finding at all. The one field
/// left at its empty value is `reason_category`: it answers "why is this
/// being returned", and this example is an approval, which returns nothing to
/// answer for. See [`Verdict::reason_category`] for when it becomes required.
///
/// `gate_adequacy.mutation_evidence` is shown as [`MutationEvidence::Demonstrated`]
/// — the ordinary case — describing the same finding the example already
/// carries, so a reader sees one coherent story rather than two unrelated
/// fixtures. See [`GateAdequacy::mutation_evidence`] for
/// [`MutationEvidence::Exempt`], the declared-exemption case this example
/// does not need.
fn example_verdict() -> Verdict {
    Verdict {
        reviewer_actor_id: "reviewer-example".to_owned(),
        decision: Decision::Approved,
        reason_category: None,
        findings: vec![Finding {
            severity: FindingSeverity::Medium,
            location: "src/example.rs:42".to_owned(),
            detail: "off-by-one at the upper boundary".to_owned(),
            disposition: Disposition::Resolved,
        }],
        gate_adequacy: GateAdequacy {
            gates_observe_acceptance: true,
            unobserved_behaviors: vec![],
            basis: "ran the registered gates and probed each acceptance behavior directly"
                .to_owned(),
            mutation_evidence: Some(MutationEvidence::Demonstrated {
                mutation: "widened the upper boundary in src/example.rs:42 by one".to_owned(),
                failing_test: "rejects_the_value_at_the_upper_boundary".to_owned(),
                oracle: "gate.unit".to_owned(),
            }),
        },
        residual_risks: vec!["none identified".to_owned()],
        human_reviewer: true,
    }
}

/// Which of `verdict`'s top-level fields `read_verdict` accepts as absent.
///
/// Established by asking the real deserializer, field by field, rather than
/// read off a list maintained by hand: for each field this removes it from
/// the serialized document and asks whether `Verdict` still parses without
/// it. A hand-maintained claim about the schema is exactly what #108 exists
/// to stop from silently drifting from what the parser actually accepts, and
/// that argument applies to this advisory exactly as it does to the example
/// document itself.
///
/// # Errors
///
/// Returns an error when `verdict` cannot be serialized to inspect.
fn optional_fields(verdict: &Verdict) -> Result<Vec<String>, HarnessError> {
    let value = serde_json::to_value(verdict)?;
    let object = value.as_object().ok_or_else(|| HarnessError::Control {
        reason: "the example verdict did not serialize to a document with fields".to_owned(),
        code: ErrorCode::InternalEncoding,
    })?;
    let mut optional: Vec<String> = object
        .keys()
        .filter(|key| {
            let mut reduced = object.clone();
            reduced.remove(key.as_str());
            serde_json::from_value::<Verdict>(serde_json::Value::Object(reduced)).is_ok()
        })
        .cloned()
        .collect();
    optional.sort();
    Ok(optional)
}

/// Emits a complete, valid review-verdict example.
///
/// #108 constraint 1: reachable without a control repository, a project, or a
/// card — [`ExampleArgs`] carries nothing to open one with. #108 constraint
/// 2: listed under `review --help` because it is an ordinary [`ReviewCommand`]
/// variant like any other.
///
/// The document is YAML: every fixture and every `SKILL.md` example that
/// names a verdict file spells it `verdict.yaml`, and `read_verdict` parses
/// with `serde_yaml_ng`, which accepts JSON as the YAML it syntactically is —
/// so YAML is both what this codebase already writes by convention and the
/// strictly more general of the two accepted shapes.
///
/// Built by constructing a [`Verdict`] and serializing it, never by writing
/// the document out by hand, so this and [`read_verdict`] can never disagree
/// about the shape: they are the same `serde` implementation.
///
/// # Errors
///
/// Returns an error when the example cannot be rendered.
fn run_example() -> Result<CommandOutcome, HarnessError> {
    let verdict = example_verdict();
    let example = serde_yaml_ng::to_string(&verdict).map_err(|source| HarnessError::Control {
        reason: format!("failed to render the example verdict: {source}"),
        code: ErrorCode::InternalEncoding,
    })?;
    let optional = optional_fields(&verdict)?;
    Ok(CommandOutcome::new(
        "review.example",
        example.clone(),
        serde_json::json!({
            "format": "yaml",
            "example": example,
            "optional_fields": optional,
        }),
    )
    .with_warning(format!(
        "every field above is shown so its shape is visible, including these, which default \
         when the key is absent: {}. `reason_category` additionally becomes required — not \
         merely accepted — when `decision` is `changes_requested` under a project with a \
         configured convergence policy that requires one (`require_review_return_reason`)",
        optional.join(", ")
    )))
}

/// Whether a decision returns work to the feature actor.
///
/// `Blocked` halts work pending a decision outside the reviewer's own remit
/// — see [`Decision::Blocked`] — it is a park, not a verdict on the
/// candidate, and nobody has been told to change anything. Recording a
/// convergence attempt against it, or demanding a `reason_category` before it
/// can be filed, would count and gate a return that never happened. Only
/// `ChangesRequested` sends the candidate back to the actor who wrote it, so
/// only `ChangesRequested` is a review return.
const fn is_review_return(decision: Decision) -> bool {
    matches!(decision, Decision::ChangesRequested)
}

/// Every reason category that exists, so an error message can render the set
/// [`AttemptKind::admits`] actually accepts instead of a second, hand-typed
/// list that could silently drift from it.
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

/// The reasons a review return may declare, rendered for an error message.
fn admissible_review_return_reasons() -> String {
    ALL_REASON_CATEGORIES
        .into_iter()
        .filter(|reason| AttemptKind::ReviewReturn.admits(*reason))
        .map(reason_wire_name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Refuses a `changes_requested` verdict, under a configured convergence
/// policy, that does not declare an admissible `reason_category`.
///
/// #9 requires every review return to be recorded with its reason, and the
/// codebase's rule for a declared fact is that the harness never infers one
/// — see the [`Verdict::reason_category`] doc for the full argument. This is
/// where that rule is enforced rather than merely stated: called from both
/// `run_record` and `preview_record`, in the same place relative to their
/// other checks, so the dry run can never promise a write the real command
/// would refuse.
///
/// Silent whenever the question does not apply: no policy configured, or a
/// decision — including `Blocked`, see [`is_review_return`] — that returns no
/// work to anyone. This is `review record`'s ledger obligation, not a general
/// shape rule for every verdict.
///
/// # Errors
///
/// Returns [`ErrorCode::PolicyIncompleteReview`] when `reason_category` is
/// absent, or present but not one [`AttemptKind::ReviewReturn`] admits.
fn require_review_return_reason(
    policy: Option<&ConvergencePolicy>,
    verdict: &Verdict,
) -> Result<(), HarnessError> {
    if policy.is_none() || !is_review_return(verdict.decision) {
        return Ok(());
    }
    let Some(reason) = verdict.reason_category else {
        return Err(HarnessError::Control {
            reason: format!(
                "verdict is missing `reason_category`; a changes-requested decision under a configured convergence policy must declare why the work is being returned — one of: {}",
                admissible_review_return_reasons()
            ),
            code: ErrorCode::PolicyIncompleteReview,
        });
    };
    if !AttemptKind::ReviewReturn.admits(reason) {
        return Err(HarnessError::Control {
            reason: format!(
                "verdict declares `reason_category: {}`, which a review return cannot declare; admissible reasons are: {}",
                reason_wire_name(reason),
                admissible_review_return_reasons()
            ),
            code: ErrorCode::PolicyIncompleteReview,
        });
    }
    Ok(())
}

/// Reports what `review record` would write, without writing it.
///
/// Tier 3 defect 24: a preview that skips a check the real command enforces
/// is worse than no preview — it tells an operator a command will succeed
/// when it will not, and the operator then runs the real one having been
/// told it is safe. This one skipped the staleness question entirely for
/// every decision, approvals included, because `require_current_handoff`
/// was never called here at all. It now asks exactly what `run_record` asks,
/// with the same `HandoffScope` split by decision.
///
/// Order matters here, not just presence, and this now checks five things in
/// the same order `run_record` does: whether the verdict declares an
/// admissible reason for a review return, then whether the card's
/// convergence budget is spent, then whether a review round ever opened for
/// this handoff, then staleness, then independence. A verdict that is both a
/// self-review and stale is refused as stale first — the independence check
/// pre-existed the staleness fix and ran first; moved it after staleness so a
/// case failing both reasons reports the same one in both forms, which is
/// the entire point of a preview. Caught by review round 1 of this exact
/// card, on the fixture where a revoked handoff is also a self-review. The
/// review-begun check runs ahead of staleness and independence because it
/// asks the most basic question: whether there is a review to be stale or
/// self-reviewing about at all. The reason-declaration check runs ahead of
/// everything else: 71-R2 requires it to refuse before anything is written,
/// and nothing above it needs a card, a handoff, or a worktree to answer —
/// it needs only the project's configured policy and the verdict the caller
/// already supplied. The convergence-budget check (72-2) runs immediately
/// after: it is the first thing that needs the loaded card record, so it is
/// the first check able to refuse once that record exists, ahead of the
/// review-begun, staleness, and independence checks below, which all need a
/// handoff besides.
fn preview_record(
    args: &RecordArgs,
    card_id: &CardId,
    verdict: &Verdict,
) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.common.control)?;
    let config = control.project()?;
    require_review_return_reason(config.convergence_policy.as_ref(), verdict)?;
    let (record, state) = load_card(&control, card_id)?;
    require_convergence_budget(&control, &config, &record)?;
    let handoff = latest_handoff(&control, card_id)?.ok_or_else(|| HarnessError::Control {
        reason: format!("card {card_id} has no handoff"),
        code: ErrorCode::PreconditionNotFound,
    })?;
    let scope = if verdict.decision == Decision::Approved {
        HandoffScope::Whole
    } else {
        require_review_begun(&EventStore::new(&control), card_id, &handoff.handoff_id)?;
        HandoffScope::Bindings
    };
    require_current_handoff(&control, card_id, &handoff, &state.current_digest, scope)?;
    check_independence(&verdict.reviewer_actor_id, &handoff.actor_id)?;
    Ok(CommandOutcome::new(
        "review.record",
        format!(
            "Dry run: would record `{}` for card {card_id}; nothing was changed",
            verdict.decision.name()
        ),
        serde_json::json!({
            "dry_run": true,
            "card_id": card_id.to_string(),
            "decision": verdict.decision.name(),
        }),
    ))
}

/// The card state a decision moves the card to.
const fn state_for(decision: Decision) -> CardState {
    match decision {
        Decision::Approved => CardState::Approved,
        Decision::ChangesRequested => CardState::ChangesRequested,
        Decision::Blocked => CardState::Blocked,
    }
}

/// How much of the handoff a verdict has to still be current against.
#[derive(Clone, Copy, Eq, PartialEq)]
enum HandoffScope {
    /// Every question, including the branch head. What an approval answers.
    Whole,
    /// Only what the handoff was bound to. What a non-approval answers.
    Bindings,
}

/// Refuses a review whose handoff no longer stands.
///
/// A review recorded against a superseded handoff would approve code nobody
/// looked at, which is the same failure `SPIKE-001` F-1 found one stage earlier.
///
/// Dependency bindings are deliberately excluded here — the one site that
/// passes [`DEPENDENCIES_NOT_CHECKED`]. A dependency moving does not change the
/// candidate the reviewer is holding, and refusing to file their verdict would
/// leave a card whose only exit is abandonment. That is defect 21 exactly: a
/// verdict the schema offers, discarded because a transition refused it. The
/// dependency question is asked where it decides something — of the review, at
/// integration.
///
/// [`HandoffScope::Bindings`] asks only what the reviewer was measuring
/// against, and the argument for each question it drops is at
/// [`HandoffRecord::binding_staleness`]. It needs no worktree to answer what
/// remains, which is why the lease is resolved only for
/// [`HandoffScope::Whole`] — the previous early return on a card holding no
/// lease skipped the card-digest question too, and there is no reason it
/// should.
fn require_current_handoff(
    control: &ControlRepository,
    card_id: &CardId,
    handoff: &crate::domain::handoff::HandoffRecord,
    card_digest: &crate::domain::digest::Digest,
    scope: HandoffScope,
) -> Result<(), HarnessError> {
    let staleness = match scope {
        HandoffScope::Bindings => handoff.binding_staleness(card_digest, DEPENDENCIES_NOT_CHECKED),
        HandoffScope::Whole => {
            let Some(lease) = held_lease(control, card_id)? else {
                return Ok(());
            };
            let head = inspect::resolve_commit(&GitScope::work_tree(&lease.worktree_path), "HEAD")?;
            handoff.staleness(&head, card_digest, DEPENDENCIES_NOT_CHECKED)
        }
    };
    if let Some(reason) = staleness {
        return Err(HarnessError::Control {
            reason: format!("cannot review a superseded handoff: {reason}"),
            code: ErrorCode::PolicyStaleHandoff,
        });
    }
    Ok(())
}

/// Refuses a verdict recorded against a handoff no review round ever opened.
///
/// `review begin` emits a `review.begun` event tagged with the exact handoff
/// it opened review for. That event is append-only control history, which is
/// why this asks it rather than the card's current state: `handoff revoke`
/// overwrites the card state to `active` regardless of whether a review had
/// begun, so the state alone cannot answer whether one ever did. Without
/// this, `handoff create` followed immediately by `handoff revoke` left a
/// verdict recordable with no review round at any point — reachable through
/// `blocked` and `changes_requested`, both of which `active` admits directly.
/// Called only for a non-approval. An approval cannot reach this gap the
/// same way — `Approved` is only admitted from `review_pending`, which
/// `review begin` is what reaches — and calling this unconditionally would
/// change what an approval against a revoked, never-reviewed handoff
/// reports: staleness (`CH-POLICY-STALE-HANDOFF`) already refuses that case
/// correctly, and review round 1 found that running this check first there
/// silently swapped the reported reason. A non-approval has no such
/// structural guard, since `active` admits both `Blocked` and
/// `ChangesRequested` directly.
fn require_review_begun(
    events: &EventStore<'_>,
    card_id: &CardId,
    handoff_id: &str,
) -> Result<(), HarnessError> {
    let began = events.for_card(card_id)?.iter().any(|event| {
        event.event_type == "review.begun"
            && event
                .metadata
                .get("handoff_id")
                .and_then(serde_json::Value::as_str)
                == Some(handoff_id)
    });
    if began {
        return Ok(());
    }
    Err(HarnessError::Control {
        reason: format!("card {card_id} has no review round open for handoff {handoff_id}"),
        code: ErrorCode::PolicyReviewNotBegun,
    })
}

// 71-R2's reason refusal and its fact emission both have to run inside this
// one transaction, in this exact program order relative to the writes around
// them — splitting either into a helper the length limit would otherwise
// invite risks losing that ordering at a call site instead of at a glance.
#[allow(clippy::too_many_lines)]
fn run_record(args: &RecordArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;
    let verdict = read_verdict(&args.verdict)?;

    if args.dry_run {
        return preview_record(args, &card_id, &verdict);
    }

    with_transaction(
        &args.common.control,
        "review.record",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let config = control.project()?;
            // Before anything else is even read: 71-R2 requires this refusal
            // to happen before any write, and this needs nothing but the
            // policy and the verdict the caller already supplied.
            require_review_return_reason(config.convergence_policy.as_ref(), &verdict)?;
            let (record, state) = load_card(control, &card_id)?;
            // 72-2: the first check able to refuse once the card record
            // exists — see `require_convergence_budget`.
            require_convergence_budget(control, &config, &record)?;
            let handoff =
                latest_handoff(control, &card_id)?.ok_or_else(|| HarnessError::Control {
                    reason: format!("card {card_id} has no handoff to review"),
                    code: ErrorCode::PreconditionNotFound,
                })?;

            // Only an approval answers the candidate question. A verdict that
            // found problems is a true statement about the candidate it was
            // reached against, and it stays true when the branch moves;
            // refusing to file it destroys the reviewer's work to protect an
            // invariant that only approvals carry. The comment on the function
            // above already makes this argument for dependency staleness — "a
            // verdict the schema offers, discarded because a transition
            // refused it" — and it applies one step over, to the candidate
            // itself.
            //
            // Found by making the mistake four times on one card: each time a
            // reviewer's findings were fixed before the verdict was filed, the
            // branch moved, and the refusal below meant the verdict could
            // never be recorded at all. Three of that card's review rounds
            // survive only as prose inside a `handoff revoke --reason`.
            //
            // That relaxation was first written as an `if` around the whole
            // call, which dropped the card-digest question too — so a card
            // revised out from under a handoff fell through to the transition
            // table, which names a state change rather than the reason. The
            // scope is the fix: a non-approval stops asking about the delivery
            // and still answers for what it was measured against.
            let scope = if verdict.decision == Decision::Approved {
                HandoffScope::Whole
            } else {
                // An approval can never reach this gap: `Approved` is only
                // admitted from `review_pending`, which `review begin` is
                // what reaches. A non-approval has no such guarantee — both
                // `active` successors it can land on are reachable directly
                // from a handoff that was revoked before any review began.
                require_review_begun(events, &card_id, &handoff.handoff_id)?;
                HandoffScope::Bindings
            };
            require_current_handoff(control, &card_id, &handoff, &state.current_digest, scope)?;

            let next_state = state_for(verdict.decision);
            state.state.check_transition(next_state)?;

            let previous = reviews_for(control, &card_id)?.into_iter().next_back();
            let review_id = next_review_id(control)?;
            let review = ReviewRecord {
                schema: REVIEW_SCHEMA.to_owned(),
                review_id: review_id.clone(),
                card_id: card_id.clone(),
                card_revision: state.current_revision,
                card_digest: state.current_digest.clone(),
                cycle_id: record.cycle_id.clone(),
                baseline_sha: handoff.baseline_sha.clone(),
                candidate_sha: handoff.candidate_sha.clone(),
                dependency_bindings: handoff.dependency_bindings.clone(),
                handoff_id: handoff.handoff_id.clone(),
                handoff_digest: handoff.digest()?,
                reviewer_actor_id: verdict.reviewer_actor_id.clone(),
                feature_actor_id: handoff.actor_id.clone(),
                decision: verdict.decision,
                findings: verdict.findings.clone(),
                gate_adequacy: verdict.gate_adequacy.clone(),
                residual_risks: verdict.residual_risks.clone(),
                human_reviewer: verdict.human_reviewer,
                supersedes: previous.as_ref().map(|review| review.review_id.clone()),
                reviewed_at: clock.now(),
                canonical_algorithm: CANONICAL_ALGORITHM.to_owned(),
            };
            review.validate()?;
            review.check_risk_policy(record.risk)?;
            if let Some(superseded) = previous.as_ref() {
                review.check_supersedes(superseded)?;
            }
            let digest = review.digest()?;

            control.write_atomic(
                &ReviewRecord::relative_path(&review_id),
                &format!("{}\n", serde_json::to_string_pretty(&review)?),
            )?;
            store_card_state(control, &record, &state, next_state)?;

            events.append(
                &config.project_id,
                EventDraft::new("review.recorded", &verdict.reviewer_actor_id)
                    .cycle(record.cycle_id.clone())
                    .card(
                        card_id.clone(),
                        state.current_revision,
                        state.current_digest.clone(),
                    )
                    .transition(Some(state.state.name()), next_state.name())
                    .head(handoff.candidate_sha.clone())
                    .meta("review_id", serde_json::json!(review_id.to_string()))
                    .meta("review_digest", serde_json::json!(digest.as_str()))
                    .meta("decision", serde_json::json!(verdict.decision.name()))
                    .meta("findings", serde_json::json!(review.findings.len()))
                    .meta(
                        "gates_observe_acceptance",
                        serde_json::json!(review.gate_adequacy.gates_observe_acceptance),
                    ),
                clock,
            )?;

            // A review return under a configured policy records exactly one
            // bound `convergence.attempt_recorded` fact, in the same
            // transaction as `review.recorded` — #9's ledger, written where
            // the return itself is written rather than reconstructed later
            // from the review history. `reason_category` is `Some` and
            // admissible here because `require_review_return_reason` already
            // refused otherwise, above, before this transaction wrote
            // anything; `Blocked` never reaches this arm — see
            // `is_review_return` — so it neither needs a declared reason nor
            // ever emits a fact.
            if is_review_return(verdict.decision)
                && let (Some(policy), Some(reason)) =
                    (config.convergence_policy.as_ref(), verdict.reason_category)
            {
                let policy_digest = policy.digest()?;
                events.append(
                    &config.project_id,
                    EventDraft::new(ATTEMPT_RECORDED_EVENT, &verdict.reviewer_actor_id)
                        .cycle(record.cycle_id.clone())
                        .card(
                            card_id.clone(),
                            state.current_revision,
                            state.current_digest.clone(),
                        )
                        .head(handoff.candidate_sha.clone())
                        .meta(
                            "attempt_kind",
                            serde_json::to_value(AttemptKind::ReviewReturn)?,
                        )
                        .meta("reason_category", serde_json::to_value(reason)?)
                        .meta(
                            "evidence_ref",
                            serde_json::json!(format!("review:{review_id}")),
                        )
                        .meta("policy_digest", serde_json::json!(policy_digest.as_str())),
                    clock,
                )?;
            }

            control.commit(
                expected,
                &format!("review: {} {card_id}", verdict.decision.name()),
            )?;

            // Read back after the commit so the history includes this round;
            // the trend is about the sequence, and the round being recorded is
            // the most informative point in it.
            let history = reviews_for(control, &card_id)?;
            Ok(report_review(
                &review,
                &digest,
                next_state,
                &config.project_id,
                &history,
            ))
        },
    )
}

/// Turns a committed review into the command's outcome.
fn report_review(
    review: &ReviewRecord,
    digest: &crate::domain::digest::Digest,
    next_state: CardState,
    project_id: &crate::domain::ids::ProjectId,
    history: &[ReviewRecord],
) -> CommandOutcome {
    let text = format!(
        "Recorded `{}` for card {}\nreview: {}\nreviewer: {}\ncandidate: {}\nfindings: {}\ncard state: {next_state}",
        review.decision.name(),
        review.card_id,
        review.review_id,
        review.reviewer_actor_id,
        review.candidate_sha,
        review.findings.len()
    );

    let mut outcome = CommandOutcome::new(
        "review.record",
        text,
        serde_json::json!({
            "review": review,
            "review_digest": digest.as_str(),
            "state": next_state.name(),
        }),
    )
    .with_project(project_id.clone());

    // Lagging signal. Every round this card has had, including this one,
    // reduced to open findings and where they landed. Silent until three
    // rounds exist and silent while the card is converging — a signal that
    // fires on healthy work is one people learn to skip.
    let rounds: Vec<Round> = history
        .iter()
        .map(|past| {
            Round::new(
                past.findings
                    .iter()
                    .filter(|finding| finding.disposition.blocks_approval())
                    .map(|finding| finding.location.as_str()),
            )
        })
        .collect();
    if let Some(advisory) = Trend::measure(&rounds).advisory() {
        outcome = outcome.with_warning(advisory);
    }

    if !review.gate_adequacy.gates_observe_acceptance {
        // SPIKE-001 F-5: surfaced rather than buried. A green gate that cannot
        // observe an acceptance behavior is the exact situation both spike
        // reviewers found and the plan had no field for.
        outcome = outcome.with_warning(format!(
            "the reviewer reports the gates cannot observe {} acceptance behavior(s): {}",
            review.gate_adequacy.unobserved_behaviors.len(),
            review.gate_adequacy.unobserved_behaviors.join("; ")
        ));
    }
    outcome
}

fn run_inspect(args: &CardArgs) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;
    let control = ControlRepository::open(&args.common.control)?;
    let config = control.project()?;
    let (record, state) = load_card(&control, &card_id)?;
    let reviews = reviews_for(&control, &card_id)?;
    let scope = GitScope::work_tree(&config.repository);

    let candidate = held_lease(&control, &card_id)?.and_then(|lease| {
        inspect::resolve_commit(&GitScope::work_tree(&lease.worktree_path), "HEAD").ok()
    });
    let approval = candidate.as_ref().and_then(|sha| {
        current_approval(
            &control,
            &scope,
            &card_id,
            sha,
            &state.current_digest,
            &record.depends_on,
        )
        .ok()
        .flatten()
    });

    let mut text = format!(
        "Card {card_id}\ncandidate: {}\nreviews: {}\ncurrent approval: {}",
        candidate.as_deref().unwrap_or("no allocation"),
        reviews.len(),
        approval
            .as_ref()
            .map_or_else(|| "none".to_owned(), |review| review.review_id.to_string())
    );
    let mut latest_stale_reason = None;
    let mut standings = Vec::new();
    for review in &reviews {
        standings = dependency_standings(
            &control,
            &scope,
            &record.depends_on,
            &review.dependency_bindings,
        )?;
        let no_longer_applies = candidate
            .as_ref()
            .and_then(|sha| review.staleness(sha, &state.current_digest, &standings));
        latest_stale_reason.clone_from(&no_longer_applies);
        let _ = write!(
            text,
            "\n  {} {} by {} ({} finding(s)){}",
            review.review_id,
            review.decision.name(),
            review.reviewer_actor_id,
            review.findings.len(),
            no_longer_applies.map_or_else(String::new, |reason| format!(" — stale: {reason}"))
        );
    }

    Ok(CommandOutcome::new(
        "review.inspect",
        text,
        serde_json::json!({
            "card_id": card_id.to_string(),
            "candidate_sha": candidate,
            "reviews": reviews,
            "current_approval": approval,
            "has_current_approval": approval.is_some(),
            // Why the card's own latest verdict no longer applies. Reported so
            // a caller can act on the reason without re-deriving it, which is
            // the whole point of recording the binding.
            "latest_stale_reason": latest_stale_reason,
            "dependency_standings": standings,
        }),
    )
    .with_project(config.project_id.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_order_by_number_across_a_width_boundary() {
        // Round 4 of F-029's review, reached with the real allocator: as text,
        // `RV-1000000` sorts before `RV-999999`, so the round that came last
        // arrives first and `Trend` measures a card that is converging as one
        // that is not. Zero padding hides this for the first 999,999 reviews.
        let mut names = vec![
            "RV-1000000".to_owned(),
            "RV-999999".to_owned(),
            "RV-000002".to_owned(),
            "RV-999998".to_owned(),
            "RV-000010".to_owned(),
        ];
        sort_oldest_first(&mut names);
        assert_eq!(
            names,
            vec![
                "RV-000002",
                "RV-000010",
                "RV-999998",
                "RV-999999",
                "RV-1000000",
            ],
            "oldest first, by number"
        );
    }

    #[test]
    fn an_identifier_with_no_trailing_number_is_ordered_by_name() {
        // No identifier looks like this today. The point is that it sorts
        // somewhere defined rather than panicking or being dropped.
        assert_eq!(ordinal("RV-000007"), Some(7));
        assert_eq!(ordinal("7"), Some(7));
        assert_eq!(ordinal("RV-"), None);
        assert_eq!(ordinal(""), None);

        // Round 5 of this card's own review: the assertions above name a
        // fallback ordering and never reach one, so inverting the tie-break
        // survived them. Numberless names sort before numbered ones and among
        // themselves by name.
        let mut names = vec![
            "RV-000001".to_owned(),
            "RV-".to_owned(),
            String::new(),
            "RV-000002".to_owned(),
        ];
        sort_oldest_first(&mut names);
        assert_eq!(names, vec!["", "RV-", "RV-000001", "RV-000002"]);
    }
}
