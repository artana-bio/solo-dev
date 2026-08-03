//! Named gate registry commands.
//!
//! The registry is the trusted side of D-008: gates are defined here, by
//! project policy, and cards may only name them. Registration is therefore a
//! deliberate act with its own command, not a side effect of authoring a card.

use std::{fmt::Write as _, fs, path::PathBuf};

use clap::{Args, Subcommand};

use crate::{
    cli::output::CommandOutcome,
    commands::CONTROL_ENV,
    commands::{card::load_card, transaction::with_transaction, work::held_lease},
    control::{event_store::EventDraft, repository::ControlRepository},
    domain::{
        clock::Clock,
        cycle::CycleRecord,
        digest::Digest,
        gate::{GATE_DIR, GateDefinition, NetworkPolicy},
        ids::{CardId, ReceiptId},
    },
    error::{ErrorCode, HarnessError},
    git::{command::GitScope, inspect},
    policy::progressive_validation::{ValidationProgress, plan, progress, stages_before_satisfied},
    runner::{
        environment_fingerprint,
        receipt::{LOG_DIR, RECEIPT_DIR, RECEIPT_SCHEMA, Receipt, evidence_is_acceptable},
        run_attempt,
    },
};

/// Subcommands under `gate`.
#[derive(Debug, Subcommand)]
pub enum GateCommand {
    /// Validate a gate definition without storing it.
    Validate(DefinitionArgs),
    /// Register or revise a gate definition.
    Register(RegisterArgs),
    /// List registered gates.
    List(CommonArgs),
    /// Show one registered gate and its digest.
    Show(ShowArgs),
    /// Run a named gate against a card's candidate.
    Run(RunArgs),
    /// Show the deterministic validation stages for an activated card.
    Preflight(PreflightArgs),
    /// Report a card's gate evidence and whether it still applies.
    Status(StatusArgs),
}

impl GateCommand {
    /// Its dotted command path, as the result envelope reports it.
    ///
    /// The error envelope used to carry only the group — `gate` — while a
    /// success carried the full path, so a consumer matching on `command` got a
    /// different granularity depending on whether the command worked.
    #[must_use]
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Validate(..) => "gate.validate",
            Self::Register(..) => "gate.register",
            Self::List(..) => "gate.list",
            Self::Show(..) => "gate.show",
            Self::Run(..) => "gate.run",
            Self::Preflight(..) => "gate.preflight",
            Self::Status(..) => "gate.status",
        }
    }
}

/// Arguments accepted by `gate run`.
#[derive(Debug, Args)]
pub struct RunArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The card whose candidate the gate runs against.
    #[arg(long)]
    pub card_id: String,
    /// The gate to run.
    #[arg(long)]
    pub gate_id: String,
    /// Report planned mutations without performing them.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `gate preflight`.
#[derive(Debug, Args)]
pub struct PreflightArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The activated card whose validation ladder to project.
    #[arg(long)]
    pub card_id: String,
}

/// Arguments accepted by `gate status`.
#[derive(Debug, Args)]
pub struct StatusArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The card to report on.
    #[arg(long)]
    pub card_id: String,
}

/// Arguments shared by registry subcommands.
#[derive(Debug, Args)]
pub struct CommonArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
    pub control: PathBuf,
    /// Identifies the acting party. Declared, not proven; see D-013.
    #[arg(long, default_value = "operator")]
    pub actor: String,
}

/// Arguments accepted by `gate validate`.
#[derive(Debug, Args)]
pub struct DefinitionArgs {
    /// Path to the gate definition, in YAML or JSON.
    #[arg(long)]
    pub definition: PathBuf,
}

/// Arguments accepted by `gate register`.
#[derive(Debug, Args)]
pub struct RegisterArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// Path to the gate definition, in YAML or JSON.
    #[arg(long)]
    pub definition: PathBuf,
    /// Report planned mutations without performing them.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `gate show`.
#[derive(Debug, Args)]
pub struct ShowArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The gate to display.
    #[arg(long)]
    pub gate_id: String,
}

/// Executes a `gate` subcommand.
///
/// # Errors
///
/// Returns a configuration or precondition error as appropriate.
pub fn execute(command: &GateCommand, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    match command {
        GateCommand::Validate(args) => run_validate(args),
        GateCommand::Register(args) => run_register(args, clock),
        GateCommand::List(args) => run_list(args),
        GateCommand::Show(args) => run_show(args),
        GateCommand::Run(args) => run_gate(args, clock),
        GateCommand::Preflight(args) => run_preflight(args),
        GateCommand::Status(args) => run_status(args),
    }
}

