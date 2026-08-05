//! Disposition commands: authorized decisions that change what an escalated
//! card may do next.
//!
//! #74 defines six dispositions. `renew` is the first: an authorized actor
//! grants a card's exhausted convergence budget one more configured limit,
//! in exactly the dimension that is exhausted, so the card can be delivered
//! and reviewed again. `Escalated` cards otherwise have no way out —
//! [`crate::policy::convergence::NextPermittedAction::RecordAuthorizedDisposition`]
//! named this command before it existed; this is what satisfies it.
//!
//! `rebaseline` is the second, and the largest: it is a project-wide
//! decision rather than a per-card one. 79-1 and 79-2 gave
//! `project set-convergence-policy` a careful, correct refusal for exactly
//! the moment this command exists to resolve — an open cycle or a recorded
//! fact under the current digest — and that refusal names this card by
//! number in its own message. Retiring the old digest, installing the new
//! one, and re-pinning every non-terminal cycle's `project_revision` to it
//! happen in one transaction, because doing only the middle step (74-3's own
//! `refuse_orphaning_facts` doc comment calls this "the same defect as #79
//! with another name") would leave every open cycle broken at its very next
//! gate command.
//!
//! `abandon` is the third: an authorized actor permanently ends an escalated
//! card instead of granting it another attempt. There is no way back —
//! `CardState::Abandoned` has no successors, so a repeated `disposition
//! abandon` against the same card is refused by the ordinary lifecycle
//! transition check itself, the same check `card abandon` already relies on
//! for its own, *unauthorized* exit from a card that was never escalated in
//! the first place. What actually has to hold, and is easy to get wrong: an
//! abandon fact names no `dimension`, and `project` must keep it away from
//! the `DispositionMetadata` parse that requires one — otherwise one
//! abandoned card would refuse `card status` and every budget-gated command
//! for every *other* card sharing its cycle, which is exactly the isolation
//! #74 promises.

use std::{
    fs,
    path::{Path, PathBuf},
};

use clap::{Args, Subcommand};

use crate::{
    cli::output::CommandOutcome,
    commands::{
        CONTROL_ENV,
        card::{load_card, store_card_state},
        transaction::with_transaction,
    },
    config::{ConvergencePolicy, FieldError, ProjectConfig},
    control::{
        event_store::{EventDraft, EventStore},
        repository::{ControlRepository, PROJECT_FILE},
    },
    domain::{
        card::{CardRecord, CardState},
        clock::Clock,
        cycle::{CYCLE_DIR, CycleRecord},
        digest::Digest,
        ids::CardId,
    },
    error::{ErrorCode, HarnessError},
    git::authority::inspect_authority,
    policy::convergence::{
        CardConvergence, CardDimension, DISPOSITION_RECORDED_EVENT, DispositionKind, assess_card,
        project,
    },
};

/// Subcommands under `disposition`.
#[derive(Debug, Subcommand)]
pub enum DispositionCommand {
    /// Renew a card's exhausted convergence budget in one dimension.
    Renew(RenewArgs),
    /// Retire the currently configured convergence policy digest, install a
    /// new one, and re-pin every non-terminal cycle's `project_revision` to
    /// it, all in one transaction.
    Rebaseline(RebaselineArgs),
    /// Permanently end an escalated card — the authorized exit from an
    /// escalation, in place of another renewal.
    Abandon(AbandonArgs),
}

impl DispositionCommand {
    /// Its dotted command path, as the result envelope reports it.
    #[must_use]
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Renew(..) => "disposition.renew",
            Self::Rebaseline(..) => "disposition.rebaseline",
            Self::Abandon(..) => "disposition.abandon",
        }
    }
}

/// Arguments shared by disposition subcommands that touch control state.
#[derive(Debug, Args)]
pub struct CommonArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
    pub control: PathBuf,
    /// Identifies the acting party. Declared, not proven; see D-013.
    #[arg(long, default_value = "operator")]
    pub actor: String,
}

/// The `--dimension` option's values.
///
/// A closed set, spelled with hyphens the way every other multi-word CLI
/// value in this surface is: `clap` rejects anything else outright, so a
/// caller can never spell a fifth "dimension" into existence at this layer.
/// Kept as its own type, converted to [`CardDimension`] below, rather than
/// deriving `ValueEnum` on `CardDimension` itself: that type lives in
/// `policy::convergence`, which this card must not change.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum DimensionArg {
    #[value(name = "review-returns")]
    ReviewReturns,
    #[value(name = "repair-attempts")]
    RepairAttempts,
    #[value(name = "gate-failures")]
    GateFailures,
    #[value(name = "material-scope-revisions")]
    MaterialScopeRevisions,
}

