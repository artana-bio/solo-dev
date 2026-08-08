//! Cycle lifecycle commands.

use std::{fmt::Write as _, fs, path::PathBuf};

use clap::{Args, Subcommand};

use crate::{
    cli::output::CommandOutcome,
    commands::CONTROL_ENV,
    commands::transaction::with_transaction,
    config::ProjectConfig,
    control::{
        event_store::{Event, EventDraft, EventStore},
        repository::ControlRepository,
    },
    domain::{
        clock::Clock,
        cycle::{AtomicGroup, CYCLE_SCHEMA, CycleRecord, CycleStatus, status_from_events},
        cycle_plan::{CYCLE_PLAN_SCHEMA, CyclePlan},
        digest::Digest,
        ids::CycleId,
    },
    error::{ErrorCode, HarnessError},
    git::{command::GitScope, inspect},
    policy::convergence::{
        CycleConvergence, CycleDimension, NextPermittedAction, assess_cycle, project,
    },
};

/// Subcommands under `cycle`.
#[derive(Debug, Subcommand)]
pub enum CycleCommand {
    /// Declare a new cycle in draft.
    Create(CreateArgs),
    /// Freeze the cycle baseline and open it for cards.
    Activate(ActivateArgs),
    /// Freeze the current card membership without stopping existing cards.
    Seal(SealArgs),
    /// Declare a set of cards that must land together.
    DeclareGroup(DeclareGroupArgs),
    /// Validate and persist the complete cycle distribution manifest.
    Plan(PlanArgs),
    /// Report a cycle's derived status.
    Status(StatusArgs),
    /// List every cycle in authoritative identifier order.
    List(ListArgs),
    /// Replay a cycle's journaled history as the assembly-line animation.
    ///
    /// Read-only: derives the playback from the event store and cross-checks
    /// the evidence along the way, flashing any discrepancy at the moment in
    /// history it was recorded.
    Replay(ReplayArgs),
    /// Abandon a cycle that will not be landed.
    Abandon(AbandonArgs),
}

impl CycleCommand {
    /// Its dotted command path, as the result envelope reports it.
    ///
    /// The error envelope used to carry only the group — `cycle` — while a
    /// success carried the full path, so a consumer matching on `command` got a
    /// different granularity depending on whether the command worked.
    #[must_use]
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Create(..) => "cycle.create",
            Self::Activate(..) => "cycle.activate",
            Self::Seal(..) => "cycle.seal",
            Self::DeclareGroup(..) => "cycle.declare-group",
            Self::Plan(..) => "cycle.plan",
            Self::Status(..) => "cycle.status",
            Self::List(..) => "cycle.list",
            Self::Replay(..) => "cycle.replay",
            Self::Abandon(..) => "cycle.abandon",
        }
    }
}

/// Arguments shared by every cycle subcommand.
#[derive(Debug, Args)]
pub struct CommonArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
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

/// Arguments accepted by `cycle seal`.
#[derive(Debug, Args)]
pub struct SealArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The active cycle whose current membership will be frozen.
    #[arg(long)]
    pub cycle_id: String,
    /// Report planned mutations without performing them.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `cycle declare-group`.
#[derive(Debug, Args)]
pub struct DeclareGroupArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The cycle the group belongs to.
    #[arg(long)]
    pub cycle_id: String,
    /// Name for the group.
    #[arg(long)]
    pub name: String,
    /// A card in the group. Repeat the option; at least two are expected.
    #[arg(long = "card-id")]
    pub card_ids: Vec<String>,
    /// Report planned mutations without performing them.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct PlanArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    #[arg(long)]
    pub plan_id: String,
    #[arg(long)]
    pub file: PathBuf,
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

/// Arguments accepted by `cycle list`.
#[derive(Debug, Args)]
pub struct ListArgs {
    #[command(flatten)]
    pub common: CommonArgs,
}

