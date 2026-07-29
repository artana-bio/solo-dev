//! Named gate registry commands.
//!
//! The registry is the trusted side of D-008: gates are defined here, by
//! project policy, and cards may only name them. Registration is therefore a
//! deliberate act with its own command, not a side effect of authoring a card.

use std::{fmt::Write as _, fs, path::PathBuf};

use clap::{Args, Subcommand};

use crate::{
    cli::output::CommandOutcome,
    commands::{card::load_card, transaction::with_transaction, work::held_lease},
    control::{event_store::EventDraft, repository::ControlRepository},
    domain::{
        clock::Clock,
        gate::{GATE_DIR, GateDefinition},
        ids::{CardId, ReceiptId},
    },
    error::{ErrorCode, HarnessError},
    git::{command::GitScope, inspect},
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
    /// Report a card's gate evidence and whether it still applies.
    Status(StatusArgs),
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
        GateCommand::Run(args) => run_gate(args, clock),
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
    Ok(CommandOutcome::new(
        "gate.run",
        format!(
            "Dry run: would run gate `{}` for card {card_id} as {:?}; nothing was changed",
            gate.gate_id, gate.argv
        ),
        serde_json::json!({
            "dry_run": true,
            "card_id": card_id.to_string(),
            "gate_id": gate.gate_id,
            "argv": gate.argv,
        }),
    ))
}

/// Counts prior attempts for this exact gate definition and commit.
///
/// Numbering continues across runs so a retry is visible as a retry rather than
/// as a fresh first attempt. Section 14.2.
fn next_attempt_number(
    existing: &[Receipt],
    gate_id: &str,
    evaluated_sha: &str,
    gate_digest: &crate::domain::digest::Digest,
) -> u32 {
    u32::try_from(
        existing
            .iter()
            .filter(|receipt| {
                receipt.gate_id == gate_id
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
        |control, events, expected| {
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

            let existing = receipts_for(control, &card_id)?;
            let attempt =
                next_attempt_number(&existing, &gate.gate_id, &evaluated_sha, &gate_digest);

            let started_at = clock.now();
            let log_root = control.path(LOG_DIR).join(card_id.as_str());
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