/// Reads and parses a gate definition from disk.
fn read_definition(path: &PathBuf) -> Result<GateDefinition, HarnessError> {
    let raw = fs::read_to_string(path).map_err(|source| HarnessError::Control {
        reason: format!("cannot read gate definition {}: {source}", path.display()),
        code: ErrorCode::ConfigMalformed,
    })?;
    parse_definition(&raw)
}

/// Parses a gate definition from YAML or JSON.
///
/// # Errors
///
/// Returns a configuration error when the document is malformed.
pub fn parse_definition(raw: &str) -> Result<GateDefinition, HarnessError> {
    serde_yaml_ng::from_str(raw).map_err(|source| HarnessError::Control {
        reason: format!("gate definition is malformed: {source}"),
        code: ErrorCode::ConfigMalformed,
    })
}

/// Reads one registered gate.
///
/// # Errors
///
/// Returns a configuration error when the gate is not registered.
pub fn load_gate(
    control: &ControlRepository,
    gate_id: &str,
) -> Result<GateDefinition, HarnessError> {
    let relative = GateDefinition::relative_path(gate_id);
    if !control.path(&relative).exists() {
        return Err(HarnessError::Control {
            reason: format!("gate `{gate_id}` is not registered"),
            code: ErrorCode::ConfigUnknownGate,
        });
    }
    serde_json::from_str(&control.read(&relative)?).map_err(|source| HarnessError::Control {
        reason: format!("gate `{gate_id}` is malformed: {source}"),
        code: ErrorCode::InternalControlCorrupt,
    })
}

/// Requires every named gate to be registered.
///
/// Called from card activation, so a card can never name a check that does not
/// exist. Without this a card could pass activation and only fail much later,
/// at the point where its evidence was supposed to be produced.
///
/// # Errors
///
/// Returns a configuration error naming the first unregistered gate.
pub fn require_registered<'a>(
    control: &ControlRepository,
    gate_ids: impl IntoIterator<Item = &'a String>,
) -> Result<(), HarnessError> {
    for gate_id in gate_ids {
        load_gate(control, gate_id)?;
    }
    Ok(())
}

/// Every registered gate, sorted by identifier.
///
/// # Errors
///
/// Returns an error when the registry cannot be read.
pub fn all_gates(control: &ControlRepository) -> Result<Vec<GateDefinition>, HarnessError> {
    let directory = control.path(GATE_DIR);
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
    names.iter().map(|name| load_gate(control, name)).collect()
}

fn run_validate(args: &DefinitionArgs) -> Result<CommandOutcome, HarnessError> {
    let gate = read_definition(&args.definition)?;
    gate.validate()?;
    Ok(CommandOutcome::new(
        "gate.validate",
        format!(
            "Gate `{}` revision {} is valid\nargv: {:?}\ntimeout: {}s\nnetwork: {}\nmax attempts: {}",
            gate.gate_id,
            gate.revision,
            gate.argv,
            gate.timeout_seconds,
            gate.network_policy.describe(),
            gate.retry_policy.max_attempts
        ),
        serde_json::json!({
            "gate_id": gate.gate_id,
            "revision": gate.revision,
            "digest": gate.digest()?.as_str(),
            "valid": true,
            "network_policy_enforced": NetworkPolicy::ENFORCED,
        }),
    ))
}

