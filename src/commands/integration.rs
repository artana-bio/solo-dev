//! Selecting approved candidates into a deterministic integration plan.
//!
//! `SPIKE-001` finding F-3 is the reason `integration ready` exists. The spike
//! recorded approvals that nothing ever consumed: the approval was durable, and
//! the knowledge of what was awaiting integration lived only in the operator's
//! head. An actor arriving with no context could not recover it. `ready` makes
//! that question answerable from control state alone, and answers the negative
//! case too — a card that is approved but not integrable is listed with the
//! reason, because "not in the ready list" is not a diagnosis.

use std::fs;

use clap::{Args, Subcommand};

use crate::{
    cli::output::CommandOutcome,
    commands::CONTROL_ENV,
    commands::{
        acceptance::{
            EXCEPTION_TRIGGER_NOT_ENABLED_RECOVERY,
            FINAL_AUTHORIZATION_ACTOR_NOT_AUTHORIZED_RECOVERY,
            FINAL_AUTHORIZATION_POLICY_NOT_CONFIGURED_RECOVERY, FINAL_AUTHORIZATION_STALE_RECOVERY,
            FINAL_INTEGRATION_SEAL_STALE_RECOVERY, acceptance_for,
            validate_final_authorization_for_promotion,
        },
        audit::check_control_anchors,
        card::load_card,
        gate::{load_gate, next_receipt_id, receipts_for, require_before_integration},
        handoff::latest_handoff,
        review::{current_approval, dependency_standings, reviews_for},
        transaction::{Steps, with_transaction},
        work::held_lease,
    },
    control::{
        event_store::{EventDraft, EventStore},
        repository::ControlRepository,
    },
    domain::{
        card::{CardRecord, CardState},
        clock::Clock,
        cycle::{CycleRecord, CycleStatus},
        digest::{CANONICAL_ALGORITHM, Digest},
        ids::{CardId, CycleId, IntegrationId},
        integration::{
            ClaimClassification, INTEGRATION_DIR, INTEGRATION_SCHEMA, IntegrationMember,
            IntegrationMode, IntegrationRecord, IntegrationStatus, Interaction, InvariantCheck,
            LEGACY_PLAN_MIGRATION_PROVENANCE, VERIFICATION_SCHEMA, VerificationRecord,
            interactions, topological_order,
        },
        review::ReviewRecord,
    },
    error::{ErrorCode, HarnessError},
    git::{
        authority::{
            PromotionOutcome, inspect_authority, promote as promote_authority, stage_objects,
            unstage_objects,
        },
        command::{GitScope, run, run_ok},
        inspect, integration_worktree, landing,
        merge::{Conflict, ConflictClass, merge_tree},
    },
    policy::{
        actors,
        convergence::{
            ATTEMPT_RECORDED_EVENT, AttemptKind, CycleConvergence, CycleDimension, ReasonCategory,
            assess_cycle, project,
        },
    },
    runner::{
        environment_fingerprint,
        receipt::{
            LOG_DIR, ProofMapBinding, ProvenanceDimension, ProvenanceSubject,
            RECEIPT_PROVENANCE_SCHEMA, RECEIPT_SCHEMA, Receipt, ReceiptProvenanceV1,
        },
        run_attempt,
    },
};

/// Subcommands under `integration`.
#[derive(Debug, Subcommand)]
pub enum IntegrationCommand {
    /// List approved cards awaiting integration, and why others are not ready.
    Ready(ReadyArgs),
    /// Select approved candidates into a deterministic integration plan.
    Prepare(PrepareArgs),
    /// Simulate the merge sequence without changing anything.
    Preflight(InspectArgs),
    /// Combine the selected candidates in a disposable worktree.
    Merge(MergeArgs),
    /// Build the landing commit without moving the protected branch.
    Land(MergeArgs),
    /// Rerun every required gate against the landing commit.
    Verify(MergeArgs),
    /// Record an independent review of the verified integration.
    Review(ReviewArgs),
    /// Move the protected branch to the accepted landing commit.
    Promote(PromoteArgs),
    /// Abandon an integration that will not land, releasing its cycle.
    Abandon(AbandonArgs),
    /// Report a prepared integration.
    Inspect(InspectArgs),
    /// Produce a read-only decision packet for one complete sealed cycle.
    DecisionPacket(InspectArgs),
    /// Raise or resolve a declared human-decision exception.
    #[command(subcommand)]
    Exception(ExceptionCommand),
}

/// Subcommands under `integration exception`.
#[derive(Debug, Subcommand)]
pub enum ExceptionCommand {
    /// Raise a declared human-decision exception for a reviewed final integration.
    Raise(ExceptionRaiseArgs),
    /// Resolve one pending exception with the configured final authorizer.
    Resolve(ExceptionResolveArgs),
}

impl IntegrationCommand {
    /// Its dotted command path, as the result envelope reports it.
    ///
    /// The error envelope used to carry only the group — `integration` — while a
    /// success carried the full path, so a consumer matching on `command` got a
    /// different granularity depending on whether the command worked.
    #[must_use]
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Ready(..) => "integration.ready",
            Self::Prepare(..) => "integration.prepare",
            Self::Preflight(..) => "integration.preflight",
            Self::Merge(..) => "integration.merge",
            Self::Land(..) => "integration.land",
            Self::Verify(..) => "integration.verify",
            Self::Review(..) => "integration.review",
            Self::Promote(..) => "integration.promote",
            Self::Abandon(..) => "integration.abandon",
            Self::Inspect(..) => "integration.inspect",
            Self::DecisionPacket(..) => "integration.decision-packet",
            Self::Exception(ExceptionCommand::Raise(..)) => "integration.exception.raise",
            Self::Exception(ExceptionCommand::Resolve(..)) => "integration.exception.resolve",
        }
    }
}

/// Arguments accepted by `integration merge`.
#[derive(Debug, Args)]
pub struct MergeArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
    pub control: std::path::PathBuf,
    /// The integration to combine.
    #[arg(long)]
    pub integration_id: String,
    /// Who is performing the merge.
    #[arg(long)]
    pub actor_id: String,
    /// Declared verifier principal/session boundary; identity is not host-attested.
    #[arg(long)]
    pub actor_principal_id: Option<String>,
    #[arg(long)]
    pub actor_session_id: Option<String>,
    /// A registered gate to run after each candidate is merged. Repeatable.
    ///
    /// These are cheap intermediate checks, not combined verification: their
    /// job is to name *which* candidate broke the combination, which a single
    /// run at the end cannot do.
    #[arg(long = "smoke-gate")]
    pub smoke_gates: Vec<String>,
    /// Simulate and report without building the worktree.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `integration ready`.
#[derive(Debug, Args)]
pub struct ReadyArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
    pub control: std::path::PathBuf,
    /// The cycle to report on.
    #[arg(long)]
    pub cycle_id: String,
}

/// Arguments accepted by `integration prepare`.
#[derive(Debug, Args)]
pub struct PrepareArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
    pub control: std::path::PathBuf,
    /// The cycle to integrate.
    #[arg(long)]
    pub cycle_id: String,
    /// Who is preparing the integration.
    #[arg(long)]
    pub actor_id: String,
    /// Cards to select. Repeat the option; omit it to select every ready card.
    #[arg(long = "card-id")]
    pub card_ids: Vec<String>,
    /// Whether this integration lands one card or several.
    #[arg(long, value_enum)]
    pub mode: Option<ModeArg>,
    /// Prepare the complete, auditable integration for a sealed cycle.
    #[arg(long = "final")]
    pub final_for_cycle: bool,
    /// Explicitly preserve the pre-plan, non-final behavior for a legacy
    /// sealed cycle. This is a migration marker, not a general bypass.
    #[arg(long)]
    pub legacy_migration_provenance: Option<String>,
    /// Validate and report the plan without recording it.
    #[arg(long)]
    pub dry_run: bool,
}

/// The `--mode` option's values.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum ModeArg {
    /// Exactly one card lands.
    Individual,
    /// Several cards land together.
    Batch,
}

impl From<ModeArg> for IntegrationMode {
    fn from(value: ModeArg) -> Self {
        match value {
            ModeArg::Individual => Self::Individual,
            ModeArg::Batch => Self::Batch,
        }
    }
}

/// Arguments accepted by `integration inspect`.
#[derive(Debug, Args)]
pub struct InspectArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
    pub control: std::path::PathBuf,
    /// The integration to report.
    #[arg(long)]
    pub integration_id: String,
}

/// Arguments accepted by `integration exception raise`.
#[derive(Debug, Args)]
pub struct ExceptionRaiseArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
    pub control: std::path::PathBuf,
    /// The reviewed final integration requiring a decision.
    #[arg(long)]
    pub integration_id: String,
    /// Declared actor raising the exception.
    #[arg(long)]
    pub actor_id: String,
    /// One configured exception trigger.
    #[arg(long)]
    pub trigger: String,
    /// Privacy-safe evidence locator. Repeat for more than one.
    #[arg(long = "evidence-ref", required = true)]
    pub evidence_refs: Vec<String>,
    /// Report the exact outcome without recording it.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `integration exception resolve`.
#[derive(Debug, Args)]
pub struct ExceptionResolveArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
    pub control: std::path::PathBuf,
    /// The final integration carrying the exception.
    #[arg(long)]
    pub integration_id: String,
    /// The immutable raised event to resolve.
    #[arg(long)]
    pub exception_event_id: String,
    /// Configured final authorizer making the decision.
    #[arg(long)]
    pub authorizer_actor_id: String,
    /// The only non-terminal resolution shipped by this slice.
    #[arg(long, default_value = "continue")]
    pub resolution: String,
    /// Report the exact outcome without recording it.
    #[arg(long)]
    pub dry_run: bool,
}

/// Executes an `integration` subcommand.
///
/// # Errors
///
/// Returns a precondition, policy, or conflict error as appropriate.
pub fn execute(
    command: &IntegrationCommand,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    match command {
        IntegrationCommand::Ready(args) => run_ready(args),
        IntegrationCommand::Prepare(args) => run_prepare(args, clock),
        IntegrationCommand::Preflight(args) => run_preflight(args),
        IntegrationCommand::Merge(args) => run_merge(args, clock),
        IntegrationCommand::Land(args) => run_land(args, clock),
        IntegrationCommand::Verify(args) => run_verify(args, clock),
        IntegrationCommand::Review(args) => run_integration_review(args, clock),
        IntegrationCommand::Promote(args) => run_promote(args, clock),
        IntegrationCommand::Abandon(args) => run_abandon(args, clock),
        IntegrationCommand::Inspect(args) => run_inspect(args),
        IntegrationCommand::DecisionPacket(args) => run_decision_packet(args),
        IntegrationCommand::Exception(ExceptionCommand::Raise(args)) => {
            run_exception_raise(args, clock)
        }
        IntegrationCommand::Exception(ExceptionCommand::Resolve(args)) => {
            run_exception_resolve(args, clock)
        }
    }
}

/// Reads a cycle record.
pub(crate) fn load_cycle(
    control: &ControlRepository,
    cycle_id: &CycleId,
) -> Result<CycleRecord, HarnessError> {
    let relative = CycleRecord::relative_path(cycle_id);
    if !control.path(&relative).exists() {
        return Err(HarnessError::Control {
            reason: format!("cycle {cycle_id} does not exist"),
            code: ErrorCode::PreconditionNotFound,
        });
    }
    serde_json::from_str(&control.read(&relative)?).map_err(|source| HarnessError::Control {
        reason: format!("cycle {cycle_id} is malformed: {source}"),
        code: ErrorCode::InternalControlCorrupt,
    })
}

/// Renders a [`CycleDimension`] using the same spelling it would serialize
/// to, so a refusal message never hand-spells a name `serde` already owns.
/// Mirrors `dimension_wire_name` in `card.rs`; kept as its own copy rather
/// than shared, the same reason `disposition.rs` keeps its own — this card's
/// file scope does not extend to `card.rs` or `cycle.rs`, which each already
/// keep a copy of this exact function for the same reason.
fn cycle_dimension_wire_name(dimension: CycleDimension) -> String {
    match serde_json::to_value(dimension) {
        Ok(serde_json::Value::String(name)) => name,
        _ => format!("{dimension:?}"),
    }
}

/// Refuses a controlled action on a cycle whose integration-failure
/// convergence budget is spent.
///
/// 73-2: the cycle-level counterpart of `card.rs`'s `require_convergence_
/// budget` (72-2) — read that function's doc comment first; this one calls
/// out only where the cycle case differs. Every governed path in this file
/// and in `acceptance.rs` that advances an integration toward promotion —
/// preparation, the merge/land/verify/review sequence, final authorization,
/// and promotion itself — calls this before writing anything, so a raw retry
/// cannot dodge the escalation through the ordinary CLI surface. It is
/// deliberately *not* called from `integration abandon`, `cycle abandon`, or
/// any inspect/report command: those are exits and windows, never advances,
/// and gating either is the dead end #73 exists to avoid.
///
/// `config.convergence_policy` is read exactly once, into `policy`, and that
/// same value feeds both [`project`] and [`assess_cycle`] below, for the same
/// reason `require_convergence_budget` reads it once: `assess_cycle` never
/// checks that the projection it is handed was built under the policy
/// supplying its limits, so this is the one place that has to guarantee they
/// agree.
///
/// A malformed, duplicate, foreign, or unbound convergence fact makes
/// `project` refuse the whole projection, propagated as-is rather than
/// treated as an unspent budget — a cycle whose recorded history cannot be
/// trusted does not get to proceed as though it were `Within`.
///
/// Where this genuinely differs from the card case: a card escalation has
/// six authorized dispositions (#74) to answer it, most of them card-scoped.
/// A cycle escalation has exactly two, neither of them a per-dimension
/// renewal — every disposition except `rebaseline` takes `--card-id`, so
/// there is no cycle-scoped `renew`. `disposition rebaseline` retires the
/// policy digest under which the exhausted `integration_failure` facts were
/// recorded, so `project` stops counting them (see the `under_retired_digest`
/// handling there) without erasing them from the record; `cycle abandon` ends
/// the cycle outright. An enforcement that leaves an operator with no exit
/// they can act on immediately is the #79 failure in another form, so both
/// are named, by command, in the refusal below — not left to a generic
/// pointer the way `NextPermittedAction::RecordAuthorizedDisposition` alone
/// would.
///
/// # Errors
///
/// Returns [`ErrorCode::PolicyConvergenceEscalated`] — the same code
/// `require_convergence_budget` uses; this enforcement does not invent a
/// second one — when the cycle's integration-failure dimension is at or over
/// its configured limit. Propagates a control-read or projection failure
/// unchanged otherwise.
pub(crate) fn require_cycle_convergence_budget(
    control: &ControlRepository,
    config: &crate::config::ProjectConfig,
    cycle_id: &CycleId,
) -> Result<(), HarnessError> {
    let policy = config.convergence_policy.as_ref();
    let cycle_events = EventStore::new(control).for_cycle(cycle_id)?;
    let view = project(policy, &config.project_id, cycle_id, &cycle_events).map_err(|error| {
        HarnessError::Control {
            reason: format!("convergence projection for cycle {cycle_id} is unusable: {error}"),
            code: ErrorCode::InternalControlCorrupt,
        }
    })?;

    let CycleConvergence::Escalated { exhausted, .. } = assess_cycle(policy, &view) else {
        return Ok(());
    };

    let detail = exhausted
        .iter()
        .map(|dimension| {
            format!(
                "{}: {}/{} (evidence: {})",
                cycle_dimension_wire_name(dimension.dimension),
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
            "cycle {cycle_id} is escalated; convergence budget exhausted: {detail}. An \
             escalated cycle cannot be advanced toward promotion; its only two exits are \
             `disposition rebaseline`, which retires the current policy digest so these \
             integration_failure facts stop counting, and `cycle abandon`, which ends the \
             cycle."
        ),
        code: ErrorCode::PolicyConvergenceEscalated,
    })
}

/// One card's integrability, with the reason when it is not integrable.
struct Candidacy {
    record: CardRecord,
    state: CardState,
    approval: Option<ReviewRecord>,
    /// Absent when the card is ready; otherwise why it is not.
    blocked_by: Option<String>,
}

/// Why the card's own latest verdict no longer applies, if it does not.
///
/// Reported so a coordinator reading `integration ready` sees the dependency
/// that voided the approval rather than the generic sentence. The latest review
/// is the standing one; an earlier approval it superseded is not what the
/// coordinator is waiting on.
fn latest_review_staleness(
    control: &ControlRepository,
    scope: &GitScope,
    depends_on: &[CardId],
    reviews: &[ReviewRecord],
    candidate: Option<&str>,
    card_digest: &crate::domain::digest::Digest,
) -> Result<Option<String>, HarnessError> {
    let (Some(review), Some(candidate)) = (reviews.last(), candidate) else {
        return Ok(None);
    };
    let standings = dependency_standings(control, scope, depends_on, &review.dependency_bindings)?;
    Ok(review.staleness(candidate, card_digest, &standings))
}

/// Assesses every card in a cycle.
///
/// The negative cases are collected rather than filtered out. A card sitting in
/// `approved` whose candidate branch has since moved looks identical to one
/// that was never approved if the only output is a list of ready cards, and the
/// difference is exactly what the actor needs to know.
fn assess(
    control: &ControlRepository,
    cycle: &CycleRecord,
) -> Result<Vec<Candidacy>, HarnessError> {
    let scope = GitScope::work_tree(&control.project()?.repository);
    let mut assessed = Vec::new();
    for card_id in &cycle.card_ids {
        crate::commands::cycle::require_card_plan_binding(control, cycle, card_id)?;
        let Ok((record, state)) = load_card(control, card_id) else {
            // Declared but never activated: it has claimed nothing and cannot
            // be integrated, and that is not an error in this report.
            continue;
        };

        if state.state != CardState::Approved {
            assessed.push(Candidacy {
                record,
                state: state.state,
                approval: None,
                blocked_by: Some(format!("card is `{}`, not `approved`", state.state.name())),
            });
            continue;
        }

        let handoff = latest_handoff(control, card_id)?;
        // The approval is checked against the branch head, not against the SHA
        // the handoff recorded. Comparing a record to itself always agrees; a
        // branch that gained a commit after approval would stay "ready" and
        // then integrate a commit nobody reviewed, which is `SPIKE-001` F-1
        // one stage later. `review record` applies the same rule.
        let head = match held_lease(control, card_id)? {
            Some(lease) => Some(inspect::resolve_commit(
                &GitScope::work_tree(&lease.worktree_path),
                "HEAD",
            )?),
            None => None,
        };
        let candidate = head.clone().or_else(|| {
            handoff
                .as_ref()
                .map(|handoff| handoff.candidate_sha.clone())
        });
        let approval = match &candidate {
            Some(candidate) => current_approval(
                control,
                &scope,
                card_id,
                candidate,
                &state.current_digest,
                &record.depends_on,
            )?,
            None => None,
        };

        let reviews = reviews_for(control, card_id)?;
        let mut blocked_by = match (&handoff, &approval) {
            (None, _) => Some("card has no handoff".to_owned()),
            (Some(handoff), None) => Some(
                // Section 15.2: an approval is void once the candidate SHA, the
                // card digest, or a required dependency SHA changes. The last
                // of those can void the review while leaving the handoff
                // standing, so both are consulted rather than only the first.
                if reviews.is_empty() {
                    "card has no review".to_owned()
                } else {
                    let standings = dependency_standings(
                        control,
                        &scope,
                        &record.depends_on,
                        &handoff.dependency_bindings,
                    )?;
                    let explanation = head
                        .as_deref()
                        .and_then(|head| handoff.staleness(head, &state.current_digest, &standings))
                        .map_or_else(
                            || {
                                latest_review_staleness(
                                    control,
                                    &scope,
                                    &record.depends_on,
                                    &reviews,
                                    candidate.as_deref(),
                                    &state.current_digest,
                                )
                            },
                            |reason| Ok(Some(reason)),
                        )?;
                    explanation.map_or_else(
                        || {
                            "approval no longer describes the current candidate or card revision"
                                .to_owned()
                        },
                        |reason| {
                            format!("approval no longer describes the current candidate: {reason}")
                        },
                    )
                },
            ),
            (Some(_), Some(_)) => None,
        };

        if blocked_by.is_none()
            && let Some(candidate) = candidate.as_deref()
            && let Err(error) = require_before_integration(control, card_id, candidate)
        {
            blocked_by = Some(error.to_string());
        }

        assessed.push(Candidacy {
            record,
            state: state.state,
            approval,
            blocked_by,
        });
    }
    Ok(assessed)
}

/// Every integration recorded for a cycle.
fn integrations_for(
    control: &ControlRepository,
    cycle_id: &CycleId,
) -> Result<Vec<IntegrationRecord>, HarnessError> {
    let directory = control.path(INTEGRATION_DIR);
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
    for name in names {
        let raw = control.read(&format!("{INTEGRATION_DIR}/{name}.json"))?;
        let record: IntegrationRecord =
            serde_json::from_str(&raw).map_err(|source| HarnessError::Control {
                reason: format!("integration {name} is malformed: {source}"),
                code: ErrorCode::InternalControlCorrupt,
            })?;
        if record.cycle_id == *cycle_id {
            records.push(record);
        }
    }
    Ok(records)
}

