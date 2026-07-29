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
        card::load_card,
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
            IntegrationRecord, IntegrationStatus, topological_order,
        },
        review::ReviewRecord,
    },
    error::{ErrorCode, HarnessError},
    git::{authority::inspect_authority, command::GitScope, inspect},
};

/// Subcommands under `integration`.
#[derive(Debug, Subcommand)]
pub enum IntegrationCommand {
    /// List approved cards awaiting integration, and why others are not ready.
    Ready(ReadyArgs),
    /// Select approved candidates into a deterministic integration plan.
    Prepare(PrepareArgs),
    /// Report a prepared integration.
    Inspect(InspectArgs),
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
        |control, events, expected| {
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
    let text = format!(
        "Prepared integration {} ({})\ncycle: {}\nmode: {}\nexpected authority baseline: {}\nmerge order:\n  {}",
        record.integration_id,
        record.status.name(),
        record.cycle_id,
        record.mode.name(),
        record.expected_main_sha,
        order.join("\n  ")
    );

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
            "members": record.members,
        }),
    )
    .with_project(project_id.clone())
}

fn run_inspect(args: &InspectArgs) -> Result<CommandOutcome, HarnessError> {
    let integration_id: IntegrationId = args.integration_id.parse()?;
    let control = ControlRepository::open(&args.control)?;
    let config = control.project()?;

    let relative = IntegrationRecord::relative_path(&integration_id);
    if !control.path(&relative).exists() {
        return Err(HarnessError::Control {
            reason: format!("integration {integration_id} does not exist"),
            code: ErrorCode::PreconditionNotFound,
        });
    }
    let record: IntegrationRecord =
        serde_json::from_str(&control.read(&relative)?).map_err(|source| {
            HarnessError::Control {
                reason: format!("integration {integration_id} is malformed: {source}"),
                code: ErrorCode::InternalControlCorrupt,
            }
        })?;
    let digest = record.digest()?;

    Ok(report_integration(
        "integration.inspect",
        &record,
        &digest,
        &config.project_id,
    ))
}