impl From<DimensionArg> for CardDimension {
    fn from(value: DimensionArg) -> Self {
        match value {
            DimensionArg::ReviewReturns => Self::ReviewReturns,
            DimensionArg::RepairAttempts => Self::RepairAttempts,
            DimensionArg::GateFailures => Self::GateFailures,
            DimensionArg::MaterialScopeRevisions => Self::MaterialScopeRevisions,
        }
    }
}

/// Arguments accepted by `disposition renew`.
#[derive(Debug, Args)]
pub struct RenewArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The card whose budget is being renewed.
    #[arg(long)]
    pub card_id: String,
    /// The exhausted dimension to renew.
    #[arg(long, value_enum)]
    pub dimension: DimensionArg,
    /// Why this renewal is authorized.
    #[arg(long)]
    pub rationale: String,
    /// Report every check without writing anything.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `disposition rebaseline`.
#[derive(Debug, Args)]
pub struct RebaselineArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// Path to the new convergence policy to install, in the shape
    /// `ConvergencePolicy` deserializes.
    #[arg(long)]
    pub policy: PathBuf,
    /// Why this rebaseline is authorized.
    #[arg(long)]
    pub rationale: String,
    /// Report every check without writing anything.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `disposition abandon`.
#[derive(Debug, Args)]
pub struct AbandonArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The escalated card being permanently abandoned.
    #[arg(long)]
    pub card_id: String,
    /// Why this abandonment is authorized.
    #[arg(long)]
    pub rationale: String,
    /// Report every check without writing anything.
    #[arg(long)]
    pub dry_run: bool,
}

/// Executes a `disposition` subcommand.
///
/// # Errors
///
/// Returns a policy, precondition, or configuration error as appropriate.
pub fn execute(
    command: &DispositionCommand,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    match command {
        DispositionCommand::Renew(args) => run_renew(args, clock),
        DispositionCommand::Rebaseline(args) => run_rebaseline(args, clock),
        DispositionCommand::Abandon(args) => run_abandon(args, clock),
    }
}

/// Renders a [`CardDimension`] using the same spelling it would serialize
/// to, so a refusal message never hand-spells a name `serde` already owns.
/// Mirrors `dimension_wire_name` in `card.rs`; kept as its own copy rather
/// than shared, the way `handoff.rs` and `review.rs` each keep their own
/// `reason_wire_name` — this card's file scope does not extend to `card.rs`.
fn dimension_wire_name(dimension: CardDimension) -> String {
    match serde_json::to_value(dimension) {
        Ok(serde_json::Value::String(name)) => name,
        _ => format!("{dimension:?}"),
    }
}