/// Allocates the next integration identifier.
fn next_integration_id(control: &ControlRepository) -> Result<IntegrationId, HarnessError> {
    let directory = control.path(INTEGRATION_DIR);
    let highest = if directory.exists() {
        fs::read_dir(&directory)
            .map_err(|source| HarnessError::ControlIo {
                path: directory,
                source,
            })?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                entry
                    .path()
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .and_then(|stem| stem.strip_prefix("INT-"))
                    .and_then(|digits| digits.parse::<u64>().ok())
            })
            .max()
            .unwrap_or(0)
    } else {
        0
    };
    format!("INT-{:03}", highest + 1).parse()
}

fn run_ready(args: &ReadyArgs) -> Result<CommandOutcome, HarnessError> {
    let cycle_id: CycleId = args.cycle_id.parse()?;
    let control = ControlRepository::open(&args.control)?;
    let config = control.project()?;
    let cycle = load_cycle(&control, &cycle_id)?;
    let assessed = assess(&control, &cycle)?;

    let outstanding: Vec<&IntegrationRecord> = Vec::new();
    let integrations = integrations_for(&control, &cycle_id)?;
    let outstanding: Vec<&IntegrationRecord> = integrations
        .iter()
        .filter(|record| record.status.holds_lease())
        .chain(outstanding)
        .collect();

    let ready: Vec<&Candidacy> = assessed
        .iter()
        .filter(|candidacy| candidacy.blocked_by.is_none())
        .collect();
    let waiting: Vec<&Candidacy> = assessed
        .iter()
        .filter(|candidacy| candidacy.blocked_by.is_some())
        .collect();

    let mut text = format!(
        "Cycle {cycle_id} ({})\nready to integrate: {}",
        cycle.status,
        ready.len()
    );
    for candidacy in &ready {
        let approval = candidacy.approval.as_ref();
        let _ = std::fmt::Write::write_fmt(
            &mut text,
            format_args!(
                "\n  {} r{} at {} (approved by {} in {})",
                candidacy.record.card_id,
                candidacy.record.revision,
                approval.map_or("unknown", |review| &review.candidate_sha),
                approval.map_or("unknown", |review| &review.reviewer_actor_id),
                approval.map_or("unknown".to_owned(), |review| review.review_id.to_string()),
            ),
        );
    }
    if !waiting.is_empty() {
        let _ =
            std::fmt::Write::write_fmt(&mut text, format_args!("\nnot ready: {}", waiting.len()));
        for candidacy in &waiting {
            let _ = std::fmt::Write::write_fmt(
                &mut text,
                format_args!(
                    "\n  {}: {}",
                    candidacy.record.card_id,
                    candidacy.blocked_by.as_deref().unwrap_or("unknown")
                ),
            );
        }
    }
    for record in &outstanding {
        let _ = std::fmt::Write::write_fmt(
            &mut text,
            format_args!(
                "\nopen integration: {} ({})",
                record.integration_id,
                record.status.name()
            ),
        );
    }

    Ok(CommandOutcome::new(
        "integration.ready",
        text,
        serde_json::json!({
            "cycle_id": cycle_id.to_string(),
            "cycle_status": cycle.status.to_string(),
            "ready": ready.iter().map(|candidacy| serde_json::json!({
                "card_id": candidacy.record.card_id.to_string(),
                "card_revision": candidacy.record.revision,
                "candidate_sha": candidacy.approval.as_ref().map(|review| review.candidate_sha.clone()),
                "review_id": candidacy.approval.as_ref().map(|review| review.review_id.to_string()),
                "reviewer_actor_id": candidacy.approval.as_ref().map(|review| review.reviewer_actor_id.clone()),
                "depends_on": candidacy.record.depends_on.iter().map(ToString::to_string).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "not_ready": waiting.iter().map(|candidacy| serde_json::json!({
                "card_id": candidacy.record.card_id.to_string(),
                "state": candidacy.state.name(),
                "reason": candidacy.blocked_by,
            })).collect::<Vec<_>>(),
            "open_integrations": outstanding.iter().map(|record| serde_json::json!({
                "integration_id": record.integration_id.to_string(),
                "status": record.status.name(),
            })).collect::<Vec<_>>(),
        }),
    )
    .with_project(config.project_id))
}

/// Resolves the requested selection against what is actually integrable.
fn select<'a>(
    assessed: &'a [Candidacy],
    requested: &[CardId],
) -> Result<Vec<&'a Candidacy>, HarnessError> {
    if requested.is_empty() {
        return Ok(assessed
            .iter()
            .filter(|candidacy| candidacy.blocked_by.is_none())
            .collect());
    }

    let mut selected = Vec::new();
    for card_id in requested {
        let candidacy = assessed
            .iter()
            .find(|candidacy| candidacy.record.card_id == *card_id)
            .ok_or_else(|| HarnessError::Control {
                reason: format!("card {card_id} is not declared in this cycle"),
                code: ErrorCode::PreconditionNotFound,
            })?;
        if let Some(reason) = &candidacy.blocked_by {
            return Err(HarnessError::Control {
                reason: format!("card {card_id} cannot be integrated: {reason}"),
                code: ErrorCode::PolicyNotIntegrable,
            });
        }
        selected.push(candidacy);
    }
    Ok(selected)
}

/// Refuses a selection whose dependencies are neither included nor landed.
fn check_dependencies(selected: &[&Candidacy], assessed: &[Candidacy]) -> Result<(), HarnessError> {
    let included: Vec<&CardId> = selected
        .iter()
        .map(|candidacy| &candidacy.record.card_id)
        .collect();

    for candidacy in selected {
        for dependency in &candidacy.record.depends_on {
            if included.contains(&dependency) {
                continue;
            }
            // A dependency already on the protected branch is satisfied; one
            // that is merely approved is not, because it would land after.
            let landed = assessed.iter().any(|other| {
                other.record.card_id == *dependency
                    && matches!(other.state, CardState::Landed | CardState::Closed)
            });
            if !landed {
                return Err(HarnessError::Control {
                    reason: format!(
                        "card {} depends on {dependency}, which is neither selected nor landed",
                        candidacy.record.card_id
                    ),
                    code: ErrorCode::PolicyDependencyUnsatisfied,
                });
            }
        }
    }
    Ok(())
}

/// Refuses a selection that splits an atomic group.
///
/// Section 10.2 declares atomic groups on the cycle. A group exists precisely
/// because its cards are not independently landable, so integrating part of one
/// would promote a state the coordinator declared invalid.
fn check_atomic_groups(
    cycle: &CycleRecord,
    selected: &[&Candidacy],
) -> Result<Vec<String>, HarnessError> {
    let included: Vec<&CardId> = selected
        .iter()
        .map(|candidacy| &candidacy.record.card_id)
        .collect();

    let mut complete = Vec::new();
    for group in &cycle.atomic_groups {
        let present: Vec<&CardId> = group
            .card_ids
            .iter()
            .filter(|card_id| included.contains(card_id))
            .collect();
        if present.is_empty() {
            continue;
        }
        if present.len() != group.card_ids.len() {
            let missing: Vec<&str> = group
                .card_ids
                .iter()
                .filter(|card_id| !included.contains(card_id))
                .map(CardId::as_str)
                .collect();
            return Err(HarnessError::Control {
                reason: format!(
                    "atomic group `{}` must land whole; missing: {}",
                    group.name,
                    missing.join(", ")
                ),
                code: ErrorCode::PolicyAtomicGroupSplit,
            });
        }
        complete.push(group.name.clone());
    }
    Ok(complete)
}

/// The atomic group a card belongs to, when it has one.
fn group_of(cycle: &CycleRecord, card_id: &CardId) -> Option<String> {
    cycle
        .atomic_groups
        .iter()
        .find(|group| group.card_ids.contains(card_id))
        .map(|group| group.name.clone())
}

/// Builds the ordered member list for a selection.
fn plan_members(
    control: &ControlRepository,
    cycle: &CycleRecord,
    selected: &[&Candidacy],
) -> Result<Vec<IntegrationMember>, HarnessError> {
    let graph: Vec<(CardId, Vec<CardId>)> = selected
        .iter()
        .map(|candidacy| {
            (
                candidacy.record.card_id.clone(),
                candidacy.record.depends_on.clone(),
            )
        })
        .collect();
    let order = topological_order(&graph)?;

    let mut members = Vec::with_capacity(order.len());
    for card_id in order {
        let candidacy = selected
            .iter()
            .find(|candidacy| candidacy.record.card_id == card_id)
            .ok_or_else(|| HarnessError::Control {
                reason: format!("ordering produced unknown card {card_id}"),
                code: ErrorCode::InternalControlCorrupt,
            })?;
        let approval = candidacy
            .approval
            .as_ref()
            .ok_or_else(|| HarnessError::Control {
                reason: format!("card {card_id} passed selection without an approval"),
                code: ErrorCode::InternalControlCorrupt,
            })?;
        let handoff = latest_handoff(control, &card_id)?.ok_or_else(|| HarnessError::Control {
            reason: format!("card {card_id} passed selection without a handoff"),
            code: ErrorCode::InternalControlCorrupt,
        })?;

        members.push(IntegrationMember {
            card_id: card_id.clone(),
            card_revision: approval.card_revision,
            card_digest: approval.card_digest.clone(),
            candidate_sha: approval.candidate_sha.clone(),
            branch: handoff.branch.clone(),
            review_id: approval.review_id.clone(),
            review_digest: approval.digest()?,
            handoff_id: approval.handoff_id.clone(),
            atomic_group: group_of(cycle, &card_id),
        });
    }
    Ok(members)
}

/// Chooses the recorded mode, refusing a declaration the selection contradicts.
fn resolve_mode(
    requested: Option<ModeArg>,
    members: usize,
) -> Result<IntegrationMode, HarnessError> {
    let mode = requested.map_or(
        if members == 1 {
            IntegrationMode::Individual
        } else {
            IntegrationMode::Batch
        },
        IntegrationMode::from,
    );
    if matches!(mode, IntegrationMode::Individual) && members != 1 {
        return Err(HarnessError::Control {
            reason: format!("mode `individual` requires exactly one card; {members} were selected"),
            code: ErrorCode::UsageConflictingOptions,
        });
    }
    Ok(mode)
}

fn run_prepare(args: &PrepareArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let cycle_id: CycleId = args.cycle_id.parse()?;
    let requested: Vec<CardId> = args
        .card_ids
        .iter()
        .map(|raw| raw.parse())
        .collect::<Result<_, _>>()?;

    if args.final_for_cycle && !requested.is_empty() {
        return Err(HarnessError::Control {
            reason: "`--final` selects the complete sealed cycle and cannot be combined with `--card-id`".to_owned(),
            code: ErrorCode::UsageConflictingOptions,
        });
    }

    if args.dry_run {
        return preview_prepare(args, &cycle_id, &requested);
    }

    // Plan policy failures must be reported before opening a journaled
    // integration mutation. This is especially important for a missing plan
    // or a split atomic group: neither case created any external state and
    // should not strand an operation that requires recovery.
    {
        let control = ControlRepository::open(&args.control)?;
        let config = control.project()?;
        require_cycle_convergence_budget(&control, &config, &cycle_id)?;
        let cycle = load_cycle(&control, &cycle_id)?;
        let _ = build_plan(&control, &cycle, &requested, args)?;
    }

    with_transaction(
        &args.control,
        "integration.prepare",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let config = control.project()?;
            // 73-2: the first check able to refuse once the project config
            // exists — a prepared plan is this file's own entry point onto
            // the path toward promotion, so it is where the gate has to
            // start. See `require_cycle_convergence_budget`.
            require_cycle_convergence_budget(control, &config, &cycle_id)?;
            let cycle = load_cycle(control, &cycle_id)?;
            let plan = build_plan(control, &cycle, &requested, args)?;
            let deferred = plan.deferred.clone();

            let integration_id = next_integration_id(control)?;
            let record = build_record(
                &config,
                &cycle,
                integration_id.clone(),
                plan,
                &args.actor_id,
                clock,
            )?;
            let digest = record.digest()?;

            control.write_atomic(
                &IntegrationRecord::relative_path(&integration_id),
                &format!("{}\n", serde_json::to_string_pretty(&record)?),
            )?;

            // Each selected card moves to `integrating`, which is what stops a
            // second integration from selecting the same candidate.
            for member in &record.members {
                let (card, state) = load_card(control, &member.card_id)?;
                state.state.check_transition(CardState::Integrating)?;
                crate::commands::card::store_card_state(
                    control,
                    &card,
                    &state,
                    CardState::Integrating,
                )?;
            }

            events.append(
                &config.project_id,
                EventDraft::new("integration.prepared", &args.actor_id)
                    .cycle(cycle_id.clone())
                    .head(record.expected_main_sha.clone())
                    .transition(None::<&str>, IntegrationStatus::Prepared.name())
                    .meta(
                        "integration_id",
                        serde_json::json!(integration_id.to_string()),
                    )
                    .meta("integration_digest", serde_json::json!(digest.as_str()))
                    .meta("mode", serde_json::json!(record.mode.name()))
                    .meta(
                        "cards",
                        serde_json::json!(
                            record
                                .card_ids()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                        ),
                    ),
                clock,
            )?;
            control.commit(
                expected,
                &format!("integration: prepare {integration_id} for {cycle_id}"),
            )?;

            Ok(warn_about_deferred(
                report_integration("integration.prepare", &record, &digest, &config.project_id),
                &deferred,
            ))
        },
    )
}

/// Assembles the record a validated plan becomes.
///
/// The authority's protected commit is read here rather than taken from
/// configuration, because `expected_main_sha` must be what the branch actually
/// pointed at when this plan was built. Section 13.6 compares against it at
/// promotion time, and that comparison is only meaningful if the recorded value
/// was observed rather than assumed.
fn build_record(
    config: &crate::config::ProjectConfig,
    cycle: &CycleRecord,
    integration_id: IntegrationId,
    plan: Plan,
    actor_id: &str,
    clock: &dyn Clock,
) -> Result<IntegrationRecord, HarnessError> {
    let authority = inspect_authority(&config.authority_repository, &config.protected_branch)?;
    let expected_main_sha = authority
        .protected_sha
        .ok_or_else(|| HarnessError::Control {
            reason: format!(
                "authority has no `{}` branch to integrate against",
                config.protected_branch
            ),
            code: ErrorCode::PreconditionNotFound,
        })?;
    let baseline_sha = cycle
        .baseline_sha
        .clone()
        .ok_or_else(|| HarnessError::Control {
            reason: format!("cycle {} has no frozen baseline", cycle.cycle_id),
            code: ErrorCode::PreconditionNotFound,
        })?;

    Ok(IntegrationRecord {
        schema: INTEGRATION_SCHEMA.to_owned(),
        integration_id,
        cycle_id: cycle.cycle_id.clone(),
        plan_id: cycle.plan_id.clone(),
        plan_digest: cycle.plan_digest.clone(),
        plan_revision: cycle.plan_revision,
        plan_migration_provenance: plan.legacy_migration_provenance,
        status: IntegrationStatus::Prepared,
        mode: plan.mode,
        baseline_sha,
        expected_main_sha,
        members: plan.members,
        final_for_cycle: plan.final_for_cycle,
        sealed_cycle_digest: plan.sealed_cycle_digest,
        abandoned_card_ids: plan.abandoned_card_ids,
        atomic_groups: plan.atomic_groups,
        integration_head: None,
        integration_tree: None,
        merged_at: None,
        landing_sha: None,
        landed_at: None,
        prepared_by: actor_id.to_owned(),
        prepared_at: clock.now(),
        canonical_algorithm: CANONICAL_ALGORITHM.to_owned(),
    })
}

/// The parts of a prepared plan derived before anything is written.
struct Plan {
    members: Vec<IntegrationMember>,
    atomic_groups: Vec<String>,
    mode: IntegrationMode,
    final_for_cycle: bool,
    legacy_migration_provenance: Option<String>,
    sealed_cycle_digest: Option<Digest>,
    abandoned_card_ids: Vec<CardId>,
    /// Cards the cycle holds that this plan left out, and why.
    ///
    /// Only ever populated when the coordinator named no cards. `select` then
    /// takes everything ready, which means a card that stopped being ready is
    /// dropped rather than refused — deliberately, because a cycle almost
    /// always holds work that is not finished, and refusing would make the
    /// common case an error. What was wrong was doing it silently: a batch that
    /// ships fewer cards than the coordinator believes is the same surprise as
    /// one that ships more. Named cards still refuse; see `select`.
    deferred: Vec<(CardId, String)>,
}

/// Binds a final plan to one sealed membership snapshot and refuses omissions.
fn final_cycle_binding(
    cycle: &CycleRecord,
    assessed: &[Candidacy],
    selected: &[&Candidacy],
    requested: bool,
) -> Result<(bool, Option<Digest>, Vec<CardId>), HarnessError> {
    if !requested {
        return Ok((false, None, Vec::new()));
    }
    if cycle.status != CycleStatus::Sealed {
        return Err(HarnessError::Control {
            reason: format!(
                "cycle {} is `{}`; `integration prepare --final` requires a sealed cycle",
                cycle.cycle_id,
                cycle.status.name()
            ),
            code: ErrorCode::PolicyCycleNotSealed,
        });
    }

    let selected_ids: std::collections::BTreeSet<&CardId> = selected
        .iter()
        .map(|candidacy| &candidacy.record.card_id)
        .collect();
    let by_id: std::collections::BTreeMap<&CardId, &Candidacy> = assessed
        .iter()
        .map(|candidacy| (&candidacy.record.card_id, candidacy))
        .collect();
    let mut abandoned_card_ids = Vec::new();
    for card_id in &cycle.card_ids {
        let candidacy = by_id.get(card_id).ok_or_else(|| HarnessError::Control {
            reason: format!(
                "sealed cycle {} is incomplete: card {card_id} was declared but has no activated record",
                cycle.cycle_id
            ),
            code: ErrorCode::PolicyFinalCycleIncomplete,
        })?;
        if candidacy.state == CardState::Abandoned {
            abandoned_card_ids.push(card_id.clone());
            continue;
        }
        if !selected_ids.contains(card_id) {
            let reason = candidacy
                .blocked_by
                .as_deref()
                .unwrap_or("card is not selected");
            return Err(HarnessError::Control {
                reason: format!(
                    "sealed cycle {} is incomplete: card {card_id} cannot be included: {reason}",
                    cycle.cycle_id
                ),
                code: ErrorCode::PolicyFinalCycleIncomplete,
            });
        }
    }
    abandoned_card_ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    Ok((true, Some(Digest::of_canonical(cycle)?), abandoned_card_ids))
}

/// Resolves the sealed-cycle default and the only supported compatibility
/// bypass. New sealed cycles always use final authorization unless the caller
/// supplies the exact typed migration provenance.
fn resolve_final_mode(
    cycle: &CycleRecord,
    args: &PrepareArgs,
) -> Result<(bool, Option<String>), HarnessError> {
    if cycle.status != CycleStatus::Sealed {
        if args.legacy_migration_provenance.is_some() {
            return Err(HarnessError::Control {
                reason: "legacy migration provenance is valid only for sealed-cycle compatibility"
                    .to_owned(),
                code: ErrorCode::PolicyInvalidCycle,
            });
        }
        return Ok((args.final_for_cycle, None));
    }
    if let Some(provenance) = &args.legacy_migration_provenance {
        if provenance != LEGACY_PLAN_MIGRATION_PROVENANCE {
            return Err(HarnessError::Control {
                reason: format!(
                    "unsupported legacy migration provenance `{provenance}`; expected `{LEGACY_PLAN_MIGRATION_PROVENANCE}`"
                ),
                code: ErrorCode::PolicyInvalidCycle,
            });
        }
        if args.final_for_cycle {
            return Err(HarnessError::Control {
                reason: "legacy migration provenance cannot be combined with `--final`".to_owned(),
                code: ErrorCode::UsageConflictingOptions,
            });
        }
        if cycle.plan_migration_provenance.as_deref() != Some(provenance.as_str()) {
            return Err(HarnessError::Control {
                reason: "legacy migration provenance must already be recorded on the cycle by `cycle migrate-legacy`; prepare cannot mint it".to_owned(),
                code: ErrorCode::PolicyInvalidCycle,
            });
        }
        return Ok((false, Some(provenance.clone())));
    }
    Ok((true, None))
}

/// Recovery guidance for `integration prepare --final` when a sealed cycle
/// never had a card, distinct from every other empty-selection outcome.
///
/// `final_cycle_binding`'s own loop over `cycle.card_ids` requires each
/// member to end up either abandoned or selected — anything else refuses
/// earlier with `PolicyFinalCycleIncomplete`. So when that loop finishes
/// with `abandoned_card_ids` still empty, the only way `cycle.card_ids`
/// could also be non-empty is contradicted; it must have been empty from
/// the start. `PreconditionNotFound`'s shared-table default ("Create the
/// named record, or correct the identifier") reads as though the cycle
/// identifier were wrong, and it is not — this is a real, sealed cycle that
/// simply never had a card declared into it. #178.
const FINAL_EMPTY_SEAL_RECOVERY: &str = "This cycle was sealed with no cards, so there is nothing to integrate and nothing was abandoned — `--final`'s allowance for an all-abandoned cycle does not apply here. A sealed cycle cannot return to `active` to accept cards. If it was sealed by mistake, run `cycle abandon` with `--cycle-id` and `--reason` to close it out, then start the intended work in a new cycle with `cycle create`.";

