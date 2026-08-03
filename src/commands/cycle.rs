//! Cycle lifecycle commands.

use std::{fmt::Write as _, fs, path::PathBuf};

use clap::{Args, Subcommand};

use crate::{
    cli::output::CommandOutcome,
    commands::CONTROL_ENV,
    commands::transaction::with_transaction,
    control::{
        event_store::{EventDraft, EventStore},
        repository::ControlRepository,
    },
    domain::{
        clock::Clock,
        cycle::{AtomicGroup, CYCLE_SCHEMA, CycleRecord, CycleStatus, status_from_events},
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
    /// Freeze the current card membership without stopping existing cards.
    Seal(SealArgs),
    /// Declare a set of cards that must land together.
    DeclareGroup(DeclareGroupArgs),
    /// Report a cycle's derived status.
    Status(StatusArgs),
    /// List every cycle in authoritative identifier order.
    List(ListArgs),
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
            Self::Status(..) => "cycle.status",
            Self::List(..) => "cycle.list",
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
        CycleCommand::Status(args) => run_status(args),
        CycleCommand::List(args) => run_list(args),
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