fn run_register(args: &RegisterArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let gate = read_definition(&args.definition)?;
    gate.validate()?;
    let digest = gate.digest()?;

    if args.dry_run {
        let control = ControlRepository::open(&args.common.control)?;
        let previous = load_gate(&control, &gate.gate_id).ok();
        return Ok(CommandOutcome::new(
            "gate.register",
            format!(
                "Dry run: would register gate `{}` revision {} with digest {digest}; nothing was changed",
                gate.gate_id, gate.revision
            ),
            serde_json::json!({
                "dry_run": true,
                "gate_id": gate.gate_id,
                "revision": gate.revision,
                "digest": digest.as_str(),
                "supersedes_revision": previous.map(|gate| gate.revision),
            }),
        ));
    }

    with_transaction(
        &args.common.control,
        "gate.register",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let config = control.project()?;
            let previous = load_gate(control, &gate.gate_id).ok();

            if let Some(existing) = &previous {
                // A revision must move forward by exactly one, so a receipt can
                // be traced to a definition rather than to whichever version
                // happened to be on disk.
                if gate.revision != existing.revision + 1 {
                    return Err(HarnessError::Control {
                        reason: format!(
                            "gate `{}` is at revision {}; the next revision must be {}, not {}",
                            gate.gate_id,
                            existing.revision,
                            existing.revision + 1,
                            gate.revision
                        ),
                        code: ErrorCode::ConfigInvalidGate,
                    });
                }
            } else if gate.revision != 1 {
                return Err(HarnessError::Control {
                    reason: format!(
                        "gate `{}` is not registered, so its first revision must be 1, not {}",
                        gate.gate_id, gate.revision
                    ),
                    code: ErrorCode::ConfigInvalidGate,
                });
            }

            control.write_atomic(
                &GateDefinition::relative_path(&gate.gate_id),
                &format!("{}\n", serde_json::to_string_pretty(&gate)?),
            )?;

            let mut draft = EventDraft::new("gate.registered", &args.common.actor)
                .meta("gate_id", serde_json::json!(gate.gate_id))
                .meta("revision", serde_json::json!(gate.revision))
                .meta("gate_digest", serde_json::json!(digest.as_str()));
            if let Some(existing) = &previous {
                draft = draft
                    .meta("superseded_revision", serde_json::json!(existing.revision))
                    .meta(
                        "superseded_digest",
                        serde_json::json!(existing.digest()?.as_str()),
                    );
            }
            events.append(&config.project_id, draft, clock)?;
            control.commit(
                expected,
                &format!("gate: register {} r{}", gate.gate_id, gate.revision),
            )?;

            let supersedes = previous.as_ref().map(|gate| gate.revision);
            Ok(CommandOutcome::new(
                "gate.register",
                format!(
                    "Registered gate `{}` revision {}\ndigest: {digest}{}",
                    gate.gate_id,
                    gate.revision,
                    supersedes.map_or_else(String::new, |revision| format!(
                        "\nsupersedes revision {revision}; receipts bound to its digest are now stale"
                    ))
                ),
                serde_json::json!({
                    "gate_id": gate.gate_id,
                    "revision": gate.revision,
                    "digest": digest.as_str(),
                    "supersedes_revision": supersedes,
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}

fn run_list(args: &CommonArgs) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.control)?;
    let config = control.project()?;
    let gates = all_gates(&control)?;

    let mut text = format!("{} registered gate(s)", gates.len());
    let mut payload = Vec::new();
    for gate in &gates {
        let digest = gate.digest()?;
        let _ = write!(
            text,
            "\n  {} r{} {:?} timeout {}s",
            gate.gate_id, gate.revision, gate.argv, gate.timeout_seconds
        );
        payload.push(serde_json::json!({
            "gate_id": gate.gate_id,
            "revision": gate.revision,
            "digest": digest.as_str(),
            "argv": gate.argv,
            "timeout_seconds": gate.timeout_seconds,
        }));
    }

    Ok(
        CommandOutcome::new("gate.list", text, serde_json::json!({ "gates": payload }))
            .with_project(config.project_id.clone()),
    )
}

fn run_show(args: &ShowArgs) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.common.control)?;
    let config = control.project()?;
    let gate = load_gate(&control, &args.gate_id)?;
    let digest = gate.digest()?;

    Ok(CommandOutcome::new(
        "gate.show",
        format!(
            "Gate `{}` revision {}\ndigest: {digest}\nargv: {:?}\nworking directory: {}\ntimeout: {}s\nnetwork: {}\nmax attempts: {}",
            gate.gate_id,
            gate.revision,
            gate.argv,
            if gate.working_directory.is_empty() {
                "."
            } else {
                &gate.working_directory
            },
            gate.timeout_seconds,
            gate.network_policy.describe(),
            gate.retry_policy.max_attempts
        ),
        serde_json::json!({
            "definition": gate,
            "digest": digest.as_str(),
            "network_policy_enforced": NetworkPolicy::ENFORCED,
        }),
    )
    .with_project(config.project_id.clone()))
}

/// Builds the one authoritative progression projection used by preview, real
/// execution, handoff, and integration. Keeping the checks here avoids a
/// report that says one thing while a mutating boundary accepts another.
///
/// # Errors
///
/// Returns a refusal when frozen card/cycle/policy inputs are invalid or the
/// plan cannot be built from the registered gate definitions.
pub fn validation_progress(
    control: &ControlRepository,
    card_id: &CardId,
    candidate_sha: Option<&str>,
) -> Result<ValidationProgress, HarnessError> {
    let config = control.project()?;
    let (record, state) = load_card(control, card_id)?;
    let recomputed_card_digest = record.digest()?;
    if recomputed_card_digest != state.current_digest {
        return Err(HarnessError::Control {
            reason: format!(
                "card {card_id} revision {} recomputes to {recomputed_card_digest}, but state records {}; the immutable card record was altered",
                record.revision, state.current_digest
            ),
            code: ErrorCode::InternalControlCorrupt,
        });
    }
    let cycle: CycleRecord =
        serde_json::from_str(&control.read(&CycleRecord::relative_path(&record.cycle_id))?)
            .map_err(|source| HarnessError::Control {
                reason: format!("cycle {} is malformed: {source}", record.cycle_id),
                code: ErrorCode::InternalControlCorrupt,
            })?;
    let baseline = cycle
        .baseline_sha
        .as_deref()
        .ok_or_else(|| HarnessError::Control {
            reason: format!("cycle {} has no frozen baseline", cycle.cycle_id),
            code: ErrorCode::PolicyInvalidCycle,
        })?;
    if record.base_sha != baseline {
        return Err(HarnessError::Control {
            reason: format!(
                "card {card_id} declares base {}, but cycle {} is frozen at {baseline}",
                record.base_sha, cycle.cycle_id
            ),
            code: ErrorCode::PolicyCycleBaselineMismatch,
        });
    }
    let project_digest = Digest::of_canonical(&config)?;
    if cycle.project_revision != project_digest {
        return Err(HarnessError::Control {
            reason: format!(
                "cycle {} was created under project revision {}, but the current project configuration is {}; start a new cycle or restore the frozen policy",
                cycle.cycle_id, cycle.project_revision, project_digest
            ),
            code: ErrorCode::PolicyInvalidCycle,
        });
    }
    let policy_digest = Digest::of_canonical(&config.validation_policy)?;
    let plan = plan(
        &record,
        recomputed_card_digest,
        &config.validation_policy,
        policy_digest,
        &all_gates(control)?,
    )?;
    Ok(progress(
        plan,
        candidate_sha,
        &receipts_for(control, card_id)?,
    ))
}

/// Builds the same read-only plan every caller must use before attempting
/// progressive validation.
fn run_preflight(args: &PreflightArgs) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;
    let control = ControlRepository::open(&args.common.control)?;
    let config = control.project()?;
    let candidate = held_lease(&control, &card_id)?.and_then(|lease| {
        inspect::resolve_commit(&GitScope::work_tree(&lease.worktree_path), "HEAD").ok()
    });
    let progress = validation_progress(&control, &card_id, candidate.as_deref())?;
    Ok(render_preflight(&card_id, &config.project_id, progress))
}