/// Validates a selection and derives its plan, changing nothing.
fn build_plan(
    control: &ControlRepository,
    cycle: &CycleRecord,
    requested: &[CardId],
    args: &PrepareArgs,
) -> Result<Plan, HarnessError> {
    if !cycle.card_ids.is_empty() {
        crate::commands::cycle::require_active_plan(control, cycle)?;
    }
    // One outstanding integration per cycle. Section 11.3 has no state for two
    // concurrent plans, and two plans built against the same protected commit
    // would each believe they were the one about to land.
    if let Some(open) = integrations_for(control, &cycle.cycle_id)?
        .into_iter()
        .find(|record| record.status.holds_lease())
    {
        return Err(HarnessError::Control {
            reason: format!(
                "integration {} is already open for cycle {} ({})",
                open.integration_id,
                cycle.cycle_id,
                open.status.name()
            ),
            code: ErrorCode::PolicyIntegrationOpen,
        });
    }

    let (final_for_cycle, legacy_migration_provenance) = resolve_final_mode(cycle, args)?;
    if final_for_cycle && !requested.is_empty() {
        return Err(HarnessError::Control {
            reason: "the default sealed-cycle final integration selects the complete cycle and cannot be combined with `--card-id`".to_owned(),
            code: ErrorCode::UsageConflictingOptions,
        });
    }
    let assessed = assess(control, cycle)?;
    let selected = select(&assessed, requested)?;
    let (final_for_cycle, sealed_cycle_digest, abandoned_card_ids) =
        final_cycle_binding(cycle, &assessed, &selected, final_for_cycle)?;
    // An empty selection is refused unless `--final` can account for it by
    // abandonment. `abandoned_card_ids` is non-empty here if and only if the
    // sealed cycle held cards and every one of them was abandoned — see
    // `final_cycle_binding`'s own loop, which requires each of
    // `cycle.card_ids` to be classified abandoned or selected before it
    // returns successfully. A sealed cycle that never had a card reaches
    // this same `selected.is_empty()` with `abandoned_card_ids` empty too:
    // there was nothing to select and nothing to abandon, so the exemption
    // does not apply and `--final` is refused the same as an ordinary
    // empty selection, with guidance specific to the sealed-and-empty case.
    if selected.is_empty() && abandoned_card_ids.is_empty() {
        if final_for_cycle {
            return Err(HarnessError::ControlWithRecovery {
                reason: format!(
                    "cycle {} was sealed with no cards: `--final` has nothing to integrate and nothing was abandoned",
                    cycle.cycle_id
                ),
                code: ErrorCode::PreconditionNotFound,
                recovery: FINAL_EMPTY_SEAL_RECOVERY,
            });
        }
        return Err(HarnessError::Control {
            reason: format!("cycle {} has no cards ready to integrate", cycle.cycle_id),
            code: ErrorCode::PreconditionNotFound,
        });
    }

    check_dependencies(&selected, &assessed)?;
    let atomic_groups = check_atomic_groups(cycle, &selected)?;
    let mode = if final_for_cycle {
        if matches!(args.mode, Some(ModeArg::Individual)) {
            return Err(HarnessError::Control {
                reason: "`--final` always prepares a batch integration and cannot use `--mode individual`".to_owned(),
                code: ErrorCode::UsageConflictingOptions,
            });
        }
        IntegrationMode::Batch
    } else {
        resolve_mode(args.mode, selected.len())?
    };
    require_execution_classification(control, cycle, &selected, mode)?;
    let members = plan_members(control, cycle, &selected)?;

    let deferred = if requested.is_empty() {
        assessed
            .iter()
            .filter_map(|candidacy| {
                candidacy
                    .blocked_by
                    .as_ref()
                    .map(|reason| (candidacy.record.card_id.clone(), reason.clone()))
            })
            .collect()
    } else {
        Vec::new()
    };

    Ok(Plan {
        members,
        atomic_groups,
        mode,
        final_for_cycle,
        legacy_migration_provenance,
        sealed_cycle_digest,
        abandoned_card_ids,
        deferred,
    })
}

/// Binds the requested integration shape to the plan's declared execution
/// classification. A plan is not merely a fingerprint for the card set.
fn require_execution_classification(
    control: &ControlRepository,
    cycle: &CycleRecord,
    selected: &[&Candidacy],
    mode: IntegrationMode,
) -> Result<(), HarnessError> {
    let Some(plan_id) = cycle.plan_id.as_deref() else {
        return Ok(());
    };
    let plan: crate::domain::cycle_plan::CyclePlan = serde_json::from_str(
        &control.read(&format!("plans/{plan_id}.json"))?,
    )
    .map_err(|source| HarnessError::Control {
        reason: format!("cycle plan {plan_id} is malformed: {source}"),
        code: ErrorCode::InternalControlCorrupt,
    })?;
    for candidate in selected {
        let planned = plan
            .cards
            .iter()
            .find(|planned| planned.card_id == candidate.record.card_id.to_string())
            .ok_or_else(|| HarnessError::Control {
                reason: format!(
                    "cycle plan {plan_id} has no execution classification for {}",
                    candidate.record.card_id
                ),
                code: ErrorCode::PolicyInvalidCycle,
            })?;
        let invalid = match planned.distribution {
            crate::domain::cycle_plan::Distribution::JointIntegration => {
                mode != IntegrationMode::Batch
            }
            crate::domain::cycle_plan::Distribution::Sequential => {
                selected.len() > 1 && mode != IntegrationMode::Batch
            }
            crate::domain::cycle_plan::Distribution::Parallel => false,
        };
        if invalid {
            return Err(HarnessError::Control {
                reason: format!(
                    "plan {plan_id} classifies card {} as {:?}, which does not permit integration mode `{}`",
                    candidate.record.card_id,
                    planned.distribution,
                    mode.name()
                ),
                code: ErrorCode::PolicyInvalidCycle,
            });
        }
    }
    Ok(())
}

/// Attaches one advisory per card a plan left out.
fn warn_about_deferred(
    mut outcome: CommandOutcome,
    deferred: &[(CardId, String)],
) -> CommandOutcome {
    for (card_id, reason) in deferred {
        outcome = outcome.with_warning(format!(
            "card {card_id} is in this cycle but was left out of the plan: {reason}"
        ));
    }
    outcome
}

fn preview_prepare(
    args: &PrepareArgs,
    cycle_id: &CycleId,
    requested: &[CardId],
) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.control)?;
    let config = control.project()?;
    // 73-2: same placement as `run_prepare` — right after `config` exists,
    // before anything else — so a preview can never promise a plan the real
    // command would refuse for an escalated cycle. See
    // `require_cycle_convergence_budget`.
    require_cycle_convergence_budget(&control, &config, cycle_id)?;
    let cycle = load_cycle(&control, cycle_id)?;
    let plan = build_plan(&control, &cycle, requested, args)?;

    let order: Vec<String> = plan
        .members
        .iter()
        .map(|member| member.card_id.to_string())
        .collect();
    let outcome = CommandOutcome::new(
        "integration.prepare",
        format!(
            "Dry run: would prepare a {} integration of {} card(s)\nmerge order: {}\nnothing was changed",
            plan.mode.name(),
            plan.members.len(),
            order.join(" → ")
        ),
        serde_json::json!({
            "dry_run": true,
            "cycle_id": cycle_id.to_string(),
            "mode": plan.mode.name(),
            "final_for_cycle": plan.final_for_cycle,
            "sealed_cycle_digest": plan.sealed_cycle_digest,
            "abandoned_card_ids": plan.abandoned_card_ids,
            "merge_order": order,
            "atomic_groups": plan.atomic_groups,
            "deferred": plan.deferred.iter().map(|(card_id, reason)| serde_json::json!({
                "card_id": card_id.to_string(),
                "reason": reason,
            })).collect::<Vec<_>>(),
        }),
    );
    Ok(warn_about_deferred(
        outcome.with_project(config.project_id),
        &plan.deferred,
    ))
}

/// Turns a recorded integration into the command's outcome.
fn report_integration(
    command: &str,
    record: &IntegrationRecord,
    digest: &crate::domain::digest::Digest,
    project_id: &crate::domain::ids::ProjectId,
) -> CommandOutcome {
    let order: Vec<String> = record
        .members
        .iter()
        .map(|member| format!("{} at {}", member.card_id, &member.candidate_sha))
        .collect();
    // A final integration whose sealed cycle held cards but saw every one of
    // them abandoned (see `final_cycle_binding`) is a real, deliberate
    // record: an integration with zero members. Reusing the ordinary "merge
    // order:" heading over an empty list would render a blank line under
    // it — reading as a truncated report, not the surprising thing this
    // actually is (§8, #178). `delivers_no_cards` names it explicitly, both
    // here in the human text and in the JSON payload below, rather than
    // leaving a reader to notice `members` is empty and infer why.
    let delivers_no_cards = record.final_for_cycle && record.members.is_empty();
    let mut text = if delivers_no_cards {
        format!(
            "Integration {} ({}) delivers no cards: every member of sealed cycle {} was abandoned\ncycle: {}\nmode: {}\nexpected authority baseline: {}\nabandoned: {}",
            record.integration_id,
            record.status.name(),
            record.cycle_id,
            record.cycle_id,
            record.mode.name(),
            record.expected_main_sha,
            record
                .abandoned_card_ids
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        )
    } else {
        format!(
            "Integration {} ({})\ncycle: {}\nmode: {}\nexpected authority baseline: {}\nmerge order:\n  {}",
            record.integration_id,
            record.status.name(),
            record.cycle_id,
            record.mode.name(),
            record.expected_main_sha,
            order.join("\n  ")
        )
    };
    if let Some(head) = &record.integration_head {
        let _ = std::fmt::Write::write_fmt(&mut text, format_args!("\nintegration head: {head}"));
    }
    if let Some(landing) = &record.landing_sha {
        let _ = std::fmt::Write::write_fmt(&mut text, format_args!("\nlanding commit: {landing}"));
    }
    // #112: `land` builds a real artifact but is not a transition (see
    // `IntegrationStatus`'s own doc comment), so a landed integration still
    // reports `prepared` above — indistinguishable from one that was never
    // merged unless this line says which. Gated on `record.landing_sha`
    // rather than resolving `refs/harness/landing/<id>` in git: that field's
    // own doc comment is why it exists on the record at all instead of
    // being re-derived from the ref ("a ref can be deleted, and the record
    // is what promotion reloads"), and `run_decision_packet` already uses
    // this exact field and condition to reach this exact same next action.
    let next_permitted_action = (record.status == IntegrationStatus::Prepared
        && record.landing_sha.is_some())
    .then_some("integration.verify");
    if let Some(next_action) = next_permitted_action {
        let _ = std::fmt::Write::write_fmt(&mut text, format_args!("\nnext action: {next_action}"));
    }

    CommandOutcome::new(
        command,
        text,
        serde_json::json!({
            "integration_id": record.integration_id.to_string(),
            "integration_digest": digest.as_str(),
            "cycle_id": record.cycle_id.to_string(),
            "status": record.status.name(),
            "mode": record.mode.name(),
            "final_for_cycle": record.final_for_cycle,
            "delivers_no_cards": delivers_no_cards,
            "sealed_cycle_digest": record.sealed_cycle_digest,
            "abandoned_card_ids": record.abandoned_card_ids,
            "baseline_sha": record.baseline_sha,
            "expected_main_sha": record.expected_main_sha,
            "atomic_groups": record.atomic_groups,
            "integration_head": record.integration_head,
            "integration_tree": record.integration_tree,
            "landing_sha": record.landing_sha,
            "next_permitted_action": next_permitted_action,
            "members": record.members,
        }),
    )
    .with_project(project_id.clone())
}

/// Reads one integration record.
///
/// # Errors
///
/// Returns a precondition error when it does not exist, or an internal error
/// when the stored record cannot be parsed.
pub fn load_integration(
    control: &ControlRepository,
    integration_id: &IntegrationId,
) -> Result<IntegrationRecord, HarnessError> {
    let relative = IntegrationRecord::relative_path(integration_id);
    if !control.path(&relative).exists() {
        return Err(HarnessError::Control {
            reason: format!("integration {integration_id} does not exist"),
            code: ErrorCode::PreconditionNotFound,
        });
    }
    serde_json::from_str(&control.read(&relative)?).map_err(|source| HarnessError::Control {
        reason: format!("integration {integration_id} is malformed: {source}"),
        code: ErrorCode::InternalControlCorrupt,
    })
}

/// Revalidates the plan pinned by an integration at every authorization
/// boundary. Historical unplanned records are accepted only when they carry
/// the explicit typed migration provenance written by the new prepare path.
///
/// # Errors
///
/// Returns a policy error when the plan is missing, stale, or contradictory.
pub fn require_plan_binding(
    control: &ControlRepository,
    record: &IntegrationRecord,
) -> Result<(), HarnessError> {
    let cycle = load_cycle(control, &record.cycle_id)?;
    let fully_bound =
        record.plan_id.is_some() && record.plan_digest.is_some() && record.plan_revision.is_some();
    if fully_bound {
        crate::commands::cycle::require_active_plan(control, &cycle)?;
        if record.plan_id != cycle.plan_id
            || record.plan_digest != cycle.plan_digest
            || record.plan_revision != cycle.plan_revision
            || record.plan_migration_provenance.is_some()
        {
            return Err(HarnessError::Control {
                reason: format!(
                    "integration {} no longer matches its pinned cycle plan",
                    record.integration_id
                ),
                code: ErrorCode::PolicyInvalidCycle,
            });
        }
        return Ok(());
    }
    if record.plan_id.is_none()
        && record.plan_digest.is_none()
        && record.plan_revision.is_none()
        && record.plan_migration_provenance.as_deref() == Some(LEGACY_PLAN_MIGRATION_PROVENANCE)
        && cycle.plan_id.is_none()
        && cycle.plan_digest.is_none()
        && cycle.plan_revision.is_none()
    {
        return Ok(());
    }
    Err(HarnessError::Control {
        reason: format!(
            "integration {} has no complete cycle-plan binding or explicit legacy migration provenance",
            record.integration_id
        ),
        code: ErrorCode::PolicyInvalidCycle,
    })
}

fn run_inspect(args: &InspectArgs) -> Result<CommandOutcome, HarnessError> {
    let integration_id: IntegrationId = args.integration_id.parse()?;
    let control = ControlRepository::open(&args.control)?;
    let config = control.project()?;
    let record = load_integration(&control, &integration_id)?;
    let digest = record.digest()?;

    Ok(report_integration(
        "integration.inspect",
        &record,
        &digest,
        &config.project_id,
    ))
}

/// Reports the evidence an owner would need before recording acceptance.
///
/// This is deliberately a projection, not a record: it cannot authorize,
/// promote, or alter the integration.  The packet only applies to the one
/// complete integration prepared for a sealed cycle; presenting an ordinary
/// per-card integration as a final decision would be misleading.
#[allow(clippy::too_many_lines)]
fn run_decision_packet(args: &InspectArgs) -> Result<CommandOutcome, HarnessError> {
    let integration_id: IntegrationId = args.integration_id.parse()?;
    let control = ControlRepository::open(&args.control)?;
    let config = control.project()?;
    let record = load_integration(&control, &integration_id)?;
    if !record.final_for_cycle {
        return Err(HarnessError::Control {
            reason: format!(
                "integration {integration_id} is not the final integration for its sealed cycle"
            ),
            code: ErrorCode::PolicyDecisionPacketFinalOnly,
        });
    }
    let cycle = load_cycle(&control, &record.cycle_id)?;
    let Some(sealed_cycle_digest) = record.sealed_cycle_digest.as_ref() else {
        return Err(HarnessError::Control {
            reason: format!("final integration {integration_id} has no sealed cycle digest"),
            code: ErrorCode::InternalControlCorrupt,
        });
    };
    if cycle.status != CycleStatus::Sealed || &Digest::of_canonical(&cycle)? != sealed_cycle_digest
    {
        return Err(HarnessError::Control {
            reason: format!("final integration {integration_id} no longer binds its sealed cycle"),
            code: ErrorCode::InternalControlCorrupt,
        });
    }

    let selected: std::collections::BTreeSet<String> = record
        .members
        .iter()
        .map(|member| member.card_id.to_string())
        .collect();
    let abandoned: std::collections::BTreeSet<String> = record
        .abandoned_card_ids
        .iter()
        .map(ToString::to_string)
        .collect();
    let mut cards = Vec::new();
    for card_id in &cycle.card_ids {
        let key = card_id.to_string();
        if !selected.contains(&key) && !abandoned.contains(&key) {
            return Err(HarnessError::Control {
                reason: format!("final integration {integration_id} omits sealed card {card_id}"),
                code: ErrorCode::InternalControlCorrupt,
            });
        }
        let (card, _) = load_card(&control, card_id)?;
        cards.push(serde_json::json!({
            "card_id": key,
            "disposition": if selected.contains(&card_id.to_string()) { "selected" } else { "abandoned" },
            "rollback_strategy": card.rollback_strategy,
        }));
    }
    cards.sort_by(|left, right| left["card_id"].as_str().cmp(&right["card_id"].as_str()));

    let exceptions = exception_projection(&control, &config, &record)?;
    let (landing, verification, review, mut next_action) = match record.status {
        IntegrationStatus::Prepared
            if record.integration_head.is_none() && record.landing_sha.is_none() =>
        {
            (
                serde_json::json!({"state":"not_built","sha":null,"tree":null}),
                serde_json::Value::Null,
                serde_json::Value::Null,
                "integration.merge",
            )
        }
        IntegrationStatus::Prepared if record.landing_sha.is_some() => (
            serde_json::json!({"state":"built","sha":record.landing_sha,"tree":record.integration_tree}),
            serde_json::Value::Null,
            serde_json::Value::Null,
            "integration.verify",
        ),
        IntegrationStatus::Prepared => (
            serde_json::json!({"state":"not_built","sha":null,"tree":null}),
            serde_json::Value::Null,
            serde_json::Value::Null,
            "integration.land",
        ),
        IntegrationStatus::Verified | IntegrationStatus::Reviewed | IntegrationStatus::Accepted => {
            let Some(landing_sha) = record.landing_sha.as_ref() else {
                return Err(HarnessError::Control {
                    reason: format!(
                        "{integration_id} is {} without a landing commit",
                        record.status.name()
                    ),
                    code: ErrorCode::InternalControlCorrupt,
                });
            };
            let verified = load_verification(&control, &integration_id)?;
            if verified.landing_sha != *landing_sha || !verified.passed() {
                return Err(HarnessError::Control {
                    reason: format!(
                        "verification for {integration_id} does not prove its exact landing commit"
                    ),
                    code: ErrorCode::InternalControlCorrupt,
                });
            }
            (
                serde_json::json!({"state":"built","sha":landing_sha,"tree":record.integration_tree}),
                serde_json::json!({"landing_sha":verified.landing_sha,"landing_tree":verified.landing_tree,"receipt_ids":verified.receipt_ids,"verified_by":verified.verified_by}),
                if matches!(
                    record.status,
                    IntegrationStatus::Reviewed | IntegrationStatus::Accepted
                ) {
                    serde_json::json!({"state":"recorded"})
                } else {
                    serde_json::json!({"state":"not_recorded"})
                },
                if record.status == IntegrationStatus::Accepted {
                    "integration.promote"
                } else if record.status == IntegrationStatus::Reviewed {
                    "acceptance.record"
                } else {
                    "integration.review"
                },
            )
        }
        _ => {
            return Err(HarnessError::Control {
                reason: format!(
                    "decision packet is unavailable while integration {integration_id} is `{}`",
                    record.status.name()
                ),
                code: ErrorCode::PolicyInvalidTransition,
            });
        }
    };

    if exceptions["state"] == "pending" {
        next_action = "integration.exception.resolve";
    }

    let data = serde_json::json!({
        "packet_schema":"harness.decision-packet/v1",
        "packet_version":1,
        "integration_id":record.integration_id,
        "integration_digest":record.digest()?.as_str(),
        "cycle":{"cycle_id":cycle.cycle_id,"objective":cycle.objective,"sealed_cycle_digest":sealed_cycle_digest},
        "authority":{"expected_main_sha":record.expected_main_sha,"integration_head":record.integration_head,"integration_tree":record.integration_tree},
        "landing":landing,
        "accounting":{"selected_card_ids":record.members.iter().map(|m|m.card_id.to_string()).collect::<Vec<_>>(),"abandoned_card_ids":record.abandoned_card_ids,"cards":cards},
        "verification":verification,
        "review":review,
        "residual_risks":[],
        "final_rollback":"not_recorded",
        "exceptions":exceptions,
        "decision_readiness":{"current":record.status.name(),"next_permitted_action":next_action,"is_authorization":false}
    });
    Ok(CommandOutcome::new(
        "integration.decision-packet",
        format!("Decision packet for {integration_id}\ncycle: {}\nnext action: {next_action}\nThis packet is not authorization.", record.cycle_id),
        data,
    ).with_project(config.project_id))
}

