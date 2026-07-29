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
    commands::{
        acceptance::acceptance_for,
        card::load_card,
        gate::{load_gate, next_receipt_id, receipts_for},
        handoff::latest_handoff,
        review::{current_approval, reviews_for},
        transaction::with_transaction,
        work::held_lease,
    },
    control::{event_store::EventDraft, repository::ControlRepository},
    domain::{
        card::{CardRecord, CardState},
        clock::Clock,
        cycle::CycleRecord,
        digest::CANONICAL_ALGORITHM,
        ids::{CardId, CycleId, IntegrationId},
        integration::{
            INTEGRATION_DIR, INTEGRATION_SCHEMA, IntegrationMember, IntegrationMode,
            IntegrationRecord, IntegrationStatus, Interaction, InvariantCheck, VERIFICATION_SCHEMA,
            VerificationRecord, interactions, topological_order,
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
    runner::{
        environment_fingerprint,
        receipt::{LOG_DIR, RECEIPT_SCHEMA, Receipt},
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
}

/// Arguments accepted by `integration merge`.
#[derive(Debug, Args)]
pub struct MergeArgs {
    /// Path to the control repository.
    #[arg(long)]
    pub control: std::path::PathBuf,
    /// The integration to combine.
    #[arg(long)]
    pub integration_id: String,
    /// Who is performing the merge.
    #[arg(long)]
    pub actor_id: String,
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
    #[arg(long)]
    pub control: std::path::PathBuf,
    /// The cycle to report on.
    #[arg(long)]
    pub cycle_id: String,
}

/// Arguments accepted by `integration prepare`.
#[derive(Debug, Args)]
pub struct PrepareArgs {
    /// Path to the control repository.
    #[arg(long)]
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
    #[arg(long)]
    pub control: std::path::PathBuf,
    /// The integration to report.
    #[arg(long)]
    pub integration_id: String,
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
    }
}

/// Reads a cycle record.
fn load_cycle(
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

/// One card's integrability, with the reason when it is not integrable.
struct Candidacy {
    record: CardRecord,
    state: CardState,
    approval: Option<ReviewRecord>,
    /// Absent when the card is ready; otherwise why it is not.
    blocked_by: Option<String>,
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
    let mut assessed = Vec::new();
    for card_id in &cycle.card_ids {
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
            Some(candidate) => {
                current_approval(control, card_id, candidate, &state.current_digest)?
            }
            None => None,
        };

        let blocked_by = match (&handoff, &approval) {
            (None, _) => Some("card has no handoff".to_owned()),
            (Some(handoff), None) => Some(
                // Section 15.2: an approval is void once the candidate SHA or
                // the card digest changes.
                if reviews_for(control, card_id)?.is_empty() {
                    "card has no review".to_owned()
                } else if let Some(reason) = head
                    .as_deref()
                    .and_then(|head| handoff.staleness(head, &state.current_digest))
                {
                    format!("approval no longer describes the current candidate: {reason}")
                } else {
                    "approval no longer describes the current candidate or card revision".to_owned()
                },
            ),
            (Some(_), Some(_)) => None,
        };

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

    if args.dry_run {
        return preview_prepare(args, &cycle_id, &requested);
    }

    with_transaction(
        &args.control,
        "integration.prepare",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let config = control.project()?;
            let cycle = load_cycle(control, &cycle_id)?;
            let plan = build_plan(control, &cycle, &requested, args)?;

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

            Ok(report_integration(
                "integration.prepare",
                &record,
                &digest,
                &config.project_id,
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
        status: IntegrationStatus::Prepared,
        mode: plan.mode,
        baseline_sha,
        expected_main_sha,
        members: plan.members,
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
}

/// Validates a selection and derives its plan, changing nothing.
fn build_plan(
    control: &ControlRepository,
    cycle: &CycleRecord,
    requested: &[CardId],
    args: &PrepareArgs,
) -> Result<Plan, HarnessError> {
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

    let assessed = assess(control, cycle)?;
    let selected = select(&assessed, requested)?;
    if selected.is_empty() {
        return Err(HarnessError::Control {
            reason: format!("cycle {} has no cards ready to integrate", cycle.cycle_id),
            code: ErrorCode::PreconditionNotFound,
        });
    }

    check_dependencies(&selected, &assessed)?;
    let atomic_groups = check_atomic_groups(cycle, &selected)?;
    let mode = resolve_mode(args.mode, selected.len())?;
    let members = plan_members(control, cycle, &selected)?;

    Ok(Plan {
        members,
        atomic_groups,
        mode,
    })
}

fn preview_prepare(
    args: &PrepareArgs,
    cycle_id: &CycleId,
    requested: &[CardId],
) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.control)?;
    let config = control.project()?;
    let cycle = load_cycle(&control, cycle_id)?;
    let plan = build_plan(&control, &cycle, requested, args)?;

    let order: Vec<String> = plan
        .members
        .iter()
        .map(|member| member.card_id.to_string())
        .collect();
    Ok(CommandOutcome::new(
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
            "merge_order": order,
            "atomic_groups": plan.atomic_groups,
        }),
    )
    .with_project(config.project_id))
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
    let mut text = format!(
        "Integration {} ({})\ncycle: {}\nmode: {}\nexpected authority baseline: {}\nmerge order:\n  {}",
        record.integration_id,
        record.status.name(),
        record.cycle_id,
        record.mode.name(),
        record.expected_main_sha,
        order.join("\n  ")
    );
    if let Some(head) = &record.integration_head {
        let _ = std::fmt::Write::write_fmt(&mut text, format_args!("\nintegration head: {head}"));
    }
    if let Some(landing) = &record.landing_sha {
        let _ = std::fmt::Write::write_fmt(&mut text, format_args!("\nlanding commit: {landing}"));
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
            "baseline_sha": record.baseline_sha,
            "expected_main_sha": record.expected_main_sha,
            "atomic_groups": record.atomic_groups,
            "integration_head": record.integration_head,
            "integration_tree": record.integration_tree,
            "landing_sha": record.landing_sha,
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

fn run_merge(args: &MergeArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let integration_id: IntegrationId = args.integration_id.parse()?;

    if args.dry_run {
        let control = ControlRepository::open(&args.control)?;
        let config = control.project()?;
        let record = load_integration(&control, &integration_id)?;
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

    with_transaction(
        &args.control,
        "integration.merge",
        clock,
        |control, events, expected, steps| {
            let config = control.project()?;
            let mut record = load_integration(control, &integration_id)?;
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
    )
}

/// Refuses an integration whose cards declare shared generated artifacts.
///
/// `WP-540` owns integration-owned artifact generation. Landing such a project
/// now would produce a tree missing whatever the artifacts were meant to be, so
/// this refuses with a stable code rather than landing something incomplete.
fn require_no_generated_artifacts(
    control: &ControlRepository,
    record: &IntegrationRecord,
) -> Result<(), HarnessError> {
    for member in &record.members {
        let (card, _) = load_card(control, &member.card_id)?;
        if !card.generated_artifacts.is_empty() {
            return Err(HarnessError::Control {
                reason: format!(
                    "card {} declares generated artifacts ({}), which integration-owned generation does not support until WP-540",
                    member.card_id,
                    card.generated_artifacts.join(", ")
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
            steps.at("landing-commit-created")?;
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
            steps.at("landing-ref-retained")?;

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
fn store_verification(
    control: &ControlRepository,
    record: &IntegrationRecord,
    cycle: &CycleRecord,
    receipts: &mut [Receipt],
    worktree_clean_after: bool,
    actor_id: &str,
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

    Ok(VerificationRecord {
        schema: VERIFICATION_SCHEMA.to_owned(),
        integration_id: record.integration_id.clone(),
        cycle_id: record.cycle_id.clone(),
        landing_sha: record.landing_sha.clone().unwrap_or_default(),
        landing_tree: record.integration_tree.clone().unwrap_or_default(),
        receipt_ids,
        failed_gates,
        invariants: cycle
            .release_invariants
            .iter()
            .map(|invariant| InvariantCheck {
                invariant: invariant.clone(),
                machine_checked: false,
            })
            .collect(),
        interactions: member_interactions(control, record)?,
        worktree_clean_after,
        verified_by: actor_id.to_owned(),
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
    #[arg(long)]
    pub control: std::path::PathBuf,
    /// The integration to review.
    #[arg(long)]
    pub integration_id: String,
    /// Who is reviewing. Must differ from whoever verified it.
    #[arg(long)]
    pub reviewer_actor_id: String,
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

fn run_integration_review(
    args: &ReviewArgs,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    let integration_id: IntegrationId = args.integration_id.parse()?;

    if args.dry_run {
        let control = ControlRepository::open(&args.control)?;
        let config = control.project()?;
        let record = load_integration(&control, &integration_id)?;
        record
            .status
            .check_transition(IntegrationStatus::Reviewed)?;
        let verification = load_verification(&control, &integration_id)?;
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
            record
                .status
                .check_transition(IntegrationStatus::Reviewed)?;
            let verification = load_verification(control, &integration_id)?;

            // Section 15.1's independence rule applies here too: whoever ran
            // the gates cannot be the one who judges what they proved.
            if verification.verified_by == args.reviewer_actor_id {
                return Err(HarnessError::Control {
                    reason: format!(
                        "{} verified this integration and cannot also review it",
                        args.reviewer_actor_id
                    ),
                    code: ErrorCode::PolicySameActor,
                });
            }
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
    #[arg(long)]
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

/// Verifies everything Section 13.6 requires before the authority is touched.
fn check_promotion(
    control: &ControlRepository,
    config: &crate::config::ProjectConfig,
    record: &IntegrationRecord,
) -> Result<PromotionChecks, HarnessError> {
    if record.status != IntegrationStatus::Accepted {
        return Err(HarnessError::Control {
            reason: format!(
                "integration {} is `{}`; only an accepted integration may be promoted",
                record.integration_id,
                record.status.name()
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
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
        return Err(HarnessError::Control {
            reason: format!("promotion is not authorized: {reason}"),
            code: ErrorCode::PolicyNotAccepted,
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
    let checks = check_promotion(&control, &config, &record)?;

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
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    let integration_id = record.integration_id.clone();
    synchronize_local_main(config, &checks.landing_sha)?;

    record.status = IntegrationStatus::Promoted;
    control.write_atomic(
        &IntegrationRecord::relative_path(&integration_id),
        &format!("{}\n", serde_json::to_string_pretty(&record)?),
    )?;
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

    with_transaction(
        &args.control,
        "integration.promote",
        clock,
        |control, events, expected, steps| {
            let config = control.project()?;
            let mut record = load_integration(control, &integration_id)?;
            let checks = check_promotion(control, &config, &record)?;

            // Steps 7, 9, and 10. Named on both sides of the authority
            // update, because "we were about to publish" and "we published and
            // had not finished recording it" need different recoveries.
            steps.at("authority-publish-attempted")?;
            publish(
                &config,
                &integration_id,
                &checks.landing_sha,
                &record.expected_main_sha,
            )?;
            steps.at("authority-published")?;

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
        if record.status != IntegrationStatus::Accepted
            || record.landing_sha.as_deref() != Some(protected.as_str())
        {
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
    #[arg(long)]
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
