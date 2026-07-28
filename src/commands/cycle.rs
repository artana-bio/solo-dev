//! Cycle lifecycle commands.

use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

use clap::{Args, Subcommand};

use crate::{
    cli::output::CommandOutcome,
    control::{
        event_store::{EventDraft, EventStore},
        journal::{Journal, OperationState},
        lock::ProjectLock,
        repository::ControlRepository,
    },
    domain::{
        clock::Clock,
        cycle::{CYCLE_SCHEMA, CycleRecord, CycleStatus, status_from_events},
        digest::Digest,
        ids::CycleId,
    },
    error::{ErrorCode, HarnessError},
    git::{command::GitScope, inspect},
};

/// Subcommands under `cycle`.
#[derive(Debug, Subcommand)]
pub enum CycleCommand {
    /// Declare a new cycle in draft.
    Create(CreateArgs),
    /// Freeze the cycle baseline and open it for cards.
    Activate(ActivateArgs),
    /// Report a cycle's derived status.
    Status(StatusArgs),
    /// Abandon a cycle that will not be landed.
    Abandon(AbandonArgs),
}

/// Arguments shared by every cycle subcommand.
#[derive(Debug, Args)]
pub struct CommonArgs {
    /// Path to the control repository.
    #[arg(long)]
    pub control: PathBuf,
    /// Identifies the acting party. Declared, not proven; see D-013.
    #[arg(long, default_value = "operator")]
    pub actor: String,
}

/// Arguments accepted by `cycle create`.
#[derive(Debug, Args)]
pub struct CreateArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// Identifier for the new cycle.
    #[arg(long)]
    pub cycle_id: String,
    /// What the cycle is for.
    #[arg(long)]
    pub objective: String,
    /// A condition the cycle must satisfy to be accepted. Repeatable.
    #[arg(long = "release-invariant")]
    pub release_invariants: Vec<String>,
    /// Report planned mutations without performing them.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `cycle activate`.
#[derive(Debug, Args)]
pub struct ActivateArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The cycle to activate.
    #[arg(long)]
    pub cycle_id: String,
    /// Report planned mutations without performing them.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `cycle status`.
#[derive(Debug, Args)]
pub struct StatusArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The cycle to report on.
    #[arg(long)]
    pub cycle_id: String,
}

/// Arguments accepted by `cycle abandon`.
#[derive(Debug, Args)]
pub struct AbandonArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The cycle to abandon.
    #[arg(long)]
    pub cycle_id: String,
    /// Why it is being abandoned.
    #[arg(long)]
    pub reason: String,
    /// Report planned mutations without performing them.
    #[arg(long)]
    pub dry_run: bool,
}

/// Executes a `cycle` subcommand.
///
/// # Errors
///
/// Returns a policy, precondition, or configuration error as appropriate.
pub fn execute(command: &CycleCommand, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    match command {
        CycleCommand::Create(args) => run_create(args, clock),
        CycleCommand::Activate(args) => run_activate(args, clock),
        CycleCommand::Status(args) => run_status(args),
        CycleCommand::Abandon(args) => run_abandon(args, clock),
    }
}

/// Reads a cycle record, or reports that it does not exist.
fn load(control: &ControlRepository, cycle_id: &CycleId) -> Result<CycleRecord, HarnessError> {
    let relative = CycleRecord::relative_path(cycle_id);
    if !control.path(&relative).exists() {
        return Err(HarnessError::Control {
            reason: format!("cycle {cycle_id} does not exist"),
            code: ErrorCode::PreconditionNotFound,
        });
    }
    let raw = control.read(&relative)?;
    serde_json::from_str(&raw).map_err(|source| HarnessError::Control {
        reason: format!("cycle {cycle_id} is malformed: {source}"),
        code: ErrorCode::InternalControlCorrupt,
    })
}

