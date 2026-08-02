//! Independent review commands.

use std::{fmt::Write as _, fs, path::PathBuf};

use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

use crate::{
    cli::output::CommandOutcome,
    commands::CONTROL_ENV,
    commands::{
        card::{load_card, store_card_state},
        handoff::{ancestry, latest_handoff},
        transaction::with_transaction,
        work::held_lease,
    },
    control::{event_store::EventDraft, repository::ControlRepository},
    domain::{
        card::CardState,
        clock::Clock,
        digest::CANONICAL_ALGORITHM,
        handoff::{DEPENDENCIES_NOT_CHECKED, DependencyBinding, DependencyStanding, HandoffStatus},
        ids::{CardId, ReviewId},
        review::{
            Decision, Finding, GateAdequacy, REVIEW_DIR, REVIEW_SCHEMA, ReviewRecord,
            check_independence,
        },
    },
    error::{ErrorCode, HarnessError},
    git::{command::GitScope, inspect},
    policy::convergence::{Round, Trend},
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
