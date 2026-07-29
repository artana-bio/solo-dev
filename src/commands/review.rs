//! Independent review commands.

use std::{fmt::Write as _, fs, path::PathBuf};

use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

use crate::{
    cli::output::CommandOutcome,
    commands::{
        card::{load_card, store_card_state},
        handoff::latest_handoff,
        transaction::with_transaction,
        work::held_lease,
    },
    control::{event_store::EventDraft, repository::ControlRepository},
    domain::{
        card::CardState,
        clock::Clock,
        digest::CANONICAL_ALGORITHM,
        handoff::HandoffStatus,
        ids::{CardId, ReviewId},
        review::{
            Decision, Finding, GateAdequacy, REVIEW_DIR, REVIEW_SCHEMA, ReviewRecord,
            check_independence,
        },
    },
    error::{ErrorCode, HarnessError},
    git::{command::GitScope, inspect},
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
}

/// Arguments shared by review subcommands.
#[derive(Debug, Args)]
pub struct CommonArgs {
    /// Path to the control repository.
    #[arg(long)]
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
    /// What the reviewer found.
    #[serde(default)]
    pub findings: Vec<Finding>,
    /// Whether the gates observe the acceptance list.
    pub gate_adequacy: GateAdequacy,
    /// Risks accepted if this is an approval.
    #[serde(default)]
    pub residual_risks: Vec<String>,
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

/// Every review recorded for one card, oldest first.
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
    names.sort();

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
/// # Errors
///
/// Returns an error when the store cannot be read.
pub fn current_approval(
    control: &ControlRepository,
    card_id: &CardId,
    candidate_sha: &str,
    card_digest: &crate::domain::digest::Digest,
) -> Result<Option<ReviewRecord>, HarnessError> {
    Ok(reviews_for(control, card_id)?.into_iter().rfind(|review| {
        review.decision == Decision::Approved && review.is_current_for(candidate_sha, card_digest)
    }))
}

fn run_begin(args: &BeginArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;

    if args.dry_run {
        let control = ControlRepository::open(&args.common.control)?;
        let (_record, state) = load_card(&control, &card_id)?;
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

/// Reads and parses a reviewer's verdict.
fn read_verdict(path: &PathBuf) -> Result<Verdict, HarnessError> {
    let raw = fs::read_to_string(path).map_err(|source| HarnessError::Control {
        reason: format!("cannot read verdict {}: {source}", path.display()),
        code: ErrorCode::ConfigMalformed,
    })?;
    serde_yaml_ng::from_str(&raw).map_err(|source| HarnessError::Control {
        reason: format!("verdict is malformed: {source}"),
        code: ErrorCode::ConfigMalformed,
    })
}

/// Reports what `review record` would write, without writing it.
fn preview_record(
    args: &RecordArgs,
    card_id: &CardId,
    verdict: &Verdict,
) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.common.control)?;
    let handoff = latest_handoff(&control, card_id)?.ok_or_else(|| HarnessError::Control {
        reason: format!("card {card_id} has no handoff"),
        code: ErrorCode::PreconditionNotFound,
    })?;
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

/// Refuses a review whose handoff no longer describes the branch.
///
/// A review recorded against a superseded handoff would approve code nobody
/// looked at, which is the same failure `SPIKE-001` F-1 found one stage earlier.
fn require_current_handoff(
    control: &ControlRepository,
    card_id: &CardId,
    handoff: &crate::domain::handoff::HandoffRecord,
    card_digest: &crate::domain::digest::Digest,
) -> Result<(), HarnessError> {
    let Some(lease) = held_lease(control, card_id)? else {
        return Ok(());
    };
    let head = inspect::resolve_commit(&GitScope::work_tree(&lease.worktree_path), "HEAD")?;
    if let Some(reason) = handoff.staleness(&head, card_digest) {
        return Err(HarnessError::Control {
            reason: format!("cannot review a superseded handoff: {reason}"),
            code: ErrorCode::PolicyStaleHandoff,
        });
    }
    Ok(())
}

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
            let (record, state) = load_card(control, &card_id)?;
            let handoff =
                latest_handoff(control, &card_id)?.ok_or_else(|| HarnessError::Control {
                    reason: format!("card {card_id} has no handoff to review"),
                    code: ErrorCode::PreconditionNotFound,
                })?;

            require_current_handoff(control, &card_id, &handoff, &state.current_digest)?;

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
                handoff_id: handoff.handoff_id.clone(),
                handoff_digest: handoff.digest()?,
                reviewer_actor_id: verdict.reviewer_actor_id.clone(),
                feature_actor_id: handoff.actor_id.clone(),
                decision: verdict.decision,
                findings: verdict.findings.clone(),
                gate_adequacy: verdict.gate_adequacy.clone(),
                residual_risks: verdict.residual_risks.clone(),
                supersedes: previous.map(|review| review.review_id),
                reviewed_at: clock.now(),
                canonical_algorithm: CANONICAL_ALGORITHM.to_owned(),
            };
            review.validate()?;
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
            control.commit(
                expected,
                &format!("review: {} {card_id}", verdict.decision.name()),
            )?;

            Ok(report_review(
                &review,
                &digest,
                next_state,
                &config.project_id,
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
    let (_record, state) = load_card(&control, &card_id)?;
    let reviews = reviews_for(&control, &card_id)?;

    let candidate = held_lease(&control, &card_id)?.and_then(|lease| {
        inspect::resolve_commit(&GitScope::work_tree(&lease.worktree_path), "HEAD").ok()
    });
    let approval = candidate.as_ref().and_then(|sha| {
        current_approval(&control, &card_id, sha, &state.current_digest)
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
    for review in &reviews {
        let no_longer_applies = candidate
            .as_ref()
            .and_then(|sha| review.staleness(sha, &state.current_digest));
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
        }),
    )
    .with_project(config.project_id.clone()))
}