/// Arguments accepted by `cycle replay`.
#[derive(Debug, Args)]
pub struct ReplayArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The cycle to replay.
    #[arg(long)]
    pub cycle_id: String,
    /// Skip the animation and print only the timeline, even on a terminal.
    #[arg(long)]
    pub no_animation: bool,
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
        CycleCommand::Seal(args) => run_seal(args, clock),
        CycleCommand::DeclareGroup(args) => run_declare_group(args, clock),
        CycleCommand::Plan(args) => run_plan(args),
        CycleCommand::Status(args) => run_status(args),
        CycleCommand::List(args) => run_list(args),
        // The process entry point routes `cycle replay` through
        // [`execute_replay`] before this dispatcher, because the animation
        // needs the resolved output format. This arm exists only for
        // exhaustiveness; passing JSON here means a caller that somehow
        // reaches it gets the timeline and never a surprise animation.
        CycleCommand::Replay(args) => execute_replay(
            args,
            crate::cli::output::OutputFormat::Json,
            &crate::cli::tty::SystemEnvironment,
        ),
        CycleCommand::Abandon(args) => run_abandon(args, clock),
    }
}

fn run_plan(args: &PlanArgs) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.common.control)?;
    let raw = fs::read_to_string(&args.file).map_err(|source| HarnessError::WorkspaceAccess {
        path: args.file.clone(),
        source,
    })?;
    let plan: CyclePlan =
        serde_json::from_str(&raw).map_err(|source| HarnessError::ControlWithRecovery {
            reason: format!("cycle plan is malformed: {source}"),
            code: ErrorCode::ConfigMalformed,
            recovery: "Fix the cycle-plan JSON and rerun `cycle plan`; no plan was persisted.",
        })?;
    if plan.schema != CYCLE_PLAN_SCHEMA || plan.plan_id != args.plan_id {
        return Err(HarnessError::Control {
            reason: "cycle plan id or schema does not match the requested plan".to_owned(),
            code: ErrorCode::PolicyInvalidCycle,
        });
    }
    plan.validate()?;
    let cycle_id: CycleId = plan.cycle_id.parse().map_err(|_| HarnessError::Control {
        reason: format!("cycle plan {} names an invalid cycle id", plan.plan_id),
        code: ErrorCode::PolicyInvalidCycle,
    })?;
    let cycle: CycleRecord = serde_json::from_str(
        &control.read(&CycleRecord::relative_path(&cycle_id))?,
    )
    .map_err(|_| HarnessError::Control {
        reason: format!("cycle {cycle_id} is not available for this plan"),
        code: ErrorCode::PreconditionNotFound,
    })?;
    let planned: std::collections::BTreeSet<_> =
        plan.cards.iter().map(|card| card.card_id.clone()).collect();
    let members: std::collections::BTreeSet<_> =
        cycle.card_ids.iter().map(ToString::to_string).collect();
    if planned != members {
        return Err(HarnessError::Control {
            reason: format!(
                "cycle plan {} does not cover exactly the cycle's complete card membership",
                plan.plan_id
            ),
            code: ErrorCode::PolicyInvalidCycle,
        });
    }
    let relative = format!("plans/{}.json", plan.plan_id);
    if control.path(&relative).exists() {
        return Err(HarnessError::Control {
            reason: format!("cycle plan {} already exists", plan.plan_id),
            code: ErrorCode::PreconditionBranchExists,
        });
    }
    control.write_atomic(
        &relative,
        &format!("{}\n", serde_json::to_string_pretty(&plan)?),
    )?;
    let expected = control.head()?;
    control.commit(
        expected.as_deref(),
        &format!("cycle: persist plan {}", plan.plan_id),
    )?;
    Ok(CommandOutcome::new(
        "cycle.plan",
        format!("Persisted cycle plan {}", plan.plan_id),
        serde_json::to_value(&plan)?,
    )
    .with_project(control.project()?.project_id))
}

/// How long each replay frame is shown before the next replaces it.
const REPLAY_FRAME_DELAY: std::time::Duration = std::time::Duration::from_millis(90);