/// Runs every check `disposition renew` must satisfy before it writes
/// anything, in the fixed order #74's contract requires, and returns the
/// configured convergence policy's digest on success — the exact value the
/// recorded fact must bind to.
///
/// Shared between the real command and its `--dry-run` preview, so neither
/// can promise or refuse something the other disagrees with.
///
/// # Errors
///
/// Returns [`ErrorCode::PolicyInvalidTransition`] when there is no
/// convergence policy configured, when the card is not currently escalated,
/// or when the named dimension is not among the ones that are exhausted.
/// Returns [`ErrorCode::PolicyNotAccepted`] when no final-authorization
/// policy is configured, or when the acting actor is not in its authorized
/// set. Returns [`ErrorCode::UsageInvalidArguments`] when `rationale` is
/// blank. Propagates a control-read or projection failure unchanged.
fn require_renewable(
    control: &ControlRepository,
    config: &ProjectConfig,
    record: &CardRecord,
    dimension: CardDimension,
    actor: &str,
    rationale: &str,
) -> Result<Digest, HarnessError> {
    // Check 1: a convergence policy must be configured at all. With none,
    // no card carries a budget in the first place (`assess_card` answers
    // `LegacyUnassessed` for every card), so there is nothing to renew.
    let Some(policy) = config.convergence_policy.as_ref() else {
        return Err(HarnessError::Control {
            reason: format!(
                "card {} cannot be renewed: no convergence policy is configured for this project, so there is no budget to renew",
                record.card_id
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    };

    // Checks 2 and 3 both read the same fresh assessment: the same
    // projection and the same `assess_card` call `require_convergence_budget`
    // itself uses, so this command can never disagree with the check it
    // exists to release a card from.
    let cycle_events = EventStore::new(control).for_cycle(&record.cycle_id)?;
    let view = project(
        Some(policy),
        &config.project_id,
        &record.cycle_id,
        &cycle_events,
    )
    .map_err(|error| HarnessError::Control {
        reason: format!(
            "convergence projection for cycle {} is unusable: {error}",
            record.cycle_id
        ),
        code: ErrorCode::InternalControlCorrupt,
    })?;
    let convergence = assess_card(Some(policy), &view, &record.card_id, record.risk);

    // Check 2: the card must be escalated *right now*. This is what makes a
    // renewal a response to escalation rather than a blank cheque: a budget
    // cannot be pre-renewed ahead of exhaustion, and after a renewal
    // succeeds the card is `Within` again — `assess_card` recomputes fresh
    // from every recorded fact, including the one this command is about to
    // append — so an immediate second renewal is refused by this exact same
    // check, with no extra bookkeeping needed to remember one already
    // happened.
    let CardConvergence::Escalated { exhausted, .. } = &convergence else {
        return Err(HarnessError::Control {
            reason: format!(
                "card {} is not currently escalated; there is nothing to renew",
                record.card_id
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    };

    // Check 3: the *named* dimension specifically must be one of the
    // exhausted ones. A card can be escalated in one dimension while
    // another still has budget; renewing the one that still has budget
    // would grant budget ahead of exhaustion, which is exactly the silent
    // expansion #74 forbids. The refusal names every dimension that really
    // is exhausted, so an operator does not have to guess or re-run
    // `card status` to find the right one.
    if !exhausted.iter().any(|item| item.dimension == dimension) {
        let actually_exhausted = exhausted
            .iter()
            .map(|item| dimension_wire_name(item.dimension))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(HarnessError::Control {
            reason: format!(
                "card {} cannot renew `{}`: that dimension still has budget; the exhausted dimension(s) are: {actually_exhausted}",
                record.card_id,
                dimension_wire_name(dimension)
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    }

    // Check 4: the actor must be authorized. The authorized set is
    // `final_authorization_policy.authorizer_actor_ids`, resolved exactly as
    // `commands::acceptance::validate_final_authorization` already resolves
    // it to authorize a sealed cycle's final integration — the one place in
    // this codebase that reads this exact field before this card: a missing
    // policy and an actor absent from its configured set both refuse with
    // `ErrorCode::PolicyNotAccepted`, and membership is decided by
    // `FinalAuthorizationPolicy::authorizes`, never by a hand-rolled
    // comparison. This command follows that pattern rather than re-deciding
    // the question.
    //
    // The set is deliberately *reused* rather than given its own field on
    // `ConvergencePolicy` — say, an `authorized_renewer_ids` next to
    // `card_limits`. That field's digest is embedded in every already
    // recorded `convergence.attempt_recorded` and (once this card lands)
    // `convergence.disposition_recorded` fact as `metadata.policy_digest`,
    // and `project` refuses any fact whose digest does not match the policy
    // it is handed (see its "fact names a foreign policy digest" checks).
    // Adding a field to `ConvergencePolicy` changes that digest, so every
    // fact already recorded under the old shape would instantly name a
    // foreign policy — not just facts recorded after the change, every fact
    // a project has ever recorded — and the whole projection would refuse.
    // That constraint is permanent, not specific to `renew`: none of the
    // five dispositions #74 still owes may add a field to `ConvergencePolicy`
    // either. Each will need to find its own already-configured surface to
    // reuse, the way this one reuses `final_authorization_policy`.
    let authorization = config.final_authorization_policy.as_ref().ok_or_else(|| {
        HarnessError::Control {
            reason: "final authorization is not configured for this project; explicitly configure final_authorization_policy before authorizing a convergence budget renewal".to_owned(),
            code: ErrorCode::PolicyNotAccepted,
        }
    })?;
    if !authorization.authorizes(actor) {
        return Err(HarnessError::Control {
            reason: format!(
                "actor {actor} is not configured to authorize a convergence budget renewal"
            ),
            code: ErrorCode::PolicyNotAccepted,
        });
    }

    // Check 5: a disposition with no declared reason is not a decision.
    // 74-1 already refuses the fact itself when `rationale` is blank (see
    // `project`'s disposition handling); this refuses it before the command
    // ever tries to write it. A blank `--rationale` is a usage problem, not
    // a policy one — the same class `commands::acceptance::authorizer` and
    // `commands::integration::run_exception_raise` already file a bare or
    // empty required argument under.
    if rationale.trim().is_empty() {
        return Err(HarnessError::Control {
            reason: "disposition renew requires a non-blank --rationale; a renewal with no declared reason cannot be recorded".to_owned(),
            code: ErrorCode::UsageInvalidArguments,
        });
    }

    policy.digest()
}

/// Reports what `disposition renew` would bind, without binding it.
///
/// Runs every check the real command makes, through the same
/// [`require_renewable`], so a caller can never be told a renewal would
/// succeed (or told why it would fail) and then have the real command
/// disagree.
fn preview_renew(
    args: &RenewArgs,
    card_id: &CardId,
    dimension: CardDimension,
) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.common.control)?;
    let (record, _state) = load_card(&control, card_id)?;
    let config = control.project()?;
    let policy_digest = require_renewable(
        &control,
        &config,
        &record,
        dimension,
        &args.common.actor,
        &args.rationale,
    )?;
    Ok(CommandOutcome::new(
        "disposition.renew",
        format!(
            "Dry run: would renew card {card_id}'s {} budget; nothing was changed",
            dimension_wire_name(dimension)
        ),
        serde_json::json!({
            "dry_run": true,
            "card_id": card_id.to_string(),
            "dimension": dimension_wire_name(dimension),
            "policy_digest": policy_digest.as_str(),
        }),
    ))
}

fn run_renew(args: &RenewArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;
    let dimension: CardDimension = args.dimension.into();

    if args.dry_run {
        return preview_renew(args, &card_id, dimension);
    }

    with_transaction(
        &args.common.control,
        "disposition.renew",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let (record, state) = load_card(control, &card_id)?;
            let config = control.project()?;
            let policy_digest = require_renewable(
                control,
                &config,
                &record,
                dimension,
                &args.common.actor,
                &args.rationale,
            )?;

            // `head` binds to the current revision's own `base_sha` — the
            // only exact commit SHA a card is guaranteed to carry in any
            // lifecycle state, escalated or not, the same binding 71-R5's
            // material-scope-revision fact uses in `card.rs`'s `run_revise`.
            // A candidate SHA would not do: an escalated card may not have
            // reached `handed_off` at all yet.
            let event = events.append(
                &config.project_id,
                EventDraft::new(DISPOSITION_RECORDED_EVENT, &args.common.actor)
                    .cycle(record.cycle_id.clone())
                    .card(
                        card_id.clone(),
                        state.current_revision,
                        state.current_digest.clone(),
                    )
                    .head(record.base_sha.clone())
                    .meta("disposition", serde_json::to_value(DispositionKind::Renew)?)
                    .meta("dimension", serde_json::to_value(dimension)?)
                    .meta("rationale", serde_json::json!(args.rationale))
                    .meta("authorized_by", serde_json::json!(args.common.actor))
                    .meta("policy_digest", serde_json::json!(policy_digest.as_str())),
                clock,
            )?;

            control.commit(
                expected,
                &format!(
                    "disposition: renew {card_id} {}",
                    dimension_wire_name(dimension)
                ),
            )?;

            Ok(CommandOutcome::new(
                "disposition.renew",
                format!(
                    "Renewed card {card_id}'s {} budget\nrationale: {}\nauthorized by: {}",
                    dimension_wire_name(dimension),
                    args.rationale,
                    args.common.actor
                ),
                serde_json::json!({
                    "card_id": card_id.to_string(),
                    "dimension": dimension_wire_name(dimension),
                    "event_id": event.event_id.to_string(),
                    "policy_digest": policy_digest.as_str(),
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}

/// Reads, parses, and validates the convergence policy named by `--policy`.
///
/// Mirrors `commands::project::read_convergence_policy` step for step: same
/// read, same parse, same validation. That function is private to
/// `project.rs`, and #74-4's frozen contract confines this card to
/// `disposition.rs` and `tests/convergence.rs` — widening its visibility, or
/// otherwise editing `project.rs`, is out of scope here. Flagged rather than
/// silently repeated, per the contract's own instruction: this is the one
/// place `read_convergence_policy`'s shape is duplicated in this card, and
/// it exists only because the original is unreachable from this file.
fn read_new_policy(path: &Path) -> Result<ConvergencePolicy, HarnessError> {
    let raw = fs::read_to_string(path).map_err(|source| HarnessError::Control {
        reason: format!(
            "cannot read convergence policy {}: {source}",
            path.display()
        ),
        code: ErrorCode::ConfigMalformed,
    })?;
    let policy: ConvergencePolicy =
        serde_json::from_str(&raw).map_err(|source| HarnessError::Control {
            reason: format!("convergence policy is malformed: {source}"),
            code: ErrorCode::ConfigMalformed,
        })?;
    policy.validate().map_err(FieldError::into_error)?;
    Ok(policy)
}

/// Every cycle in the control repository whose status is not terminal — not
/// `closed` and not `abandoned` — sorted by identifier.
///
/// A different question from `project::blocks_convergence_policy_change`,
/// which asks what would break *today* if the configured digest changed
/// under a cycle: there, `draft` answers no, because a draft cycle cannot
/// yet hold a card that reaches `gate::validation_progress`'s comparison at
/// `src/commands/gate.rs:791`. This asks what would collide *once a cycle
/// activates*: a cycle's `project_revision` is pinned once, at `cycle
/// create`, and never moves on its own (`cycle activate` never touches it),
/// so a draft left un-re-pinned here would still be carrying the retired
/// digest the moment it later activates and starts accepting cards —
/// colliding then instead of now. Re-pinning every non-terminal cycle is
/// what closes that deferred collision. A terminal cycle's history is over
/// and is left exactly as it was: it cannot activate again to collide with
/// anything.
///
/// Mirrors the directory walk `project::all_cycles` already performs, for
/// this different question: that function is private to `project.rs`, out
/// of this card's file scope, so this is a fresh walk rather than a shared
/// helper.
///
/// # Errors
///
/// Returns an error when the cycle directory cannot be read or a record is
/// malformed.
fn non_terminal_cycles(control: &ControlRepository) -> Result<Vec<CycleRecord>, HarnessError> {
    let directory = control.path(CYCLE_DIR);
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut names: Vec<String> = fs::read_dir(&directory)
        .map_err(|source| HarnessError::ControlIo {
            path: directory.clone(),
            source,
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| {
            Path::new(name)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        })
        .collect();
    names.sort();

    let cycles: Vec<CycleRecord> = names
        .into_iter()
        .map(|name| {
            let relative = format!("{CYCLE_DIR}/{name}");
            serde_json::from_str(&control.read(&relative)?).map_err(|source| {
                HarnessError::Control {
                    reason: format!("cycle record {relative} is malformed: {source}"),
                    code: ErrorCode::InternalControlCorrupt,
                }
            })
        })
        .collect::<Result<Vec<_>, HarnessError>>()?;

    Ok(cycles
        .into_iter()
        .filter(|cycle| !cycle.status.is_terminal())
        .collect())
}

/// What `disposition rebaseline` binds once every check passes: the new
/// policy to install, the digest it retires, and the digest it installs.
struct RebaselinePreflight {
    new_policy: ConvergencePolicy,
    retired_digest: Digest,
    new_digest: Digest,
}

/// Runs every check `disposition rebaseline` must satisfy before it writes
/// anything, in the fixed order #74-4's contract requires. Shared between
/// the real command and its `--dry-run` preview, so neither can promise or
/// refuse something the other disagrees with.
///
/// # Errors
///
/// Returns [`ErrorCode::ConfigMalformed`] when the new policy cannot be read
/// or parsed, or whatever code [`ConvergencePolicy::validate`] assigns when
/// it fails its own checks. Returns [`ErrorCode::PolicyInvalidTransition`]
/// when no convergence policy is configured yet — there is no digest to
/// retire — or when the new policy's digest is identical to the one already
/// in force. Returns [`ErrorCode::PolicyNotAccepted`] when no
/// final-authorization policy is configured, or when the acting actor is
/// not in its authorized set. Returns [`ErrorCode::UsageInvalidArguments`]
/// when `rationale` is blank.
fn require_rebaseline(
    config: &ProjectConfig,
    policy_path: &Path,
    actor: &str,
    rationale: &str,
) -> Result<RebaselinePreflight, HarnessError> {
    // Check 1: the new policy must be readable and pass its own validation.
    let new_policy = read_new_policy(policy_path)?;
    let new_digest = new_policy.digest()?;

    // Check 2: a policy must already be configured, or there is no digest
    // to retire. Named explicitly, so an operator pointed here by mistake
    // is sent to the command that actually applies: installing the first
    // policy is `project set-convergence-policy`, not a rebaseline.
    let Some(current_policy) = config.convergence_policy.as_ref() else {
        return Err(HarnessError::Control {
            reason: format!(
                "project {} has no convergence policy configured, so there is no policy digest to retire; install the first one with `project set-convergence-policy`",
                config.project_id
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    };
    let retired_digest = current_policy.digest()?;

    // Check 3: retiring a digest to reinstall it unchanged is a decision
    // with no effect. It would still retire every non-terminal cycle's
    // current pin and record a fact for each — pure noise in the record for
    // zero change in what any cycle enforces.
    if new_digest == retired_digest {
        return Err(HarnessError::Control {
            reason: format!(
                "the new convergence policy is identical to the one already installed (digest {new_digest}); retiring a policy digest to reinstall it unchanged has no effect and would only add noise to the record"
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    }

    // Check 4: the actor must be authorized, by the same
    // `final_authorization_policy.authorizer_actor_ids` path
    // `require_renewable` above already resolves it through — see that
    // function's own long note for why this set is reused rather than
    // given its own field on `ConvergencePolicy`.
    let authorization = config.final_authorization_policy.as_ref().ok_or_else(|| {
        HarnessError::Control {
            reason: "final authorization is not configured for this project; explicitly configure final_authorization_policy before authorizing a convergence policy rebaseline".to_owned(),
            code: ErrorCode::PolicyNotAccepted,
        }
    })?;
    if !authorization.authorizes(actor) {
        return Err(HarnessError::Control {
            reason: format!(
                "actor {actor} is not configured to authorize a convergence policy rebaseline"
            ),
            code: ErrorCode::PolicyNotAccepted,
        });
    }

    // Check 5: a disposition with no declared reason is not a decision.
    if rationale.trim().is_empty() {
        return Err(HarnessError::Control {
            reason: "disposition rebaseline requires a non-blank --rationale; a rebaseline with no declared reason cannot be recorded".to_owned(),
            code: ErrorCode::UsageInvalidArguments,
        });
    }

    Ok(RebaselinePreflight {
        new_policy,
        retired_digest,
        new_digest,
    })
}

/// Renders the "N non-terminal cycle(s)[: id, id, ...]" clause shared by the
/// dry-run preview and the real command's own report, so the two can never
/// describe the same set of cycles differently.
fn describe_repinned(cycle_ids: &[String]) -> String {
    if cycle_ids.is_empty() {
        format!("{} non-terminal cycle(s)", cycle_ids.len())
    } else {
        format!(
            "{} non-terminal cycle(s): {}",
            cycle_ids.len(),
            cycle_ids.join(", ")
        )
    }
}

/// Reports what `disposition rebaseline` would retire, install, and re-pin,
/// without doing any of it.
fn preview_rebaseline(args: &RebaselineArgs) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.common.control)?;
    let config = control.project()?;
    let preflight = require_rebaseline(&config, &args.policy, &args.common.actor, &args.rationale)?;
    let repinned: Vec<String> = non_terminal_cycles(&control)?
        .iter()
        .map(|cycle| cycle.cycle_id.to_string())
        .collect();

    Ok(CommandOutcome::new(
        "disposition.rebaseline",
        format!(
            "Dry run: would retire policy digest {} and install {}\nwould re-pin {}\nwould record {} rebaseline fact(s)\nnothing was changed",
            preflight.retired_digest,
            preflight.new_digest,
            describe_repinned(&repinned),
            repinned.len(),
        ),
        serde_json::json!({
            "dry_run": true,
            "retired_policy_digest": preflight.retired_digest.as_str(),
            "policy_digest": preflight.new_digest.as_str(),
            "repinned_cycles": repinned,
            "fact_count": repinned.len(),
        }),
    )
    .with_project(config.project_id.clone()))
}

/// Runs `disposition rebaseline`: retires the currently configured
/// convergence policy digest, installs the new one, and re-pins every
/// non-terminal cycle's `project_revision` to it — all three effects inside
/// one [`with_transaction`] call, so a failure at any point lands none of
/// them.
///
/// # Errors
///
/// Returns whatever [`require_rebaseline`] refuses with. Returns
/// [`ErrorCode::ConfigProtectedBranch`] when the authority's protected
/// branch does not resolve to one exact commit. Propagates a control-write
/// or projection failure unchanged.
fn run_rebaseline(
    args: &RebaselineArgs,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    if args.dry_run {
        return preview_rebaseline(args);
    }

    with_transaction(
        &args.common.control,
        "disposition.rebaseline",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let mut config = control.project()?;
            let preflight =
                require_rebaseline(&config, &args.policy, &args.common.actor, &args.rationale)?;
            let targets = non_terminal_cycles(control)?;

            // The head this decision binds to is the protected branch's
            // exact commit right now — resolved through the same
            // `inspect_authority` call `project::authority_health` and
            // every promotion boundary in `integration.rs` already use to
            // read the authority's protected-branch commit, never a
            // candidate SHA or a frozen cycle baseline: a rebaseline is a
            // project-wide decision bound to no one card's candidate, so
            // there is no other exact SHA it could name.
            let authority =
                inspect_authority(&config.authority_repository, &config.protected_branch)?;
            let head_sha = authority.protected_sha.ok_or_else(|| HarnessError::Control {
                reason: format!(
                    "authority branch `{}` does not resolve to one commit in {}; a rebaseline needs an exact protected-branch commit to bind its facts to",
                    config.protected_branch,
                    config.authority_repository.display()
                ),
                code: ErrorCode::ConfigProtectedBranch,
            })?;

            config.convergence_policy = Some(preflight.new_policy.clone());
            let new_project_revision = Digest::of_canonical(&config)?;
            control.write_atomic(PROJECT_FILE, &format!("{}\n", config.to_json()?))?;

            let mut repinned = Vec::with_capacity(targets.len());
            for mut cycle in targets {
                cycle.project_revision = new_project_revision.clone();
                control.write_atomic(
                    &CycleRecord::relative_path(&cycle.cycle_id),
                    &format!("{}\n", serde_json::to_string_pretty(&cycle)?),
                )?;
                events.append(
                    &config.project_id,
                    EventDraft::new(DISPOSITION_RECORDED_EVENT, &args.common.actor)
                        .cycle(cycle.cycle_id.clone())
                        .head(head_sha.clone())
                        .meta(
                            "disposition",
                            serde_json::to_value(DispositionKind::Rebaseline)?,
                        )
                        .meta(
                            "retired_policy_digest",
                            serde_json::json!(preflight.retired_digest.as_str()),
                        )
                        .meta("rationale", serde_json::json!(args.rationale))
                        .meta("authorized_by", serde_json::json!(args.common.actor))
                        .meta(
                            "policy_digest",
                            serde_json::json!(preflight.new_digest.as_str()),
                        ),
                    clock,
                )?;
                repinned.push(cycle.cycle_id.to_string());
            }

            control.commit(
                expected,
                &format!(
                    "disposition: rebaseline {} -> {}",
                    preflight.retired_digest, preflight.new_digest
                ),
            )?;

            Ok(CommandOutcome::new(
                "disposition.rebaseline",
                format!(
                    "Rebaselined convergence policy: retired {} and installed {}\nre-pinned {}\nrationale: {}\nauthorized by: {}",
                    preflight.retired_digest,
                    preflight.new_digest,
                    describe_repinned(&repinned),
                    args.rationale,
                    args.common.actor,
                ),
                serde_json::json!({
                    "retired_policy_digest": preflight.retired_digest.as_str(),
                    "policy_digest": preflight.new_digest.as_str(),
                    "repinned_cycles": repinned,
                    "fact_count": repinned.len(),
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}

/// Runs every check `disposition abandon` must satisfy before it writes
/// anything, in the fixed order #74-5's contract requires, and returns the
/// configured convergence policy's digest on success — the exact value the
/// recorded fact must bind to.
///
/// Shared between the real command and its `--dry-run` preview, so neither
/// can promise or refuse something the other disagrees with.
///
/// # Errors
///
/// Returns [`ErrorCode::PolicyInvalidTransition`] when there is no
/// convergence policy configured, when the card is not currently escalated,
/// or when its current lifecycle state does not permit the transition to
/// `CardState::Abandoned` — whatever [`CardState::check_transition`] itself
/// returns for that. Returns [`ErrorCode::PolicyNotAccepted`] when no
/// final-authorization policy is configured, or when the acting actor is
/// not in its authorized set. Returns [`ErrorCode::UsageInvalidArguments`]
/// when `rationale` is blank. Propagates a control-read or projection
/// failure unchanged.
fn require_abandonable(
    control: &ControlRepository,
    config: &ProjectConfig,
    record: &CardRecord,
    current_state: CardState,
    actor: &str,
    rationale: &str,
) -> Result<Digest, HarnessError> {
    // Check 1: a convergence policy must be configured at all. With none, no
    // card carries a budget in the first place (`assess_card` answers
    // `LegacyUnassessed` for every card), so no card can ever be escalated,
    // and there is no digest to bind the recorded fact to.
    let Some(policy) = config.convergence_policy.as_ref() else {
        return Err(HarnessError::Control {
            reason: format!(
                "card {} cannot be abandoned this way: no convergence policy is configured for this project, so no card can be escalated",
                record.card_id
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    };

    // Check 2: the card must be escalated *right now* — read the same way
    // `require_renewable` reads it: a fresh `EventStore::new(control).
    // for_cycle(...)` projected and assessed, so this command can never
    // disagree with the check it exists to release a card from.
    // `disposition abandon` is the authorized *escalation exit*, not a
    // general-purpose abandon — a card that is `Within` is refused here and
    // pointed at `card abandon`, the ordinary route that already covers it,
    // so an operator refused here is never left with nowhere to go.
    let cycle_events = EventStore::new(control).for_cycle(&record.cycle_id)?;
    let view = project(
        Some(policy),
        &config.project_id,
        &record.cycle_id,
        &cycle_events,
    )
    .map_err(|error| HarnessError::Control {
        reason: format!(
            "convergence projection for cycle {} is unusable: {error}",
            record.cycle_id
        ),
        code: ErrorCode::InternalControlCorrupt,
    })?;
    let convergence = assess_card(Some(policy), &view, &record.card_id, record.risk);
    let CardConvergence::Escalated { .. } = &convergence else {
        return Err(HarnessError::Control {
            reason: format!(
                "card {} is not currently escalated; `disposition abandon` only ends an active escalation — to abandon a card that is not escalated, use `card abandon` instead",
                record.card_id
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    };

    // Check 3: the card's lifecycle state must permit the transition to
    // `Abandoned`. This is where "a repeated disposition refuses" comes
    // from, and it comes for free: `Abandoned` has no successors, so a
    // second `disposition abandon` against the same card reaches this exact
    // check and is refused by it. An already-abandoned card's recorded
    // facts do not disappear, so it still assesses as `Escalated` and still
    // passes check 2 above every time — it is this check, not check 2, that
    // stops a repeat.
    current_state.check_transition(CardState::Abandoned)?;

    // Check 4: the actor must be authorized, by the same
    // `final_authorization_policy.authorizer_actor_ids` path
    // `require_renewable` and `require_rebaseline` already resolve it
    // through — see `require_renewable`'s own long note for why this set is
    // reused rather than given its own field on `ConvergencePolicy`.
    let authorization = config.final_authorization_policy.as_ref().ok_or_else(|| {
        HarnessError::Control {
            reason: "final authorization is not configured for this project; explicitly configure final_authorization_policy before authorizing a convergence disposition abandon".to_owned(),
            code: ErrorCode::PolicyNotAccepted,
        }
    })?;
    if !authorization.authorizes(actor) {
        return Err(HarnessError::Control {
            reason: format!(
                "actor {actor} is not configured to authorize a convergence disposition abandon"
            ),
            code: ErrorCode::PolicyNotAccepted,
        });
    }

    // Check 5: a disposition with no declared reason is not a decision.
    if rationale.trim().is_empty() {
        return Err(HarnessError::Control {
            reason: "disposition abandon requires a non-blank --rationale; an abandon with no declared reason cannot be recorded".to_owned(),
            code: ErrorCode::UsageInvalidArguments,
        });
    }

    policy.digest()
}

/// Reports what `disposition abandon` would bind, without binding it.
///
/// Runs every check the real command makes, through the same
/// [`require_abandonable`], so a caller can never be told an abandon would
/// succeed (or told why it would fail) and then have the real command
/// disagree.
fn preview_abandon(args: &AbandonArgs, card_id: &CardId) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.common.control)?;
    let (record, state) = load_card(&control, card_id)?;
    let config = control.project()?;
    let policy_digest = require_abandonable(
        &control,
        &config,
        &record,
        state.state,
        &args.common.actor,
        &args.rationale,
    )?;
    Ok(CommandOutcome::new(
        "disposition.abandon",
        format!("Dry run: would abandon card {card_id}; nothing was changed"),
        serde_json::json!({
            "dry_run": true,
            "card_id": card_id.to_string(),
            "policy_digest": policy_digest.as_str(),
        }),
    ))
}

fn run_abandon(args: &AbandonArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;

    if args.dry_run {
        return preview_abandon(args, &card_id);
    }

    with_transaction(
        &args.common.control,
        "disposition.abandon",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let (record, state) = load_card(control, &card_id)?;
            let previous = state.state;
            let config = control.project()?;
            let policy_digest = require_abandonable(
                control,
                &config,
                &record,
                previous,
                &args.common.actor,
                &args.rationale,
            )?;

            store_card_state(control, &record, &state, CardState::Abandoned)?;

            // `head` binds to the current revision's own `base_sha` — the
            // same binding `renew` uses, and for the same reason: it is the
            // only exact commit SHA an escalated card is guaranteed to
            // carry in any lifecycle state.
            let event = events.append(
                &config.project_id,
                EventDraft::new(DISPOSITION_RECORDED_EVENT, &args.common.actor)
                    .cycle(record.cycle_id.clone())
                    .card(
                        card_id.clone(),
                        state.current_revision,
                        state.current_digest.clone(),
                    )
                    .head(record.base_sha.clone())
                    .transition(Some(previous.name()), CardState::Abandoned.name())
                    .meta(
                        "disposition",
                        serde_json::to_value(DispositionKind::Abandon)?,
                    )
                    .meta("rationale", serde_json::json!(args.rationale))
                    .meta("authorized_by", serde_json::json!(args.common.actor))
                    .meta("policy_digest", serde_json::json!(policy_digest.as_str())),
                clock,
            )?;

            control.commit(expected, &format!("disposition: abandon {card_id}"))?;

            Ok(CommandOutcome::new(
                "disposition.abandon",
                format!(
                    "Abandoned card {card_id}\nrationale: {}\nauthorized by: {}",
                    args.rationale, args.common.actor
                ),
                serde_json::json!({
                    "card_id": card_id.to_string(),
                    "state": CardState::Abandoned.name(),
                    "event_id": event.event_id.to_string(),
                    "policy_digest": policy_digest.as_str(),
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}