/// Writes a cycle record.
fn store(control: &ControlRepository, cycle: &CycleRecord) -> Result<(), HarnessError> {
    cycle.validate()?;
    control.write_atomic(
        &CycleRecord::relative_path(&cycle.cycle_id),
        &format!("{}\n", serde_json::to_string_pretty(cycle)?),
    )
}

/// The status implied by a cycle's authoritative events.
///
/// `WP-200` acceptance requires derivation from events, so the stored `status`
/// field is a cache. When the two disagree, history wins and the disagreement
/// is surfaced rather than smoothed over.
fn derived_status(
    events: &EventStore<'_>,
    cycle_id: &CycleId,
) -> Result<CycleStatus, HarnessError> {
    let records = events.for_cycle(cycle_id)?;
    let transitions: Vec<&str> = records
        .iter()
        .filter_map(|event| event.next_state.as_deref())
        .collect();
    Ok(status_from_events(transitions))
}

/// Runs a mutating cycle command inside the lock and the journal.
fn with_transaction<F>(
    control_path: &Path,
    command_name: &str,
    clock: &dyn Clock,
    body: F,
) -> Result<CommandOutcome, HarnessError>
where
    F: FnOnce(
        &ControlRepository,
        &EventStore<'_>,
        Option<&str>,
    ) -> Result<CommandOutcome, HarnessError>,
{
    let control = ControlRepository::open(control_path)?;
    let _lock = ProjectLock::acquire(control.root(), command_name, clock)?;
    let journal = Journal::new(&control);
    journal.require_settled()?;

    let expected_head = control.head()?;
    let mut operation = journal.begin(command_name, expected_head.clone(), clock)?;
    let events = EventStore::new(&control);

    match body(&control, &events, expected_head.as_deref()) {
        Ok(outcome) => {
            journal.finish(&mut operation, OperationState::Completed, None, clock)?;
            Ok(outcome.with_operation(operation.operation_id.clone()))
        }
        Err(error) => {
            // A failure before any write leaves nothing partial; the journal
            // records that distinction so recovery knows whether to look.
            let state = if control.is_clean()? {
                OperationState::FailedClean
            } else {
                OperationState::FailedPartial
            };
            journal.finish(&mut operation, state, Some(error.to_string()), clock)?;
            Err(error)
        }
    }
}