/// The immutable facts of one raised exception, projected from the event log.
#[derive(Clone, Debug)]
pub(crate) struct RaisedException {
    pub event_id: String,
    pub trigger: String,
    pub evidence_refs: Vec<String>,
    pub raiser_actor_id: String,
    pub raised_at: String,
    pub policy_digest: String,
    pub integration_digest: String,
    pub sealed_cycle_digest: String,
    pub resolution: Option<ExceptionResolution>,
}

#[derive(Clone, Debug)]
pub(crate) struct ExceptionResolution {
    pub event_id: String,
    pub authorizer_actor_id: String,
    pub resolved_at: String,
}

/// Reads the append-only exception facts for a final integration and refuses
/// malformed or mismatched facts rather than treating them as absent.
#[allow(clippy::too_many_lines)]
pub(crate) fn exceptions_for(
    control: &ControlRepository,
    config: &crate::config::ProjectConfig,
    record: &IntegrationRecord,
) -> Result<Vec<RaisedException>, HarnessError> {
    // Exception authority exists only for the new final-cycle path.  Ordinary
    // v1 integrations must retain their historical acceptance/promotion
    // behaviour even when a project later opts into final authorization.
    if !record.final_for_cycle {
        return Ok(Vec::new());
    }
    let events =
        crate::control::event_store::EventStore::new(control).for_cycle(&record.cycle_id)?;
    let has_exception_fact = events.iter().any(|event| {
        matches!(
            event.event_type.as_str(),
            "integration.exception_raised" | "integration.exception_resolved"
        ) && event
            .metadata
            .get("integration_id")
            .and_then(serde_json::Value::as_str)
            == Some(record.integration_id.as_str())
    });
    if !has_exception_fact {
        return Ok(Vec::new());
    }
    let expected_integration_digest = record.substantive_digest()?.as_str().to_owned();
    let policy = config.final_authorization_policy.as_ref().ok_or_else(|| {
        HarnessError::ControlWithRecovery {
            reason: "final authorization policy is not configured for exception validation"
                .to_owned(),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_AUTHORIZATION_POLICY_NOT_CONFIGURED_RECOVERY,
        }
    })?;
    let expected_policy_digest = policy.digest()?.as_str().to_owned();
    let expected_seal = record
        .sealed_cycle_digest
        .as_ref()
        .ok_or_else(|| HarnessError::Control {
            reason: format!(
                "final integration {} has no sealed cycle digest",
                record.integration_id
            ),
            code: ErrorCode::InternalControlCorrupt,
        })?
        .as_str()
        .to_owned();
    let mut raised = Vec::new();
    let mut resolutions = std::collections::BTreeMap::new();
    for event in &events {
        let names_integration = event
            .metadata
            .get("integration_id")
            .and_then(serde_json::Value::as_str);
        if names_integration != Some(record.integration_id.as_str()) {
            continue;
        }
        if event.event_type == "integration.exception_raised" {
            let required = |key: &str| -> Result<String, HarnessError> {
                event
                    .metadata
                    .get(key)
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| HarnessError::Control {
                        reason: format!("exception event {} has no valid `{key}`", event.event_id),
                        code: ErrorCode::InternalControlCorrupt,
                    })
            };
            let integration_digest = required("integration_digest")?;
            let sealed_cycle_digest = required("sealed_cycle_digest")?;
            let policy_digest = required("policy_digest")?;
            if integration_digest != expected_integration_digest
                || sealed_cycle_digest != expected_seal
                || policy_digest != expected_policy_digest
            {
                return Err(HarnessError::ControlWithRecovery {
                    reason: format!(
                        "exception event {} no longer binds final integration {}",
                        event.event_id, record.integration_id
                    ),
                    code: ErrorCode::PolicyNotAccepted,
                    recovery: FINAL_AUTHORIZATION_STALE_RECOVERY,
                });
            }
            let evidence_refs = event
                .metadata
                .get("evidence_refs")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| HarnessError::Control {
                    reason: format!(
                        "exception event {} has no valid evidence_refs",
                        event.event_id
                    ),
                    code: ErrorCode::InternalControlCorrupt,
                })?
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(ToOwned::to_owned)
                        .ok_or_else(|| HarnessError::Control {
                            reason: format!(
                                "exception event {} has a non-string evidence reference",
                                event.event_id
                            ),
                            code: ErrorCode::InternalControlCorrupt,
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            if evidence_refs.is_empty() {
                return Err(HarnessError::Control {
                    reason: format!(
                        "exception event {} has no evidence references",
                        event.event_id
                    ),
                    code: ErrorCode::InternalControlCorrupt,
                });
            }
            raised.push(RaisedException {
                event_id: event.event_id.to_string(),
                trigger: required("trigger")?,
                evidence_refs,
                raiser_actor_id: event.actor_id.clone(),
                raised_at: event.occurred_at.to_string(),
                policy_digest,
                integration_digest,
                sealed_cycle_digest,
                resolution: None,
            });
        } else if event.event_type == "integration.exception_resolved" {
            // A malformed, mismatched, outsider, unknown-target, or duplicate
            // resolution is not authority.  It is preserved for audit, while
            // the original raised event remains pending and continues to gate
            // acceptance/promotion fail-closed.
            let target = event
                .metadata
                .get("exception_event_id")
                .and_then(serde_json::Value::as_str);
            let valid = target.is_some()
                && event
                    .metadata
                    .get("resolution")
                    .and_then(serde_json::Value::as_str)
                    == Some("continue")
                && event
                    .metadata
                    .get("policy_digest")
                    .and_then(serde_json::Value::as_str)
                    == Some(expected_policy_digest.as_str())
                && event
                    .metadata
                    .get("integration_digest")
                    .and_then(serde_json::Value::as_str)
                    == Some(expected_integration_digest.as_str())
                && event
                    .metadata
                    .get("sealed_cycle_digest")
                    .and_then(serde_json::Value::as_str)
                    == Some(expected_seal.as_str())
                && policy.authorizes(&event.actor_id)
                && target.is_some_and(|id| raised.iter().any(|item| item.event_id == id))
                && target.is_some_and(|id| !resolutions.contains_key(id));
            if valid {
                let target = target.expect("checked above");
                resolutions.insert(
                    target.to_owned(),
                    ExceptionResolution {
                        event_id: event.event_id.to_string(),
                        authorizer_actor_id: event.actor_id.clone(),
                        resolved_at: event.occurred_at.to_string(),
                    },
                );
            }
        }
    }
    for exception in &mut raised {
        exception.resolution = resolutions.remove(&exception.event_id);
    }
    Ok(raised)
}

pub(crate) fn require_no_pending_exception(
    control: &ControlRepository,
    config: &crate::config::ProjectConfig,
    record: &IntegrationRecord,
) -> Result<(), HarnessError> {
    if let Some(exception) = exceptions_for(control, config, record)?
        .into_iter()
        .find(|item| item.resolution.is_none())
    {
        return Err(HarnessError::Control {
            reason: format!(
                "integration {} is waiting for exception {} ({})",
                record.integration_id, exception.event_id, exception.trigger
            ),
            code: ErrorCode::PolicyExceptionPending,
        });
    }
    Ok(())
}

fn exception_projection(
    control: &ControlRepository,
    config: &crate::config::ProjectConfig,
    record: &IntegrationRecord,
) -> Result<serde_json::Value, HarnessError> {
    let exceptions = exceptions_for(control, config, record)?;
    let pending = exceptions.iter().any(|item| item.resolution.is_none());
    let resolved_next_action = if record.status == IntegrationStatus::Accepted {
        "integration.promote"
    } else {
        "acceptance.record"
    };
    Ok(serde_json::json!({
        "state": if pending { "pending" } else if exceptions.is_empty() { "none" } else { "resolved" },
        "items": exceptions.into_iter().map(|item| serde_json::json!({
            "event_id":item.event_id,
            "trigger":item.trigger,
            "evidence_refs":item.evidence_refs,
            "raiser_actor_id":item.raiser_actor_id,
            "raised_at":item.raised_at,
            "policy_digest":item.policy_digest,
            "integration_digest":item.integration_digest,
            "sealed_cycle_digest":item.sealed_cycle_digest,
            "state":if item.resolution.is_none(){"pending"}else{"resolved"},
            "resolution":item.resolution.as_ref().map(|resolution|serde_json::json!({"event_id":resolution.event_id,"authorizer_actor_id":resolution.authorizer_actor_id,"resolved_at":resolution.resolved_at,"disposition":"continue"})),
            "next_action":if item.resolution.is_none(){"integration.exception.resolve or integration.abandon --reason exception:<event-id>"}else{resolved_next_action},
        })).collect::<Vec<_>>(),
    }))
}

fn exception_bindings(
    control: &ControlRepository,
    config: &crate::config::ProjectConfig,
    record: &IntegrationRecord,
) -> Result<
    (
        crate::domain::digest::Digest,
        crate::domain::digest::Digest,
        crate::domain::digest::Digest,
    ),
    HarnessError,