/// Executes `cycle replay`: derives the floor screenplay from the cycle's
/// journaled events, cross-checks the evidence, and plays the animation on
/// standard error when the environment allows it.
///
/// Read-only and lock-free, like `cycle status`. A discrepancy the
/// cross-check finds becomes a flash in the playback and a warning on the
/// result — not a failure. `audit cycle` is the command whose exit code
/// enforces evidence; this one is a viewer, and a viewer that refuses to
/// show a broken history is useless exactly when it matters most.
///
/// # Errors
///
/// Returns an error when the cycle does not exist or a record cannot be
/// read.
pub fn execute_replay(
    args: &ReplayArgs,
    format: crate::cli::output::OutputFormat,
    environment: &dyn crate::cli::tty::Environment,
) -> Result<CommandOutcome, HarnessError> {
    use crate::cli::{
        floor,
        replay::{EvidenceFlash, derive},
        tty::{SkipReason, skip_reason, terminal_width},
    };

    let cycle_id: CycleId = args.cycle_id.parse()?;
    let control = ControlRepository::open(&args.common.control)?;
    let config = control.project()?;
    let cycle = load(&control, &cycle_id)?;
    let events = EventStore::new(&control);
    let status = derived_status(&events, &cycle_id)?;
    let history = events.for_cycle(&cycle_id)?;

    let evidence = crate::commands::audit::cross_check_cycle(&control, &config, &cycle_id, &cycle)?;
    let flashes: Vec<EvidenceFlash> = evidence
        .discrepancies
        .iter()
        .map(|discrepancy| EvidenceFlash {
            // Subjects read `receipt R-000123`; the trailing token is the
            // record identifier the event metadata carries.
            record_id: discrepancy
                .subject
                .rsplit(' ')
                .next()
                .unwrap_or(&discrepancy.subject)
                .to_owned(),
            text: format!(
                "✗ evidence: {} claims {}; found {}",
                discrepancy.subject, discrepancy.claim, discrepancy.found
            ),
        })
        .collect();

    let derived = derive(
        &cycle_id.to_string(),
        cycle.baseline_sha.as_deref(),
        &history,
        &flashes,
    );

    let skip = skip_reason(format, args.no_animation, environment);
    if skip.is_none() {
        let width = terminal_width(environment);
        let mut sink = floor::TerminalSink::new(std::io::stderr());
        floor::play(
            &floor::frames_for(&derived.script, width),
            &mut sink,
            REPLAY_FRAME_DELAY,
        );
    }

    let text = if skip.is_none() {
        format!(
            "Replayed cycle {cycle_id} ({status}): {} event(s), {}",
            history.len(),
            evidence_summary(evidence.discrepancies.len()),
        )
    } else {
        timeline_text(
            &cycle_id,
            &cycle,
            status,
            &derived.timeline,
            &evidence.discrepancies,
        )
    };

    let mut outcome = CommandOutcome::new(
        "cycle.replay",
        text,
        serde_json::json!({
            "schema": "harness.cycle-replay/v1",
            "cycle_id": cycle_id.to_string(),
            "status": status.name(),
            "objective": cycle.objective,
            "baseline_sha": cycle.baseline_sha,
            "played": skip.is_none(),
            "skip_reason": skip.map(SkipReason::code),
            "event_count": history.len(),
            "beat_count": derived.script.beats.len(),
            "timeline": derived.timeline,
            "discrepancies": evidence.discrepancies,
        }),
    )
    .with_project(config.project_id.clone());

    for discrepancy in &evidence.discrepancies {
        outcome = outcome.with_warning(format!(
            "evidence: {} claims {}; found {}",
            discrepancy.subject, discrepancy.claim, discrepancy.found
        ));
    }
    Ok(outcome)
}

/// The evidence clause of the replay summary line.
fn evidence_summary(discrepancies: usize) -> String {
    if discrepancies == 0 {
        "evidence holds".to_owned()
    } else {
        format!("{discrepancies} discrepancy(ies)")
    }
}