fn run_create(args: &CreateArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let cycle_id: CycleId = args.cycle_id.parse()?;

    if args.dry_run {
        let control = ControlRepository::open(&args.common.control)?;
        if control
            .path(&CycleRecord::relative_path(&cycle_id))
            .exists()
        {
            return Err(HarnessError::Control {
                reason: format!("cycle {cycle_id} already exists"),
                code: ErrorCode::PolicyInvalidCycle,
            });
        }
        return Ok(CommandOutcome::new(
            "cycle.create",
            format!("Dry run: would create cycle {cycle_id} in draft; nothing was changed"),
            serde_json::json!({ "dry_run": true, "cycle_id": cycle_id.to_string() }),
        ));
    }

    with_transaction(
        &args.common.control,
        "cycle.create",
        clock,
        |control, events, expected| {
            if control
                .path(&CycleRecord::relative_path(&cycle_id))
                .exists()
            {
                return Err(HarnessError::Control {
                    reason: format!("cycle {cycle_id} already exists"),
                    code: ErrorCode::PolicyInvalidCycle,
                });
            }
            let config = control.project()?;
            let cycle = CycleRecord {
                schema: CYCLE_SCHEMA.to_owned(),
                cycle_id: cycle_id.clone(),
                objective: args.objective.clone(),
                status: CycleStatus::INITIAL,
                baseline_sha: None,
                harness_version: env!("CARGO_PKG_VERSION").to_owned(),
                project_revision: Digest::of_canonical(&config)?,
                release_invariants: args.release_invariants.clone(),
                card_ids: Vec::new(),
                atomic_groups: Vec::new(),
                created_by: args.common.actor.clone(),
                created_at: clock.now(),
                activated_at: None,
            };
            store(control, &cycle)?;
            events.append(
                &config.project_id,
                EventDraft::new("cycle.created", &args.common.actor)
                    .cycle(cycle_id.clone())
                    .transition(None::<String>, CycleStatus::Draft.name()),
                clock,
            )?;
            control.commit(expected, &format!("cycle: create {cycle_id}"))?;

            Ok(CommandOutcome::new(
                "cycle.create",
                format!(
                    "Created cycle {cycle_id} in draft\nobjective: {}",
                    args.objective
                ),
                serde_json::json!({
                    "cycle_id": cycle_id.to_string(),
                    "status": CycleStatus::Draft.name(),
                    "objective": args.objective,
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}

fn run_activate(args: &ActivateArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let cycle_id: CycleId = args.cycle_id.parse()?;

    if args.dry_run {
        let control = ControlRepository::open(&args.common.control)?;
        let cycle = load(&control, &cycle_id)?;
        cycle.status.check_transition(CycleStatus::Active)?;
        let baseline = resolve_baseline(&control)?;
        return Ok(CommandOutcome::new(
            "cycle.activate",
            format!(
                "Dry run: would freeze cycle {cycle_id} at authority baseline {baseline}; nothing was changed"
            ),
            serde_json::json!({
                "dry_run": true,
                "cycle_id": cycle_id.to_string(),
                "baseline_sha": baseline,
            }),
        ));
    }

    with_transaction(
        &args.common.control,
        "cycle.activate",
        clock,
        |control, events, expected| {
            let mut cycle = load(control, &cycle_id)?;
            let previous = cycle.status;
            previous.check_transition(CycleStatus::Active)?;

            // Freezing happens once. Re-activating a cycle that already has a
            // baseline would silently move every card's starting point.
            if let Some(existing) = &cycle.baseline_sha {
                return Err(HarnessError::Control {
                    reason: format!(
                        "cycle {cycle_id} already froze its baseline at {existing}; a frozen baseline never moves"
                    ),
                    code: ErrorCode::PolicyInvalidCycle,
                });
            }

            let config = control.project()?;
            let baseline = resolve_baseline(control)?;
            cycle.status = CycleStatus::Active;
            cycle.baseline_sha = Some(baseline.clone());
            cycle.activated_at = Some(clock.now());
            store(control, &cycle)?;

            events.append(
                &config.project_id,
                EventDraft::new("cycle.activated", &args.common.actor)
                    .cycle(cycle_id.clone())
                    .transition(Some(previous.name()), CycleStatus::Active.name())
                    .head(baseline.clone())
                    .meta("baseline_sha", serde_json::json!(baseline)),
                clock,
            )?;
            control.commit(
                expected,
                &format!("cycle: activate {cycle_id} at {baseline}"),
            )?;

            Ok(CommandOutcome::new(
                "cycle.activate",
                format!("Activated cycle {cycle_id}\nfrozen baseline: {baseline}"),
                serde_json::json!({
                    "cycle_id": cycle_id.to_string(),
                    "status": CycleStatus::Active.name(),
                    "baseline_sha": baseline,
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}

/// Resolves the authority repository's protected branch to one exact commit.
///
/// The baseline comes from the authority, not the candidate. The candidate's
/// branch is whatever a local actor last did; the authority's is what has been
/// accepted.
fn resolve_baseline(control: &ControlRepository) -> Result<String, HarnessError> {
    let config = control.project()?;
    let scope = GitScope::git_dir(&config.authority_repository);
    inspect::resolve_commit(&scope, &format!("refs/heads/{}", config.protected_branch)).map_err(
        |_| HarnessError::Control {
            reason: format!(
                "authority branch `{}` does not resolve to one commit in {}",
                config.protected_branch,
                config.authority_repository.display()
            ),
            code: ErrorCode::ConfigProtectedBranch,
        },
    )
}

fn run_status(args: &StatusArgs) -> Result<CommandOutcome, HarnessError> {
    let cycle_id: CycleId = args.cycle_id.parse()?;
    let control = ControlRepository::open(&args.common.control)?;
    let config = control.project()?;
    let cycle = load(&control, &cycle_id)?;
    let events = EventStore::new(&control);
    let derived = derived_status(&events, &cycle_id)?;
    let history = events.for_cycle(&cycle_id)?;

    let drift = derived != cycle.status;
    let mut text = format!(
        "Cycle {cycle_id}\nobjective: {}\nstatus: {derived} (derived from {} event(s))\nbaseline: {}\ncards: {}",
        cycle.objective,
        history.len(),
        cycle.baseline_sha.as_deref().unwrap_or("not frozen"),
        if cycle.card_ids.is_empty() {
            "none".to_owned()
        } else {
            cycle
                .card_ids
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        }
    );
    for event in &history {
        let _ = write!(
            text,
            "\n  {} {} -> {}",
            event.occurred_at,
            event.previous_state.as_deref().unwrap_or("none"),
            event.next_state.as_deref().unwrap_or("none")
        );
    }

    let mut outcome = CommandOutcome::new(
        "cycle.status",
        text,
        serde_json::json!({
            "cycle_id": cycle_id.to_string(),
            "status": derived.name(),
            "stored_status": cycle.status.name(),
            "status_matches_history": !drift,
            "baseline_sha": cycle.baseline_sha,
            "objective": cycle.objective,
            "card_ids": cycle.card_ids,
            "event_count": history.len(),
        }),
    )
    .with_project(config.project_id.clone());

    if drift {
        // Surfaced rather than corrected: history is authoritative, and a
        // divergent cached field means something wrote state outside the
        // harness.
        outcome = outcome.with_warning(format!(
            "stored status `{}` disagrees with history `{derived}`; history is authoritative",
            cycle.status
        ));
    }
    Ok(outcome)
}

fn run_abandon(args: &AbandonArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let cycle_id: CycleId = args.cycle_id.parse()?;

    if args.dry_run {
        let control = ControlRepository::open(&args.common.control)?;
        let cycle = load(&control, &cycle_id)?;
        cycle.status.check_transition(CycleStatus::Abandoned)?;
        return Ok(CommandOutcome::new(
            "cycle.abandon",
            format!("Dry run: would abandon cycle {cycle_id}; nothing was changed"),
            serde_json::json!({ "dry_run": true, "cycle_id": cycle_id.to_string() }),
        ));
    }

    with_transaction(
        &args.common.control,
        "cycle.abandon",
        clock,
        |control, events, expected| {
            let mut cycle = load(control, &cycle_id)?;
            let previous = cycle.status;
            previous.check_transition(CycleStatus::Abandoned)?;

            let config = control.project()?;
            cycle.status = CycleStatus::Abandoned;
            store(control, &cycle)?;

            events.append(
                &config.project_id,
                EventDraft::new("cycle.abandoned", &args.common.actor)
                    .cycle(cycle_id.clone())
                    .transition(Some(previous.name()), CycleStatus::Abandoned.name())
                    .meta("reason", serde_json::json!(args.reason)),
                clock,
            )?;
            control.commit(expected, &format!("cycle: abandon {cycle_id}"))?;

            Ok(CommandOutcome::new(
                "cycle.abandon",
                format!("Abandoned cycle {cycle_id}\nreason: {}", args.reason),
                serde_json::json!({
                    "cycle_id": cycle_id.to_string(),
                    "status": CycleStatus::Abandoned.name(),
                    "reason": args.reason,
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::cycle::parse_status;

    #[test]
    fn status_parsing_covers_every_documented_name() {
        for status in [
            CycleStatus::Draft,
            CycleStatus::Active,
            CycleStatus::Integrating,
            CycleStatus::Accepted,
            CycleStatus::Landed,
            CycleStatus::Closed,
            CycleStatus::Blocked,
            CycleStatus::Abandoned,
        ] {
            assert_eq!(parse_status(status.name()), Some(status));
        }
        assert_eq!(parse_status("invented"), None);
    }
}