> {
    if !record.final_for_cycle
        || !matches!(
            record.status,
            IntegrationStatus::Reviewed | IntegrationStatus::Accepted
        )
    {
        return Err(HarnessError::Control {
            reason: format!(
                "exceptions may be raised only for reviewed or accepted final integrations; {} is `{}`",
                record.integration_id,
                record.status.name()
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    }
    let policy = config.final_authorization_policy.as_ref().ok_or_else(|| {
        HarnessError::ControlWithRecovery {
            reason: "final authorization policy does not declare exception triggers".to_owned(),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_AUTHORIZATION_POLICY_NOT_CONFIGURED_RECOVERY,
        }
    })?;
    let cycle = load_cycle(control, &record.cycle_id)?;
    let seal = record
        .sealed_cycle_digest
        .as_ref()
        .ok_or_else(|| HarnessError::Control {
            reason: format!(
                "final integration {} has no sealed cycle digest",
                record.integration_id
            ),
            code: ErrorCode::InternalControlCorrupt,
        })?;
    let actual = Digest::of_canonical(&cycle)?;
    if cycle.status != CycleStatus::Sealed || &actual != seal {
        return Err(HarnessError::ControlWithRecovery {
            reason: format!(
                "final integration {} no longer binds its sealed cycle",
                record.integration_id
            ),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_INTEGRATION_SEAL_STALE_RECOVERY,
        });
    }
    Ok((policy.digest()?, record.substantive_digest()?, seal.clone()))
}

fn validate_exception_authorizer(
    config: &crate::config::ProjectConfig,
    authorizer: &str,
) -> Result<(), HarnessError> {
    let policy = config.final_authorization_policy.as_ref().ok_or_else(|| {
        HarnessError::ControlWithRecovery {
            reason: "final authorization policy is not configured".to_owned(),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_AUTHORIZATION_POLICY_NOT_CONFIGURED_RECOVERY,
        }
    })?;
    if policy.authorizes(authorizer) {
        Ok(())
    } else {
        Err(HarnessError::ControlWithRecovery {
            reason: format!(
                "actor {authorizer} is not configured to resolve final integration exceptions"
            ),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_AUTHORIZATION_ACTOR_NOT_AUTHORIZED_RECOVERY,
        })
    }
}

fn run_exception_raise(
    args: &ExceptionRaiseArgs,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    let integration_id: IntegrationId = args.integration_id.parse()?;
    let trigger = crate::config::ExceptionTrigger::parse(&args.trigger)?;
    if args
        .evidence_refs
        .iter()
        .any(|value| value.trim().is_empty())
    {
        return Err(HarnessError::Control {
            reason: "exception evidence references must be non-empty".to_owned(),
            code: ErrorCode::UsageInvalidArguments,
        });
    }
    let validate = |control: &ControlRepository| -> Result<
        (
            crate::config::ProjectConfig,
            IntegrationRecord,
            crate::domain::digest::Digest,
            crate::domain::digest::Digest,
            crate::domain::digest::Digest,
        ),
        HarnessError,
    > {
        let config = control.project()?;
        let record = load_integration(control, &integration_id)?;
        let (policy_digest, integration_digest, seal) =
            exception_bindings(control, &config, &record)?;
        let policy = config
            .final_authorization_policy
            .as_ref()
            .expect("checked above");
        if !policy.enables_exception(trigger) {
            return Err(HarnessError::ControlWithRecovery {
                reason: format!(
                    "exception trigger `{}` is not enabled by final authorization policy",
                    trigger.name()
                ),
                code: ErrorCode::PolicyNotAccepted,
                recovery: EXCEPTION_TRIGGER_NOT_ENABLED_RECOVERY,
            });
        }
        require_no_pending_exception(control, &config, &record)?;
        Ok((config, record, policy_digest, integration_digest, seal))
    };
    if args.dry_run {
        let control = ControlRepository::open(&args.control)?;
        let (config, _record, _, _, _) = validate(&control)?;
        return Ok(CommandOutcome::new("integration.exception.raise", format!("Dry run: would raise `{}` for {integration_id}\\nnothing was changed", trigger.name()), serde_json::json!({"dry_run":true,"integration_id":integration_id.to_string(),"trigger":trigger.name(),"evidence_refs":args.evidence_refs})).with_project(config.project_id));
    }
    with_transaction(
        &args.control,
        "integration.exception.raise",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let (config, record, policy_digest, integration_digest, seal) = validate(control)?;
            let event = events.append(
                &config.project_id,
                EventDraft::new("integration.exception_raised", &args.actor_id)
                    .cycle(record.cycle_id.clone())
                    .head(record.landing_sha.clone().unwrap_or_default())
                    .meta(
                        "integration_id",
                        serde_json::json!(integration_id.to_string()),
                    )
                    .meta("trigger", serde_json::json!(trigger.name()))
                    .meta("evidence_refs", serde_json::json!(args.evidence_refs))
                    .meta("policy_digest", serde_json::json!(policy_digest.as_str()))
                    .meta(
                        "integration_digest",
                        serde_json::json!(integration_digest.as_str()),
                    )
                    .meta("sealed_cycle_digest", serde_json::json!(seal.as_str())),
                clock,
            )?;
            control.commit(
                expected,
                &format!("integration: raise exception {integration_id}"),
            )?;
            Ok(CommandOutcome::new("integration.exception.raise", format!("Raised exception {} for {integration_id}; acceptance and promotion are blocked until it is resolved or the integration is abandoned", event.event_id), serde_json::json!({"integration_id":integration_id.to_string(),"exception_event_id":event.event_id.to_string(),"trigger":trigger.name(),"status":"pending"})).with_project(config.project_id))
        },
    )
}

fn run_exception_resolve(
    args: &ExceptionResolveArgs,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    let integration_id: IntegrationId = args.integration_id.parse()?;
    if args.resolution != "continue" {
        return Err(HarnessError::Control { reason: "only exception resolution `continue` is supported; use integration abandon --reason exception:<event-id> to stop".to_owned(), code: ErrorCode::UsageInvalidArguments });
    }
    let validate = |control: &ControlRepository| -> Result<
        (
            crate::config::ProjectConfig,
            IntegrationRecord,
            RaisedException,
        ),
        HarnessError,
    > {
        let config = control.project()?;
        let record = load_integration(control, &integration_id)?;
        exception_bindings(control, &config, &record)?;
        validate_exception_authorizer(&config, &args.authorizer_actor_id)?;
        let exception = exceptions_for(control, &config, &record)?
            .into_iter()
            .find(|item| item.event_id == args.exception_event_id)
            .ok_or_else(|| HarnessError::Control {
                reason: format!(
                    "exception {} does not belong to integration {integration_id}",
                    args.exception_event_id
                ),
                code: ErrorCode::PreconditionNotFound,
            })?;
        if exception.resolution.is_some() {
            return Err(HarnessError::Control {
                reason: format!("exception {} is already resolved", args.exception_event_id),
                code: ErrorCode::PolicyInvalidTransition,
            });
        }
        Ok((config, record, exception))
    };
    if args.dry_run {
        let control = ControlRepository::open(&args.control)?;
        let (config, _, _) = validate(&control)?;
        return Ok(CommandOutcome::new("integration.exception.resolve", format!("Dry run: would resolve exception {} with `continue`\\nnothing was changed", args.exception_event_id), serde_json::json!({"dry_run":true,"integration_id":integration_id.to_string(),"exception_event_id":args.exception_event_id,"resolution":"continue"})).with_project(config.project_id));
    }
    with_transaction(
        &args.control,
        "integration.exception.resolve",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let (config, record, _) = validate(control)?;
            let (policy_digest, integration_digest, sealed_cycle_digest) =
                exception_bindings(control, &config, &record)?;
            let event = events.append(
                &config.project_id,
                EventDraft::new("integration.exception_resolved", &args.authorizer_actor_id)
                    .cycle(record.cycle_id.clone())
                    .head(record.landing_sha.clone().unwrap_or_default())
                    .meta(
                        "integration_id",
                        serde_json::json!(integration_id.to_string()),
                    )
                    .meta(
                        "exception_event_id",
                        serde_json::json!(args.exception_event_id),
                    )
                    .meta("resolution", serde_json::json!("continue"))
                    .meta("policy_digest", serde_json::json!(policy_digest.as_str()))
                    .meta(
                        "integration_digest",
                        serde_json::json!(integration_digest.as_str()),
                    )
                    .meta(
                        "sealed_cycle_digest",
                        serde_json::json!(sealed_cycle_digest.as_str()),
                    ),
                clock,
            )?;
            control.commit(
                expected,
                &format!("integration: resolve exception {integration_id}"),
            )?;
            Ok(CommandOutcome::new("integration.exception.resolve", format!("Resolved exception {} with `continue`; this does not authorize {integration_id}", args.exception_event_id), serde_json::json!({"integration_id":integration_id.to_string(),"exception_event_id":args.exception_event_id,"resolution_event_id":event.event_id.to_string(),"resolution":"continue","status":"resolved","authorization":"not_recorded"})).with_project(config.project_id))
        },
    )
}

/// Simulates the merge sequence without touching a ref, index, or worktree.
///
/// Each step feeds the next: `merge-tree` yields a tree, and merging the next
/// candidate needs a commit, so an unreachable commit object carries the state
/// forward. Nothing points at those commits, so a preflight leaves no state a
/// later reader can observe — which is what makes it safe to run at any time.
///
/// The sequence stops at the first conflict. Continuing would merge later
/// candidates against a state that will never exist, and reporting conflicts
/// from an imaginary tree is worse than reporting none.
fn simulate(
    repository: &std::path::Path,
    record: &IntegrationRecord,
) -> Result<Preflight, HarnessError> {
    let mut head = record.expected_main_sha.clone();
    let mut steps = Vec::new();

    for (index, member) in record.members.iter().enumerate() {
        let preview = merge_tree(repository, &head, &member.candidate_sha)?;
        let clean = preview.is_clean();
        steps.push(PreflightStep {
            card_id: member.card_id.clone(),
            candidate_sha: member.candidate_sha.clone(),
            conflicts: preview.conflicts.clone(),
        });
        if !clean {
            return Ok(Preflight {
                steps,
                unevaluated: record.members.len() - index - 1,
                tree: None,
            });
        }
        head = integration_worktree::commit_tree(
            repository,
            &preview.tree,
            &[&head, &member.candidate_sha],
            &format!(
                "preflight: {} into {}",
                member.card_id, record.integration_id
            ),
        )?;
    }

    let tree = integration_worktree::tree_of(repository, &head)?;
    Ok(Preflight {
        steps,
        unevaluated: 0,
        tree: Some(tree),
    })
}

/// One candidate's simulated merge.
struct PreflightStep {
    card_id: CardId,
    candidate_sha: String,
    conflicts: Vec<Conflict>,
}

/// The whole simulated sequence.
struct Preflight {
    steps: Vec<PreflightStep>,
    /// Members after the first conflict, which were deliberately not simulated.
    unevaluated: usize,
    /// The tree the sequence would produce, when it is conflict-free.
    tree: Option<String>,
}

impl Preflight {
    /// True when every member merged cleanly.
    fn is_clean(&self) -> bool {
        self.steps.iter().all(|step| step.conflicts.is_empty())
    }

    /// The first step that conflicted.
    fn blocking(&self) -> Option<&PreflightStep> {
        self.steps.iter().find(|step| !step.conflicts.is_empty())
    }

    /// The machine payload shared by `preflight` and a refused `merge`.
    fn payload(&self, record: &IntegrationRecord) -> serde_json::Value {
        serde_json::json!({
            "integration_id": record.integration_id.to_string(),
            "expected_main_sha": record.expected_main_sha,
            "clean": self.is_clean(),
            "resulting_tree": self.tree,
            "unevaluated_members": self.unevaluated,
            "steps": self.steps.iter().map(|step| serde_json::json!({
                "card_id": step.card_id.to_string(),
                "candidate_sha": step.candidate_sha,
                "conflicts": step.conflicts,
                "textual_conflicts": step.conflicts.iter()
                    .filter(|conflict| conflict.kind.class() == ConflictClass::Textual)
                    .count(),
                "structural_conflicts": step.conflicts.iter()
                    .filter(|conflict| conflict.kind.class() == ConflictClass::Structural)
                    .count(),
            })).collect::<Vec<_>>(),
        })
    }

    /// The human rendering.
    fn text(&self, record: &IntegrationRecord) -> String {
        let mut text = format!(
            "Preflight for {} against {}",
            record.integration_id, record.expected_main_sha
        );
        for step in &self.steps {
            if step.conflicts.is_empty() {
                let _ = std::fmt::Write::write_fmt(
                    &mut text,
                    format_args!("\n  {} merges cleanly", step.card_id),
                );
                continue;
            }
            let _ = std::fmt::Write::write_fmt(
                &mut text,
                format_args!("\n  {} CONFLICTS:", step.card_id),
            );
            for conflict in &step.conflicts {
                let _ = std::fmt::Write::write_fmt(
                    &mut text,
                    format_args!(
                        "\n    [{}] {}: {}",
                        conflict.kind.name(),
                        conflict.paths.join(", "),
                        conflict.detail
                    ),
                );
            }
        }
        if self.unevaluated > 0 {
            let _ = std::fmt::Write::write_fmt(
                &mut text,
                format_args!(
                    "\n  {} later member(s) were not simulated: the sequence stops at the first conflict",
                    self.unevaluated
                ),
            );
        }
        if !self.is_clean() {
            text.push_str(
                "\nresolve this in an integration fix card; the harness never resolves a conflict for you",
            );
        }
        text
    }
}

fn run_preflight(args: &InspectArgs) -> Result<CommandOutcome, HarnessError> {
    let integration_id: IntegrationId = args.integration_id.parse()?;
    let control = ControlRepository::open(&args.control)?;
    let config = control.project()?;
    let record = load_integration(&control, &integration_id)?;

    let preflight = simulate(&config.repository, &record)?;
    Ok(CommandOutcome::new(
        "integration.preflight",
        preflight.text(&record),
        preflight.payload(&record),
    )
    .with_project(config.project_id))
}

/// Refuses a member whose branch no longer holds the approved candidate.
///
/// The plan pinned an exact commit. If the branch has moved since, merging the
/// pinned commit would silently drop work the actor believes is included, and
/// merging the branch would integrate something no reviewer approved. Neither
/// is acceptable, so the integration is refused and re-prepared instead.
fn require_pinned_candidates(
    repository: &std::path::Path,
    record: &IntegrationRecord,
) -> Result<(), HarnessError> {
    for member in &record.members {
        let actual = inspect::resolve_commit(
            &GitScope::work_tree(repository),
            &format!("refs/heads/{}", member.branch),
        )?;
        if actual != member.candidate_sha {
            return Err(HarnessError::Control {
                reason: format!(
                    "card {} was planned at {} but branch {} is now {actual}; re-prepare the integration",
                    member.card_id, member.candidate_sha, member.branch
                ),
                code: ErrorCode::PolicyNotIntegrable,
            });
        }
    }
    Ok(())
}

/// Runs the intermediate gates against the integration worktree as it stands.
///
/// Failure names the candidate that had just been merged. Running these only
/// once at the end would prove the combination is broken without saying which
/// addition broke it, which is the question an actor actually has.
fn run_smoke_gates(
    control: &ControlRepository,
    worktree: &std::path::Path,
    integration_id: &IntegrationId,
    after: &CardId,
    gates: &[String],
    clock: &dyn Clock,
) -> Result<(), HarnessError> {
    for gate_id in gates {
        let gate = load_gate(control, gate_id)?;
        let log_root = control
            .path(LOG_DIR)
            .join(integration_id.as_str())
            .join(after.as_str());
        let outcome = run_attempt(&gate, worktree, &log_root, 1, clock)?;
        if !outcome.passed() {
            return Err(HarnessError::Control {
                reason: format!(
                    "smoke gate {gate_id} failed after merging {after} into {integration_id}"
                ),
                code: ErrorCode::GateFailed,
            });
        }
    }
    Ok(())
}

/// Combines the planned candidates in a disposable worktree.
///
/// The worktree is removed on every path, success or failure. Leaving a
/// conflicted merge on disk would block the next attempt and invite someone to
/// "fix" it by hand, which would produce a landing tree no plan describes.
fn build_integration(
    control: &ControlRepository,
    config: &crate::config::ProjectConfig,
    record: &IntegrationRecord,
    smoke_gates: &[String],
    clock: &dyn Clock,
) -> Result<(String, String), HarnessError> {
    let repository = config.repository.as_path();
    let path =
        integration_worktree::path_for(&config.worktree_root, record.integration_id.as_str());
    integration_worktree::remove(repository, &path)?;
    integration_worktree::create(repository, &path, &record.expected_main_sha)?;

    let outcome = (|| -> Result<(String, String), HarnessError> {
        let mut head = record.expected_main_sha.clone();
        for member in &record.members {
            head = integration_worktree::merge(
                &path,
                &member.candidate_sha,
                &format!(
                    "integrate {} into {}",
                    member.card_id, record.integration_id
                ),
            )
            .inspect_err(|_| integration_worktree::abort_merge(&path))?;
            run_smoke_gates(
                control,
                &path,
                &record.integration_id,
                &member.card_id,
                smoke_gates,
                clock,
            )?;
        }
        // Section 13.2: the integration worktree must be clean before final
        // verification. A dirty one here means a merge left something behind.
        if !integration_worktree::is_clean(&path)? {
            return Err(HarnessError::Control {
                reason: format!(
                    "integration worktree {} is not clean after merging",
                    path.display()
                ),
                code: ErrorCode::ConflictMergeFailed,
            });
        }
        let tree = integration_worktree::tree_of(&path, &head)?;
        Ok((head, tree))
    })();

    integration_worktree::remove(repository, &path)?;
    outcome
}

// 71-R4's post-transaction sequencing — capturing the merge transaction's
// result in full before ever deciding whether to record a failure fact — has
// to stay visible in this one function: splitting it into a helper the
// length limit would otherwise invite risks hiding, at a call site, exactly
// the "never nested inside the failed transaction" ordering the frozen
// contract requires.
#[allow(clippy::too_many_lines)]
fn run_merge(args: &MergeArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let integration_id: IntegrationId = args.integration_id.parse()?;

    if args.dry_run {
        let control = ControlRepository::open(&args.control)?;
        let config = control.project()?;
        let record = load_integration(&control, &integration_id)?;
        // 73-2: the first check able to refuse once the integration record
        // and project config both exist — see `require_cycle_convergence_
        // budget`. A preview must never promise a merge the real command
        // would refuse for an escalated cycle.
        require_cycle_convergence_budget(&control, &config, &record.cycle_id)?;
        require_fresh_authority(&config, &record)?;
        require_pinned_candidates(&config.repository, &record)?;
        let preflight = simulate(&config.repository, &record)?;
        let mut payload = preflight.payload(&record);
        payload["dry_run"] = serde_json::json!(true);
        return Ok(CommandOutcome::new(
            "integration.merge",
            format!("Dry run: {}\nnothing was changed", preflight.text(&record)),
            payload,
        )
        .with_project(config.project_id));
    }

    let outcome = with_transaction(
        &args.control,
        "integration.merge",
        clock,
        |control, events, expected, steps| {
            let config = control.project()?;
            let mut record = load_integration(control, &integration_id)?;
            // 73-2: the first check able to refuse once the record and
            // config both exist, before any write — a raw retry of a merge
            // that already exhausted the cycle's budget must not be able to
            // try again. See `require_cycle_convergence_budget`.
            require_cycle_convergence_budget(control, &config, &record.cycle_id)?;
            if record.status != IntegrationStatus::Prepared {
                return Err(HarnessError::Control {
                    reason: format!(
                        "integration {integration_id} is `{}`; only a prepared integration can be merged",
                        record.status.name()
                    ),
                    code: ErrorCode::PolicyInvalidTransition,
                });
            }
            // Merging twice would build a second head from the same plan and
            // overwrite the first, leaving whatever `WP-430` and `WP-440`
            // already did pointing at a commit the record no longer names.
            if let Some(existing) = &record.integration_head {
                return Err(HarnessError::Control {
                    reason: format!(
                        "integration {integration_id} was already merged at {existing}; abandon it and prepare again to rebuild"
                    ),
                    code: ErrorCode::PolicyInvalidTransition,
                });
            }
            require_fresh_authority(&config, &record)?;
            require_pinned_candidates(&config.repository, &record)?;

            // The preflight runs first so a conflict is reported with its
            // classification rather than as a bare Git failure, and so nothing
            // is built when the sequence cannot succeed.
            let preflight = simulate(&config.repository, &record)?;
            if !preflight.is_clean() {
                let blocking = preflight
                    .blocking()
                    .map_or_else(|| "unknown".to_owned(), |step| step.card_id.to_string());
                return Err(HarnessError::Control {
                    reason: format!(
                        "integration {integration_id} cannot be merged: {blocking} conflicts; run `integration preflight` for detail"
                    ),
                    code: ErrorCode::ConflictMergeFailed,
                });
            }

            steps.at("integration-worktree-built")?;
            let (head, tree) =
                build_integration(control, &config, &record, &args.smoke_gates, clock)?;
            record.integration_head = Some(head.clone());
            record.integration_tree = Some(tree.clone());
            record.merged_at = Some(clock.now());
            let digest = record.digest()?;

            control.write_atomic(
                &IntegrationRecord::relative_path(&integration_id),
                &format!("{}\n", serde_json::to_string_pretty(&record)?),
            )?;

            events.append(
                &config.project_id,
                EventDraft::new("integration.merged", &args.actor_id)
                    .cycle(record.cycle_id.clone())
                    .head(head.clone())
                    .meta(
                        "integration_id",
                        serde_json::json!(integration_id.to_string()),
                    )
                    .meta("integration_digest", serde_json::json!(digest.as_str()))
                    .meta("integration_tree", serde_json::json!(tree))
                    .meta("members", serde_json::json!(record.members.len())),
                clock,
            )?;
            control.commit(expected, &format!("integration: merge {integration_id}"))?;

            Ok(CommandOutcome::new(
                "integration.merge",
                format!(
                    "Merged integration {integration_id}\nintegration head: {head}\nintegration tree: {tree}\nmembers merged: {}\nthe disposable worktree was removed",
                    record.members.len()
                ),
                serde_json::json!({
                    "integration_id": integration_id.to_string(),
                    "integration_digest": digest.as_str(),
                    "status": record.status.name(),
                    "expected_main_sha": record.expected_main_sha,
                    "integration_head": head,
                    "integration_tree": tree,
                    "members": record.members,
                }),
            )
            .with_project(config.project_id.clone()))
        },
    );

    // 71-R4: the transaction above has just failed and rolled back — every
    // path that reaches `ConflictMergeFailed` inside it, in `build_integration`
    // and in the preflight check, returns before writing the integration
    // record, appending `integration.merged`, or calling `control.commit`, so
    // nothing about the attempted integration is ever durable. What runs next
    // is a second, independent transaction, never nested inside the one that
    // just failed — see `record_integration_failure` for why a refusal path
    // writes at all and what "writes the fact and nothing else" is checked
    // against.
    match outcome {
        Err(error) if error.code() == ErrorCode::ConflictMergeFailed => Err(finalize_conflict(
            &args.control,
            &integration_id,
            &args.actor_id,
            clock,
            error,
        )),
        other => other,
    }
}

/// The bare text of a [`HarnessError`], without the `"control state: "`
/// prefix both [`HarnessError::Control`] and
/// [`HarnessError::ControlWithRecovery`] add through the same
/// `#[error(...)]` Display (see #106).
///
/// Exists only so `finalize_conflict` can join two error reasons into one
/// sentence without nesting that prefix in its own middle.
fn plain_reason(error: &HarnessError) -> String {
    match error {
        HarnessError::Control { reason, .. } | HarnessError::ControlWithRecovery { reason, .. } => {
            reason.clone()
        }
        other => other.to_string(),
    }
}

/// Turns a `ConflictMergeFailed` from `run_merge` into the error it actually
/// returns, after attempting to record it.
///
/// Recording succeeding changes nothing about the refusal: the conflict is
/// still returned, unchanged, because a fact was recorded about it, not
/// instead of it. Recording failing must not be silent — rule 4 of the frozen
/// contract requires the returned error to say both that the integration
/// conflicted and that the failure could not be recorded — so this combines
/// the two rather than choosing one.
fn finalize_conflict(
    control_path: &std::path::Path,
    integration_id: &IntegrationId,
    actor_id: &str,
    clock: &dyn Clock,
    error: HarnessError,
) -> HarnessError {
    match record_integration_failure(control_path, integration_id, actor_id, clock) {
        Ok(()) => error,
        Err(record_error) => HarnessError::Control {
            reason: format!(
                "{}; additionally, the integration failure could not be recorded as a convergence fact: {}",
                plain_reason(&error),
                plain_reason(&record_error)
            ),
            code: ErrorCode::ConflictMergeFailed,
        },
    }
}

/// Records a cycle-bound `integration_failure` convergence fact for a merge
/// that just refused with [`ErrorCode::ConflictMergeFailed`].
///
/// This is a new precedent: every convergence fact recorded before 71-R4 was
/// written from a command's own successful transaction, alongside the state
/// that command was already writing. There is no such transaction here — the
/// merge failed and rolled back, on purpose, before this function is ever
/// called (see `finalize_conflict`'s caller) — so this opens a second,
/// complete transaction of its own, entirely after the first one's has ended.
/// Its only effect is to append one event and commit it: no integration
/// record update, no card state, no refs, no worktrees. That is what makes it
/// safe to write from what is otherwise a read-only refusal path — what is
/// committed is a complete record of one failed attempt, never a
/// half-finished integration.
///
/// Nesting the append inside the failed transaction instead was not an
/// option: that transaction must keep returning `Err` without calling
/// `control.commit`, which is exactly what leaves it clean and rolled back,
/// so any write placed inside it would either never be committed or would
/// force the failed merge itself to become durable to carry it.
///
/// Without a configured convergence policy, this returns `Ok(())` having
/// opened no transaction at all — not even one that would append nothing —
/// so an unconfigured project's refusal stays byte-for-byte what it was
/// before this fact existed.
///
/// # Errors
///
/// Returns an error when a policy is configured but the fact could not be
/// durably recorded. The caller combines this with the original conflict
/// rather than swallowing it.
fn record_integration_failure(
    control_path: &std::path::Path,
    integration_id: &IntegrationId,
    actor_id: &str,
    clock: &dyn Clock,
) -> Result<(), HarnessError> {
    let control = ControlRepository::open(control_path)?;
    let config = control.project()?;
    let Some(policy) = config.convergence_policy.as_ref() else {
        return Ok(());
    };
    let policy_digest = policy.digest()?;
    let record = load_integration(&control, integration_id)?;
    let project_id = config.project_id.clone();
    let cycle_id = record.cycle_id;
    // The field of the integration record naming the exact commit the merge
    // was attempted against: captured fresh from the authority at `integration
    // prepare` time (`build_record`) and re-verified equal to the live
    // protected branch by `require_fresh_authority`, which both
    // `ConflictMergeFailed` sites in `run_merge` run after. `integration_head`
    // is not a candidate — it is exactly the field this failed attempt never
    // got to set.
    let head = record.expected_main_sha;

    with_transaction(
        control_path,
        "integration.merge-failure",
        clock,
        move |control, events, expected, _steps| {
            events.append(
                &project_id,
                EventDraft::new(ATTEMPT_RECORDED_EVENT, actor_id)
                    .cycle(cycle_id)
                    .head(head)
                    .meta(
                        "attempt_kind",
                        serde_json::to_value(AttemptKind::IntegrationFailure)?,
                    )
                    .meta(
                        "reason_category",
                        serde_json::to_value(ReasonCategory::IntegrationConflict)?,
                    )
                    .meta(
                        "evidence_ref",
                        serde_json::json!(format!("integration:{integration_id}")),
                    )
                    .meta("policy_digest", serde_json::json!(policy_digest.as_str())),
                clock,
            )?;
            control.commit(
                expected,
                &format!("integration: record merge failure of {integration_id}"),
            )?;
            Ok(CommandOutcome::new(
                "integration.merge-failure",
                format!(
                    "recorded one integration_failure convergence fact for integration {integration_id}"
                ),
                serde_json::json!({"integration_id": integration_id.to_string()}),
            )
            .with_project(project_id))
        },
    )?;
    Ok(())
}

/// Refuses an integration whose cards declare *shared* generated artifacts.
///
/// `WP-540` classified generated paths and enforced their ownership, but did
/// not build integration-owned generation — a shared artifact still has nobody
/// to produce it. Landing one would ship a tree missing whatever it was meant
/// to contain, so it is refused with a stable code.
///
/// The other three classes now land normally. Before classification existed
/// this guard could not tell them apart and refused all of them, which was the
/// conservative reading available at the time.
fn require_no_generated_artifacts(
    control: &ControlRepository,
    record: &IntegrationRecord,
) -> Result<(), HarnessError> {
    for member in &record.members {
        let (card, _) = load_card(control, &member.card_id)?;
        let shared: Vec<&str> = card
            .generated_artifacts
            .iter()
            .filter(|artifact| artifact.class == crate::domain::artifact::ArtifactClass::Shared)
            .map(|artifact| artifact.path.as_str())
            .collect();
        if !shared.is_empty() {
            return Err(HarnessError::Control {
                reason: format!(
                    "card {} declares shared generated artifacts ({}), and nothing generates them during integration yet",
                    member.card_id,
                    shared.join(", ")
                ),
                code: ErrorCode::PolicyUnsupportedUntilWp540,
            });
        }
    }
    Ok(())
}

/// Builds the Section 13.5 trailers for an integration.
///
/// Every identifier a later reader needs to re-derive the landing is present in
/// the commit itself, so the commit remains explicable even to someone who has
/// only the candidate repository and not the control repository.
fn landing_trailers(
    control: &ControlRepository,
    record: &IntegrationRecord,
    digest: &crate::domain::digest::Digest,
) -> Result<Vec<(String, String)>, HarnessError> {
    // Read here, before `run_land` reaches its own `control.commit` later in
    // the same transaction — so this is the control head the landing was
    // actually built from, not the one landing this integration will
    // eventually produce. Confirmed by reading the flow rather than assuming
    // it from the name: nothing between `with_transaction` opening the
    // control repository and this call ever mutates control (`write_atomic`,
    // `EventStore::append`, and every `Journal` write only touch the working
    // tree; `ControlRepository::commit` is the sole place control's HEAD
    // moves, and `run_land` does not call it until after the landing message,
    // which embeds this trailer, is composed).
    //
    // `None` means control has no commits at all. That cannot happen here:
    // reaching a `Prepared`, merged integration record requires a chain of
    // earlier successful control commits (at minimum `project init`'s own —
    // `write_project` pairs writing `project/project.json` with an
    // unconditional `control.commit`, and that first write can never be the
    // "nothing to commit" no-op case), and `Journal::require_settled`, which
    // every mutating command checks first, refuses all further work while any
    // of those commands failed to reach a clean terminal state. So the
    // refusal below is insurance against that invariant breaking, not a case
    // this card expects to exercise — an anchor that cannot be established
    // must still refuse rather than emit a placeholder.
    let Some(control_head) = control.head()? else {
        return Err(HarnessError::Control {
            reason: format!(
                "control repository has no commits yet; integration {} cannot be anchored to an unborn control history",
                record.integration_id
            ),
            code: ErrorCode::InternalControlCorrupt,
        });
    };

    let mut trailers = vec![
        (
            landing::TRAILER_INTEGRATION.to_owned(),
            record.integration_id.to_string(),
        ),
        (
            landing::TRAILER_INTEGRATION_DIGEST.to_owned(),
            digest.as_str().to_owned(),
        ),
        (
            landing::TRAILER_CYCLE.to_owned(),
            record.cycle_id.to_string(),
        ),
        (landing::TRAILER_CONTROL.to_owned(), control_head),
    ];
    for member in &record.members {
        trailers.push((
            landing::TRAILER_CARD.to_owned(),
            format!(
                "{} r{} {} review {}",
                member.card_id, member.card_revision, member.candidate_sha, member.review_id
            ),
        ));
        for receipt in receipts_for(control, &member.card_id)? {
            trailers.push((
                landing::TRAILER_RECEIPT.to_owned(),
                format!(
                    "{} {} {}",
                    receipt.receipt_id, receipt.gate_id, receipt.evaluated_sha
                ),
            ));
        }
    }
    Ok(trailers)
}

/// The deterministic subject Section 13.5 requires.
fn landing_subject(record: &IntegrationRecord) -> String {
    format!(
        "Land {} ({} card{}, {})",
        record.integration_id,
        record.members.len(),
        if record.members.len() == 1 { "" } else { "s" },
        record.mode.name()
    )
}

/// Refuses an integration whose plan was built against a superseded authority.
///
/// Checked at merge as well as at landing. Merging against a stale base is not
/// unsafe — landing and promotion both refuse it later — but it is wasted work
/// that produces an integration head missing whatever landed in the meantime,
/// which is exactly the kind of object someone inspects and misreads.
fn require_fresh_authority(
    config: &crate::config::ProjectConfig,
    record: &IntegrationRecord,
) -> Result<(), HarnessError> {
    let authority = inspect_authority(&config.authority_repository, &config.protected_branch)?;
    if authority.protected_sha.as_deref() == Some(record.expected_main_sha.as_str()) {
        return Ok(());
    }
    Err(HarnessError::Control {
        reason: format!(
            "authority `{}` is now {} but integration {} was planned against {}; re-prepare it",
            config.protected_branch,
            authority.protected_sha.as_deref().unwrap_or("unborn"),
            record.integration_id,
            record.expected_main_sha
        ),
        code: ErrorCode::ConflictControlHeadMoved,
    })
}

/// Validates a merged integration and returns everything landing needs.
fn landing_inputs(
    config: &crate::config::ProjectConfig,
    record: &IntegrationRecord,
) -> Result<(String, String), HarnessError> {
    if record.status != IntegrationStatus::Prepared {
        return Err(HarnessError::Control {
            reason: format!(
                "integration {} is `{}`; a landing commit is built from a prepared, merged integration",
                record.integration_id,
                record.status.name()
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    }
    let (Some(head), Some(tree)) = (&record.integration_head, &record.integration_tree) else {
        return Err(HarnessError::Control {
            reason: format!(
                "integration {} has not been merged; run `integration merge` first",
                record.integration_id
            ),
            code: ErrorCode::PreconditionNotFound,
        });
    };

    // Exact tree validation: the recorded tree must still be what the
    // integration head carries. If they disagree, something rewrote the head
    // after the merge and the recorded tree describes a state that no longer
    // exists.
    let actual = integration_worktree::tree_of(&config.repository, head)?;
    if actual != *tree {
        return Err(HarnessError::Control {
            reason: format!(
                "integration {} recorded tree {tree} but head {head} now carries {actual}",
                record.integration_id
            ),
            code: ErrorCode::ConflictMergeFailed,
        });
    }

    require_fresh_authority(config, record)?;
    Ok((head.clone(), tree.clone()))
}

/// Validates everything landing needs and reports it, building nothing.
fn preview_land(
    args: &MergeArgs,
    integration_id: &IntegrationId,
) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.control)?;
    let config = control.project()?;
    let record = load_integration(&control, integration_id)?;
    // 73-2: the first check able to refuse, before anything else — see
    // `require_cycle_convergence_budget`.
    require_cycle_convergence_budget(&control, &config, &record.cycle_id)?;
    require_no_generated_artifacts(&control, &record)?;
    let (head, tree) = landing_inputs(&config, &record)?;

    Ok(CommandOutcome::new(
        "integration.land",
        format!(
            "Dry run: would build `{}`\n  first parent: {}\n  second parent: {head}\n  tree: {tree}\nnothing was changed",
            landing_subject(&record),
            record.expected_main_sha
        ),
        serde_json::json!({
            "dry_run": true,
            "integration_id": integration_id.to_string(),
            "subject": landing_subject(&record),
            "first_parent": record.expected_main_sha,
            "second_parent": head,
            "tree": tree,
        }),
    )
    .with_project(config.project_id))
}

/// Turns a built landing commit into the command's outcome.
fn report_landing(
    object: &landing::LandingObject,
    integration_id: &IntegrationId,
    digest: &crate::domain::digest::Digest,
    project_id: &crate::domain::ids::ProjectId,
) -> CommandOutcome {
    let reference = landing::landing_ref(integration_id.as_str());
    CommandOutcome::new(
        "integration.land",
        format!(
            "Built landing commit {}\nsubject: {}\nfirst parent: {}\nsecond parent: {}\ntree: {}\nretained at {reference}\nthe protected branch was not moved",
            object.sha,
            object.subject,
            object.first_parent().unwrap_or("none"),
            object.second_parent().unwrap_or("none"),
            object.tree,
        ),
        serde_json::json!({
            "integration_id": integration_id.to_string(),
            "integration_digest": digest.as_str(),
            "landing_sha": object.sha,
            "landing_ref": reference,
            "subject": object.subject,
            "first_parent": object.first_parent(),
            "second_parent": object.second_parent(),
            "tree": object.tree,
            "trailers": object.trailers,
        }),
    )
    .with_project(project_id.clone())
}

fn run_land(args: &MergeArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let integration_id: IntegrationId = args.integration_id.parse()?;

    if args.dry_run {
        return preview_land(args, &integration_id);
    }

    with_transaction(
        &args.control,
        "integration.land",
        clock,
        |control, events, expected, steps| {
            let config = control.project()?;
            let mut record = load_integration(control, &integration_id)?;
            // 73-2: the first check able to refuse, before any write — see
            // `require_cycle_convergence_budget`.
            require_cycle_convergence_budget(control, &config, &record.cycle_id)?;
            if let Some(existing) = &record.landing_sha {
                return Err(HarnessError::Control {
                    reason: format!(
                        "integration {integration_id} already has landing commit {existing}"
                    ),
                    code: ErrorCode::PolicyInvalidTransition,
                });
            }
            require_no_generated_artifacts(control, &record)?;
            let (head, tree) = landing_inputs(&config, &record)?;

            // The record's digest goes into a trailer, so it is taken before
            // the landing SHA is written back into the record.
            let planned_digest = record.digest()?;
            let message = landing::compose_message(
                &landing_subject(&record),
                &landing_trailers(control, &record, &planned_digest)?,
            );
            steps.outside_control("landing-commit-created")?;
            let landing_sha = landing::create(
                &config.repository,
                &tree,
                &record.expected_main_sha,
                &head,
                &message,
            )?;
            // Held by a harness ref so collection cannot take it before
            // promotion; the protected branch is untouched.
            landing::retain(&config.repository, integration_id.as_str(), &landing_sha)?;
            steps.outside_control("landing-ref-retained")?;

            record.landing_sha = Some(landing_sha.clone());
            record.landed_at = Some(clock.now());
            let digest = record.digest()?;
            control.write_atomic(
                &IntegrationRecord::relative_path(&integration_id),
                &format!("{}\n", serde_json::to_string_pretty(&record)?),
            )?;

            events.append(
                &config.project_id,
                EventDraft::new("integration.landing-built", &args.actor_id)
                    .cycle(record.cycle_id.clone())
                    .head(landing_sha.clone())
                    .meta(
                        "integration_id",
                        serde_json::json!(integration_id.to_string()),
                    )
                    .meta("integration_digest", serde_json::json!(digest.as_str()))
                    .meta("landing_sha", serde_json::json!(landing_sha))
                    .meta("first_parent", serde_json::json!(record.expected_main_sha))
                    .meta("second_parent", serde_json::json!(head)),
                clock,
            )?;
            control.commit(
                expected,
                &format!("integration: build landing commit for {integration_id}"),
            )?;

            let object = landing::inspect(&config.repository, &landing_sha)?;
            Ok(report_landing(
                &object,
                &integration_id,
                &digest,
                &config.project_id,
            ))
        },
    )
}

/// The distinct gates a landing must pass, with the cards that require them.
///
/// Every gate any member names — feature, review, and integration alike — is
/// rerun against the landing commit. A feature gate that passed on an isolated
/// candidate proves nothing about the combined tree, which is the whole reason
/// this rerun exists.
fn required_gates(
    control: &ControlRepository,
    record: &IntegrationRecord,
) -> Result<Vec<String>, HarnessError> {
    let mut gates: Vec<String> = Vec::new();
    for member in &record.members {
        let (card, _) = load_card(control, &member.card_id)?;
        for gate_id in card
            .named_gates
            .feature
            .iter()
            .chain(&card.named_gates.review)
            .chain(&card.named_gates.integration)
        {
            if !gates.contains(gate_id) {
                gates.push(gate_id.clone());
            }
        }
    }
    // Sorted so the run order — and therefore the receipt order — is a
    // function of the plan rather than of card iteration.
    gates.sort();
    Ok(gates)
}

/// Builds the interaction checklist from the members' declared contracts.
fn member_interactions(
    control: &ControlRepository,
    record: &IntegrationRecord,
) -> Result<Vec<Interaction>, HarnessError> {
    let mut declared = Vec::new();
    for member in &record.members {
        let (card, _) = load_card(control, &member.card_id)?;
        declared.push((
            member.card_id.clone(),
            card.contract_changes.clone(),
            card.contract_reads.clone(),
        ));
    }
    Ok(interactions(&declared))
}

/// Runs every required gate against the landing commit in a clean worktree.
///
/// The worktree is checked for cleanliness after the gates as well as before:
/// a gate that writes into the tree it is checking invalidates its own result,
/// and would leave the landing tree and the verified tree describing different
/// things.
fn verify_landing(
    control: &ControlRepository,
    config: &crate::config::ProjectConfig,
    record: &IntegrationRecord,
    landing_sha: &str,
    clock: &dyn Clock,
) -> Result<(Vec<Receipt>, bool), HarnessError> {
    let path =
        integration_worktree::path_for(&config.worktree_root, record.integration_id.as_str());
    integration_worktree::remove(&config.repository, &path)?;
    integration_worktree::create(&config.repository, &path, landing_sha)?;

    let outcome = (|| -> Result<(Vec<Receipt>, bool), HarnessError> {
        let mut receipts = Vec::new();
        for gate_id in required_gates(control, record)? {
            let gate = load_gate(control, &gate_id)?;
            let log_root = control.path(LOG_DIR).join(record.integration_id.as_str());
            // Checked before each gate rather than once after all of them. The
            // worktree is clean by construction before the first, but a gate
            // that leaves residue changes what the next one runs against, and a
            // single flag for the whole batch cannot say which receipt that
            // affected.
            let worktree_clean = integration_worktree::is_clean(&path)?;
            let started_at = clock.now();
            let attempt = run_attempt(&gate, &path, &log_root, 1, clock)?;
            let finished_at = clock.now();

            receipts.push(Receipt {
                schema: RECEIPT_SCHEMA.to_owned(),
                receipt_id: "R-000000".parse()?,
                project_id: config.project_id.clone(),
                cycle_id: record.cycle_id.clone(),
                card_id: None,
                card_digest: None,
                integration_id: Some(record.integration_id.clone()),
                evaluated_sha: landing_sha.to_owned(),
                gate_id: gate.gate_id.clone(),
                gate_digest: gate.digest()?,
                harness_version: env!("CARGO_PKG_VERSION").to_owned(),
                environment_fingerprint: environment_fingerprint(&gate),
                started_at,
                finished_at,
                duration_ms: attempt.duration_ms,
                exit_code: attempt.exit_code,
                termination: attempt.termination,
                stdout_digest: attempt.stdout_digest.clone(),
                stderr_digest: attempt.stderr_digest.clone(),
                artifact_digests: attempt.artifact_digests.clone(),
                log_location: attempt.log_location.clone(),
                attempt: 1,
                passed: attempt.passed(),
                worktree_clean: Some(worktree_clean),
                provenance: Some(ReceiptProvenanceV1 {
                    schema: RECEIPT_PROVENANCE_SCHEMA.to_owned(),
                    subject: ProvenanceSubject::Integration {
                        landing_sha: landing_sha.to_owned(),
                        base_sha: record.baseline_sha.clone(),
                        cycle_id: record.cycle_id.clone(),
                        integration_id: record.integration_id.clone(),
                        integration_digest: record.substantive_digest()?,
                    },
                    gate_definition_digest: gate.digest()?,
                    argv_digest: Digest::of_canonical(&gate.argv)?,
                    policy_digest: Digest::of_canonical(&config.validation_policy)?,
                    proof_map: ProofMapBinding::NotApplicable,
                    dimensions: std::collections::BTreeMap::from([
                        (
                            ProvenanceDimension::Environment,
                            Digest::of_canonical(&gate.environment)?,
                        ),
                        (
                            ProvenanceDimension::Configuration,
                            Digest::of_canonical(config)?,
                        ),
                    ]),
                    freshness_dependencies: std::collections::BTreeMap::from([
                        ("integration".to_owned(), record.substantive_digest()?),
                        ("gate".to_owned(), gate.digest()?),
                        (
                            "policy".to_owned(),
                            Digest::of_canonical(&config.validation_policy)?,
                        ),
                    ]),
                    lineage: Vec::new(),
                    validation_reservation: None,
                }),
            });
        }
        let clean = integration_worktree::is_clean(&path)?;
        Ok((receipts, clean))
    })();

    integration_worktree::remove(&config.repository, &path)?;
    outcome
}

/// Allocates receipt identifiers, stores the receipts, and builds the record.
///
/// Identifiers are allocated only once the runs are done, so a verification
/// that never completed does not consume a block of them and leave a gap that
/// reads like deleted evidence.
#[allow(clippy::too_many_arguments)]
fn store_verification(
    control: &ControlRepository,
    record: &IntegrationRecord,
    cycle: &CycleRecord,
    receipts: &mut [Receipt],
    worktree_clean_after: bool,
    actor_id: &str,
    actor_principal_id: Option<&str>,
    actor_session_id: Option<&str>,
    clock: &dyn Clock,
) -> Result<VerificationRecord, HarnessError> {
    let mut receipt_ids = Vec::new();
    let mut failed_gates = Vec::new();
    for receipt in receipts.iter_mut() {
        receipt.receipt_id = next_receipt_id(control)?;
        if !receipt.passed {
            failed_gates.push(receipt.gate_id.clone());
        }
        control.write_atomic(
            &Receipt::relative_path(&receipt.receipt_id),
            &format!("{}\n", serde_json::to_string_pretty(receipt)?),
        )?;
        receipt_ids.push(receipt.receipt_id.to_string());
    }

    let landing_sha = record.landing_sha.clone().unwrap_or_default();
    let mut proofs = Vec::new();
    for card_id in &cycle.card_ids {
        let (card, _) = load_card(control, card_id)?;
        if let Some(map) = card.proof_map {
            for entry in map.entries {
                let (Some(id), Some(oracle)) = (entry.id, entry.gate_oracle) else {
                    continue;
                };
                proofs.push((id, entry.invariant, oracle));
            }
        }
    }
    let invariant_checks = cycle
        .release_invariants
        .iter()
        .map(|invariant| {
            let proof = proofs
                .iter()
                .find(|(id, text, _)| id == invariant || text == invariant);
            let matching: Vec<String> = proof
                .map(|(_, _, oracle)| {
                    receipts
                        .iter()
                        .filter(|receipt| {
                            receipt.passed
                                && receipt.gate_id == *oracle
                                && receipt.evaluated_sha == landing_sha
                        })
                        .map(|receipt| receipt.receipt_id.to_string())
                        .collect()
                })
                .unwrap_or_default();
            let checked = proof.is_some() && !matching.is_empty();
            InvariantCheck {
                proof_entry_id: proof.map(|(id, _, _)| id.clone()),
                invariant: invariant.clone(),
                machine_checked: checked,
                observed_receipt_ids: matching,
                classification: Some(if checked {
                    ClaimClassification::MachineChecked
                } else {
                    ClaimClassification::NotTested
                }),
            }
        })
        .collect();
    Ok(VerificationRecord {
        schema: VERIFICATION_SCHEMA.to_owned(),
        integration_id: record.integration_id.clone(),
        cycle_id: record.cycle_id.clone(),
        landing_sha,
        landing_tree: record.integration_tree.clone().unwrap_or_default(),
        receipt_ids,
        failed_gates,
        invariants: invariant_checks,
        interactions: member_interactions(control, record)?,
        worktree_clean_after,
        verified_by: actor_id.to_owned(),
        verified_principal_id: actor_principal_id.map(str::to_owned),
        verified_session_id: actor_session_id.map(str::to_owned),
        verified_at: clock.now(),
        canonical_algorithm: CANONICAL_ALGORITHM.to_owned(),
    })
}

/// Reports what verification would run, running no gate.
///
/// A dry run that started the gates would be a real verification wearing the
/// wrong name: the gates have side effects by construction, and one of them
/// transitions the integration.
fn preview_verify(
    args: &MergeArgs,
    integration_id: &IntegrationId,
) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.control)?;
    let config = control.project()?;
    let record = load_integration(&control, integration_id)?;
    // 73-2: the first check able to refuse, before anything else — see
    // `require_cycle_convergence_budget`.
    require_cycle_convergence_budget(&control, &config, &record.cycle_id)?;
    record
        .status
        .check_transition(IntegrationStatus::Verified)?;
    let Some(landing_sha) = record.landing_sha.clone() else {
        return Err(HarnessError::Control {
            reason: format!(
                "integration {integration_id} has no landing commit; run `integration land` first"
            ),
            code: ErrorCode::PreconditionNotFound,
        });
    };
    let gates = required_gates(&control, &record)?;

    Ok(CommandOutcome::new(
        "integration.verify",
        format!(
            "Dry run: would run {} gate(s) against landing commit {landing_sha}\n  {}\nnothing was changed",
            gates.len(),
            gates.join("\n  ")
        ),
        serde_json::json!({
            "dry_run": true,
            "integration_id": integration_id.to_string(),
            "landing_sha": landing_sha,
            "gates": gates,
            "interactions": member_interactions(&control, &record)?,
        }),
    )
    .with_project(config.project_id))
}

