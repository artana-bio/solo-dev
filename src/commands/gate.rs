//! Named gate registry commands.
//!
//! The registry is the trusted side of D-008: gates are defined here, by
//! project policy, and cards may only name them. Registration is therefore a
//! deliberate act with its own command, not a side effect of authoring a card.

use std::{fmt::Write as _, fs, path::PathBuf};

use clap::{Args, Subcommand};

use crate::{
    cli::output::CommandOutcome,
    commands::transaction::with_transaction,
    control::{event_store::EventDraft, repository::ControlRepository},
    domain::{
        clock::Clock,
        gate::{GATE_DIR, GateDefinition},
    },
    error::{ErrorCode, HarnessError},
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
}

/// Arguments shared by registry subcommands.
#[derive(Debug, Args)]
pub struct CommonArgs {
    /// Path to the control repository.
    #[arg(long)]
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
            "Gate `{}` revision {} is valid\nargv: {:?}\ntimeout: {}s\nnetwork: {:?}\nmax attempts: {}",
            gate.gate_id,
            gate.revision,
            gate.argv,
            gate.timeout_seconds,
            gate.network_policy,
            gate.retry_policy.max_attempts
        ),
        serde_json::json!({
            "gate_id": gate.gate_id,
            "revision": gate.revision,
            "digest": gate.digest()?.as_str(),
            "valid": true,
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
        |control, events, expected| {
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
            "Gate `{}` revision {}\ndigest: {digest}\nargv: {:?}\nworking directory: {}\ntimeout: {}s\nnetwork: {:?}\nmax attempts: {}",
            gate.gate_id,
            gate.revision,
            gate.argv,
            if gate.working_directory.is_empty() {
                "."
            } else {
                &gate.working_directory
            },
            gate.timeout_seconds,
            gate.network_policy,
            gate.retry_policy.max_attempts
        ),
        serde_json::json!({
            "definition": gate,
            "digest": digest.as_str(),
        }),
    )
    .with_project(config.project_id.clone()))
}
