//! Disposition commands: authorized decisions that change what an escalated
//! card may do next.
//!
//! #74 defines six dispositions. `renew` is the first: an authorized actor
//! grants a card's exhausted convergence budget one more configured limit,
//! in exactly the dimension that is exhausted, so the card can be delivered
//! and reviewed again. `Escalated` cards otherwise have no way out —
//! [`crate::policy::convergence::NextPermittedAction::RecordAuthorizedDisposition`]
//! named this command before it existed; this is what satisfies it.

use std::path::PathBuf;

use clap::{Args, Subcommand};

use crate::{
    cli::output::CommandOutcome,
    commands::{CONTROL_ENV, card::load_card, transaction::with_transaction},
    config::ProjectConfig,
    control::{
        event_store::{EventDraft, EventStore},
        repository::ControlRepository,
    },
    domain::{card::CardRecord, clock::Clock, digest::Digest, ids::CardId},
    error::{ErrorCode, HarnessError},
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
}

impl DispositionCommand {
    /// Its dotted command path, as the result envelope reports it.
    #[must_use]
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Renew(..) => "disposition.renew",
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