fn run_verify(args: &MergeArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let integration_id: IntegrationId = args.integration_id.parse()?;

    if args.dry_run {
        return preview_verify(args, &integration_id);
    }

    with_transaction(
        &args.control,
        "integration.verify",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let config = control.project()?;
            let mut record = load_integration(control, &integration_id)?;
            // 73-2: the first check able to refuse, before any write — see
            // `require_cycle_convergence_budget`.
            require_cycle_convergence_budget(control, &config, &record.cycle_id)?;
            record
                .status
                .check_transition(IntegrationStatus::Verified)?;
            let Some(landing_sha) = record.landing_sha.clone() else {
                return Err(HarnessError::Control {
                    reason: format!(
                        "integration {integration_id} has no landing commit; run `integration land` first"
                    ),
                    code: ErrorCode::PreconditionNotFound,
                });
            };

            let cycle = load_cycle(control, &record.cycle_id)?;
            let (mut receipts, worktree_clean_after) =
                verify_landing(control, &config, &record, &landing_sha, clock)?;

            let verification = store_verification(
                control,
                &record,
                &cycle,
                &mut receipts,
                worktree_clean_after,
                &args.actor_id,
                args.actor_principal_id.as_deref(),
                args.actor_session_id.as_deref(),
                clock,
            )?;
            let receipt_ids = verification.receipt_ids.clone();
            let failed_gates = verification.failed_gates.clone();
            let passed = verification.passed();
            let digest = verification.digest()?;

            // Both outcomes are recorded. A failed verification that left no
            // trace would let a retry present itself as the first attempt.
            control.write_atomic(
                &VerificationRecord::relative_path(&integration_id),
                &format!("{}\n", serde_json::to_string_pretty(&verification)?),
            )?;
            if passed {
                record.status = IntegrationStatus::Verified;
                control.write_atomic(
                    &IntegrationRecord::relative_path(&integration_id),
                    &format!("{}\n", serde_json::to_string_pretty(&record)?),
                )?;
            }

            events.append(
                &config.project_id,
                EventDraft::new("integration.verified", &args.actor_id)
                    .cycle(record.cycle_id.clone())
                    .head(landing_sha.clone())
                    .transition(
                        Some(IntegrationStatus::Prepared.name()),
                        if passed {
                            IntegrationStatus::Verified.name()
                        } else {
                            IntegrationStatus::Prepared.name()
                        },
                    )
                    .meta(
                        "integration_id",
                        serde_json::json!(integration_id.to_string()),
                    )
                    .meta("verification_digest", serde_json::json!(digest.as_str()))
                    .meta("passed", serde_json::json!(passed))
                    .meta("receipt_ids", serde_json::json!(receipt_ids))
                    .meta("failed_gates", serde_json::json!(failed_gates)),
                clock,
            )?;
            control.commit(expected, &format!("integration: verify {integration_id}"))?;

            if !passed {
                return Err(HarnessError::Control {
                    reason: if worktree_clean_after {
                        format!(
                            "combined verification of {integration_id} failed: {}",
                            failed_gates.join(", ")
                        )
                    } else {
                        format!(
                            "the integration worktree was dirty after verifying {integration_id}; a gate wrote into the tree it was checking"
                        )
                    },
                    code: ErrorCode::GateFailed,
                });
            }

            Ok(report_verification(
                &verification,
                &digest,
                &config.project_id,
            ))
        },
    )
}

/// Turns a verification into the command's outcome.
fn report_verification(
    verification: &VerificationRecord,
    digest: &crate::domain::digest::Digest,
    project_id: &crate::domain::ids::ProjectId,
) -> CommandOutcome {
    let mut text = format!(
        "Verified integration {} against landing commit {}\ngates passed: {}\nworktree clean after gates: {}",
        verification.integration_id,
        verification.landing_sha,
        verification.receipt_ids.len(),
        verification.worktree_clean_after
    );
    if !verification.invariants.is_empty() {
        text.push_str("\ncycle invariants a reviewer must still judge:");
        for check in &verification.invariants {
            let _ =
                std::fmt::Write::write_fmt(&mut text, format_args!("\n  - {}", check.invariant));
        }
    }
    if verification.interactions.is_empty() {
        text.push_str("\nno declared contract interactions between members");
    } else {
        text.push_str("\ncombined interactions to review:");
        for interaction in &verification.interactions {
            let _ = std::fmt::Write::write_fmt(
                &mut text,
                format_args!(
                    "\n  - {} changes {}, which {} reads",
                    interaction.changes,
                    interaction.shared.join(", "),
                    interaction.reads
                ),
            );
        }
    }

    CommandOutcome::new(
        "integration.verify",
        text,
        serde_json::json!({
            "integration_id": verification.integration_id.to_string(),
            "verification_digest": digest.as_str(),
            "landing_sha": verification.landing_sha,
            "receipt_ids": verification.receipt_ids,
            "failed_gates": verification.failed_gates,
            "invariants": verification.invariants,
            "interactions": verification.interactions,
            "worktree_clean_after": verification.worktree_clean_after,
            "passed": verification.passed(),
        }),
    )
    .with_project(project_id.clone())
}

