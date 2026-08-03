//! Named gate registry commands.
//!
//! The registry is the trusted side of D-008: gates are defined here, by
//! project policy, and cards may only name them. Registration is therefore a
//! deliberate act with its own command, not a side effect of authoring a card.

use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use serde::{Deserialize, Serialize};

use clap::{Args, Subcommand};

use crate::{
    cli::output::CommandOutcome,
    commands::CONTROL_ENV,
    commands::{
        card::{CardStateRecord, load_card},
        transaction::with_transaction,
        work::held_lease,
    },
    control::{event_store::EventDraft, repository::ControlRepository},
    domain::{
        card::CardRecord,
        clock::Clock,
        cycle::CycleRecord,
        digest::Digest,
        gate::{GATE_DIR, GateDefinition, NetworkPolicy},
        ids::{CardId, IntegrationId, ReceiptId, ValidationReservationId},
        lease::LeaseRecord,
        validation_reservation::{
            VALIDATION_EXECUTION_PERMIT_SCHEMA, VALIDATION_RESERVATION_DIR,
            VALIDATION_RESERVATION_KEY_SCHEMA, VALIDATION_RESERVATION_SCHEMA,
            VALIDATION_RESERVATION_SETTLEMENT_SCHEMA, ValidationExecutionMode,
            ValidationExecutionPermitRecord, ValidationReservationKeyV1,
            ValidationReservationOutcome, ValidationReservationRecord,
            ValidationReservationSettlementRecord,
        },
    },
    error::{ErrorCode, HarnessError},
    git::{command::GitScope, inspect},
    policy::{
        progressive_validation::{ValidationProgress, plan, progress, stages_before_satisfied},
        receipt_compatibility::{
            CompatibilityDecision, CompatibilityRequest, IntegrationCompatibilityDecision,
            IntegrationCompatibilityRequestV1, evaluate, evaluate_integration,
        },
    },
    runner::{
        AttemptOutcome, environment_fingerprint,
        receipt::{
            LOG_DIR, ProofMapBinding, ProvenanceDimension, ProvenanceSubject, RECEIPT_DIR,
            RECEIPT_PROVENANCE_SCHEMA, RECEIPT_SCHEMA, Receipt, ReceiptProvenanceV1,
            ValidationReservationBinding, evidence_is_acceptable,
        },
        run_attempt, run_attempt_with_validation_cache,
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
    /// Reserve one exact, expensive validation run without executing it.
    Reserve(ReserveArgs),
    /// Record the terminal outcome of an exact validation reservation.
    Settle(SettleArgs),
    /// Explicitly abandon one expired reservation held by this actor.
    Abandon(AbandonArgs),
    /// Execute a fixed mutation campaign under one exact reservation.
    Mutate(MutateArgs),
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
            Self::Reserve(..) => "gate.reserve",
            Self::Settle(..) => "gate.settle",
            Self::Abandon(..) => "gate.abandon",
            Self::Mutate(..) => "gate.mutate",
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
    /// Exact live reservation authorizing this expensive run.
    #[arg(long)]
    pub reservation_id: Option<String>,
    /// Report planned mutations without performing them.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `gate reserve`.
#[derive(Debug, Args)]
pub struct ReserveArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The card whose next required check is being reserved.
    #[arg(long)]
    pub card_id: String,
    /// The exact next permitted gate to reserve.
    #[arg(long)]
    pub gate_id: String,
    /// Reservation mode: `named-gate` or `declared-mutations`.
    #[arg(long, default_value = "named-gate")]
    pub execution_mode: String,
    /// Versioned mutation campaign required by `declared-mutations`.
    #[arg(long)]
    pub campaign: Option<PathBuf>,
    /// Versioned CPU profile required by `cpu-heavy`.
    #[arg(long)]
    pub cpu_profile: Option<PathBuf>,
    /// Report the authoritative reservation decision without writing it.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `gate mutate`.
#[derive(Debug, Args)]
pub struct MutateArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The live reservation authorizing the campaign.
    #[arg(long)]
    pub reservation_id: String,
    /// The exact declared campaign bound to the reservation.
    #[arg(long)]
    pub campaign: PathBuf,
}

/// Arguments accepted by `gate settle`.
#[derive(Debug, Args)]
pub struct SettleArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The reservation whose exact terminal outcome is being recorded.
    #[arg(long)]
    pub reservation_id: String,
    /// A card-run receipt that exactly matches the reservation identity.
    #[arg(long)]
    pub receipt_id: Option<String>,
    /// Terminal non-receipt outcome: `failed` or `abandoned`.
    #[arg(long)]
    pub outcome: Option<String>,
}

/// Arguments accepted by `gate abandon`.
#[derive(Debug, Args)]
pub struct AbandonArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The expired reservation being explicitly abandoned.
    #[arg(long)]
    pub reservation_id: String,
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
    pub card_id: Option<String>,
    /// The final integration to report. Mutually exclusive with `--card-id`.
    #[arg(long)]
    pub integration_id: Option<String>,
    /// A frozen, privacy-safe compatibility request to evaluate against the
    /// card's stored receipts. This is read-only and never authorizes work.
    #[arg(long)]
    pub compatibility_request: Option<PathBuf>,
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
        GateCommand::Reserve(args) => run_reserve(args, clock),
        GateCommand::Settle(args) => run_settle(args, clock),
        GateCommand::Abandon(args) => run_abandon(args, clock),
        GateCommand::Mutate(args) => run_mutate(args, clock),
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