/// Renders a plan without adding another source of truth beside the structured
/// plan itself.
fn render_preflight(
    card_id: &CardId,
    project_id: &crate::domain::ids::ProjectId,
    progress: ValidationProgress,
) -> CommandOutcome {
    let plan = &progress.plan;
    let mut text = format!(
        "Validation preflight for {card_id} r{}\nbase: {}\nrisk: {}\nnext permitted stage: {}",
        plan.card_revision,
        plan.base_sha,
        plan.risk,
        progress
            .next_permitted_stage
            .map_or("complete", |stage| stage.name())
    );
    for stage in &plan.stages {
        let names = stage
            .checks
            .iter()
            .map(|check| check.gate_id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let _ = write!(
            text,
            "\n  {}: {}",
            stage.stage.name(),
            if names.is_empty() {
                "no checks"
            } else {
                &names
            }
        );
    }
    CommandOutcome::new(
        "gate.preflight",
        text,
        serde_json::to_value(progress).expect("plan is serializable"),
    )
    .with_project(project_id.clone())
}

/// Allocates the next receipt identifier.
/// Allocates the next receipt identifier.
///
/// # Errors
///
/// Returns an error when the receipt directory cannot be read.
pub fn next_receipt_id(control: &ControlRepository) -> Result<ReceiptId, HarnessError> {
    let directory = control.path(RECEIPT_DIR);
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
                    .and_then(|stem| stem.strip_prefix("R-"))
                    .and_then(|digits| digits.parse::<u64>().ok())
            })
            .max()
            .unwrap_or(0)
    } else {
        0
    };
    format!("R-{:06}", highest + 1).parse()
}