/// Arguments accepted by `integration review`.
#[derive(Debug, Args)]
pub struct ReviewArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
    pub control: std::path::PathBuf,
    /// The integration to review.
    #[arg(long)]
    pub integration_id: String,
    /// Who is reviewing. Must differ from whoever verified it.
    #[arg(long)]
    pub reviewer_actor_id: String,
    /// Declared reviewer principal/session boundary; identity is not host-attested.
    #[arg(long)]
    pub reviewer_principal_id: Option<String>,
    #[arg(long)]
    pub reviewer_session_id: Option<String>,
    /// A risk accepted by approving this integration. Repeatable.
    #[arg(long = "residual-risk")]
    pub residual_risks: Vec<String>,
    /// A cycle invariant the reviewer confirms holds. Repeatable.
    #[arg(long = "invariant-holds")]
    pub invariants_held: Vec<String>,
    /// Report the decision without recording it.
    #[arg(long)]
    pub dry_run: bool,
}

/// Who implemented each card this integration carries, as `(card, actor)`.
///
/// Read from each member's approving review rather than from its handoff: the
/// review already records `feature_actor_id`, the member already pins the
/// review by id and digest, and going through the handoff would re-derive the
/// same fact from a record the approval was bound to anyway.
///
/// Fails closed when a member's review cannot be found. An earlier version
/// skipped it, so that a damaged control repository could not deadlock the
/// lifecycle — and a reviewer proved the consequence by deleting one review
/// file and then self-accepting as the implementer. Absence of evidence became
/// a granted authorization, which is the worst possible direction for this
/// particular check to fail in. The member pins the review by id *and* digest,
/// so a review that will not load is not an ordinary missing file; it is the
/// record the approval was bound to, gone. `audit` reports it as a discrepancy
/// (D-058), and `project recover` or a restored backup is the way out.
///
/// # Errors
///
/// Returns a precondition error naming the member whose review is missing, and
/// any error the control repository itself raises.
pub fn member_implementers(
    control: &ControlRepository,
    record: &IntegrationRecord,
) -> Result<Vec<(String, String)>, HarnessError> {
    let mut found = Vec::new();
    for member in &record.members {
        let reviews = crate::commands::review::reviews_for(control, &member.card_id)?;
        let review = reviews
            .iter()
            .find(|review| review.review_id == member.review_id)
            .ok_or_else(|| HarnessError::Control {
                reason: format!(
                    "integration {} names review {} for card {}, and it cannot be read; the approval this step relies on is missing, so who implemented {} cannot be established. Run `audit` to see what else is affected",
                    record.integration_id, member.review_id, member.card_id, member.card_id
                ),
                code: ErrorCode::PreconditionNotFound,
            })?;

        // Matching the id alone trusted the file's *name*. The member pins a
        // digest too, and editing a review's `feature_actor_id` changed who the
        // separation check compared against — turning the record that proves
        // who wrote the code into the thing an author edits in order to approve
        // it. Everything else in this harness binds to a digest for exactly
        // that reason; this lookup did not.
        if review.digest()? != member.review_digest {
            return Err(HarnessError::Control {
                reason: format!(
                    "review {} for card {} no longer digests to what integration {} pinned; the approval was altered after it was recorded. Run `audit`",
                    member.review_id, member.card_id, record.integration_id
                ),
                code: ErrorCode::PolicyNotIntegrable,
            });
        }
        found.push((member.card_id.to_string(), review.feature_actor_id.clone()));
    }
    Ok(found)
}

/// Reads an integration's verification record.
///
/// # Errors
///
/// Returns a precondition error when the integration has not been verified.
pub fn load_verification(
    control: &ControlRepository,
    integration_id: &IntegrationId,
) -> Result<VerificationRecord, HarnessError> {
    let relative = VerificationRecord::relative_path(integration_id);
    if !control.path(&relative).exists() {
        return Err(HarnessError::Control {
            reason: format!(
                "integration {integration_id} has not been verified; run `integration verify` first"
            ),
            code: ErrorCode::PreconditionNotFound,
        });
    }
    serde_json::from_str(&control.read(&relative)?).map_err(|source| HarnessError::Control {
        reason: format!("verification for {integration_id} is malformed: {source}"),
        code: ErrorCode::InternalControlCorrupt,
    })
}

/// Refuses a review that leaves a declared cycle invariant unaddressed.
///
/// Free-text invariants are the reviewer's job precisely because no gate can
/// evaluate them. Letting a review pass without naming each one would make the
/// cycle's own release conditions decorative.
fn require_invariants_addressed(
    verification: &VerificationRecord,
    held: &[String],
) -> Result<(), HarnessError> {
    let missing: Vec<&str> = verification
        .invariants
        .iter()
        .map(|check| check.invariant.as_str())
        .filter(|invariant| !held.iter().any(|value| value == invariant))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(HarnessError::Control {
        reason: format!(
            "the cycle declares invariants this review does not confirm: {}",
            missing.join("; ")
        ),
        code: ErrorCode::PolicyInvariantUnaddressed,
    })
}

/// Refuses an integration review recorded by whoever ran the verification.
///
/// Section 15.1's independence rule applies here too: whoever ran the gates
/// cannot be the one who judges what they proved.
///
/// The identifier validation runs here rather than a bare comparison, so this
/// site inherits the unnamed-actor refusal the other separation steps have. It
/// accepted an empty reviewer id until now — the hole this card closed one step
/// later at acceptance and left open here.
fn refuse_verifier_reviewing(
    verification: &VerificationRecord,
    reviewer_actor_id: &str,
    reviewer_principal_id: Option<&str>,
    reviewer_session_id: Option<&str>,
) -> Result<(), HarnessError> {
    actors::refuse_unusable("integration reviewer", reviewer_actor_id)?;
    actors::refuse_unusable("verifier", &verification.verified_by)?;
    if actors::same(&verification.verified_by, reviewer_actor_id) {
        return Err(HarnessError::Control {
            reason: format!(
                "{reviewer_actor_id} verified this integration and cannot also review it"
            ),
            code: ErrorCode::PolicySameActor,
        });
    }
    let reviewer = actors::ActorIdentity {
        actor_kind: "integration_reviewer",
        actor_id: reviewer_actor_id,
        principal_id: reviewer_principal_id,
        session_id: reviewer_session_id,
    };
    let verifier = actors::ActorIdentity {
        actor_kind: "integration_verifier",
        actor_id: &verification.verified_by,
        principal_id: verification.verified_principal_id.as_deref(),
        session_id: verification.verified_session_id.as_deref(),
    };
    if reviewer.same_boundary(&verifier) {
        return Err(HarnessError::Control {
            reason: "integration reviewer shares the verifier principal/session boundary"
                .to_owned(),
            code: ErrorCode::PolicySameActor,
        });
    }
    Ok(())
}