const DECLARED_MUTATION_CAMPAIGN_SCHEMA: &str = "harness.declared-mutation-campaign/v1";
const CPU_HEAVY_PROFILE_SCHEMA: &str = "harness.cpu-heavy-validation-profile/v1";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DeclaredMutationCampaign {
    schema: String,
    mutations: Vec<DeclaredMutation>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DeclaredMutation {
    id: String,
    path: String,
    expected_utf8: String,
    replacement_utf8: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CpuHeavyProfile {
    schema: String,
    risk: String,
    expected_duration_seconds: u64,
    resource_cost: CpuResourceCost,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CpuResourceCost {
    cpu_cores: u32,
    memory_mib: u32,
}

#[derive(Clone, Debug, Serialize)]
struct MutationWitness {
    mutation_id: String,
    intended_diff_digest: Digest,
    baseline_digest: Digest,
    observed_verdict: String,
    restoration_digest: Digest,
}

fn read_declared_campaign(path: &Path) -> Result<(DeclaredMutationCampaign, Digest), HarnessError> {
    let raw = fs::read_to_string(path).map_err(|source| HarnessError::Control {
        reason: format!("cannot read mutation campaign {}: {source}", path.display()),
        code: ErrorCode::ConfigMalformed,
    })?;
    let campaign: DeclaredMutationCampaign =
        serde_json::from_str(&raw).map_err(|source| HarnessError::Control {
            reason: format!(
                "mutation campaign {} is malformed: {source}",
                path.display()
            ),
            code: ErrorCode::ConfigMalformed,
        })?;
    if campaign.schema != DECLARED_MUTATION_CAMPAIGN_SCHEMA || campaign.mutations.is_empty() {
        return Err(HarnessError::Control {
            reason: "mutation campaign must use the supported schema and contain mutations"
                .to_owned(),
            code: ErrorCode::ConfigMalformed,
        });
    }
    let mut ids = std::collections::BTreeSet::new();
    for mutation in &campaign.mutations {
        if mutation.id.is_empty()
            || mutation.path.is_empty()
            || mutation.path.starts_with('/')
            || mutation.path.split('/').any(|part| part == "..")
            || mutation.expected_utf8 == mutation.replacement_utf8
            || !ids.insert(mutation.id.clone())
        {
            return Err(HarnessError::Control {
                reason: "mutation campaign declarations must be unique, relative, and reversible"
                    .to_owned(),
                code: ErrorCode::ConfigMalformed,
            });
        }
    }
    let digest = Digest::of_canonical(&campaign)?;
    Ok((campaign, digest))
}

fn campaign_digest_for_reservation(
    mode: ValidationExecutionMode,
    campaign: Option<&Path>,
) -> Result<Option<Digest>, HarnessError> {
    match (mode, campaign) {
        (ValidationExecutionMode::NamedGate | ValidationExecutionMode::CpuHeavy, None) => Ok(None),
        (ValidationExecutionMode::NamedGate | ValidationExecutionMode::CpuHeavy, Some(_)) => {
            Err(HarnessError::Control {
                reason: "--campaign requires --execution-mode declared-mutations".to_owned(),
                code: ErrorCode::UsageInvalidArguments,
            })
        }
        (ValidationExecutionMode::DeclaredMutations, Some(path)) => {
            Ok(Some(read_declared_campaign(path)?.1))
        }
        (ValidationExecutionMode::DeclaredMutations, None) => Err(HarnessError::Control {
            reason: "declared-mutations requires --campaign".to_owned(),
            code: ErrorCode::UsageInvalidArguments,
        }),
    }
}

fn cpu_profile_digest_for_reservation(
    mode: ValidationExecutionMode,
    profile: Option<&Path>,
) -> Result<Option<Digest>, HarnessError> {
    match (mode, profile) {
        (ValidationExecutionMode::CpuHeavy, Some(path)) => {
            let raw = fs::read_to_string(path).map_err(|source| HarnessError::Control {
                reason: format!("cannot read CPU profile {}: {source}", path.display()),
                code: ErrorCode::ConfigMalformed,
            })?;
            let profile: CpuHeavyProfile =
                serde_json::from_str(&raw).map_err(|source| HarnessError::Control {
                    reason: format!("CPU profile {} is malformed: {source}", path.display()),
                    code: ErrorCode::ConfigMalformed,
                })?;
            if profile.schema != CPU_HEAVY_PROFILE_SCHEMA
                || !matches!(
                    profile.risk.as_str(),
                    "low" | "medium" | "high" | "critical"
                )
                || profile.expected_duration_seconds == 0
                || profile.resource_cost.cpu_cores == 0
                || profile.resource_cost.memory_mib == 0
            {
                return Err(HarnessError::Control {
                    reason:
                        "CPU profile has unsupported schema or invalid risk, duration, or resources"
                            .to_owned(),
                    code: ErrorCode::ConfigMalformed,
                });
            }
            Ok(Some(Digest::of_canonical(&profile)?))
        }
        (ValidationExecutionMode::CpuHeavy, None) => Err(HarnessError::Control {
            reason: "cpu-heavy requires --cpu-profile".to_owned(),
            code: ErrorCode::UsageInvalidArguments,
        }),
        (_, Some(_)) => Err(HarnessError::Control {
            reason: "--cpu-profile requires --execution-mode cpu-heavy".to_owned(),
            code: ErrorCode::UsageInvalidArguments,
        }),
        (_, None) => Ok(None),
    }
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

/// Returns only the receipts pinned by one immutable integration verification.
///
/// A final-integration reuse decision must never search all receipts by gate
/// name: a same-named receipt outside this verification proves a different
/// landing state. The verification record is the authoritative inventory.
///
/// # Errors
///
/// Returns an error when the verification is absent or malformed, a named
/// receipt cannot be read, or a named receipt belongs to another subject.
pub fn receipts_for_integration_verification(
    control: &ControlRepository,
    integration_id: &IntegrationId,
) -> Result<(crate::domain::integration::VerificationRecord, Vec<Receipt>), HarnessError> {
    let verification = crate::commands::integration::load_verification(control, integration_id)?;
    let mut receipts = Vec::with_capacity(verification.receipt_ids.len());
    for receipt_id in &verification.receipt_ids {
        let receipt_id: ReceiptId = receipt_id.parse()?;
        let raw = control.read(&Receipt::relative_path(&receipt_id))?;
        let receipt: Receipt =
            serde_json::from_str(&raw).map_err(|source| HarnessError::Control {
                reason: format!("verification receipt {receipt_id} is malformed: {source}"),
                code: ErrorCode::InternalControlCorrupt,
            })?;
        if receipt.integration_id.as_ref() != Some(integration_id) {
            return Err(HarnessError::Control {
                reason: format!(
                    "verification {integration_id} names receipt {receipt_id} for a different subject"
                ),
                code: ErrorCode::InternalControlCorrupt,
            });
        }
        receipts.push(receipt);
    }
    Ok((verification, receipts))
}

/// Reports what `gate run` would execute, without running it.
fn preview_run(
    args: &RunArgs,
    card_id: &CardId,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
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
    let reservation = requires_execution_reservation(&progress, &args.gate_id)
        .then(|| live_reservation_for_run(&control, args, card_id, clock))
        .transpose()?;
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
            "reservation_id": reservation.as_ref().map(|record| record.reservation_id.to_string()),
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

/// Builds the partial, privacy-safe provenance a normal card gate can know.
///
/// Inputs, fixtures, cache policy, trust mode, and actual toolchain identity
/// are intentionally absent until the runtime can bind them honestly.  #57
/// consequently returns rerun-required instead of treating this partial
/// record as compatible reuse evidence.
struct CardProvenanceContext<'a> {
    config: &'a crate::config::ProjectConfig,
    record: &'a CardRecord,
    card_digest: Digest,
    card_revision: u32,
    lease: &'a LeaseRecord,
    gate: &'a GateDefinition,
    gate_digest: Digest,
    evaluated_sha: &'a str,
}

fn card_receipt_provenance(
    context: CardProvenanceContext<'_>,
) -> Result<ReceiptProvenanceV1, HarnessError> {
    let proof_map = context
        .record
        .proof_map
        .as_ref()
        .map(Digest::of_canonical)
        .transpose()?
        .map_or(ProofMapBinding::NotApplicable, ProofMapBinding::Bound);
    Ok(ReceiptProvenanceV1 {
        schema: RECEIPT_PROVENANCE_SCHEMA.to_owned(),
        subject: ProvenanceSubject::Card {
            candidate_sha: context.evaluated_sha.to_owned(),
            base_sha: context.record.base_sha.clone(),
            cycle_id: context.record.cycle_id.clone(),
            card_id: context.record.card_id.clone(),
            card_revision: context.card_revision,
            card_digest: context.card_digest.clone(),
            lease_id: context.lease.lease_id.clone(),
        },
        gate_definition_digest: context.gate_digest.clone(),
        argv_digest: Digest::of_canonical(&context.gate.argv)?,
        policy_digest: Digest::of_canonical(&context.config.validation_policy)?,
        proof_map,
        dimensions: BTreeMap::from([
            (
                ProvenanceDimension::Environment,
                Digest::of_canonical(&context.gate.environment)?,
            ),
            (
                ProvenanceDimension::Configuration,
                Digest::of_canonical(context.config)?,
            ),
        ]),
        freshness_dependencies: BTreeMap::from([
            ("card".to_owned(), context.card_digest),
            ("gate".to_owned(), context.gate_digest),
            ("lease".to_owned(), Digest::of_canonical(context.lease)?),
            (
                "policy".to_owned(),
                Digest::of_canonical(&context.config.validation_policy)?,
            ),
        ]),
        lineage: Vec::new(),
        validation_reservation: None,
    })
}

fn card_run_provenance(
    context: CardProvenanceContext<'_>,
    existing: &[Receipt],
    actor_id: &str,
    recorded_at: crate::domain::clock::Timestamp,
) -> Result<ReceiptProvenanceV1, HarnessError> {
    let gate_id = context.gate.gate_id.clone();
    let mut provenance = card_receipt_provenance(context)?;
    if let Some(prior) = existing
        .iter()
        .rev()
        .find(|receipt| receipt.gate_id == gate_id)
        && let Some(lineage) = provenance.lineage_from_prior(prior, actor_id, recorded_at)?
    {
        provenance.lineage.push(lineage);
    }
    Ok(provenance)
}

fn persist_receipt(control: &ControlRepository, receipt: &Receipt) -> Result<(), HarnessError> {
    control.write_atomic(
        &Receipt::relative_path(&receipt.receipt_id),
        &format!("{}\n", serde_json::to_string_pretty(receipt)?),
    )
}

fn reservation_key(
    control: &ControlRepository,
    card_id: &CardId,
    gate_id: &str,
    execution_mode: ValidationExecutionMode,
    campaign_digest: Option<Digest>,
    cpu_profile_digest: Option<Digest>,
    require_next: bool,
) -> Result<ValidationReservationKeyV1, HarnessError> {
    let (record, state) = load_card(control, card_id)?;
    let lease = held_lease(control, card_id)?.ok_or_else(|| HarnessError::Control {
        reason: format!("card {card_id} holds no lease; run `work start` first"),
        code: ErrorCode::PreconditionNotFound,
    })?;
    let candidate_sha =
        inspect::resolve_commit(&GitScope::work_tree(&lease.worktree_path), "HEAD")?;
    let progress = validation_progress(control, card_id, Some(&candidate_sha))?;
    if require_next {
        require_next_gate(&progress, gate_id)?;
    }
    let (current_stage, check) = progress
        .plan
        .stages
        .iter()
        .find_map(|entry| {
            entry
                .checks
                .iter()
                .find(|check| check.gate_id == gate_id)
                .cloned()
                .map(|check| (entry.stage, check))
        })
        .ok_or_else(|| HarnessError::Control {
            reason: format!(
                "gate `{gate_id}` is not a required validation check for card {card_id}"
            ),
            code: ErrorCode::PolicyInvalidTransition,
        })?;
    Ok(ValidationReservationKeyV1 {
        schema: VALIDATION_RESERVATION_KEY_SCHEMA.to_owned(),
        card_id: card_id.clone(),
        cycle_id: record.cycle_id.clone(),
        card_revision: state.current_revision,
        card_digest: state.current_digest,
        lease_id: lease.lease_id,
        candidate_sha,
        base_sha: record.base_sha,
        stage: current_stage,
        check,
        policy_digest: progress.plan.policy_digest,
        proof_map_digest: progress.plan.proof_map_digest,
        execution_mode,
        campaign_digest,
        cpu_profile_digest,
    })
}

fn reservations_for(
    control: &ControlRepository,
) -> Result<Vec<ValidationReservationRecord>, HarnessError> {
    let directory = control.path(VALIDATION_RESERVATION_DIR);
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut paths: Vec<_> = fs::read_dir(&directory)
        .map_err(|source| HarnessError::ControlIo {
            path: directory,
            source,
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    paths.sort();
    paths
        .iter()
        .map(|path| {
            let record: ValidationReservationRecord =
                serde_json::from_str(&fs::read_to_string(path).map_err(|source| {
                    HarnessError::ControlIo {
                        path: path.clone(),
                        source,
                    }
                })?)
                .map_err(|source| HarnessError::Control {
                    reason: format!(
                        "validation reservation {} is malformed: {source}",
                        path.display()
                    ),
                    code: ErrorCode::InternalControlCorrupt,
                })?;
            if record.schema != VALIDATION_RESERVATION_SCHEMA
                || record.key.schema != VALIDATION_RESERVATION_KEY_SCHEMA
                || record.key.digest()? != record.key_digest
            {
                return Err(HarnessError::Control {
                    reason: format!(
                        "validation reservation {} has an invalid immutable key",
                        path.display()
                    ),
                    code: ErrorCode::InternalControlCorrupt,
                });
            }
            Ok(record)
        })
        .collect()
}

fn next_reservation_id(
    control: &ControlRepository,
) -> Result<ValidationReservationId, HarnessError> {
    let highest = reservations_for(control)?
        .iter()
        .filter_map(|record| record.reservation_id.as_str().strip_prefix("VR-"))
        .filter_map(|digits| digits.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    format!("VR-{:06}", highest + 1).parse()
}

fn newest_matching_reservation(
    control: &ControlRepository,
    key: &ValidationReservationKeyV1,
    key_digest: &Digest,
) -> Result<Option<ValidationReservationRecord>, HarnessError> {
    Ok(reservations_for(control)?
        .into_iter()
        .filter(|record| record.key_digest == *key_digest && record.key == *key)
        .max_by_key(|record| record.generation))
}

fn retry_lineage(
    control: &ControlRepository,
    record: Option<&ValidationReservationRecord>,
    clock: &dyn Clock,
) -> Result<Option<(u32, ValidationReservationId)>, HarnessError> {
    let Some(record) = record else {
        return Ok(None);
    };
    let Some(settlement) = settlement_for(control, record)? else {
        return Ok(None);
    };
    if reservation_is_expired(record, clock)
        && settlement.outcome == ValidationReservationOutcome::Abandoned
    {
        return Ok(Some((record.generation + 1, record.reservation_id.clone())));
    }
    Ok(None)
}

fn settlement_for(
    control: &ControlRepository,
    reservation: &ValidationReservationRecord,
) -> Result<Option<ValidationReservationSettlementRecord>, HarnessError> {
    let reservation_id = &reservation.reservation_id;
    let path = control.path(&ValidationReservationSettlementRecord::relative_path(
        reservation_id,
    ));
    if !path.exists() {
        return Ok(None);
    }
    let record: ValidationReservationSettlementRecord = serde_json::from_str(&control.read(
        &ValidationReservationSettlementRecord::relative_path(reservation_id),
    )?)
    .map_err(|source| HarnessError::Control {
        reason: format!(
            "validation reservation settlement {reservation_id} is malformed: {source}"
        ),
        code: ErrorCode::InternalControlCorrupt,
    })?;
    if record.schema != VALIDATION_RESERVATION_SETTLEMENT_SCHEMA
        || record.reservation_id != *reservation_id
        || record.reservation_key_digest != reservation.key_digest
        || record.holder_actor_id != reservation.holder_actor_id
    {
        return Err(HarnessError::Control {
            reason: format!(
                "validation reservation settlement {reservation_id} has an invalid identity"
            ),
            code: ErrorCode::InternalControlCorrupt,
        });
    }
    Ok(Some(record))
}

fn reservation_is_expired(reservation: &ValidationReservationRecord, clock: &dyn Clock) -> bool {
    reservation.expires_at <= clock.now()
}

fn live_reservation_for_run(
    control: &ControlRepository,
    args: &RunArgs,
    card_id: &CardId,
    clock: &dyn Clock,
) -> Result<ValidationReservationRecord, HarnessError> {
    let reservation_id: ValidationReservationId = args
        .reservation_id
        .as_deref()
        .ok_or_else(|| HarnessError::Control {
            reason: "gate run requires --reservation-id".to_owned(),
            code: ErrorCode::UsageInvalidArguments,
        })?
        .parse()?;
    let reservation = reservations_for(control)?
        .into_iter()
        .find(|record| record.reservation_id == reservation_id)
        .ok_or_else(|| HarnessError::Control {
            reason: format!("validation reservation {reservation_id} does not exist"),
            code: ErrorCode::PreconditionNotFound,
        })?;
    if reservation.holder_actor_id != args.common.actor {
        return Err(HarnessError::Control {
            reason: format!(
                "only reservation holder {} may run {}",
                reservation.holder_actor_id, reservation_id
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    }
    if settlement_for(control, &reservation)?.is_some() {
        return Err(HarnessError::Control {
            reason: format!("validation reservation {reservation_id} is already settled"),
            code: ErrorCode::PolicyInvalidTransition,
        });
    }
    if reservation_is_expired(&reservation, clock) {
        return Err(HarnessError::Control {
            reason: format!(
                "validation reservation {reservation_id} is expired and requires recovery"
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    }
    let key = reservation_key(
        control,
        card_id,
        &args.gate_id,
        ValidationExecutionMode::NamedGate,
        None,
        None,
        true,
    )?;
    let key_digest = key.digest()?;
    if reservation.key != key || reservation.key_digest != key_digest {
        return Err(HarnessError::Control {
            reason: format!(
                "validation reservation {reservation_id} does not match the current gate execution key"
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    }
    Ok(reservation)
}

fn requires_execution_reservation(progress: &ValidationProgress, gate_id: &str) -> bool {
    progress
        .plan
        .stages
        .iter()
        .any(|stage| stage.checks.iter().any(|check| check.gate_id == gate_id))
}

struct DisposableExecution {
    _root: tempfile::TempDir,
    source: PathBuf,
    cache: PathBuf,
}

impl DisposableExecution {
    fn create(
        control: &ControlRepository,
        repository: &Path,
        reservation: &ValidationReservationRecord,
        candidate_worktree: &Path,
    ) -> Result<Self, HarnessError> {
        let execution_root = control.path("validation-executions");
        fs::create_dir_all(&execution_root).map_err(|source| HarnessError::ControlIo {
            path: execution_root.clone(),
            source,
        })?;
        let root = tempfile::Builder::new()
            .prefix(&format!(
                "{}-{}-",
                reservation.reservation_id,
                &reservation.key_digest.as_str()[..12]
            ))
            .tempdir_in(&execution_root)
            .map_err(|source| HarnessError::ControlIo {
                path: execution_root,
                source,
            })?;
        let source = root.path().join("source");
        let cache = root.path().join(format!(
            "cache-{}-{}",
            reservation.reservation_id,
            &reservation.key_digest.as_str()[..12]
        ));
        let clone = Command::new("git")
            .args(["clone", "--no-checkout"])
            .arg(repository)
            .arg(&source)
            .output()
            .map_err(|source_error| HarnessError::Control {
                reason: format!("could not create disposable validation source: {source_error}"),
                code: ErrorCode::GateRunnerError,
            })?;
        if !clone.status.success() {
            return Err(HarnessError::Control {
                reason: format!(
                    "could not create disposable validation source: {}",
                    String::from_utf8_lossy(&clone.stderr).trim()
                ),
                code: ErrorCode::GateRunnerError,
            });
        }
        let checkout = Command::new("git")
            .args(["-C"])
            .arg(&source)
            .args(["checkout", "--detach", &reservation.key.candidate_sha])
            .output()
            .map_err(|source_error| HarnessError::Control {
                reason: format!("could not checkout reserved validation SHA: {source_error}"),
                code: ErrorCode::GateRunnerError,
            })?;
        if !checkout.status.success() {
            return Err(HarnessError::Control {
                reason: format!(
                    "could not checkout reserved validation SHA: {}",
                    String::from_utf8_lossy(&checkout.stderr).trim()
                ),
                code: ErrorCode::GateRunnerError,
            });
        }
        fs::create_dir(&cache).map_err(|source_error| HarnessError::ControlIo {
            path: cache.clone(),
            source: source_error,
        })?;
        let source_canonical =
            fs::canonicalize(&source).map_err(|source_error| HarnessError::ControlIo {
                path: source.clone(),
                source: source_error,
            })?;
        let candidate_canonical = fs::canonicalize(candidate_worktree).map_err(|source_error| {
            HarnessError::ControlIo {
                path: candidate_worktree.to_path_buf(),
                source: source_error,
            }
        })?;
        if source_canonical == candidate_canonical
            || fs::canonicalize(&cache)
                .map_err(|source_error| HarnessError::ControlIo {
                    path: cache.clone(),
                    source: source_error,
                })?
                .starts_with(&candidate_canonical)
        {
            return Err(HarnessError::Control {
                reason: "disposable validation environment overlaps the candidate workspace"
                    .to_owned(),
                code: ErrorCode::PolicyInvalidTransition,
            });
        }
        let actual = inspect::resolve_commit(&GitScope::work_tree(&source), "HEAD")?;
        if actual != reservation.key.candidate_sha {
            return Err(HarnessError::Control {
                reason: "disposable validation source does not match the reserved candidate SHA"
                    .to_owned(),
                code: ErrorCode::PolicyInvalidTransition,
            });
        }
        Ok(Self {
            _root: root,
            source,
            cache,
        })
    }
}

fn receipt_matches_reservation(
    receipt: &Receipt,
    reservation: &ValidationReservationRecord,
    project_id: &crate::domain::ids::ProjectId,
) -> bool {
    let Some(card_id) = receipt.card_id.as_ref() else {
        return false;
    };
    let Some(card_digest) = receipt.card_digest.as_ref() else {
        return false;
    };
    let Some(provenance) = receipt.provenance.as_ref() else {
        return false;
    };
    if provenance.validate().is_err()
        || receipt.schema != reservation.key.check.receipt_schema
        || receipt.project_id != *project_id
        || receipt.integration_id.is_some()
        || card_id != &reservation.key.card_id
        || card_digest != &reservation.key.card_digest
        || receipt.cycle_id != reservation.key.cycle_id
        || receipt.evaluated_sha != reservation.key.candidate_sha
        || receipt.gate_id != reservation.key.check.gate_id
        || receipt.gate_digest != reservation.key.check.gate_digest
        || provenance.gate_definition_digest != reservation.key.check.gate_digest
        || provenance.policy_digest != reservation.key.policy_digest
    {
        return false;
    }
    let expected_proof = reservation
        .key
        .proof_map_digest
        .as_ref()
        .map_or(ProofMapBinding::NotApplicable, |digest| {
            ProofMapBinding::Bound(digest.clone())
        });
    match &provenance.subject {
        ProvenanceSubject::Card {
            candidate_sha,
            base_sha,
            cycle_id,
            card_id: provenance_card_id,
            card_revision,
            card_digest: provenance_card_digest,
            lease_id,
        } => {
            candidate_sha == &reservation.key.candidate_sha
                && base_sha == &reservation.key.base_sha
                && cycle_id == &reservation.key.cycle_id
                && provenance_card_id == &reservation.key.card_id
                && *card_revision == reservation.key.card_revision
                && provenance_card_digest == &reservation.key.card_digest
                && lease_id == &reservation.key.lease_id
                && provenance.proof_map == expected_proof
        }
        ProvenanceSubject::Integration { .. } => false,
    }
}

fn settlement_outcome(
    control: &ControlRepository,
    reservation: &ValidationReservationRecord,
    receipt_id: Option<&str>,
    outcome: Option<&str>,
    project_id: &crate::domain::ids::ProjectId,
) -> Result<ValidationReservationOutcome, HarnessError> {
    if let Some(receipt_id) = receipt_id {
        let receipt_id: ReceiptId = receipt_id.parse()?;
        let receipt: Receipt = serde_json::from_str(
            &control.read(&Receipt::relative_path(&receipt_id))?,
        )
        .map_err(|source| HarnessError::Control {
            reason: format!("receipt {receipt_id} is malformed: {source}"),
            code: ErrorCode::InternalControlCorrupt,
        })?;
        if !receipt_matches_reservation(&receipt, reservation, project_id) {
            return Err(HarnessError::Control {
                reason: format!(
                    "receipt {receipt_id} does not exactly match validation reservation {}",
                    reservation.reservation_id
                ),
                code: ErrorCode::PolicyInvalidTransition,
            });
        }
        return Ok(ValidationReservationOutcome::ReceiptRecorded {
            receipt_id: receipt_id.to_string(),
            receipt_digest: receipt.digest()?,
        });
    }
    match outcome {
        Some("failed") => Ok(ValidationReservationOutcome::Failed),
        Some("abandoned") => Ok(ValidationReservationOutcome::Abandoned),
        _ => Err(HarnessError::Control {
            reason: "--outcome must be `failed` or `abandoned`".to_owned(),
            code: ErrorCode::UsageInvalidArguments,
        }),
    }
}

fn run_settle(args: &SettleArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let reservation_id: ValidationReservationId = args.reservation_id.parse()?;
    if u8::from(args.receipt_id.is_some()) + u8::from(args.outcome.is_some()) != 1 {
        return Err(HarnessError::Control {
            reason: "provide exactly one of --receipt-id or --outcome".to_owned(),
            code: ErrorCode::UsageInvalidArguments,
        });
    }
    with_transaction(
        &args.common.control,
        "gate.settle",
        clock,
        |control, events, expected, steps| {
            let config = control.project()?;
            let reservation = reservations_for(control)?
                .into_iter()
                .find(|record| record.reservation_id == reservation_id)
                .ok_or_else(|| HarnessError::Control {
                    reason: format!("validation reservation {reservation_id} does not exist"),
                    code: ErrorCode::PreconditionNotFound,
                })?;
            if settlement_for(control, &reservation)?.is_some() {
                return Err(HarnessError::Control {
                    reason: format!("validation reservation {reservation_id} is already settled"),
                    code: ErrorCode::PolicyInvalidTransition,
                });
            }
            if reservation.holder_actor_id != args.common.actor {
                return Err(HarnessError::Control {
                    reason: format!(
                        "only reservation holder {} may settle {reservation_id}",
                        reservation.holder_actor_id
                    ),
                    code: ErrorCode::PolicyInvalidTransition,
                });
            }
            let outcome = settlement_outcome(
                control,
                &reservation,
                args.receipt_id.as_deref(),
                args.outcome.as_deref(),
                &config.project_id,
            )?;
            let settlement = ValidationReservationSettlementRecord {
                schema: VALIDATION_RESERVATION_SETTLEMENT_SCHEMA.to_owned(),
                reservation_id: reservation_id.clone(),
                reservation_key_digest: reservation.key_digest.clone(),
                holder_actor_id: reservation.holder_actor_id.clone(),
                settled_by_actor_id: args.common.actor.clone(),
                settled_at: clock.now(),
                outcome,
            };
            steps.at("reservation-settlement-write")?;
            control.write_atomic(
                &ValidationReservationSettlementRecord::relative_path(&reservation_id),
                &format!("{}\n", serde_json::to_string_pretty(&settlement)?),
            )?;
            events.append(
                &config.project_id,
                EventDraft::new("validation.reservation_settled", &args.common.actor)
                    .cycle(reservation.key.cycle_id.clone())
                    .card(
                        reservation.key.card_id.clone(),
                        reservation.key.card_revision,
                        reservation.key.card_digest.clone(),
                    )
                    .meta(
                        "reservation_id",
                        serde_json::json!(reservation_id.to_string()),
                    )
                    .meta(
                        "reservation_key_digest",
                        serde_json::json!(reservation.key_digest.as_str()),
                    ),
                clock,
            )?;
            control.commit(
                expected,
                &format!("gate: settle validation reservation {reservation_id}"),
            )?;
            Ok(CommandOutcome::new(
                "gate.settle",
                format!("Settled validation reservation {reservation_id}"),
                serde_json::json!({ "settlement": settlement }),
            )
            .with_project(config.project_id))
        },
    )
}

fn run_abandon(args: &AbandonArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let reservation_id: ValidationReservationId = args.reservation_id.parse()?;
    with_transaction(
        &args.common.control,
        "gate.abandon",
        clock,
        |control, events, expected, steps| {
            let config = control.project()?;
            let reservation = reservations_for(control)?
                .into_iter()
                .find(|record| record.reservation_id == reservation_id)
                .ok_or_else(|| HarnessError::Control {
                    reason: format!("validation reservation {reservation_id} does not exist"),
                    code: ErrorCode::PreconditionNotFound,
                })?;
            if reservation.holder_actor_id != args.common.actor
                || settlement_for(control, &reservation)?.is_some()
                || !reservation_is_expired(&reservation, clock)
            {
                return Err(HarnessError::Control {
                    reason: format!(
                        "only the original holder may abandon an expired live reservation {reservation_id}"
                    ),
                    code: ErrorCode::PolicyInvalidTransition,
                });
            }
            let settlement = ValidationReservationSettlementRecord {
                schema: VALIDATION_RESERVATION_SETTLEMENT_SCHEMA.to_owned(),
                reservation_id: reservation_id.clone(),
                reservation_key_digest: reservation.key_digest.clone(),
                holder_actor_id: reservation.holder_actor_id.clone(),
                settled_by_actor_id: args.common.actor.clone(),
                settled_at: clock.now(),
                outcome: ValidationReservationOutcome::Abandoned,
            };
            steps.at("reservation-settlement-write")?;
            control.write_atomic(
                &ValidationReservationSettlementRecord::relative_path(&reservation_id),
                &format!("{}\n", serde_json::to_string_pretty(&settlement)?),
            )?;
            events.append(
                &config.project_id,
                EventDraft::new("validation.reservation_abandoned", &args.common.actor)
                    .cycle(reservation.key.cycle_id.clone())
                    .card(
                        reservation.key.card_id.clone(),
                        reservation.key.card_revision,
                        reservation.key.card_digest.clone(),
                    )
                    .meta(
                        "reservation_id",
                        serde_json::json!(reservation_id.to_string()),
                    ),
                clock,
            )?;
            control.commit(
                expected,
                &format!("gate: abandon validation reservation {reservation_id}"),
            )?;
            Ok(CommandOutcome::new(
                "gate.abandon",
                format!("Abandoned expired validation reservation {reservation_id}"),
                serde_json::json!({ "settlement": settlement }),
            )
            .with_project(config.project_id))
        },
    )
}

fn reservation_outcome(
    disposition: &str,
    record: &ValidationReservationRecord,
    dry_run: bool,
) -> CommandOutcome {
    CommandOutcome::new(
        "gate.reserve",
        format!(
            "{disposition}: validation reservation {}",
            record.reservation_id
        ),
        serde_json::json!({
            "schema": "harness.validation-reservation-decision/v1",
            "dry_run": dry_run,
            "disposition": { "kind": disposition },
            "reservation": record,
        }),
    )
}

fn settled_reservation_outcome(
    record: &ValidationReservationRecord,
    settlement: &ValidationReservationSettlementRecord,
    dry_run: bool,
) -> CommandOutcome {
    CommandOutcome::new(
        "gate.reserve",
        format!("settled: validation reservation {}", record.reservation_id),
        serde_json::json!({
            "schema": "harness.validation-reservation-decision/v1",
            "dry_run": dry_run,
            "disposition": { "kind": "settled" },
            "reservation": record,
            "settlement": settlement,
        }),
    )
}

fn run_reserve(args: &ReserveArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    // A reservation is intentionally tiny, and a simultaneous request should
    // observe the durable winner rather than leak the project lock's transient
    // fail-fast implementation detail to an agent. This is not a general lock
    // queue (#20): after a short bounded retry window the honest lock refusal
    // still wins.
    for attempt in 0..20 {
        match reserve_once(args, clock) {
            Err(error) if error.code() == ErrorCode::PolicyLockHeld && attempt < 19 => {
                std::thread::sleep(Duration::from_millis(10));
            }
            outcome => return outcome,
        }
    }
    unreachable!("the bounded retry loop always returns")
}

#[allow(clippy::too_many_lines)]
fn reserve_once(args: &ReserveArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;
    let execution_mode: ValidationExecutionMode = args.execution_mode.parse()?;
    let campaign_digest =
        campaign_digest_for_reservation(execution_mode, args.campaign.as_deref())?;
    let cpu_profile_digest =
        cpu_profile_digest_for_reservation(execution_mode, args.cpu_profile.as_deref())?;
    if args.dry_run {
        let control = ControlRepository::open(&args.common.control)?;
        let key = reservation_key(
            &control,
            &card_id,
            &args.gate_id,
            execution_mode,
            campaign_digest.clone(),
            cpu_profile_digest.clone(),
            false,
        )?;
        let key_digest = key.digest()?;
        let prior = newest_matching_reservation(&control, &key, &key_digest)?;
        if let Some(record) = &prior {
            if let Some(settlement) = settlement_for(&control, record)?
                && retry_lineage(&control, Some(record), clock)?.is_none()
            {
                return Ok(settled_reservation_outcome(record, &settlement, true));
            }
            if settlement_for(&control, record)?.is_none() && reservation_is_expired(record, clock)
            {
                return Ok(reservation_outcome(
                    "expired_recovery_required",
                    record,
                    true,
                ));
            }
            if settlement_for(&control, record)?.is_none() {
                return Ok(reservation_outcome("wait_for_reserved_run", record, true));
            }
        }
        let key = reservation_key(
            &control,
            &card_id,
            &args.gate_id,
            execution_mode,
            campaign_digest.clone(),
            cpu_profile_digest.clone(),
            true,
        )?;
        let key_digest = key.digest()?;
        let now = clock.now();
        let record = ValidationReservationRecord {
            schema: VALIDATION_RESERVATION_SCHEMA.to_owned(),
            reservation_id: next_reservation_id(&control)?,
            key,
            key_digest,
            holder_actor_id: args.common.actor.clone(),
            reserved_at: now,
            expires_at: crate::domain::clock::Timestamp::from_unix_seconds(
                now.unix_seconds() + 3600,
            )?,
            recovery_policy: "explicit_recovery_required".to_owned(),
            generation: retry_lineage(&control, prior.as_ref(), clock)?
                .map_or(1, |lineage| lineage.0),
            predecessor_reservation_id: retry_lineage(&control, prior.as_ref(), clock)?
                .map(|lineage| lineage.1),
        };
        return Ok(reservation_outcome("reserved", &record, true));
    }
    with_transaction(
        &args.common.control,
        "gate.reserve",
        clock,
        |control, events, expected, steps| {
            let config = control.project()?;
            let key = reservation_key(
                control,
                &card_id,
                &args.gate_id,
                execution_mode,
                campaign_digest.clone(),
                cpu_profile_digest.clone(),
                false,
            )?;
            let key_digest = key.digest()?;
            let prior = newest_matching_reservation(control, &key, &key_digest)?;
            if let Some(record) = &prior {
                if let Some(settlement) = settlement_for(control, record)?
                    && retry_lineage(control, Some(record), clock)?.is_none()
                {
                    return Ok(settled_reservation_outcome(record, &settlement, false)
                        .with_project(config.project_id));
                }
                if settlement_for(control, record)?.is_none()
                    && reservation_is_expired(record, clock)
                {
                    return Ok(
                        reservation_outcome("expired_recovery_required", record, false)
                            .with_project(config.project_id),
                    );
                }
                if settlement_for(control, record)?.is_none() {
                    return Ok(reservation_outcome("wait_for_reserved_run", record, false)
                        .with_project(config.project_id));
                }
            }
            let key = reservation_key(
                control,
                &card_id,
                &args.gate_id,
                execution_mode,
                campaign_digest.clone(),
                cpu_profile_digest.clone(),
                true,
            )?;
            let key_digest = key.digest()?;
            let now = clock.now();
            let record = ValidationReservationRecord {
                schema: VALIDATION_RESERVATION_SCHEMA.to_owned(),
                reservation_id: next_reservation_id(control)?,
                key,
                key_digest,
                holder_actor_id: args.common.actor.clone(),
                reserved_at: now,
                expires_at: crate::domain::clock::Timestamp::from_unix_seconds(
                    now.unix_seconds() + 3600,
                )?,
                recovery_policy: "explicit_recovery_required".to_owned(),
                generation: retry_lineage(control, prior.as_ref(), clock)?
                    .map_or(1, |lineage| lineage.0),
                predecessor_reservation_id: retry_lineage(control, prior.as_ref(), clock)?
                    .map(|lineage| lineage.1),
            };
            steps.at("reservation-record-write")?;
            control.write_atomic(
                &ValidationReservationRecord::relative_path(&record.reservation_id),
                &format!("{}\n", serde_json::to_string_pretty(&record)?),
            )?;
            events.append(
                &config.project_id,
                EventDraft::new("validation.reserved", &args.common.actor)
                    .cycle(record.key.cycle_id.clone())
                    .meta(
                        "reservation_id",
                        serde_json::json!(record.reservation_id.to_string()),
                    )
                    .meta("key_digest", serde_json::json!(record.key_digest.as_str())),
                clock,
            )?;
            control.commit(
                expected,
                &format!("gate: reserve {} for {card_id}", args.gate_id),
            )?;
            Ok(reservation_outcome("reserved", &record, false).with_project(config.project_id))
        },
    )
}

fn live_campaign_reservation(
    control: &ControlRepository,
    args: &MutateArgs,
    campaign_digest: &Digest,
    clock: &dyn Clock,
) -> Result<ValidationReservationRecord, HarnessError> {
    let reservation_id: ValidationReservationId = args.reservation_id.parse()?;
    let reservation = reservations_for(control)?
        .into_iter()
        .find(|record| record.reservation_id == reservation_id)
        .ok_or_else(|| HarnessError::Control {
            reason: format!("validation reservation {reservation_id} does not exist"),
            code: ErrorCode::PreconditionNotFound,
        })?;
    if reservation.holder_actor_id != args.common.actor
        || settlement_for(control, &reservation)?.is_some()
        || reservation.key.execution_mode != ValidationExecutionMode::DeclaredMutations
        || reservation.key.campaign_digest.as_ref() != Some(campaign_digest)
    {
        return Err(HarnessError::Control {
            reason: format!("reservation {reservation_id} is not a live exact campaign permit"),
            code: ErrorCode::PolicyInvalidTransition,
        });
    }
    if reservation_is_expired(&reservation, clock) {
        return Err(HarnessError::Control {
            reason: format!("reservation {reservation_id} is expired and requires recovery"),
            code: ErrorCode::PolicyInvalidTransition,
        });
    }
    let current = reservation_key(
        control,
        &reservation.key.card_id,
        &reservation.key.check.gate_id,
        ValidationExecutionMode::DeclaredMutations,
        Some(campaign_digest.clone()),
        None,
        true,
    )?;
    if current != reservation.key || current.digest()? != reservation.key_digest {
        return Err(HarnessError::Control {
            reason: format!(
                "reservation {reservation_id} does not match the current declared mutation campaign key"
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    }
    Ok(reservation)
}

#[allow(clippy::too_many_lines)]
fn run_mutate(args: &MutateArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let (campaign, campaign_digest) = read_declared_campaign(&args.campaign)?;
    with_transaction(
        &args.common.control,
        "gate.mutate",
        clock,
        |control, events, expected, steps| {
            let config = control.project()?;
            let reservation = live_campaign_reservation(control, args, &campaign_digest, clock)?;
            let lease = held_lease(control, &reservation.key.card_id)?.ok_or_else(|| {
                HarnessError::Control {
                    reason: format!("card {} holds no lease", reservation.key.card_id),
                    code: ErrorCode::PreconditionNotFound,
                }
            })?;
            let gate = load_gate(control, &reservation.key.check.gate_id)?;
            steps.at("mutation-execution-setup")?;
            let execution = DisposableExecution::create(
                control,
                &config.repository,
                &reservation,
                &lease.worktree_path,
            )?;
            let log_root = control.path(LOG_DIR).join(reservation.key.card_id.as_str());
            let mut witnesses = Vec::new();
            for (index, mutation) in campaign.mutations.iter().enumerate() {
                let target = execution.source.join(&mutation.path);
                let baseline = fs::read(&target).map_err(|source| HarnessError::ControlIo {
                    path: target.clone(),
                    source,
                })?;
                let baseline_text =
                    std::str::from_utf8(&baseline).map_err(|_| HarnessError::Control {
                        reason: format!("mutation target {} is not UTF-8", mutation.path),
                        code: ErrorCode::PolicyInvalidTransition,
                    })?;
                if baseline_text != mutation.expected_utf8 {
                    return Err(HarnessError::Control {
                        reason: format!(
                            "mutation {} does not match the restored baseline",
                            mutation.id
                        ),
                        code: ErrorCode::PolicyInvalidTransition,
                    });
                }
                let baseline_digest = Digest::of_bytes(&baseline);
                let intended_diff_digest = Digest::of_canonical(mutation)?;
                fs::write(&target, &mutation.replacement_utf8).map_err(|source| {
                    HarnessError::ControlIo {
                        path: target.clone(),
                        source,
                    }
                })?;
                let outcome = run_attempt_with_validation_cache(
                    &gate,
                    &execution.source,
                    &log_root,
                    u32::try_from(index + 1).map_err(|_| HarnessError::Control {
                        reason: "mutation campaign has too many entries".to_owned(),
                        code: ErrorCode::ConfigMalformed,
                    })?,
                    clock,
                    Some(&execution.cache),
                )?;
                fs::write(&target, &baseline).map_err(|source| HarnessError::ControlIo {
                    path: target.clone(),
                    source,
                })?;
                steps.at("mutation-restoration-verify")?;
                let restoration_digest =
                    Digest::of_bytes(&fs::read(&target).map_err(|source| {
                        HarnessError::ControlIo {
                            path: target.clone(),
                            source,
                        }
                    })?);
                if restoration_digest != baseline_digest {
                    return Err(HarnessError::Control {
                        reason: format!("mutation {} failed to restore its baseline", mutation.id),
                        code: ErrorCode::PolicyInvalidTransition,
                    });
                }
                if outcome.passed() {
                    return Err(HarnessError::Control {
                        reason: format!("mutation {} survived its required gate", mutation.id),
                        code: ErrorCode::PolicyInvalidTransition,
                    });
                }
                witnesses.push(MutationWitness {
                    mutation_id: mutation.id.clone(),
                    intended_diff_digest,
                    baseline_digest,
                    observed_verdict: "failed".to_owned(),
                    restoration_digest,
                });
            }
            let final_baseline_digest = witnesses
                .last()
                .map(|w| w.restoration_digest.clone())
                .ok_or_else(|| HarnessError::Control {
                    reason: "mutation campaign is empty".to_owned(),
                    code: ErrorCode::ConfigMalformed,
                })?;
            let witness_path = format!(
                "validation-mutation-witnesses/{}.json",
                reservation.reservation_id
            );
            control.write_atomic(
                &witness_path,
                &format!(
                    "{}\n",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "schema": "harness.validation-mutation-witnesses/v1",
                        "reservation_id": reservation.reservation_id,
                        "campaign_digest": campaign_digest,
                        "mutation_witnesses": witnesses,
                        "final_baseline_digest": final_baseline_digest,
                    }))?
                ),
            )?;
            events.append(
                &config.project_id,
                EventDraft::new("validation.mutation_campaign_completed", &args.common.actor)
                    .card(
                        reservation.key.card_id.clone(),
                        reservation.key.card_revision,
                        reservation.key.card_digest.clone(),
                    )
                    .meta(
                        "reservation_id",
                        serde_json::json!(reservation.reservation_id.to_string()),
                    ),
                clock,
            )?;
            control.commit(
                expected,
                &format!("gate: mutate campaign for {}", reservation.key.card_id),
            )?;
            Ok(CommandOutcome::new(
                "gate.mutate",
                "completed declared mutation campaign",
                serde_json::json!({
                    "reservation_id": reservation.reservation_id,
                    "campaign_digest": campaign_digest,
                    "mutation_witnesses": witnesses,
                    "final_baseline_digest": final_baseline_digest,
                }),
            )
            .with_project(config.project_id))
        },
    )
}

#[derive(Clone)]
struct GovernedGateExecution {
    config: crate::config::ProjectConfig,
    record: CardRecord,
    state: CardStateRecord,
    gate: GateDefinition,
    gate_digest: Digest,
    lease: LeaseRecord,
    evaluated_sha: String,
    worktree_clean: bool,
    existing: Vec<Receipt>,
    attempt: u32,
    reservation: ValidationReservationRecord,
    permit: ValidationExecutionPermitRecord,
}

fn execution_permit_for(
    control: &ControlRepository,
    reservation: &ValidationReservationRecord,
) -> Result<Option<ValidationExecutionPermitRecord>, HarnessError> {
    let relative = ValidationExecutionPermitRecord::relative_path(&reservation.reservation_id);
    let path = control.path(&relative);
    if !path.exists() {
        return Ok(None);
    }
    let permit: ValidationExecutionPermitRecord = serde_json::from_str(&control.read(&relative)?)
        .map_err(|source| HarnessError::Control {
        reason: format!(
            "validation execution permit {} is malformed: {source}",
            reservation.reservation_id
        ),
        code: ErrorCode::InternalControlCorrupt,
    })?;
    if permit.schema != VALIDATION_EXECUTION_PERMIT_SCHEMA
        || permit.reservation_id != reservation.reservation_id
        || permit.reservation_key_digest != reservation.key_digest
        || permit.holder_actor_id != reservation.holder_actor_id
    {
        return Err(HarnessError::Control {
            reason: format!(
                "validation execution permit {} has an invalid identity",
                reservation.reservation_id
            ),
            code: ErrorCode::InternalControlCorrupt,
        });
    }
    Ok(Some(permit))
}

fn acquire_governed_gate_execution(
    args: &RunArgs,
    card_id: &CardId,
    clock: &dyn Clock,
) -> Result<GovernedGateExecution, HarnessError> {
    let mut acquired = None;
    with_transaction(
        &args.common.control,
        "gate.run.acquire",
        clock,
        |control, events, expected, steps| {
            let config = control.project()?;
            let (record, state) = load_card(control, card_id)?;
            let gate = load_gate(control, &args.gate_id)?;
            let gate_digest = gate.digest()?;
            let lease = held_lease(control, card_id)?.ok_or_else(|| HarnessError::Control {
                reason: format!("card {card_id} holds no lease; run `work start` first"),
                code: ErrorCode::PreconditionNotFound,
            })?;
            let scope = GitScope::work_tree(&lease.worktree_path);
            let evaluated_sha = inspect::resolve_commit(&scope, "HEAD")?;
            let progress = validation_progress(control, card_id, Some(&evaluated_sha))?;
            require_next_gate(&progress, &args.gate_id)?;
            let reservation = live_reservation_for_run(control, args, card_id, clock)?;
            if execution_permit_for(control, &reservation)?.is_some() {
                return Err(HarnessError::Control {
                    reason: format!(
                        "validation reservation {} is already acquired",
                        reservation.reservation_id
                    ),
                    code: ErrorCode::PolicyInvalidTransition,
                });
            }
            let existing = receipts_for(control, card_id)?;
            let attempt = next_attempt_number(
                &existing,
                &state.current_digest,
                &gate.gate_id,
                &evaluated_sha,
                &gate_digest,
            );
            let permit = ValidationExecutionPermitRecord {
                schema: VALIDATION_EXECUTION_PERMIT_SCHEMA.to_owned(),
                reservation_id: reservation.reservation_id.clone(),
                reservation_key_digest: reservation.key_digest.clone(),
                holder_actor_id: args.common.actor.clone(),
                acquired_at: clock.now(),
            };
            steps.at("execution-permit-write")?;
            control.write_atomic(
                &ValidationExecutionPermitRecord::relative_path(&reservation.reservation_id),
                &format!("{}\n", serde_json::to_string_pretty(&permit)?),
            )?;
            events.append(
                &config.project_id,
                EventDraft::new("validation.execution_acquired", &args.common.actor)
                    .cycle(record.cycle_id.clone())
                    .card(
                        card_id.clone(),
                        state.current_revision,
                        state.current_digest.clone(),
                    )
                    .meta(
                        "reservation_id",
                        serde_json::json!(reservation.reservation_id.to_string()),
                    )
                    .meta(
                        "reservation_key_digest",
                        serde_json::json!(reservation.key_digest.as_str()),
                    ),
                clock,
            )?;
            control.commit(
                expected,
                &format!("gate: acquire execution for {}", reservation.reservation_id),
            )?;
            acquired = Some(GovernedGateExecution {
                config,
                record,
                state,
                gate,
                gate_digest,
                lease,
                evaluated_sha,
                worktree_clean: inspect::worktree_state(&scope)?.clean,
                existing,
                attempt,
                reservation,
                permit,
            });
            Ok(CommandOutcome::new(
                "gate.run.acquire",
                "acquired governed validation execution",
                serde_json::json!({}),
            ))
        },
    )?;
    acquired.ok_or_else(|| HarnessError::Control {
        reason: "governed execution acquire completed without a permit".to_owned(),
        code: ErrorCode::InternalControlCorrupt,
    })
}

#[allow(clippy::too_many_lines)]
fn settle_governed_gate_execution(
    args: &RunArgs,
    execution: &GovernedGateExecution,
    outcome: &AttemptOutcome,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    with_transaction(
        &args.common.control,
        "gate.run.settle",
        clock,
        |control, events, expected, steps| {
            let permit =
                execution_permit_for(control, &execution.reservation)?.ok_or_else(|| {
                    HarnessError::Control {
                        reason: format!(
                            "validation reservation {} has no live execution permit",
                            execution.reservation.reservation_id
                        ),
                        code: ErrorCode::PolicyInvalidTransition,
                    }
                })?;
            if permit.reservation_id != execution.permit.reservation_id
                || permit.reservation_key_digest != execution.permit.reservation_key_digest
                || permit.holder_actor_id != execution.permit.holder_actor_id
                || permit.holder_actor_id != args.common.actor
            {
                return Err(HarnessError::Control {
                    reason: format!(
                        "validation reservation {} execution permit changed",
                        execution.reservation.reservation_id
                    ),
                    code: ErrorCode::PolicyInvalidTransition,
                });
            }
            if settlement_for(control, &execution.reservation)?.is_some() {
                return Err(HarnessError::Control {
                    reason: format!(
                        "validation reservation {} is already settled",
                        execution.reservation.reservation_id
                    ),
                    code: ErrorCode::PolicyInvalidTransition,
                });
            }
            // The global lock was intentionally released while the subprocess
            // ran. Recompute the authoritative key at the terminal boundary:
            // a candidate, card, lease, gate, stage, or policy change must
            // leave the permit recoverable rather than attaching a receipt to
            // state that no longer matches the execution we performed.
            let current_key = reservation_key(
                control,
                &execution.record.card_id,
                &execution.gate.gate_id,
                ValidationExecutionMode::NamedGate,
                None,
                None,
                true,
            )?;
            if current_key != execution.reservation.key
                || current_key.digest()? != execution.reservation.key_digest
            {
                return Err(HarnessError::Control {
                    reason: format!(
                        "validation reservation {} no longer matches current state at settlement",
                        execution.reservation.reservation_id
                    ),
                    code: ErrorCode::PolicyInvalidTransition,
                });
            }
            let receipt_id = next_receipt_id(control)?;
            let finished_at = clock.now();
            let mut provenance = card_run_provenance(
                CardProvenanceContext {
                    config: &execution.config,
                    record: &execution.record,
                    card_digest: execution.state.current_digest.clone(),
                    card_revision: execution.state.current_revision,
                    lease: &execution.lease,
                    gate: &execution.gate,
                    gate_digest: execution.gate_digest.clone(),
                    evaluated_sha: &execution.evaluated_sha,
                },
                &execution.existing,
                &args.common.actor,
                finished_at,
            )?;
            provenance.validation_reservation = Some(ValidationReservationBinding {
                reservation_id: execution.reservation.reservation_id.clone(),
                key_digest: execution.reservation.key_digest.clone(),
            });
            let receipt = Receipt {
                schema: RECEIPT_SCHEMA.to_owned(),
                receipt_id: receipt_id.clone(),
                project_id: execution.config.project_id.clone(),
                cycle_id: execution.record.cycle_id.clone(),
                card_id: Some(execution.record.card_id.clone()),
                card_digest: Some(execution.state.current_digest.clone()),
                integration_id: None,
                evaluated_sha: execution.evaluated_sha.clone(),
                gate_id: execution.gate.gate_id.clone(),
                gate_digest: execution.gate_digest.clone(),
                harness_version: env!("CARGO_PKG_VERSION").to_owned(),
                environment_fingerprint: environment_fingerprint(&execution.gate),
                started_at: execution.permit.acquired_at,
                finished_at,
                duration_ms: outcome.duration_ms,
                exit_code: outcome.exit_code,
                termination: outcome.termination,
                stdout_digest: outcome.stdout_digest.clone(),
                stderr_digest: outcome.stderr_digest.clone(),
                artifact_digests: outcome.artifact_digests.clone(),
                log_location: outcome.log_location.clone(),
                attempt: execution.attempt,
                passed: outcome.passed(),
                worktree_clean: Some(execution.worktree_clean),
                provenance: Some(provenance),
            };
            persist_receipt(control, &receipt)?;
            let settlement = ValidationReservationSettlementRecord {
                schema: VALIDATION_RESERVATION_SETTLEMENT_SCHEMA.to_owned(),
                reservation_id: execution.reservation.reservation_id.clone(),
                reservation_key_digest: execution.reservation.key_digest.clone(),
                holder_actor_id: execution.reservation.holder_actor_id.clone(),
                settled_by_actor_id: args.common.actor.clone(),
                settled_at: finished_at,
                outcome: ValidationReservationOutcome::ReceiptRecorded {
                    receipt_id: receipt_id.to_string(),
                    receipt_digest: receipt.digest()?,
                },
            };
            steps.at("governed-execution-settlement-write")?;
            control.write_atomic(
                &ValidationReservationSettlementRecord::relative_path(
                    &execution.reservation.reservation_id,
                ),
                &format!("{}\n", serde_json::to_string_pretty(&settlement)?),
            )?;
            fs::remove_file(
                control.path(&ValidationExecutionPermitRecord::relative_path(
                    &execution.reservation.reservation_id,
                )),
            )
            .map_err(|source| HarnessError::ControlIo {
                path: control.path(&ValidationExecutionPermitRecord::relative_path(
                    &execution.reservation.reservation_id,
                )),
                source,
            })?;
            events.append(
                &execution.config.project_id,
                EventDraft::new("validation.execution_settled", &args.common.actor)
                    .cycle(execution.record.cycle_id.clone())
                    .card(
                        execution.record.card_id.clone(),
                        execution.state.current_revision,
                        execution.state.current_digest.clone(),
                    )
                    .meta(
                        "reservation_id",
                        serde_json::json!(execution.reservation.reservation_id.to_string()),
                    )
                    .meta("receipt_id", serde_json::json!(receipt_id.to_string())),
                clock,
            )?;
            let mut event = EventDraft::new("gate.ran", &args.common.actor)
                .cycle(execution.record.cycle_id.clone())
                .card(
                    execution.record.card_id.clone(),
                    execution.state.current_revision,
                    execution.state.current_digest.clone(),
                )
                .head(execution.evaluated_sha.clone())
                .meta("gate_id", serde_json::json!(execution.gate.gate_id))
                .meta(
                    "gate_digest",
                    serde_json::json!(execution.gate_digest.as_str()),
                )
                .meta("receipt_id", serde_json::json!(receipt_id.to_string()))
                .meta("attempt", serde_json::json!(execution.attempt))
                .meta("termination", serde_json::json!(outcome.termination.name()))
                .meta("passed", serde_json::json!(receipt.passed));
            event = event
                .meta(
                    "reservation_id",
                    serde_json::json!(execution.reservation.reservation_id.to_string()),
                )
                .meta(
                    "reservation_key_digest",
                    serde_json::json!(execution.reservation.key_digest.as_str()),
                );
            events.append(&execution.config.project_id, event, clock)?;
            control.commit(
                expected,
                &format!(
                    "gate: settle governed execution {}",
                    execution.reservation.reservation_id
                ),
            )?;
            report_run(&receipt, &execution.config.project_id)
        },
    )
}

fn settle_governed_gate_execution_with_retry(
    args: &RunArgs,
    execution: &GovernedGateExecution,
    outcome: &AttemptOutcome,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    for attempt in 0..20 {
        match settle_governed_gate_execution(args, execution, outcome, clock) {
            Err(error) if error.code() == ErrorCode::PolicyLockHeld && attempt < 19 => {
                std::thread::sleep(Duration::from_millis(10));
            }
            result => return result,
        }
    }
    unreachable!("the bounded settlement retry always returns")
}

fn run_governed_gate(
    args: &RunArgs,
    card_id: &CardId,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    // This retry is deliberately local to permit acquisition. It lets two
    // independently reserved runs pass the short control mutation boundary,
    // while preserving the project's normal fail-fast lock semantics for
    // every other command and for terminal settlement.
    let mut acquired = None;
    for attempt in 0..20 {
        match acquire_governed_gate_execution(args, card_id, clock) {
            Err(error) if error.code() == ErrorCode::PolicyLockHeld && attempt < 19 => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(execution) => {
                acquired = Some(execution);
                break;
            }
            Err(error) => return Err(error),
        }
    }
    let execution = acquired.ok_or_else(|| HarnessError::Control {
        reason: "governed execution permit acquisition exhausted its bounded retry".to_owned(),
        code: ErrorCode::PolicyLockHeld,
    })?;
    // A crash after this point must not make the reservation reusable: the
    // committed permit is the recovery-visible state. This injectable boundary
    // exercises precisely that otherwise hard-to-reproduce phase split.
    if std::env::var(crate::control::journal::INJECT_FAILURE_VAR)
        .ok()
        .as_deref()
        == Some("governed-execution-after-acquire")
    {
        return Err(HarnessError::Control {
            reason: "deliberate interruption after governed execution acquire".to_owned(),
            code: ErrorCode::RecoveryIncomplete,
        });
    }
    let log_root = ControlRepository::open(&args.common.control)?
        .path(LOG_DIR)
        .join(card_id.as_str());
    let disposable = DisposableExecution::create(
        &ControlRepository::open(&args.common.control)?,
        &execution.config.repository,
        &execution.reservation,
        &execution.lease.worktree_path,
    )?;
    let outcome = run_attempt_with_validation_cache(
        &execution.gate,
        &disposable.source,
        &log_root,
        execution.attempt,
        clock,
        Some(&disposable.cache),
    )?;
    settle_governed_gate_execution_with_retry(args, &execution, &outcome, clock)
}

fn run_gate(args: &RunArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;
    if args.dry_run {
        return preview_run(args, &card_id, clock);
    }
    let control = ControlRepository::open(&args.common.control)?;
    let lease = held_lease(&control, &card_id)?.ok_or_else(|| HarnessError::Control {
        reason: format!("card {card_id} holds no lease; run `work start` first"),
        code: ErrorCode::PreconditionNotFound,
    })?;
    let candidate = inspect::resolve_commit(&GitScope::work_tree(&lease.worktree_path), "HEAD")?;
    let progress = validation_progress(&control, &card_id, Some(&candidate))?;
    require_next_gate(&progress, &args.gate_id)?;
    if requires_execution_reservation(&progress, &args.gate_id) {
        return run_governed_gate(args, &card_id, clock);
    }
    run_gate_locked(args, clock)
}

// This is the intentionally linear transaction boundary: its order is the
// safety contract (load → validate → run → record → event → commit).
#[allow(clippy::too_many_lines)]
fn run_gate_locked(args: &RunArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;
    if args.dry_run {
        return preview_run(args, &card_id, clock);
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
            let reservation = requires_execution_reservation(&progress, &args.gate_id)
                .then(|| live_reservation_for_run(control, args, &card_id, clock))
                .transpose()?;
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
            let execution = if let Some(reservation) = &reservation {
                steps.at("validation-execution-setup")?;
                Some(DisposableExecution::create(
                    control,
                    &config.repository,
                    reservation,
                    &lease.worktree_path,
                )?)
            } else {
                None
            };
            steps.at("gate-attempt-started")?;
            let outcome = if let Some(execution) = &execution {
                run_attempt_with_validation_cache(
                    &gate,
                    &execution.source,
                    &log_root,
                    attempt,
                    clock,
                    Some(&execution.cache),
                )?
            } else {
                run_attempt(&gate, &lease.worktree_path, &log_root, attempt, clock)?
            };
            let finished_at = clock.now();

            let receipt_id = next_receipt_id(control)?;
            let mut provenance = card_run_provenance(
                CardProvenanceContext {
                    config: &config,
                    record: &record,
                    card_digest: state.current_digest.clone(),
                    card_revision: state.current_revision,
                    lease: &lease,
                    gate: &gate,
                    gate_digest: gate_digest.clone(),
                    evaluated_sha: &evaluated_sha,
                },
                &existing,
                &args.common.actor,
                finished_at,
            )?;
            provenance.validation_reservation =
                reservation
                    .as_ref()
                    .map(|reservation| ValidationReservationBinding {
                        reservation_id: reservation.reservation_id.clone(),
                        key_digest: reservation.key_digest.clone(),
                    });
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
                provenance: Some(provenance),
            };

            persist_receipt(control, &receipt)?;

            if let Some(reservation) = &reservation {
                let settlement = ValidationReservationSettlementRecord {
                    schema: VALIDATION_RESERVATION_SETTLEMENT_SCHEMA.to_owned(),
                    reservation_id: reservation.reservation_id.clone(),
                    reservation_key_digest: reservation.key_digest.clone(),
                    holder_actor_id: reservation.holder_actor_id.clone(),
                    settled_by_actor_id: args.common.actor.clone(),
                    settled_at: finished_at,
                    outcome: ValidationReservationOutcome::ReceiptRecorded {
                        receipt_id: receipt_id.to_string(),
                        receipt_digest: receipt.digest()?,
                    },
                };
                steps.at("reservation-settlement-write")?;
                control.write_atomic(
                    &ValidationReservationSettlementRecord::relative_path(
                        &reservation.reservation_id,
                    ),
                    &format!("{}\n", serde_json::to_string_pretty(&settlement)?),
                )?;
                events.append(
                    &config.project_id,
                    EventDraft::new("validation.reservation_settled", &args.common.actor)
                        .cycle(record.cycle_id.clone())
                        .card(
                            card_id.clone(),
                            state.current_revision,
                            state.current_digest.clone(),
                        )
                        .meta(
                            "reservation_id",
                            serde_json::json!(reservation.reservation_id.to_string()),
                        )
                        .meta(
                            "reservation_key_digest",
                            serde_json::json!(reservation.key_digest.as_str()),
                        ),
                    clock,
                )?;
            }

            // Both outcomes are recorded, per invariant 7.4.1. A failing gate
            // that left no trace would let a later run present itself as the
            // first.
            let mut event = EventDraft::new("gate.ran", &args.common.actor)
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
                .meta("passed", serde_json::json!(receipt.passed));
            if let Some(reservation) = &reservation {
                event = event
                    .meta(
                        "reservation_id",
                        serde_json::json!(reservation.reservation_id.to_string()),
                    )
                    .meta(
                        "reservation_key_digest",
                        serde_json::json!(reservation.key_digest.as_str()),
                    );
            }
            events.append(&config.project_id, event, clock)?;
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
    match (&args.card_id, &args.integration_id) {
        (None, Some(integration_id)) => {
            return run_integration_status(args, &integration_id.parse()?);
        }
        (Some(_), None) => {}
        _ => {
            return Err(HarnessError::Control {
                reason: "gate status requires exactly one of --card-id or --integration-id"
                    .to_owned(),
                code: ErrorCode::UsageConflictingOptions,
            });
        }
    }
    let card_id: CardId = args.card_id.as_deref().expect("checked above").parse()?;
    let control = ControlRepository::open(&args.common.control)?;
    let config = control.project()?;
    let (record, _state) = load_card(&control, &card_id)?;
    let receipts = receipts_for(&control, &card_id)?;
    let compatibility = args
        .compatibility_request
        .as_ref()
        .map(|path| read_compatibility_request(path, &card_id, &receipts))
        .transpose()?;

    let candidate = held_lease(&control, &card_id)?.and_then(|lease| {
        inspect::resolve_commit(&GitScope::work_tree(&lease.worktree_path), "HEAD").ok()
    });

    let mut text = format!(
        "Card {card_id}\ncandidate: {}\nreceipts: {}",
        candidate.as_deref().unwrap_or("no allocation"),
        receipts.len()
    );
    if let Some(decision) = &compatibility {
        let _ = write!(
            text,
            "\nreceipt compatibility: {}",
            match decision.disposition {
                crate::policy::receipt_compatibility::CompatibilityDisposition::CompatibleReuse { .. } => "compatible_reuse",
                crate::policy::receipt_compatibility::CompatibilityDisposition::RerunRequired { .. } => "rerun_required",
            }
        );
    }

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
            "receipt_compatibility": compatibility,
        }),
    )
    .with_project(config.project_id.clone()))
}

fn run_integration_status(
    args: &StatusArgs,
    integration_id: &IntegrationId,
) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.common.control)?;
    let config = control.project()?;
    let record = crate::commands::integration::load_integration(&control, integration_id)?;
    let (verification, receipts) = receipts_for_integration_verification(&control, integration_id)?;
    let compatibility = args
        .compatibility_request
        .as_ref()
        .map(|path| {
            read_integration_compatibility_request(
                &control,
                path,
                &record,
                &verification,
                &receipts,
            )
        })
        .transpose()?;
    Ok(CommandOutcome::new(
        "gate.status",
        format!(
            "Integration {integration_id}\nverification receipts: {}",
            receipts.len()
        ),
        serde_json::json!({
            "integration_id": integration_id.to_string(),
            "verification_digest": verification.digest()?,
            "receipts": receipts,
            "receipt_compatibility": compatibility,
        }),
    )
    .with_project(config.project_id))
}

pub(crate) fn read_integration_compatibility_request(
    control: &ControlRepository,
    path: &PathBuf,
    record: &crate::domain::integration::IntegrationRecord,
    verification: &crate::domain::integration::VerificationRecord,
    receipts: &[Receipt],
) -> Result<IntegrationCompatibilityDecision, HarnessError> {
    let request: IntegrationCompatibilityRequestV1 = load_json_request(path)?;
    let config = control.project()?;
    let gate = load_gate(control, &request.check.gate_id)?;
    if request.context.integration_id != record.integration_id
        || request.context.cycle_id != record.cycle_id
        || record.landing_sha.as_deref() != Some(request.context.landing_sha.as_str())
        || request.context.baseline_sha != record.baseline_sha
        || request.context.integration_digest != record.substantive_digest()?
        || request.context.verification_digest != verification.digest()?
        || request.context.policy_digest != Digest::of_canonical(&config.validation_policy)?
        || request.check.receipt_schema != RECEIPT_SCHEMA
        || request.check.gate_digest != gate.digest()?
        || request.check.max_attempts != gate.retry_policy.max_attempts
        || request.expected.gate_definition_digest != gate.digest()?
        || request.expected.argv_digest != Digest::of_canonical(&gate.argv)?
        || verification.integration_id != record.integration_id
        || verification.cycle_id != record.cycle_id
        || record.landing_sha.as_deref() != Some(verification.landing_sha.as_str())
    {
        return Err(HarnessError::Control { reason: "integration compatibility request is stale for the current integration or verification".to_owned(), code: ErrorCode::GateEvidenceStale });
    }
    Ok(evaluate_integration(&request, receipts))
}

/// Reads one frozen consumer context used only to explain receipt reuse.
///
/// The request is deliberately supplied rather than inferred from a prior
/// receipt: inferring environment, fixture, cache, or trust context from the
/// evidence being judged would make stale evidence look current.
pub(crate) fn read_compatibility_request(
    path: &PathBuf,
    card_id: &CardId,
    receipts: &[Receipt],
) -> Result<CompatibilityDecision, HarnessError> {
    let request = load_compatibility_request(path)?;
    let crate::runner::receipt::ProvenanceSubject::Card {
        card_id: expected_card,
        ..
    } = &request.expected.subject
    else {
        return Err(HarnessError::Control {
            reason: "receipt compatibility request for gate status must name a card subject"
                .to_owned(),
            code: ErrorCode::ConfigInvalidValue,
        });
    };
    if expected_card != card_id {
        return Err(HarnessError::Control {
            reason: format!(
                "receipt compatibility request names card {expected_card}, not {card_id}"
            ),
            code: ErrorCode::ConfigInvalidValue,
        });
    }
    Ok(evaluate(&request, receipts))
}

/// Loads the frozen read-only context used by status and audit projections.
pub(crate) fn load_compatibility_request(
    path: &PathBuf,
) -> Result<CompatibilityRequest, HarnessError> {
    load_json_request(path)
}

fn load_json_request<T: serde::de::DeserializeOwned>(path: &PathBuf) -> Result<T, HarnessError> {
    let raw = fs::read_to_string(path).map_err(|source| HarnessError::Control {
        reason: format!(
            "cannot read receipt compatibility request {}: {source}",
            path.display()
        ),
        code: ErrorCode::ConfigMalformed,
    })?;
    serde_json::from_str(&raw).map_err(|source| HarnessError::Control {
        reason: format!(
            "receipt compatibility request {} is malformed: {source}",
            path.display()
        ),
        code: ErrorCode::ConfigMalformed,
    })
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
            provenance: None,
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