/// Every receipt recorded for one card, oldest first.
///
/// # Errors
///
/// Returns an error when the store cannot be read.
pub fn receipts_for(
    control: &ControlRepository,
    card_id: &CardId,
) -> Result<Vec<Receipt>, HarnessError> {
    let directory = control.path(RECEIPT_DIR);
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

    let mut receipts = Vec::new();
    for name in names {
        let raw = control.read(&format!("{RECEIPT_DIR}/{name}.json"))?;
        let receipt: Receipt =
            serde_json::from_str(&raw).map_err(|source| HarnessError::Control {
                reason: format!("receipt {name} is malformed: {source}"),
                code: ErrorCode::InternalControlCorrupt,
            })?;
        if receipt.card_id.as_ref() == Some(card_id) {
            receipts.push(receipt);
        }
    }
    Ok(receipts)
}

/// Reports what `gate run` would execute, without running it.
fn preview_run(args: &RunArgs, card_id: &CardId) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.common.control)?;
    let gate = load_gate(&control, &args.gate_id)?;
    // The card and its lease, checked the way the real run checks them. This
    // preview reported that it would run a gate for a card with no worktree to
    // run it in, which is not something the real command can do.
    let (_record, _state) = load_card(&control, card_id)?;
    let lease = held_lease(&control, card_id)?.ok_or_else(|| HarnessError::Control {
        reason: format!("card {card_id} holds no lease; run `work start` first"),
        code: ErrorCode::PreconditionNotFound,
    })?;
    let candidate = inspect::resolve_commit(&GitScope::work_tree(&lease.worktree_path), "HEAD")?;
    let progress = validation_progress(&control, card_id, Some(&candidate))?;
    require_next_gate(&progress, &args.gate_id)?;
    Ok(CommandOutcome::new(
        "gate.run",
        format!(
            "Dry run: would run gate `{}` for card {card_id} as {:?} in {}; nothing was changed",
            gate.gate_id,
            gate.argv,
            lease.worktree_path.display()
        ),
        serde_json::json!({
            "dry_run": true,
            "card_id": card_id.to_string(),
            "gate_id": gate.gate_id,
            "argv": gate.argv,
            "worktree_path": lease.worktree_path,
        }),
    ))
}

/// Counts prior attempts for this exact card revision, gate definition, and
/// commit.
///
/// Numbering continues across runs so a retry is visible as a retry rather than
/// as a fresh first attempt. Section 14.2.
fn next_attempt_number(
    existing: &[Receipt],
    card_digest: &crate::domain::digest::Digest,
    gate_id: &str,
    evaluated_sha: &str,
    gate_digest: &crate::domain::digest::Digest,
) -> u32 {
    u32::try_from(
        existing
            .iter()
            .filter(|receipt| {
                receipt.card_digest.as_ref() == Some(card_digest)
                    && receipt.gate_id == gate_id
                    && receipt.evaluated_sha == evaluated_sha
                    && receipt.gate_digest == *gate_digest
            })
            .count(),
    )
    .unwrap_or(u32::MAX)
        + 1
}