fn refuse_member_implementer_reviewing(
    control: &ControlRepository,
    record: &IntegrationRecord,
    reviewer_actor_id: &str,
) -> Result<(), HarnessError> {
    let implementers = member_implementers(control, record)?;
    if implementers
        .iter()
        .any(|(_, actor)| actors::same(actor, reviewer_actor_id))
    {
        return Err(HarnessError::Control {
            reason: format!(
                "{reviewer_actor_id} implemented a member card and cannot review the integration"
            ),
            code: ErrorCode::PolicySameActor,
        });
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn run_integration_review(
    args: &ReviewArgs,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    let integration_id: IntegrationId = args.integration_id.parse()?;

    if args.dry_run {
        let control = ControlRepository::open(&args.control)?;
        let config = control.project()?;
        let record = load_integration(&control, &integration_id)?;
        // 73-2: the first check able to refuse, before anything else — see
        // `require_cycle_convergence_budget`.
        require_cycle_convergence_budget(&control, &config, &record.cycle_id)?;
        record
            .status
            .check_transition(IntegrationStatus::Reviewed)?;
        let verification = load_verification(&control, &integration_id)?;
        refuse_verifier_reviewing(
            &verification,
            &args.reviewer_actor_id,
            args.reviewer_principal_id.as_deref(),
            args.reviewer_session_id.as_deref(),
        )?;
        refuse_member_implementer_reviewing(&control, &record, &args.reviewer_actor_id)?;
        require_invariants_addressed(&verification, &args.invariants_held)?;
        return Ok(CommandOutcome::new(
            "integration.review",
            format!(
                "Dry run: would record an integration review of {integration_id} by {}\nnothing was changed",
                args.reviewer_actor_id
            ),
            serde_json::json!({
                "dry_run": true,
                "integration_id": integration_id.to_string(),
                "reviewer_actor_id": args.reviewer_actor_id,
            }),
        )
        .with_project(config.project_id));
    }

    with_transaction(
        &args.control,
        "integration.review",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let config = control.project()?;
            let mut record = load_integration(control, &integration_id)?;
            // 73-2: the first check able to refuse, before any write — see
            // `require_cycle_convergence_budget`.
            require_cycle_convergence_budget(control, &config, &record.cycle_id)?;
            record
                .status
                .check_transition(IntegrationStatus::Reviewed)?;
            let verification = load_verification(control, &integration_id)?;

            refuse_verifier_reviewing(
                &verification,
                &args.reviewer_actor_id,
                args.reviewer_principal_id.as_deref(),
                args.reviewer_session_id.as_deref(),
            )?;
            refuse_member_implementer_reviewing(control, &record, &args.reviewer_actor_id)?;
            require_invariants_addressed(&verification, &args.invariants_held)?;

            record.status = IntegrationStatus::Reviewed;
            control.write_atomic(
                &IntegrationRecord::relative_path(&integration_id),
                &format!("{}\n", serde_json::to_string_pretty(&record)?),
            )?;

            let digest = verification.digest()?;
            events.append(
                &config.project_id,
                EventDraft::new("integration.reviewed", &args.reviewer_actor_id)
                    .cycle(record.cycle_id.clone())
                    .head(verification.landing_sha.clone())
                    .transition(
                        Some(IntegrationStatus::Verified.name()),
                        IntegrationStatus::Reviewed.name(),
                    )
                    .meta(
                        "integration_id",
                        serde_json::json!(integration_id.to_string()),
                    )
                    .meta("verification_digest", serde_json::json!(digest.as_str()))
                    .meta("residual_risks", serde_json::json!(args.residual_risks))
                    .meta("invariants_held", serde_json::json!(args.invariants_held))
                    .meta("verified_by", serde_json::json!(verification.verified_by)),
                clock,
            )?;
            control.commit(expected, &format!("integration: review {integration_id}"))?;

            Ok(CommandOutcome::new(
                "integration.review",
                format!(
                    "Reviewed integration {integration_id} ({})\nreviewer: {}\nverified by: {}\nlanding commit: {}\nresidual risks: {}",
                    IntegrationStatus::Reviewed.name(),
                    args.reviewer_actor_id,
                    verification.verified_by,
                    verification.landing_sha,
                    if args.residual_risks.is_empty() {
                        "none declared".to_owned()
                    } else {
                        args.residual_risks.join("; ")
                    }
                ),
                serde_json::json!({
                    "integration_id": integration_id.to_string(),
                    "status": IntegrationStatus::Reviewed.name(),
                    "reviewer_actor_id": args.reviewer_actor_id,
                    "verified_by": verification.verified_by,
                    "verification_digest": digest.as_str(),
                    "landing_sha": verification.landing_sha,
                    "residual_risks": args.residual_risks,
                    "invariants_held": args.invariants_held,
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}

/// Arguments accepted by `integration promote`.
#[derive(Debug, Args)]
pub struct PromoteArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
    pub control: std::path::PathBuf,
    /// The integration to promote.
    #[arg(long)]
    pub integration_id: String,
    /// Who is promoting.
    #[arg(long)]
    pub actor_id: String,
    /// Run every precondition check and report, promoting nothing.
    #[arg(long)]
    pub dry_run: bool,
}

/// The Section 13.6 preconditions, checked before anything is published.
///
/// All of them are verified before step 9 moves the authority, because that
/// step is the only irreversible one in the sequence. Anything that can be
/// discovered afterwards is worth discovering first.
struct PromotionChecks {
    landing_sha: String,
    landing_tree: String,
    acceptance_id: String,
}

/// Refuses a promotion run by someone who wrote one of the cards it carries.
///
/// Promotion executes a decision somebody else authorized, so the author does
/// not get to run it either. The acceptance owner *may* — Section 15.1's model
/// is one human and many agent sessions, and requiring a fourth distinct party
/// to press the button would make that model impossible to follow. The full
/// policy is in [`crate::policy::actors`].
fn refuse_author_promoting(
    control: &ControlRepository,
    record: &IntegrationRecord,
    actor_id: &str,
) -> Result<(), HarnessError> {
    let implementers = member_implementers(control, record)?;
    actors::refuse_author_acting_as(
        "promoter",
        actor_id,
        &record.integration_id.to_string(),
        implementers
            .iter()
            .map(|(card, actor)| (card.as_str(), actor.as_str())),
    )
}

/// Refuses promotion when a previously-anchored control head is no longer an
/// ancestor of the control record.
///
/// #87 writes the control repository's head into every landing commit as a
/// `Change-Harness-Control` trailer; #88 built `check_control_anchors` to
/// verify each one still holds, but left it reachable only through `audit
/// anchors` — a check living in a command an operator has to remember to run
/// protects nothing on the path that matters. This calls that exact check at
/// the one boundary where the record's integrity decides whether work may
/// proceed, so a rewrite that erased an inconvenient fact cannot go on to
/// authorize the very promotion it was meant to enable.
///
/// Reuses `check_control_anchors` rather than a second copy of its walk, the
/// same reason `cross_check_cycle` is shared between `audit cycle` and `cycle
/// replay` instead of duplicated: two callers must never be able to disagree
/// about what counts as a discrepancy.
///
/// No override, deliberately. #88's design work already established why: no
/// command in this harness rewrites control history — `archive` only creates
/// refs and removes worktrees and branches, `backup` has only `create` and
/// `verify` — so a broken anchor has exactly two causes, tampering or an
/// out-of-band restore to an earlier control state, and both mean the
/// control record no longer contains history that already-landed work was
/// built on. An override would let promotion proceed onto that anyway,
/// compounding the corruption instead of surfacing it. The refusal below
/// says so explicitly, so nobody later reads the absence of a bypass flag as
/// an oversight and adds one. The remedy is not a flag but an operation:
/// restore the control repository from a `backup create` archive that
/// contains the anchored commit.
///
/// # Errors
///
/// Returns [`ErrorCode::PolicyAuditDiscrepancy`] — the same code `audit
/// anchors` itself returns for the identical finding — when
/// `check_control_anchors` reports any anchor orphaned or missing.
/// Propagates an authority- or control-read failure unchanged otherwise.
fn require_control_anchors_intact(
    control: &ControlRepository,
    config: &crate::config::ProjectConfig,
) -> Result<(), HarnessError> {
    let evidence = check_control_anchors(control, config)?;
    if evidence.discrepancies.is_empty() {
        return Ok(());
    }

    Err(HarnessError::Control {
        reason: format!(
            "promotion refuses: control history no longer supports {} previously-anchored \
             control head(s), so the record cannot be trusted to authorize this promotion: {}. \
             No command in this harness rewrites control history, so this means either \
             tampering or an out-of-band restore to an earlier control state — either way, the \
             control record no longer contains the history that already-landed work was built \
             on. The remedy is to restore the control repository from a `backup create` archive \
             that contains the anchored commit; there is deliberately no override for this \
             refusal.",
            evidence.discrepancies.len(),
            evidence
                .discrepancies
                .iter()
                .map(|entry| format!(
                    "{} claims {}, found {}",
                    entry.subject, entry.claim, entry.found
                ))
                .collect::<Vec<_>>()
                .join("; "),
        ),
        code: ErrorCode::PolicyAuditDiscrepancy,
    })
}

/// Builds the refusal for "the integration is not yet in the status a
/// status-gated command requires," adding #112's own recovery when the
/// reason is exactly the landed-but-unverified case: a built landing
/// commit that nothing has verified yet, the one status the refusing
/// command reaches with an unambiguous next command regardless of which
/// later status it was actually checking for. Every other status (draft,
/// verified-but-not-reviewed, already accepted, ...) has a different or
/// non-obvious next step, so those keep the plain code default rather than
/// guessing.
///
/// Shared by `check_promotion` below and `acceptance::require_reviewed`,
/// which hit this identical case from a different required status —
/// factored out once duplicating it a second time made `check_promotion`
/// exceed clippy's line limit.
pub(crate) fn status_gate_refusal(
    reason: String,
    record: &IntegrationRecord,
    unverified_recovery: &'static str,
) -> HarnessError {
    if record.status == IntegrationStatus::Prepared && record.landing_sha.is_some() {
        HarnessError::ControlWithRecovery {
            reason,
            code: ErrorCode::PolicyInvalidTransition,
            recovery: unverified_recovery,
        }
    } else {
        HarnessError::Control {
            reason,
            code: ErrorCode::PolicyInvalidTransition,
        }
    }
}

/// Verifies everything Section 13.6 requires before the authority is touched.
///
/// Shared between `preview_promote` and `run_promote`, so both check the
/// cycle's convergence budget through this one call rather than each risking
/// its own copy drifting from the other's.
// #179 pushed this past the default line limit by giving two of its checks
// their own per-site `recovery:` line — the same reason three other
// functions in this file already carry this same allow.
#[allow(clippy::too_many_lines)]
fn check_promotion(
    control: &ControlRepository,
    config: &crate::config::ProjectConfig,
    record: &IntegrationRecord,
    actor_id: &str,
) -> Result<PromotionChecks, HarnessError> {
    require_plan_binding(control, record)?;
    // #89: refused before anything else in this function, including the
    // convergence budget below. A rewritten control record makes every
    // later check's *inputs* untrustworthy — `require_cycle_convergence_
    // budget` projects the cycle's convergence facts from exactly the
    // control history this verifies, and every check after it reads records
    // out of that same repository. Integrity of the record precedes any
    // policy computed from it.
    require_control_anchors_intact(control, config)?;
    // 73-2: the first of Section 13.6's own preconditions — see
    // `require_cycle_convergence_budget`.
    require_cycle_convergence_budget(control, config, &record.cycle_id)?;
    if record.status != IntegrationStatus::Accepted {
        let reason = format!(
            "integration {} is `{}`; only an accepted integration may be promoted",
            record.integration_id,
            record.status.name()
        );
        return Err(status_gate_refusal(
            reason,
            record,
            "Run `integration verify` before promotion can be authorized.",
        ));
    }
    let Some(landing_sha) = record.landing_sha.clone() else {
        return Err(HarnessError::Control {
            reason: format!(
                "integration {} has no landing commit",
                record.integration_id
            ),
            code: ErrorCode::PreconditionNotFound,
        });
    };

    // Step 2 and 3: reload every object and verify it still says what the
    // decision was made against.
    let acceptance =
        acceptance_for(control, &record.integration_id)?.ok_or_else(|| HarnessError::Control {
            reason: format!(
                "integration {} has no recorded acceptance",
                record.integration_id
            ),
            code: ErrorCode::PreconditionNotFound,
        })?;
    if let Some(reason) = acceptance.blocks_promotion_of(&landing_sha) {
        return Err(HarnessError::ControlWithRecovery {
            reason: format!("promotion is not authorized: {reason}"),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_AUTHORIZATION_STALE_RECOVERY,
        });
    }
    validate_final_authorization_for_promotion(control, config, record, &acceptance)?;
    require_no_pending_exception(control, config, record)?;
    // Acceptance recorded a digest of the integration it authorized, and until
    // now nothing compared it to anything. Append a member after acceptance and
    // promotion landed it: never verified, never accepted, not in the tree the
    // acceptance owner signed off.
    let substantive = record.substantive_digest()?;
    if acceptance.integration_record_digest != substantive {
        return Err(HarnessError::ControlWithRecovery {
            reason: format!(
                "acceptance {} authorized integration digest {} but the record now digests to {substantive}; the plan changed after it was accepted",
                acceptance.acceptance_id, acceptance.integration_record_digest
            ),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_AUTHORIZATION_STALE_RECOVERY,
        });
    }
    let verification = load_verification(control, &record.integration_id)?;
    if verification.landing_sha != landing_sha {
        return Err(HarnessError::Control {
            reason: format!(
                "verification covered landing commit {} but the integration holds {landing_sha}",
                verification.landing_sha
            ),
            code: ErrorCode::PolicyNotVerified,
        });
    }

    // Deliberately after the plan has been proved intact, not before. Asking
    // who is allowed to promote a tampered plan answers the less useful
    // question first: run ahead of the digest comparison, this reported a
    // member whose review could not be found, when what the operator needed to
    // hear was that the plan had changed since acceptance. Still well before
    // anything is mutated, which is all the separation check requires.
    refuse_author_promoting(control, record, actor_id)?;

    // Step 4: the authority must still be where the plan expected.
    let authority = inspect_authority(&config.authority_repository, &config.protected_branch)?;
    if authority.protected_sha.as_deref() != Some(record.expected_main_sha.as_str()) {
        return Err(HarnessError::Control {
            reason: format!(
                "authority `{}` is now {} but the landing was built for {}",
                config.protected_branch,
                authority.protected_sha.as_deref().unwrap_or("unborn"),
                record.expected_main_sha
            ),
            code: ErrorCode::ConflictControlHeadMoved,
        });
    }

    // Steps 5 and 6: the landing commit itself must have the shape promotion
    // assumes, read from the object rather than from the record that describes
    // it.
    let object = landing::inspect(&config.repository, &landing_sha)?;
    if object.first_parent() != Some(record.expected_main_sha.as_str()) {
        return Err(HarnessError::Control {
            reason: format!(
                "landing commit {landing_sha} has first parent {} but the authority is at {}",
                object.first_parent().unwrap_or("none"),
                record.expected_main_sha
            ),
            code: ErrorCode::PolicyLandingMismatch,
        });
    }
    if Some(object.tree.as_str()) != record.integration_tree.as_deref() {
        return Err(HarnessError::Control {
            reason: format!(
                "landing commit {landing_sha} carries tree {} but the verified tree was {}",
                object.tree,
                record.integration_tree.as_deref().unwrap_or("none")
            ),
            code: ErrorCode::PolicyLandingMismatch,
        });
    }

    // Step 8: the local main worktree must be clean and at the expected commit,
    // so step 11 can fast-forward it without touching anyone's work.
    require_local_main_ready(config, &record.expected_main_sha)?;

    Ok(PromotionChecks {
        landing_sha,
        landing_tree: object.tree,
        acceptance_id: acceptance.acceptance_id.to_string(),
    })
}

/// Refuses promotion when the candidate's protected worktree is not ready.
///
/// Section 13.6 step 8. Promotion fast-forwards this worktree afterwards, and
/// fast-forwarding over uncommitted work would destroy it. Checking first means
/// the refusal costs nothing; checking after the authority moved would leave
/// the two out of step with no way back.
fn require_local_main_ready(
    config: &crate::config::ProjectConfig,
    expected: &str,
) -> Result<(), HarnessError> {
    let scope = GitScope::work_tree(&config.repository);

    // Which branch is checked out, before anything about where it points. The
    // sync below is `git merge --ff-only`, which moves whatever HEAD is on —
    // so every check that follows describes the protected branch only if HEAD
    // is the protected branch. An operator sitting on a feature branch, which
    // is the ordinary state, had the landing commit fast-forwarded onto that
    // instead, and the command reported the protected branch synchronized.
    let checked_out = run(&scope, ["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    let current = checked_out.trimmed_stdout();
    if !checked_out.success() || current != config.protected_branch {
        return Err(HarnessError::Control {
            reason: format!(
                "local repository has `{}` checked out but promotion fast-forwards `{}`; check it out first",
                if checked_out.success() {
                    current
                } else {
                    "a detached HEAD"
                },
                config.protected_branch
            ),
            code: ErrorCode::PreconditionLocalMainStale,
        });
    }

    let head = run(
        &scope,
        [
            "rev-parse",
            "--verify",
            &format!("refs/heads/{}^{{commit}}", config.protected_branch),
        ],
    )?;
    if !head.success() || head.trimmed_stdout() != expected {
        return Err(HarnessError::Control {
            reason: format!(
                "local `{}` is at {} but promotion expects {expected}",
                config.protected_branch,
                if head.success() {
                    head.trimmed_stdout()
                } else {
                    "no such branch"
                }
            ),
            code: ErrorCode::PreconditionLocalMainStale,
        });
    }

    let status = run_ok(
        &scope,
        ["status", "--porcelain", "--untracked-files=normal"],
    )?;
    if !status.trimmed_stdout().is_empty() {
        return Err(HarnessError::Control {
            reason: format!(
                "local `{}` worktree is not clean; promotion would fast-forward over uncommitted work",
                config.protected_branch
            ),
            code: ErrorCode::PreconditionWorktreeDirty,
        });
    }
    Ok(())
}

/// Fast-forwards the candidate's protected worktree onto the promoted commit.
///
/// Section 13.6 step 11. `--ff-only` is the point: if this cannot be a
/// fast-forward, something changed the worktree between the precondition check
/// and now, and merging instead would invent a commit nobody accepted.
///
/// This one keeps the project's hooks, unlike `integration_worktree::merge`,
/// and the difference is deliberate. A fast-forward writes no commit, so
/// nothing here is a harness-authored object that signing or a
/// `prepare-commit-msg` hook could alter — the landing commit was authored
/// earlier by plumbing and is already published. What a fast-forward does run
/// is `post-merge`, and this is the developer's own protected worktree being
/// brought up to date; that hook is the project's way of finishing the job, and
/// suppressing it here would leave a checkout the project considers unfinished
/// in the one place a human will actually work next. Verified that it cannot
/// block: a `post-merge` exiting 1 still leaves `git merge --ff-only` at exit 0.
fn synchronize_local_main(
    config: &crate::config::ProjectConfig,
    landing_sha: &str,
) -> Result<(), HarnessError> {
    let scope = GitScope::work_tree(&config.repository);
    let output = run(&scope, ["merge", "--ff-only", landing_sha])?;
    if !output.success() {
        return Err(HarnessError::Control {
            reason: format!(
                "authority_promoted_local_sync_pending: the authority was promoted to {landing_sha} but local `{}` could not fast-forward: {}. The authority is NOT rolled back; run `project recover`",
                config.protected_branch,
                output.diagnostic()
            ),
            code: ErrorCode::RecoveryIncomplete,
        });
    }
    Ok(())
}

/// Runs every Section 13.6 precondition and reports, promoting nothing.
fn preview_promote(
    args: &PromoteArgs,
    integration_id: &IntegrationId,
) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.control)?;
    let config = control.project()?;
    let record = load_integration(&control, integration_id)?;
    let checks = check_promotion(&control, &config, &record, &args.actor_id)?;

    Ok(CommandOutcome::new(
        "integration.promote",
        format!(
            "Dry run: every precondition for promoting {integration_id} holds\n  landing commit: {}\n  authority `{}`: {} → {}\nnothing was changed",
            checks.landing_sha,
            config.protected_branch,
            record.expected_main_sha,
            checks.landing_sha
        ),
        serde_json::json!({
            "dry_run": true,
            "integration_id": integration_id.to_string(),
            "landing_sha": checks.landing_sha,
            "expected_main_sha": record.expected_main_sha,
            "acceptance_id": checks.acceptance_id,
        }),
    )
    .with_project(config.project_id))
}

/// Moves the authority branch, staging objects first and cleaning up after.
///
/// This is the one irreversible step in the sequence, isolated so the code
/// around it reads as "everything before" and "everything after".
fn publish(
    config: &crate::config::ProjectConfig,
    integration_id: &IntegrationId,
    landing_sha: &str,
    expected_main_sha: &str,
) -> Result<(), HarnessError> {
    let incoming = stage_objects(
        &config.repository,
        &config.authority_repository,
        landing_sha,
        integration_id.as_str(),
    )?;
    let outcome = promote_authority(
        &config.authority_repository,
        &config.protected_branch,
        landing_sha,
        expected_main_sha,
    );
    unstage_objects(&config.authority_repository, &incoming);

    match outcome? {
        PromotionOutcome::Rejected {
            expected_sha,
            actual_sha,
        } => Err(HarnessError::Control {
            reason: format!(
                "authority `{}` moved to {actual_sha} while promoting; it was {expected_sha} when the checks ran",
                config.protected_branch
            ),
            code: ErrorCode::ConflictControlHeadMoved,
        }),
        PromotionOutcome::Promoted { current_sha, .. } => {
            // Confirm rather than assume. `update-ref` reported success; this
            // checks what the branch actually resolves to now.
            if current_sha == landing_sha {
                Ok(())
            } else {
                Err(HarnessError::Control {
                    reason: format!(
                        "authority `{}` resolves to {current_sha} after promotion, not {landing_sha}",
                        config.protected_branch
                    ),
                    code: ErrorCode::RecoveryIncomplete,
                })
            }
        }
    }
}

/// Records everything that follows a successful authority update.
///
/// Separated from `run_promote` because recovery needs exactly this and nothing
/// else: when the authority moved and the command then died, the work left
/// undone is the local fast-forward and the control bookkeeping. Sharing the
/// code means a resumed promotion cannot record something subtly different
/// from one that completed in a single run.
#[allow(clippy::too_many_arguments)]
fn settle_promotion(
    control: &ControlRepository,
    events: &crate::control::event_store::EventStore<'_>,
    config: &crate::config::ProjectConfig,
    record: &mut IntegrationRecord,
    checks: &PromotionChecks,
    actor_id: &str,
    expected: Option<&str>,
    steps: &mut Steps<'_>,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    let integration_id = record.integration_id.clone();
    // Everything from here happens after the authority has already moved, and
    // none of it was journaled. Section 7.4 requires a step before the mutation
    // it names; without one, an interruption anywhere in this stretch was
    // attributable to nothing and could not be reproduced deliberately, which
    // is why the window went unnoticed.
    steps.outside_control("local-main-synced")?;
    synchronize_local_main(config, &checks.landing_sha)?;

    steps.at("integration-marked-promoted")?;
    record.status = IntegrationStatus::Promoted;
    control.write_atomic(
        &IntegrationRecord::relative_path(&integration_id),
        &format!("{}\n", serde_json::to_string_pretty(&record)?),
    )?;
    steps.at("cards-marked-landed")?;
    for member in &record.members {
        let (card, state) = load_card(control, &member.card_id)?;
        // A resumed promotion may find cards already landed, which is the
        // expected shape of a half-finished run rather than an error.
        if state.state != CardState::Landed {
            state.state.check_transition(CardState::Landed)?;
            crate::commands::card::store_card_state(control, &card, &state, CardState::Landed)?;
        }
    }

    events.append(
        &config.project_id,
        EventDraft::new("integration.promoted", actor_id)
            .cycle(record.cycle_id.clone())
            .head(checks.landing_sha.clone())
            .transition(
                Some(IntegrationStatus::Accepted.name()),
                IntegrationStatus::Promoted.name(),
            )
            .meta(
                "integration_id",
                serde_json::json!(integration_id.to_string()),
            )
            .meta("acceptance_id", serde_json::json!(checks.acceptance_id))
            .meta("landing_sha", serde_json::json!(checks.landing_sha))
            .meta("landing_tree", serde_json::json!(checks.landing_tree))
            .meta(
                "previous_main_sha",
                serde_json::json!(record.expected_main_sha),
            ),
        clock,
    )?;
    control.commit(expected, &format!("integration: promote {integration_id}"))?;

    Ok(CommandOutcome::new(
        "integration.promote",
        format!(
            "Promoted {integration_id}\nauthority `{}`: {} → {}\ncards landed: {}\nlocal `{}` fast-forwarded",
            config.protected_branch,
            record.expected_main_sha,
            checks.landing_sha,
            record.members.len(),
            config.protected_branch,
        ),
        serde_json::json!({
            "integration_id": integration_id.to_string(),
            "status": IntegrationStatus::Promoted.name(),
            "landing_sha": checks.landing_sha,
            "landing_tree": checks.landing_tree,
            "previous_main_sha": record.expected_main_sha,
            "acceptance_id": checks.acceptance_id,
            "cards": record.card_ids().map(ToString::to_string).collect::<Vec<_>>(),
        }),
    )
    .with_project(config.project_id.clone()))
}

fn run_promote(args: &PromoteArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let integration_id: IntegrationId = args.integration_id.parse()?;

    if args.dry_run {
        return preview_promote(args, &integration_id);
    }

    // Validate the transition before opening an operation journal. A normal
    // policy refusal (for example, attempting to move an archived integration)
    // must not become an unresolved recovery obligation for the next command.
    {
        let control = ControlRepository::open(&args.control)?;
        let config = control.project()?;
        let record = load_integration(&control, &integration_id)?;
        check_promotion(&control, &config, &record, &args.actor_id)?;
    }

    with_transaction(
        &args.control,
        "integration.promote",
        clock,
        |control, events, expected, steps| {
            let config = control.project()?;
            let mut record = load_integration(control, &integration_id)?;
            let checks = check_promotion(control, &config, &record, &args.actor_id)?;

            // Steps 7, 9, and 10. Named on both sides of the authority
            // update, because "we were about to publish" and "we published and
            // had not finished recording it" need different recoveries.
            steps.outside_control("authority-publish-attempted")?;
            publish(
                &config,
                &integration_id,
                &checks.landing_sha,
                &record.expected_main_sha,
            )?;
            steps.outside_control("authority-published")?;

            // Past this point the authority has moved. Section 13.6 is
            // explicit that a local synchronization failure must NOT roll it
            // back: the promotion happened, and rewinding a published branch
            // is worse than leaving a recoverable gap. `settle_promotion`
            // performs the sync and everything after it, and `project recover
            // --resume` re-enters at exactly the same place.
            settle_promotion(
                control,
                events,
                &config,
                &mut record,
                &checks,
                &args.actor_id,
                expected,
                steps,
                clock,
            )
        },
    )
}

/// What a stalled promotion turned out to be.
pub enum ResumeOutcome {
    /// The authority holds the landing commit; the rest was completed.
    Completed(Box<CommandOutcome>),
    /// The authority never moved, so nothing was left half-done.
    NothingHappened,
}

/// Completes a promotion whose authority update succeeded.
///
/// The state is re-derived from the world rather than read from a journal step.
/// A marker records what the harness *believed* it had done; the authority
/// branch records what actually happened, and after a crash only the second one
/// is worth trusting.
///
/// # Errors
///
/// Returns an error when control state cannot be read, or when the local
/// fast-forward still cannot be performed.
pub fn resume_promotion(
    control: &ControlRepository,
    events: &crate::control::event_store::EventStore<'_>,
    actor_id: &str,
    expected: Option<&str>,
    steps: &mut Steps<'_>,
    clock: &dyn Clock,
) -> Result<ResumeOutcome, HarnessError> {
    let config = control.project()?;
    let authority = inspect_authority(&config.authority_repository, &config.protected_branch)?;
    let Some(protected) = authority.protected_sha else {
        return Ok(ResumeOutcome::NothingHappened);
    };

    // The one integration this could be: accepted, with a landing commit the
    // authority is now sitting on. Any other shape means the update never
    // landed and there is nothing to finish.
    let directory = control.path(INTEGRATION_DIR);
    if !directory.exists() {
        return Ok(ResumeOutcome::NothingHappened);
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

    for name in names {
        let integration_id: IntegrationId = name.parse()?;
        let mut record = load_integration(control, &integration_id)?;
        if record.landing_sha.as_deref() != Some(protected.as_str()) {
            continue;
        }
        // `Accepted` is the shape when the local sync never ran. `Promoted`
        // with a card still short of `landed` is the shape when it ran and
        // something after it did not: the record was written, the cards were
        // not, and nothing had committed. Requiring `Accepted` alone made that
        // second window unrecoverable, and reported "no promotion reached the
        // authority" over an authority that plainly held the landing commit.
        let resumable = match record.status {
            // The local sync never ran.
            IntegrationStatus::Accepted => true,
            // The sync ran and something after it did not: the record was
            // written, the cards were not, and nothing had committed. Requiring
            // `Accepted` alone made this window unrecoverable and reported "no
            // promotion reached the authority" over an authority plainly
            // holding the landing commit.
            IntegrationStatus::Promoted => {
                let mut pending = false;
                for member in &record.members {
                    let (_card, state) = load_card(control, &member.card_id)?;
                    if state.state != CardState::Landed {
                        pending = true;
                        break;
                    }
                }
                pending
            }
            // Anything else means the update never landed, or the whole thing
            // finished and moved on.
            _ => false,
        };
        if !resumable {
            continue;
        }

        let acceptance =
            acceptance_for(control, &integration_id)?.ok_or_else(|| HarnessError::Control {
                reason: format!(
                    "integration {integration_id} is accepted but has no acceptance record"
                ),
                code: ErrorCode::InternalControlCorrupt,
            })?;
        let checks = PromotionChecks {
            landing_sha: protected.clone(),
            landing_tree: record.integration_tree.clone().unwrap_or_default(),
            acceptance_id: acceptance.acceptance_id.to_string(),
        };
        let outcome = settle_promotion(
            control,
            events,
            &config,
            &mut record,
            &checks,
            actor_id,
            expected,
            steps,
            clock,
        )?;
        return Ok(ResumeOutcome::Completed(Box::new(outcome)));
    }

    Ok(ResumeOutcome::NothingHappened)
}

/// Arguments accepted by `integration abandon`.
#[derive(Debug, Args)]
pub struct AbandonArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
    pub control: std::path::PathBuf,
    /// The integration to abandon.
    #[arg(long)]
    pub integration_id: String,
    /// Who is abandoning it.
    #[arg(long)]
    pub actor_id: String,
    /// Why it is being abandoned.
    #[arg(long)]
    pub reason: String,
    /// Report the outcome without recording it.
    #[arg(long)]
    pub dry_run: bool,
}

/// Abandons an integration that will not land, releasing its cycle.
///
/// Section 11.3 permits this from every pre-promoted state, and without it a
/// plan that fails verification holds its cycle's integration lease forever —
/// a state the model defines and nothing could reach. The member cards return
/// to `approved`, because their approvals are still valid: it was the
/// combination that failed, not the candidates.
fn run_abandon(args: &AbandonArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let integration_id: IntegrationId = args.integration_id.parse()?;

    if args.dry_run {
        let control = ControlRepository::open(&args.control)?;
        let config = control.project()?;
        let record = load_integration(&control, &integration_id)?;
        record
            .status
            .check_transition(IntegrationStatus::Abandoned)?;
        return Ok(CommandOutcome::new(
            "integration.abandon",
            format!(
                "Dry run: would abandon {integration_id}, returning {} card(s) to `approved`\nnothing was changed",
                record.members.len()
            ),
            serde_json::json!({
                "dry_run": true,
                "integration_id": integration_id.to_string(),
                "reason": args.reason,
            }),
        )
        .with_project(config.project_id));
    }

    with_transaction(
        &args.control,
        "integration.abandon",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let config = control.project()?;
            let mut record = load_integration(control, &integration_id)?;
            let previous = record.status;
            previous.check_transition(IntegrationStatus::Abandoned)?;

            record.status = IntegrationStatus::Abandoned;
            control.write_atomic(
                &IntegrationRecord::relative_path(&integration_id),
                &format!("{}\n", serde_json::to_string_pretty(&record)?),
            )?;
            for member in &record.members {
                let (card, state) = load_card(control, &member.card_id)?;
                if state.state == CardState::Integrating {
                    crate::commands::card::store_card_state(
                        control,
                        &card,
                        &state,
                        CardState::Approved,
                    )?;
                }
            }
            // The landing commit, if one was built, has no further purpose and
            // its ref would otherwise keep an unreachable commit alive forever.
            landing::release(&config.repository, integration_id.as_str())?;

            events.append(
                &config.project_id,
                EventDraft::new("integration.abandoned", &args.actor_id)
                    .cycle(record.cycle_id.clone())
                    .transition(Some(previous.name()), IntegrationStatus::Abandoned.name())
                    .meta(
                        "integration_id",
                        serde_json::json!(integration_id.to_string()),
                    )
                    .meta("reason", serde_json::json!(args.reason))
                    .meta(
                        "cards",
                        serde_json::json!(
                            record
                                .card_ids()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                        ),
                    ),
                clock,
            )?;
            control.commit(expected, &format!("integration: abandon {integration_id}"))?;

            Ok(CommandOutcome::new(
                "integration.abandon",
                format!(
                    "Abandoned {integration_id} (was `{}`)\nreason: {}\ncards returned to `approved`: {}",
                    previous.name(),
                    args.reason,
                    record.members.len()
                ),
                serde_json::json!({
                    "integration_id": integration_id.to_string(),
                    "status": IntegrationStatus::Abandoned.name(),
                    "previous_status": previous.name(),
                    "reason": args.reason,
                    "cards": record.card_ids().map(ToString::to_string).collect::<Vec<_>>(),
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #106 follow-up: `ControlWithRecovery` shares `Control`'s
    /// `#[error("control state: {reason}")]` Display, so `plain_reason` must
    /// strip the same prefix from both or `finalize_conflict` nests
    /// `"control state: "` mid-sentence the first time a
    /// `ConflictMergeFailed` site migrates to the new variant. Both errors
    /// carry the identical reason text on purpose: the only thing that can
    /// make `plain_reason`'s two outputs differ is the prefix, not the
    /// input.
    ///
    /// Reachable only as a unit test: nothing in this codebase constructs a
    /// `ConflictMergeFailed` `ControlWithRecovery` today (`plain_reason` is
    /// private, and #106 deliberately migrated no second site), so there is
    /// no real refusal to drive through `finalize_conflict` from outside
    /// this module.
    #[test]
    fn plain_reason_strips_the_control_state_prefix_from_both_control_variants() {
        let shared_reason = "shared reason text";
        let control = HarnessError::Control {
            reason: shared_reason.to_owned(),
            code: ErrorCode::ConflictMergeFailed,
        };
        let with_recovery = HarnessError::ControlWithRecovery {
            reason: shared_reason.to_owned(),
            code: ErrorCode::ConflictMergeFailed,
            recovery: "some recovery text.",
        };

        assert_eq!(plain_reason(&control), shared_reason);
        assert_eq!(plain_reason(&with_recovery), shared_reason);
    }
}