/// The plain-text timeline: what a piped or `--no-animation` caller gets.
///
/// One line per event keeps this useful in a CI log, which is the honest
/// degradation of the animation rather than a consolation message.
fn timeline_text(
    cycle_id: &CycleId,
    cycle: &CycleRecord,
    status: CycleStatus,
    timeline: &[crate::cli::replay::TimelineEntry],
    discrepancies: &[crate::commands::audit::Discrepancy],
) -> String {
    let mut text = format!(
        "Replay of cycle {cycle_id} ({status})\nobjective: {}\nbaseline: {}\nevents: {}",
        cycle.objective,
        cycle.baseline_sha.as_deref().unwrap_or("not frozen"),
        timeline.len()
    );
    for entry in timeline {
        let _ = write!(
            text,
            "\n  {}  {}  {} · by {}",
            entry.at, entry.event_id, entry.description, entry.actor_id
        );
    }
    if discrepancies.is_empty() {
        text.push_str("\n\nevery recorded digest and commit still resolves");
    } else {
        let _ = write!(text, "\n\n{} discrepancy(ies):", discrepancies.len());
        for discrepancy in discrepancies {
            let _ = write!(
                text,
                "\n  {}\n    claims: {}\n    found:  {}",
                discrepancy.subject, discrepancy.claim, discrepancy.found
            );
        }
    }
    text
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
        // `for_cycle` intentionally includes the full cycle subtree for audit
        // output, so this must filter, not switch source. `card_id.is_none()`
        // alone is not enough: `card.created` deliberately omits `card_id`
        // (the card is not yet activated when it fires) and its `next_state`
        // is `draft`, which collides with `CycleStatus::Draft` — folding it
        // in would reset an active cycle to `draft` the moment any card in it
        // is created. `event_type` starting with `cycle.` is what actually
        // distinguishes a cycle's own transition from every other subject
        // that happens to share its cycle_id and, for some state name, its
        // vocabulary — card, integration, and acceptance events among them.
        .filter(|event| event.card_id.is_none() && event.event_type.starts_with("cycle."))
        .filter_map(|event| event.next_state.as_deref())
        .collect();
    Ok(status_from_events(transitions))
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
        |control, events, expected, steps| {
            steps.at("control-write")?;
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
        |control, events, expected, steps| {
            steps.at("control-write")?;
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

/// Freezes an active cycle's membership while allowing its existing cards to
/// finish normal work and review. The event carries the complete immutable
/// seal snapshot; the record's existing baseline and ordered members are not
/// reinterpreted or migrated.
fn run_seal(args: &SealArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let cycle_id: CycleId = args.cycle_id.parse()?;

    if args.dry_run {
        let control = ControlRepository::open(&args.common.control)?;
        let cycle = load(&control, &cycle_id)?;
        cycle.status.check_transition(CycleStatus::Sealed)?;
        let baseline = cycle
            .baseline_sha
            .clone()
            .ok_or_else(|| HarnessError::Control {
                reason: format!("cycle {cycle_id} cannot seal without a frozen baseline"),
                code: ErrorCode::PolicyInvalidCycle,
            })?;
        return Ok(CommandOutcome::new(
            "cycle.seal",
            format!("Dry run: would seal cycle {cycle_id}; nothing was changed"),
            serde_json::json!({
                "dry_run": true,
                "cycle_id": cycle_id.to_string(),
                "status": CycleStatus::Sealed.name(),
                "baseline_sha": baseline,
                "card_ids": cycle.card_ids,
            }),
        ));
    }

    with_transaction(
        &args.common.control,
        "cycle.seal",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let mut cycle = load(control, &cycle_id)?;
            let previous = cycle.status;
            previous.check_transition(CycleStatus::Sealed)?;
            let baseline = cycle
                .baseline_sha
                .clone()
                .ok_or_else(|| HarnessError::Control {
                    reason: format!("cycle {cycle_id} cannot seal without a frozen baseline"),
                    code: ErrorCode::PolicyInvalidCycle,
                })?;
            let card_ids = cycle.card_ids.clone();
            let config = control.project()?;

            cycle.status = CycleStatus::Sealed;
            store(control, &cycle)?;
            events.append(
                &config.project_id,
                EventDraft::new("cycle.sealed", &args.common.actor)
                    .cycle(cycle_id.clone())
                    .transition(Some(previous.name()), CycleStatus::Sealed.name())
                    .head(baseline.clone())
                    .meta("baseline_sha", serde_json::json!(baseline))
                    .meta(
                        "card_ids",
                        serde_json::json!(
                            card_ids.iter().map(ToString::to_string).collect::<Vec<_>>()
                        ),
                    ),
                clock,
            )?;
            control.commit(expected, &format!("cycle: seal {cycle_id}"))?;

            Ok(CommandOutcome::new(
                "cycle.seal",
                format!(
                    "Sealed cycle {cycle_id}\nfrozen members: {}",
                    if card_ids.is_empty() {
                        "none".to_owned()
                    } else {
                        card_ids
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                ),
                serde_json::json!({
                    "cycle_id": cycle_id.to_string(),
                    "status": CycleStatus::Sealed.name(),
                    "baseline_sha": baseline,
                    "card_ids": card_ids.iter().map(ToString::to_string).collect::<Vec<_>>(),
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

/// Renders a [`CycleDimension`] using the same spelling it would serialize
/// to, so `cycle status`'s human text never hand-spells a name `serde`
/// already owns. Mirrors `dimension_wire_name` in `card.rs`.
fn cycle_dimension_wire_name(dimension: CycleDimension) -> String {
    match serde_json::to_value(dimension) {
        Ok(serde_json::Value::String(name)) => name,
        _ => format!("{dimension:?}"),
    }
}

/// Renders a [`NextPermittedAction`] using the same spelling it would
/// serialize to. Mirrors `next_permitted_action_wire_name` in `card.rs`.
fn next_permitted_action_wire_name(action: NextPermittedAction) -> String {
    match serde_json::to_value(action) {
        Ok(serde_json::Value::String(name)) => name,
        _ => format!("{action:?}"),
    }
}

/// Assesses a cycle's convergence budget for `cycle status`, without
/// refusing.
///
/// 73-1: the cycle-level counterpart of `card.rs`'s `card_convergence`
/// (72-3) — read that one first. Same projection call, same error handling,
/// same shape of failure when a fact is malformed: a malformed, duplicate,
/// foreign, or unbound convergence fact fails here exactly as it does
/// there, because that is control-repository corruption, a different
/// problem than an exhausted budget.
///
/// Unlike `card_convergence`, this takes the cycle's events already loaded
/// by its one caller, `run_status`, instead of reading them again:
/// `EventStore::for_cycle` rescans the whole event directory from disk, and
/// `run_status` already needs that same scan for its own event-count and
/// transition-history report, so an independent second read here would pay
/// for a second full scan to get an answer it already has. There is also no
/// `require_cycle_convergence_budget` this has to agree with yet, unlike
/// `card_convergence`'s reason for reading independently — #73's first card
/// is decision-plus-report only; a later card adds that enforcement
/// function, and it will read its own events for the same reason
/// `require_convergence_budget` does today.
///
/// # Errors
///
/// Returns a control error when the recorded convergence facts cannot be
/// projected.
fn cycle_convergence(
    config: &ProjectConfig,
    cycle_id: &CycleId,
    events: &[Event],
) -> Result<CycleConvergence, HarnessError> {
    let policy = config.convergence_policy.as_ref();
    let view = project(policy, &config.project_id, cycle_id, events).map_err(|error| {
        HarnessError::Control {
            reason: format!("convergence projection for cycle {cycle_id} is unusable: {error}"),
            code: ErrorCode::InternalControlCorrupt,
        }
    })?;
    Ok(assess_cycle(policy, &view))
}

fn run_status(args: &StatusArgs) -> Result<CommandOutcome, HarnessError> {
    let cycle_id: CycleId = args.cycle_id.parse()?;
    let control = ControlRepository::open(&args.common.control)?;
    let config = control.project()?;
    let cycle = load(&control, &cycle_id)?;
    let events = EventStore::new(&control);
    let derived = derived_status(&events, &cycle_id)?;
    let history = events.for_cycle(&cycle_id)?;

    // 73-1: computed right after `history` is fetched, before `text` is
    // built, so both the JSON below and the human text read from the one
    // assessment — they can never disagree about whether the cycle is
    // escalated. `cycle status` never turns an escalated budget into a
    // refusal; see `cycle_convergence`. Mirrors how `card status` (72-3)
    // publishes `data.convergence` from `assess_card`.
    let convergence = cycle_convergence(&config, &cycle_id, &history)?;

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
    // The human text says the same thing the JSON does, not less: an
    // operator reading text output must not have to ask for JSON to learn
    // which dimension is exhausted or what they may do next. Mirrors `card
    // status`'s own escalation report exactly.
    if let CycleConvergence::Escalated {
        exhausted,
        next_permitted_action,
    } = &convergence
    {
        let _ = write!(text, "\nconvergence: escalated");
        for dimension in exhausted {
            let _ = write!(
                text,
                "\n  {}: {}/{} (evidence: {})",
                cycle_dimension_wire_name(dimension.dimension),
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
            "convergence": convergence,
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

/// Lists every valid cycle record, with status derived from its own events.
///
/// The records are deliberately read as one all-or-nothing control-plane
/// snapshot. Returning a partial list while silently skipping one malformed
/// record would make an agent believe that a cycle was absent when the control
/// authority was actually damaged.
fn run_list(args: &ListArgs) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.common.control)?;
    let config = control.project()?;
    let directory = control.path(crate::domain::cycle::CYCLE_DIR);
    let mut cycles = Vec::new();

    if directory.exists() {
        let entries = fs::read_dir(&directory).map_err(|source| HarnessError::ControlIo {
            path: directory.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| HarnessError::ControlIo {
                path: directory.clone(),
                source,
            })?;
            let path = entry.path();
            if !entry
                .file_type()
                .map_err(|source| HarnessError::ControlIo {
                    path: path.clone(),
                    source,
                })?
                .is_file()
                || path.extension().is_none_or(|extension| extension != "json")
            {
                continue;
            }

            let relative = path
                .strip_prefix(control.root())
                .expect("cycle entry is rooted in the control repository")
                .to_string_lossy()
                .into_owned();
            let raw = control.read(&relative)?;
            let cycle: CycleRecord =
                serde_json::from_str(&raw).map_err(|source| HarnessError::Control {
                    reason: format!("cycle record `{relative}` is malformed: {source}"),
                    code: ErrorCode::InternalControlCorrupt,
                })?;
            cycle.validate().map_err(|source| HarnessError::Control {
                reason: format!("cycle record `{relative}` is malformed: {source}"),
                code: ErrorCode::InternalControlCorrupt,
            })?;

            let events = EventStore::new(&control);
            let status = derived_status(&events, &cycle.cycle_id)?;
            cycles.push((cycle, status));
        }
    }

    cycles.sort_by(|(left, _), (right, _)| left.cycle_id.cmp(&right.cycle_id));
    let data_cycles: Vec<serde_json::Value> = cycles
        .iter()
        .map(|(cycle, status)| {
            serde_json::json!({
                "cycle_id": cycle.cycle_id.to_string(),
                "status": status.name(),
                "baseline_frozen": cycle.baseline_sha.is_some(),
                "member_count": cycle.card_ids.len(),
            })
        })
        .collect();
    let text = if cycles.is_empty() {
        "No cycles".to_owned()
    } else {
        let lines = cycles.iter().map(|(cycle, status)| {
            format!(
                "{}: {} (baseline frozen: {}, members: {})",
                cycle.cycle_id,
                status.name(),
                cycle.baseline_sha.is_some(),
                cycle.card_ids.len()
            )
        });
        format!("Cycles\n{}", lines.collect::<Vec<_>>().join("\n"))
    };

    Ok(CommandOutcome::new(
        "cycle.list",
        text,
        serde_json::json!({ "cycles": data_cycles }),
    )
    .with_project(config.project_id))
}

/// Declares an atomic group over cards already in the cycle.
///
/// `WP-200` defined `atomic_groups` on the cycle record and validated them, but
/// left no way to declare one, so the validation was unreachable. A group is
/// declared after its cards exist, because `CycleRecord::validate` requires
/// every member to already be declared in the cycle.
fn run_declare_group(
    args: &DeclareGroupArgs,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    let cycle_id: CycleId = args.cycle_id.parse()?;
    let card_ids: Vec<crate::domain::ids::CardId> = args
        .card_ids
        .iter()
        .map(|raw| raw.parse())
        .collect::<Result<_, _>>()?;

    if card_ids.len() < 2 {
        return Err(HarnessError::Control {
            reason: format!(
                "atomic group `{}` needs at least two cards; a single card is already atomic",
                args.name
            ),
            code: ErrorCode::PolicyInvalidCycle,
        });
    }

    if args.dry_run {
        return preview_declare_group(args, &cycle_id, &card_ids);
    }

    with_transaction(
        &args.common.control,
        "cycle.declare-group",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let mut cycle = load(control, &cycle_id)?;
            if cycle
                .atomic_groups
                .iter()
                .any(|group| group.name == args.name)
            {
                return Err(HarnessError::Control {
                    reason: format!(
                        "cycle {cycle_id} already declares an atomic group named `{}`",
                        args.name
                    ),
                    code: ErrorCode::PolicyInvalidCycle,
                });
            }

            let config = control.project()?;
            cycle.atomic_groups.push(AtomicGroup {
                name: args.name.clone(),
                card_ids: card_ids.clone(),
            });
            // Membership, duplication, and overlap are all checked here rather
            // than piecemeal above, so one rule set governs every writer.
            cycle.validate()?;
            store(control, &cycle)?;

            events.append(
                &config.project_id,
                EventDraft::new("cycle.group-declared", &args.common.actor)
                    .cycle(cycle_id.clone())
                    .meta("name", serde_json::json!(args.name))
                    .meta(
                        "card_ids",
                        serde_json::json!(
                            card_ids.iter().map(ToString::to_string).collect::<Vec<_>>()
                        ),
                    ),
                clock,
            )?;
            control.commit(
                expected,
                &format!("cycle: declare atomic group {} in {cycle_id}", args.name),
            )?;

            Ok(CommandOutcome::new(
                "cycle.declare-group",
                format!(
                    "Declared atomic group `{}` in cycle {cycle_id}\ncards: {}",
                    args.name,
                    card_ids
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                serde_json::json!({
                    "cycle_id": cycle_id.to_string(),
                    "name": args.name,
                    "card_ids": card_ids.iter().map(ToString::to_string).collect::<Vec<_>>(),
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}

/// Validates a group declaration against the stored cycle, changing nothing.
fn preview_declare_group(
    args: &DeclareGroupArgs,
    cycle_id: &CycleId,
    card_ids: &[crate::domain::ids::CardId],
) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.common.control)?;
    let mut cycle = load(&control, cycle_id)?;
    cycle.atomic_groups.push(AtomicGroup {
        name: args.name.clone(),
        card_ids: card_ids.to_vec(),
    });
    cycle.validate()?;
    Ok(CommandOutcome::new(
        "cycle.declare-group",
        format!(
            "Dry run: would declare atomic group `{}` over {} cards; nothing was changed",
            args.name,
            card_ids.len()
        ),
        serde_json::json!({
            "dry_run": true,
            "cycle_id": cycle_id.to_string(),
            "name": args.name,
            "card_ids": card_ids.iter().map(ToString::to_string).collect::<Vec<_>>(),
        }),
    ))
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
        |control, events, expected, steps| {
            steps.at("control-write")?;
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