fn run_gate(args: &RunArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;

    if args.dry_run {
        return preview_run(args, &card_id);
    }

    with_transaction(
        &args.common.control,
        "gate.run",
        clock,
        |control, events, expected, steps| {
            let config = control.project()?;
            let (record, state) = load_card(control, &card_id)?;
            let gate = load_gate(control, &args.gate_id)?;
            let gate_digest = gate.digest()?;

            let lease = held_lease(control, &card_id)?.ok_or_else(|| HarnessError::Control {
                reason: format!("card {card_id} holds no lease; run `work start` first"),
                code: ErrorCode::PreconditionNotFound,
            })?;

            let scope = GitScope::work_tree(&lease.worktree_path);
            let evaluated_sha = inspect::resolve_commit(&scope, "HEAD")?;
            let progress = validation_progress(control, &card_id, Some(&evaluated_sha))?;
            require_next_gate(&progress, &args.gate_id)?;
            // A gate run against a dirty worktree is not refused: running gates
            // while iterating is the normal development loop, and refusing
            // would make the command useless for the case it exists to serve.
            // What is refused downstream is treating that run as evidence about
            // `evaluated_sha`, which it is not. The receipt carries the
            // distinction so `staleness` can state it plainly.
            let worktree_clean = inspect::worktree_state(&scope)?.clean;

            let existing = receipts_for(control, &card_id)?;
            let attempt = next_attempt_number(
                &existing,
                &state.current_digest,
                &gate.gate_id,
                &evaluated_sha,
                &gate_digest,
            );

            let started_at = clock.now();
            let log_root = control.path(LOG_DIR).join(card_id.as_str());
            steps.at("gate-attempt-started")?;
            let outcome = run_attempt(&gate, &lease.worktree_path, &log_root, attempt, clock)?;
            let finished_at = clock.now();

            let receipt_id = next_receipt_id(control)?;
            let receipt = Receipt {
                schema: RECEIPT_SCHEMA.to_owned(),
                receipt_id: receipt_id.clone(),
                project_id: config.project_id.clone(),
                cycle_id: record.cycle_id.clone(),
                card_id: Some(card_id.clone()),
                card_digest: Some(state.current_digest.clone()),
                integration_id: None,
                evaluated_sha: evaluated_sha.clone(),
                gate_id: gate.gate_id.clone(),
                gate_digest: gate_digest.clone(),
                harness_version: env!("CARGO_PKG_VERSION").to_owned(),
                environment_fingerprint: environment_fingerprint(&gate),
                started_at,
                finished_at,
                duration_ms: outcome.duration_ms,
                exit_code: outcome.exit_code,
                termination: outcome.termination,
                stdout_digest: outcome.stdout_digest.clone(),
                stderr_digest: outcome.stderr_digest.clone(),
                artifact_digests: outcome.artifact_digests.clone(),
                log_location: outcome.log_location.clone(),
                attempt,
                passed: outcome.passed(),
                worktree_clean: Some(worktree_clean),
            };

            control.write_atomic(
                &Receipt::relative_path(&receipt_id),
                &format!("{}\n", serde_json::to_string_pretty(&receipt)?),
            )?;

            // Both outcomes are recorded, per invariant 7.4.1. A failing gate
            // that left no trace would let a later run present itself as the
            // first.
            events.append(
                &config.project_id,
                EventDraft::new("gate.ran", &args.common.actor)
                    .cycle(record.cycle_id.clone())
                    .card(
                        card_id.clone(),
                        state.current_revision,
                        state.current_digest.clone(),
                    )
                    .head(evaluated_sha.clone())
                    .meta("gate_id", serde_json::json!(gate.gate_id))
                    .meta("gate_digest", serde_json::json!(gate_digest.as_str()))
                    .meta("receipt_id", serde_json::json!(receipt_id.to_string()))
                    .meta("attempt", serde_json::json!(attempt))
                    .meta("termination", serde_json::json!(outcome.termination.name()))
                    .meta("passed", serde_json::json!(receipt.passed)),
                clock,
            )?;
            control.commit(
                expected,
                &format!("gate: run {} for {card_id} attempt {attempt}", gate.gate_id),
            )?;

            report_run(&receipt, &config.project_id)
        },
    )
}

/// Refuses attempts to skip the frozen order. Final-integration checks are
/// deliberately not runnable against one card: they are owned by combined
/// verification after a landing SHA exists.
///
/// # Errors
///
/// Returns a transition refusal when this card declares the requested gate but
/// it is not the one permitted by the current exact evidence.
pub fn require_next_gate(
    progress: &ValidationProgress,
    requested_gate: &str,
) -> Result<(), HarnessError> {
    let requested_stage = progress.plan.stages.iter().find_map(|stage| {
        stage
            .checks
            .iter()
            .any(|check| check.gate_id == requested_gate)
            .then_some(stage.stage)
    });
    // A registered gate outside this card's declared progressive proof may be
    // run while developing. It produces a receipt, but cannot stand in for a
    // required named check; preserving that existing capability avoids making
    // the ladder a second generic gate-runner.
    let Some(requested_stage) = requested_stage else {
        return Ok(());
    };
    if requested_stage == crate::config::ValidationStage::FinalIntegration {
        return Err(HarnessError::Control {
            reason: format!(
                "gate `{requested_gate}` belongs to final integration; prepare the approved candidate and let combined integration verification run it on the landing SHA"
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    }
    if progress.next_permitted_gate.as_deref() != Some(requested_gate) {
        return Err(HarnessError::Control {
            reason: format!(
                "gate `{requested_gate}` is not the next permitted check; next permitted gate is `{}` at `{}`: {}",
                progress.next_permitted_gate.as_deref().unwrap_or("none"),
                progress
                    .next_permitted_stage
                    .map_or("complete", |stage| stage.name()),
                progress
                    .blocked_reason
                    .as_deref()
                    .unwrap_or("all required per-card checks are complete")
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    }
    Ok(())
}

/// Requires the narrow proof before a handoff can bind candidate evidence.
///
/// # Errors
///
/// Returns a refusal when plan inputs are invalid or a narrow-stage receipt is
/// missing, stale, dirty, or failed.
pub fn require_before_handoff(
    control: &ControlRepository,
    card_id: &CardId,
    candidate_sha: &str,
) -> Result<ValidationProgress, HarnessError> {
    let progress = validation_progress(control, card_id, Some(candidate_sha))?;
    if !stages_before_satisfied(&progress, crate::config::ValidationStage::Handoff) {
        return Err(HarnessError::Control {
            reason: format!(
                "narrow proof is not current before handoff: {}",
                progress
                    .blocked_reason
                    .as_deref()
                    .unwrap_or("run the next permitted narrow gate")
            ),
            code: ErrorCode::GateEvidenceStale,
        });
    }
    Ok(progress)
}

/// Requires every pre-integration stage. Final-integration checks remain
/// intentionally unsatisfied until combined verification owns the landing SHA.
///
/// # Errors
///
/// Returns a refusal when plan inputs are invalid or a required pre-integration
/// receipt is missing, stale, dirty, or failed.
pub fn require_before_integration(
    control: &ControlRepository,
    card_id: &CardId,
    candidate_sha: &str,
) -> Result<ValidationProgress, HarnessError> {
    let progress = validation_progress(control, card_id, Some(candidate_sha))?;
    if !stages_before_satisfied(&progress, crate::config::ValidationStage::FinalIntegration) {
        return Err(HarnessError::Control {
            reason: format!(
                "required validation before integration is not current: {}",
                progress
                    .blocked_reason
                    .as_deref()
                    .unwrap_or("run the next permitted gate")
            ),
            code: ErrorCode::GateEvidenceStale,
        });
    }
    Ok(progress)
}

/// Turns a committed receipt into the command's outcome.
///
/// A failed gate is a refusal, not a report. The receipt is committed either
/// way, so the failure is evidence regardless; what changes is whether the
/// caller may treat the candidate as gated.
fn report_run(
    receipt: &Receipt,
    project_id: &crate::domain::ids::ProjectId,
) -> Result<CommandOutcome, HarnessError> {
    let exit = receipt
        .exit_code
        .map_or_else(|| "none (signalled)".to_owned(), |code| code.to_string());

    if !receipt.passed {
        return Err(HarnessError::Control {
            reason: format!(
                "gate `{}` did not pass for {}: termination {}, exit {exit}",
                receipt.gate_id,
                receipt.subject(),
                receipt.termination.name()
            ),
            code: ErrorCode::GateFailed,
        });
    }

    Ok(CommandOutcome::new(
        "gate.run",
        format!(
            "Gate `{}` attempt {} for {}\ncommit: {}\ntermination: {}\nexit code: {exit}\nverdict: PASS\nreceipt: {}\nlogs: {}",
            receipt.gate_id,
            receipt.attempt,
            receipt.subject(),
            receipt.evaluated_sha,
            receipt.termination.name(),
            receipt.receipt_id,
            receipt.log_location.display()
        ),
        serde_json::to_value(receipt)?,
    )
    .with_project(project_id.clone()))
}

fn run_status(args: &StatusArgs) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;
    let control = ControlRepository::open(&args.common.control)?;
    let config = control.project()?;
    let (record, _state) = load_card(&control, &card_id)?;
    let receipts = receipts_for(&control, &card_id)?;

    let candidate = held_lease(&control, &card_id)?.and_then(|lease| {
        inspect::resolve_commit(&GitScope::work_tree(&lease.worktree_path), "HEAD").ok()
    });

    let mut text = format!(
        "Card {card_id}\ncandidate: {}\nreceipts: {}",
        candidate.as_deref().unwrap_or("no allocation"),
        receipts.len()
    );

    let mut summary = Vec::new();
    for gate_id in &record.named_gates.feature {
        let gate = load_gate(&control, gate_id)?;
        let gate_digest = gate.digest()?;
        let relevant: Vec<&Receipt> = receipts
            .iter()
            .filter(|receipt| receipt.gate_id == *gate_id)
            .collect();

        let current: Vec<&&Receipt> = candidate.as_ref().map_or_else(Vec::new, |sha| {
            relevant
                .iter()
                .filter(|receipt| receipt.is_current_for(sha, &gate_digest))
                .collect()
        });
        let owned: Vec<Receipt> = current.iter().map(|receipt| (**receipt).clone()).collect();
        let satisfied = evidence_is_acceptable(&owned, gate.retry_policy.max_attempts);

        let stale_reason = candidate.as_ref().and_then(|sha| {
            relevant
                .last()
                .and_then(|receipt| receipt.staleness(sha, &gate_digest))
        });

        let _ = write!(
            text,
            "\n  {gate_id}: {} ({} run(s), {} current)",
            if satisfied {
                "satisfied"
            } else {
                "not satisfied"
            },
            relevant.len(),
            current.len()
        );
        if let Some(reason) = &stale_reason
            && !satisfied
        {
            let _ = write!(text, "\n    stale: {reason}");
        }

        summary.push(serde_json::json!({
            "gate_id": gate_id,
            "satisfied": satisfied,
            "runs": relevant.len(),
            "current_runs": current.len(),
            "stale_reason": stale_reason,
        }));
    }

    Ok(CommandOutcome::new(
        "gate.status",
        text,
        serde_json::json!({
            "card_id": card_id.to_string(),
            "candidate_sha": candidate,
            "feature_gates": summary,
            "receipts": receipts,
        }),
    )
    .with_project(config.project_id.clone()))
}

#[cfg(test)]
mod progressive_tests {
    use std::{collections::BTreeMap, path::PathBuf};

    use super::*;
    use crate::{
        domain::{
            clock::FixedClock,
            digest::Digest,
            ids::{CardId, CycleId, ProjectId, ReceiptId},
        },
        runner::receipt::Termination,
    };

    fn receipt(card_digest: Digest) -> Receipt {
        let timestamp = FixedClock::at_unix_seconds(1_785_196_800).unwrap().now();
        Receipt {
            schema: RECEIPT_SCHEMA.to_owned(),
            receipt_id: "R-000001".parse::<ReceiptId>().unwrap(),
            project_id: "example".parse::<ProjectId>().unwrap(),
            cycle_id: "C-001".parse::<CycleId>().unwrap(),
            card_id: Some("F-001".parse::<CardId>().unwrap()),
            card_digest: Some(card_digest),
            integration_id: None,
            evaluated_sha: "a".repeat(40),
            gate_id: "gate.unit".to_owned(),
            gate_digest: Digest::of_bytes(b"gate"),
            harness_version: "test".to_owned(),
            environment_fingerprint: "test".to_owned(),
            started_at: timestamp,
            finished_at: timestamp,
            duration_ms: 0,
            exit_code: Some(0),
            termination: Termination::Completed,
            stdout_digest: Digest::of_bytes(b""),
            stderr_digest: Digest::of_bytes(b""),
            artifact_digests: BTreeMap::new(),
            log_location: PathBuf::from("/tmp/gate"),
            attempt: 1,
            passed: true,
            worktree_clean: Some(true),
        }
    }

    #[test]
    fn a_new_card_revision_starts_gate_attempt_numbering_at_one() {
        let old_digest = Digest::of_bytes(b"old card revision");
        let new_digest = Digest::of_bytes(b"new card revision");
        let gate_digest = Digest::of_bytes(b"gate");
        let existing = vec![receipt(old_digest)];

        assert_eq!(
            next_attempt_number(
                &existing,
                &new_digest,
                "gate.unit",
                &"a".repeat(40),
                &gate_digest,
            ),
            1,
            "a stale prior card revision cannot consume the retry budget of the new revision"
        );
    }
}
