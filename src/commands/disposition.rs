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
//!
//! `accept-risk` is the fourth: an authorized actor accepts a **disclosed**
//! risk on one exhausted dimension of an escalated card, so the card can be
//! delivered and reviewed again — without its budget being expanded. That
//! distinction from `renew` is the entire point of this command: `renew`
//! grants the configured limit again, so a further attempt still counts and
//! can escalate the same dimension a second time; `accept-risk` grants
//! nothing, so the count keeps climbing past the limit forever and the
//! dimension simply stops being reported exhausted, because an authorized
//! actor accepted that risk. `--risk` and `--rationale` are both required
//! and distinct on purpose: `--risk` is *what* is being accepted — the
//! disclosure, without which "disclosed risk" means nothing — and
//! `--rationale` is *why* accepting it is justified.
//!
//! `split` is the fifth: an authorized actor moves the remaining work
//! behind one exhausted dimension to an **already-existing** follow-up
//! card, so the current card can be delivered and reviewed again — the
//! authorized route for #9's own criterion that "non-blocking improvements
//! can be moved to follow-up cards without holding the current card open."
//! `split` does not create the follow-up card itself: `card create`
//! refuses outside an `Active` cycle, and an escalated card is very often
//! sitting in a `Sealed` one, where card membership is deliberately
//! frozen, so a self-creating split would fail exactly where it is most
//! needed, and would stand up a second, competing implementation of card
//! creation. So the operator creates the follow-up card first, through the
//! normal governed path that already enforces every cycle rule, and
//! `split` only binds to it by id — recording an exact binding between the
//! two, which is #74's "exact target" criterion. Like `accept-risk`,
//! `split` grants no budget back: it waives the named dimension's
//! escalation through the very same `DimensionCount::escalation_waived`
//! flag `accept-risk` sets, recording *which* decision waived it in the
//! fact's own `disposition` field rather than adding a second flag next to
//! the first. Also like `accept-risk`, it moves no card state at all — not
//! the original card's, and not the follow-up's either: the follow-up card
//! is named in the record, never mutated, so a card's lifecycle state only
//! ever changes in one place.
//!
//! `redesign` is the sixth and last: an authorized actor ends an escalated
//! card because the approach itself was wrong, and names the exact card
//! that replaces it — #9's own criterion to "treat fundamental repeated
//! failures as a design or scope signal, not as an invitation to patch
//! indefinitely." Closest to `abandon`: both terminate the original card,
//! through the very same `CardState::Abandoned` transition and the very
//! same "a repeated disposition refuses" property that transition gives
//! for free, since `Abandoned` has no successors. What `redesign` adds is
//! the replacement binding — the record shows the work continued
//! elsewhere, not merely stopped — and that binding may name a card in a
//! **different** cycle, deliberately, unlike `split`'s follow-up: `split`
//! leaves the original card alive and governed, so a cross-cycle follow-up
//! would create two cycles with a live claim on one decision, while
//! `redesign` terminates the original, so no dual-governance question
//! survives. 74-7b's review found the cost of naming a foreign cycle only
//! implicitly, through `split`'s bare `follow_up_card_id`: a cross-cycle
//! fact reads as though it named a card in its own cycle unless something
//! says otherwise. So `redesign` records `replacement_cycle_id` alongside
//! `replacement_card_id`, taken from the replacement's own loaded record,
//! keeping the binding self-describing. Closest to `split` in one other
//! way and no more: `redesign`, like `split`, does not create the
//! replacement card, for the same reason — the operator names an
//! already-existing card, reached through the normal governed path, and
//! `redesign` only binds to it. Unlike `split` and `accept-risk`,
//! `redesign` waives nothing and never touches
//! `DimensionCount::escalation_waived`: the card is terminal, so there is
//! no budget question left to answer, and there is no `--dimension`
//! either — a redesign answers the whole card, not one dimension of it.

use std::{
    fs,
    path::{Path, PathBuf},
};

use clap::{Args, Subcommand};

use crate::{
    cli::output::CommandOutcome,
    commands::{
        CONTROL_ENV,
        acceptance::{
            FINAL_AUTHORIZATION_ACTOR_NOT_AUTHORIZED_RECOVERY,
            FINAL_AUTHORIZATION_POLICY_NOT_CONFIGURED_RECOVERY,
        },
        card::{load_card, store_card_state},
        transaction::{pure_recheck, with_transaction},
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
        CardConvergence, CardCounters, CardDimension, DISPOSITION_RECORDED_EVENT, DispositionKind,
        ProjectConvergence, assess_card, project,
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
    /// Accept a disclosed risk on one exhausted dimension of an escalated
    /// card, without expanding its budget — the authorized exit from an
    /// escalation that grants no further attempts, in place of a renewal.
    AcceptRisk(AcceptRiskArgs),
    /// Move the remaining work behind one exhausted dimension to an
    /// already-existing follow-up card, without expanding the original
    /// card's budget — the authorized exit from an escalation that records
    /// exactly where the deferred work went, in place of a renewal.
    Split(SplitArgs),
    /// Permanently end an escalated card because the approach itself was
    /// wrong, and name the exact card that replaces it — the authorized
    /// exit from an escalation that ends the original attempt rather than
    /// granting it another one, in place of a renewal.
    Redesign(RedesignArgs),
}

impl DispositionCommand {
    /// Its dotted command path, as the result envelope reports it.
    #[must_use]
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Renew(..) => "disposition.renew",
            Self::Rebaseline(..) => "disposition.rebaseline",
            Self::Abandon(..) => "disposition.abandon",
            Self::AcceptRisk(..) => "disposition.accept-risk",
            Self::Split(..) => "disposition.split",
            Self::Redesign(..) => "disposition.redesign",
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

/// Arguments accepted by `disposition accept-risk`.
#[derive(Debug, Args)]
pub struct AcceptRiskArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The card whose disclosed risk is being accepted.
    #[arg(long)]
    pub card_id: String,
    /// The exhausted dimension whose risk is being accepted.
    #[arg(long, value_enum)]
    pub dimension: DimensionArg,
    /// The disclosed risk being accepted — *what* is being accepted, not
    /// *why*; see `rationale` for the latter.
    #[arg(long)]
    pub risk: String,
    /// Why accepting this disclosed risk is authorized.
    #[arg(long)]
    pub rationale: String,
    /// Report every check without writing anything.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `disposition split`.
#[derive(Debug, Args)]
pub struct SplitArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The escalated card whose exhausted dimension is being split off.
    #[arg(long)]
    pub card_id: String,
    /// The exhausted dimension whose remaining work is moving to the
    /// follow-up card.
    #[arg(long, value_enum)]
    pub dimension: DimensionArg,
    /// The already-existing card the deferred work is moving to. `split`
    /// binds to it; it does not create it — see the module doc comment for
    /// why.
    #[arg(long)]
    pub follow_up_card_id: String,
    /// Why this split is authorized.
    #[arg(long)]
    pub rationale: String,
    /// Report every check without writing anything.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `disposition redesign`.
///
/// Deliberately carries no `--dimension`, unlike `renew`, `accept-risk`, and
/// `split`: a redesign answers the whole card, not one dimension of it —
/// see the module doc comment.
#[derive(Debug, Args)]
pub struct RedesignArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The escalated card whose approach was wrong and is being ended.
    #[arg(long)]
    pub card_id: String,
    /// The already-existing card that replaces it. `redesign` binds to it;
    /// it does not create it — see the module doc comment for why, the
    /// same reason `split` does not create its own follow-up card. Unlike
    /// `split`'s follow-up, this may name a card in a different cycle; see
    /// the module doc comment.
    #[arg(long)]
    pub replacement_card_id: String,
    /// Why this redesign is authorized.
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
        DispositionCommand::AcceptRisk(args) => run_accept_risk(args, clock),
        DispositionCommand::Split(args) => run_split(args, clock),
        DispositionCommand::Redesign(args) => run_redesign(args, clock),
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
        HarnessError::ControlWithRecovery {
            reason: "final authorization is not configured for this project; explicitly configure final_authorization_policy before authorizing a convergence budget renewal".to_owned(),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_AUTHORIZATION_POLICY_NOT_CONFIGURED_RECOVERY,
        }
    })?;
    if !authorization.authorizes(actor) {
        return Err(HarnessError::ControlWithRecovery {
            reason: format!(
                "actor {actor} is not configured to authorize a convergence budget renewal"
            ),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_AUTHORIZATION_ACTOR_NOT_AUTHORIZED_RECOVERY,
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

    // Match the dry-run admission before opening a journal. The transaction
    // repeats it under the project lock and maps a race to the same
    // non-persisting refusal.
    {
        let control = ControlRepository::open(&args.common.control)?;
        let (record, _) = load_card(&control, &card_id)?;
        let config = control.project()?;
        require_renewable(
            &control,
            &config,
            &record,
            dimension,
            &args.common.actor,
            &args.rationale,
        )?;
    }

    with_transaction(
        &args.common.control,
        "disposition.renew",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let (record, state) = load_card(control, &card_id)?;
            let config = control.project()?;
            let policy_digest = pure_recheck(require_renewable(
                control,
                &config,
                &record,
                dimension,
                &args.common.actor,
                &args.rationale,
            ))?;

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

/// #142: distinct call site from the parse below, so the read failure needs
/// no introspection to tell apart from a schema failure. Mirrors
/// `CONVERGENCE_POLICY_READ_RECOVERY` in `src/commands/project.rs` — same
/// duplication this function's own doc comment already flags, kept in step
/// for the same reason.
const CONVERGENCE_POLICY_READ_RECOVERY: &str = "This is a read failure, not a syntax problem: the convergence policy file above could not be opened. Confirm the path exists, is spelled correctly, and is readable by this process.";

/// Mirrors `CONVERGENCE_POLICY_SCHEMA_RECOVERY` in `src/commands/project.rs`.
const CONVERGENCE_POLICY_SCHEMA_RECOVERY: &str = "This convergence policy is valid JSON but does not match the schema; the message above names the missing or invalid field. Compare it against `project example`'s output, a complete, valid convergence policy.";

/// The syntax-failure sibling of [`CONVERGENCE_POLICY_SCHEMA_RECOVERY`].
const CONVERGENCE_POLICY_SYNTAX_RECOVERY: &str = "This convergence policy is not valid JSON; the message above names the exact line and column to fix.";

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
    let raw = fs::read_to_string(path).map_err(|source| HarnessError::ControlWithRecovery {
        reason: format!(
            "cannot read convergence policy {}: {source}",
            path.display()
        ),
        code: ErrorCode::ConfigMalformed,
        recovery: CONVERGENCE_POLICY_READ_RECOVERY,
    })?;
    let policy: ConvergencePolicy = serde_json::from_str(&raw).map_err(|source| {
        let recovery = if source.is_data() {
            CONVERGENCE_POLICY_SCHEMA_RECOVERY
        } else {
            CONVERGENCE_POLICY_SYNTAX_RECOVERY
        };
        HarnessError::ControlWithRecovery {
            reason: format!("convergence policy is malformed: {source}"),
            code: ErrorCode::ConfigMalformed,
            recovery,
        }
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
        HarnessError::ControlWithRecovery {
            reason: "final authorization is not configured for this project; explicitly configure final_authorization_policy before authorizing a convergence policy rebaseline".to_owned(),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_AUTHORIZATION_POLICY_NOT_CONFIGURED_RECOVERY,
        }
    })?;
    if !authorization.authorizes(actor) {
        return Err(HarnessError::ControlWithRecovery {
            reason: format!(
                "actor {actor} is not configured to authorize a convergence policy rebaseline"
            ),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_AUTHORIZATION_ACTOR_NOT_AUTHORIZED_RECOVERY,
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
        HarnessError::ControlWithRecovery {
            reason: "final authorization is not configured for this project; explicitly configure final_authorization_policy before authorizing a convergence disposition abandon".to_owned(),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_AUTHORIZATION_POLICY_NOT_CONFIGURED_RECOVERY,
        }
    })?;
    if !authorization.authorizes(actor) {
        return Err(HarnessError::ControlWithRecovery {
            reason: format!(
                "actor {actor} is not configured to authorize a convergence disposition abandon"
            ),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_AUTHORIZATION_ACTOR_NOT_AUTHORIZED_RECOVERY,
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

/// Whether `counters` already carries a waived escalation on `dimension`,
/// however it was waived.
///
/// Reads the raw per-dimension `escalation_waived` flag rather than going
/// through `assess_card`: once an escalation is waived, `assess_card` never
/// reports that dimension exhausted again (see its own doc comment), so its
/// output cannot tell "this dimension was never exhausted" apart from
/// "this dimension was exhausted, and its escalation was already waived" —
/// and both `require_risk_acceptable`'s own check 3 and
/// `require_splittable`'s own check 3 need exactly that distinction. One
/// shared helper, not two near-identical copies, because both checks are
/// the same question asked by two different commands: `accept-risk` and
/// `split` waive the very same flag, so whether it is already waived does
/// not depend on which of the two is asking. Kept as its own match rather
/// than shared with `project`'s identically-shaped one: that function's
/// copy operates on `&mut` references while folding a fact, and
/// `policy::convergence` is not this card's file scope to widen.
fn escalation_already_waived(counters: &CardCounters, dimension: CardDimension) -> bool {
    match dimension {
        CardDimension::ReviewReturns => counters.review_returns.escalation_waived,
        CardDimension::RepairAttempts => counters.repair_attempts.escalation_waived,
        CardDimension::GateFailures => counters.gate_failures.escalation_waived,
        CardDimension::MaterialScopeRevisions => {
            counters.material_scope_revisions.escalation_waived
        }
    }
}

/// Runs every check `disposition accept-risk` must satisfy before it writes
/// anything, in the fixed order #74-6's contract requires, and returns the
/// configured convergence policy's digest on success — the exact value the
/// recorded fact must bind to.
///
/// Shared between the real command and its `--dry-run` preview, so neither
/// can promise or refuse something the other disagrees with.
///
/// # Errors
///
/// Returns [`ErrorCode::PolicyInvalidTransition`] when there is no
/// convergence policy configured, when this dimension already carries an
/// accepted risk, when the card is not currently escalated, or when the
/// named dimension is not among the ones that are exhausted. Returns
/// [`ErrorCode::PolicyNotAccepted`] when no final-authorization policy is
/// configured, or when the acting actor is not in its authorized set.
/// Returns [`ErrorCode::UsageInvalidArguments`] when `risk` or `rationale`
/// is blank. Propagates a control-read or projection failure unchanged.
fn require_risk_acceptable(
    control: &ControlRepository,
    config: &ProjectConfig,
    record: &CardRecord,
    dimension: CardDimension,
    actor: &str,
    risk: &str,
    rationale: &str,
) -> Result<Digest, HarnessError> {
    // Check 1: a convergence policy must be configured at all. With none,
    // no card carries a budget in the first place (`assess_card` answers
    // `LegacyUnassessed` for every card), so there is no exhausted
    // dimension whose risk could ever be accepted.
    let Some(policy) = config.convergence_policy.as_ref() else {
        return Err(HarnessError::Control {
            reason: format!(
                "card {} cannot have a risk accepted: no convergence policy is configured for this project, so no dimension can be exhausted",
                record.card_id
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    };

    // Checks 2, 3, 4, and 5 all read the same fresh projection and
    // assessment: the same `EventStore::new(control).for_cycle(...)` →
    // `project(...)` → `assess_card(...)` sequence `require_renewable` and
    // `require_abandonable` already use, so this command can never
    // disagree with the checks it exists to release a card from. Both the
    // raw projected `view` and the derived `convergence` assessment are
    // kept: check 3 needs the former (`assess_card`'s own output cannot
    // distinguish "never exhausted" from "exhausted, then accepted" — see
    // `escalation_already_waived`), and checks 4 and 5 need the latter.
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

    // Check 3: this dimension must not already carry an accepted risk.
    // Checked *before* checks 4 and 5, on purpose: once a risk is accepted,
    // `assess_card` never reports that dimension exhausted again, so a
    // repeated acceptance would otherwise fall through to check 4 or 5 and
    // be refused with "the card is not escalated" or "that dimension still
    // has budget" — both true-sounding, and both misleading about what
    // actually happened. This check names the earlier acceptance instead.
    let already_accepted = match &view {
        ProjectConvergence::Configured(projection) => projection
            .cards
            .get(&record.card_id)
            .is_some_and(|counters| escalation_already_waived(counters, dimension)),
        ProjectConvergence::LegacyUnassessed => false,
    };
    if already_accepted {
        return Err(HarnessError::Control {
            reason: format!(
                "card {} already has an accepted risk on `{}`; a dimension's risk can only be accepted once",
                record.card_id,
                dimension_wire_name(dimension)
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    }

    // Check 4: the card must be escalated *right now* — the same "response
    // to escalation, not a blank cheque" property `require_renewable`'s
    // own check 2 documents.
    let CardConvergence::Escalated { exhausted, .. } = &convergence else {
        return Err(HarnessError::Control {
            reason: format!(
                "card {} is not currently escalated; there is no exhausted risk to accept",
                record.card_id
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    };

    // Check 5: the *named* dimension specifically must be one of the
    // exhausted ones — the same reasoning as `require_renewable`'s own
    // check 3: a dimension that still has budget cannot have its risk
    // pre-accepted, because that is silent expansion by another name. The
    // refusal names every dimension that really is exhausted.
    if !exhausted.iter().any(|item| item.dimension == dimension) {
        let actually_exhausted = exhausted
            .iter()
            .map(|item| dimension_wire_name(item.dimension))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(HarnessError::Control {
            reason: format!(
                "card {} cannot pre-accept risk on `{}`: that dimension still has budget; the exhausted dimension(s) are: {actually_exhausted}",
                record.card_id,
                dimension_wire_name(dimension)
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    }

    // Check 6: the actor must be authorized, by the same
    // `final_authorization_policy.authorizer_actor_ids` path
    // `require_renewable`, `require_rebaseline`, and `require_abandonable`
    // already resolve it through — see `require_renewable`'s own long note
    // for why this set is reused rather than given its own field on
    // `ConvergencePolicy`.
    let authorization = config.final_authorization_policy.as_ref().ok_or_else(|| {
        HarnessError::ControlWithRecovery {
            reason: "final authorization is not configured for this project; explicitly configure final_authorization_policy before authorizing a convergence risk acceptance".to_owned(),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_AUTHORIZATION_POLICY_NOT_CONFIGURED_RECOVERY,
        }
    })?;
    if !authorization.authorizes(actor) {
        return Err(HarnessError::ControlWithRecovery {
            reason: format!(
                "actor {actor} is not configured to authorize a convergence risk acceptance"
            ),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_AUTHORIZATION_ACTOR_NOT_AUTHORIZED_RECOVERY,
        });
    }

    // Check 7: `--risk` and `--rationale` are refused separately, with
    // distinct messages, even though both land on the same error code —
    // deliberately, so an operator who left one blank does not have to
    // guess which. `--risk` is checked first: it is *what* is being
    // accepted, without which "disclosed risk" means nothing, and reads
    // naturally as the first thing an operator would fix.
    if risk.trim().is_empty() {
        return Err(HarnessError::Control {
            reason: "disposition accept-risk requires a non-blank --risk; an accepted risk with no declared disclosure cannot be recorded".to_owned(),
            code: ErrorCode::UsageInvalidArguments,
        });
    }
    if rationale.trim().is_empty() {
        return Err(HarnessError::Control {
            reason: "disposition accept-risk requires a non-blank --rationale; an accepted risk with no declared reason cannot be recorded".to_owned(),
            code: ErrorCode::UsageInvalidArguments,
        });
    }

    policy.digest()
}

/// Reports what `disposition accept-risk` would bind, without binding it.
///
/// Runs every check the real command makes, through the same
/// [`require_risk_acceptable`], so a caller can never be told an
/// acceptance would succeed (or told why it would fail) and then have the
/// real command disagree.
fn preview_accept_risk(
    args: &AcceptRiskArgs,
    card_id: &CardId,
    dimension: CardDimension,
) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.common.control)?;
    let (record, _state) = load_card(&control, card_id)?;
    let config = control.project()?;
    let policy_digest = require_risk_acceptable(
        &control,
        &config,
        &record,
        dimension,
        &args.common.actor,
        &args.risk,
        &args.rationale,
    )?;
    Ok(CommandOutcome::new(
        "disposition.accept-risk",
        format!(
            "Dry run: would accept risk on card {card_id}'s {} dimension; nothing was changed",
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

fn run_accept_risk(
    args: &AcceptRiskArgs,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;
    let dimension: CardDimension = args.dimension.into();

    if args.dry_run {
        return preview_accept_risk(args, &card_id, dimension);
    }

    with_transaction(
        &args.common.control,
        "disposition.accept-risk",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let (record, state) = load_card(control, &card_id)?;
            let config = control.project()?;
            let policy_digest = require_risk_acceptable(
                control,
                &config,
                &record,
                dimension,
                &args.common.actor,
                &args.risk,
                &args.rationale,
            )?;

            // `head` binds to the current revision's own `base_sha` — the
            // same binding `renew` and `abandon` use, and for the same
            // reason: it is the only exact commit SHA an escalated card is
            // guaranteed to carry in any lifecycle state. No
            // `store_card_state` call and no `.transition(...)` on this
            // event: unlike `abandon`, an acceptance moves no card state —
            // the card simply becomes deliverable again where it stands.
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
                    .meta(
                        "disposition",
                        serde_json::to_value(DispositionKind::AcceptRisk)?,
                    )
                    .meta("dimension", serde_json::to_value(dimension)?)
                    .meta("risk", serde_json::json!(args.risk))
                    .meta("rationale", serde_json::json!(args.rationale))
                    .meta("authorized_by", serde_json::json!(args.common.actor))
                    .meta("policy_digest", serde_json::json!(policy_digest.as_str())),
                clock,
            )?;

            control.commit(
                expected,
                &format!(
                    "disposition: accept-risk {card_id} {}",
                    dimension_wire_name(dimension)
                ),
            )?;

            Ok(CommandOutcome::new(
                "disposition.accept-risk",
                format!(
                    "Accepted risk on card {card_id}'s {} dimension\nrisk: {}\nrationale: {}\nauthorized by: {}",
                    dimension_wire_name(dimension),
                    args.risk,
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

/// Runs every check `disposition split` must satisfy before it writes
/// anything, in the fixed order #74-7's contract requires, and returns the
/// configured convergence policy's digest on success — the exact value the
/// recorded fact must bind to.
///
/// Shared between the real command and its `--dry-run` preview, so neither
/// can promise or refuse something the other disagrees with.
///
/// # Errors
///
/// Returns [`ErrorCode::PolicyInvalidTransition`] when there is no
/// convergence policy configured, when this dimension's escalation is
/// already waived, when the card is not currently escalated, when the
/// named dimension is not among the ones that are exhausted, when the
/// follow-up card is the card being split, when the follow-up card is in a
/// different cycle, or when the follow-up card is terminal. Returns
/// whatever [`load_card`] returns when the follow-up card cannot be
/// loaded — ordinarily [`ErrorCode::PreconditionNotFound`] for one that was
/// never activated. Returns [`ErrorCode::PolicyNotAccepted`] when no
/// final-authorization policy is configured, or when the acting actor is
/// not in its authorized set. Returns [`ErrorCode::UsageInvalidArguments`]
/// when `rationale` is blank. Propagates a control-read or projection
/// failure unchanged.
// Eleven checks, each with its own explanatory comment, run past the
// default line limit — the same reason `project`, in `policy::convergence`,
// and `run_status`, in `card.rs`, both carry this same allow.
#[allow(clippy::too_many_lines)]
fn require_splittable(
    control: &ControlRepository,
    config: &ProjectConfig,
    record: &CardRecord,
    dimension: CardDimension,
    follow_up_card_id: &CardId,
    actor: &str,
    rationale: &str,
) -> Result<Digest, HarnessError> {
    // Check 1: a convergence policy must be configured at all. With none,
    // no card carries a budget in the first place (`assess_card` answers
    // `LegacyUnassessed` for every card), so there is no exhausted
    // dimension whose work could ever be split off.
    let Some(policy) = config.convergence_policy.as_ref() else {
        return Err(HarnessError::Control {
            reason: format!(
                "card {} cannot be split: no convergence policy is configured for this project, so no dimension can be exhausted",
                record.card_id
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    };

    // Checks 2, 3, 4, and 5 all read the same fresh projection and
    // assessment — the same `EventStore::new(control).for_cycle(...)` →
    // `project(...)` → `assess_card(...)` sequence every sibling disposition
    // above already uses — so this command can never disagree with the
    // checks it exists to release a card from. Both the raw projected
    // `view` and the derived `convergence` assessment are kept, for exactly
    // the reason `require_risk_acceptable` keeps both: check 3 needs the
    // former (`assess_card`'s own output cannot distinguish "never
    // exhausted" from "exhausted, then waived" — see
    // `escalation_already_waived`), and checks 4 and 5 need the latter.
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

    // Check 3: this dimension's escalation must not already be waived —
    // by an earlier split *or* an earlier accept-risk, since both set the
    // very same flag and this check cannot tell which. Checked *before*
    // checks 4 and 5, for the same reason `require_risk_acceptable` checks
    // its own equivalent there: once a dimension's escalation is waived,
    // `assess_card` never reports it exhausted again, so a repeat would
    // otherwise fall through to check 4 or 5 and be refused with "the card
    // is not escalated" or "that dimension still has budget" — both
    // true-sounding, and both misleading about what actually happened.
    // This check names the earlier waiver instead.
    let already_waived = match &view {
        ProjectConvergence::Configured(projection) => projection
            .cards
            .get(&record.card_id)
            .is_some_and(|counters| escalation_already_waived(counters, dimension)),
        ProjectConvergence::LegacyUnassessed => false,
    };
    if already_waived {
        return Err(HarnessError::Control {
            reason: format!(
                "card {} already has its `{}` escalation waived; a dimension's escalation can only be waived once",
                record.card_id,
                dimension_wire_name(dimension)
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    }

    // Check 4: the card must be escalated *right now* — the same "response
    // to escalation, not a blank cheque" property `require_renewable`'s own
    // check 2 documents.
    let CardConvergence::Escalated { exhausted, .. } = &convergence else {
        return Err(HarnessError::Control {
            reason: format!(
                "card {} is not currently escalated; there is nothing to split",
                record.card_id
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    };

    // Check 5: the *named* dimension specifically must be one of the
    // exhausted ones — the same reasoning as `require_renewable`'s own
    // check 3 and `require_risk_acceptable`'s own check 5: a dimension that
    // still has budget cannot have its remaining work split off, because
    // that is silent expansion by another name. The refusal names every
    // dimension that really is exhausted.
    if !exhausted.iter().any(|item| item.dimension == dimension) {
        let actually_exhausted = exhausted
            .iter()
            .map(|item| dimension_wire_name(item.dimension))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(HarnessError::Control {
            reason: format!(
                "card {} cannot split `{}`: that dimension still has budget; the exhausted dimension(s) are: {actually_exhausted}",
                record.card_id,
                dimension_wire_name(dimension)
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    }

    // Check 6: the follow-up card must exist — loaded through the very same
    // `load_card` every sibling disposition above uses to load the card
    // being acted on, so a missing or never-activated follow-up card is
    // refused exactly the way a missing or never-activated card being
    // renewed, abandoned, or risk-accepted would be, rather than through a
    // second, hand-rolled existence check.
    let (follow_up_record, follow_up_state) = load_card(control, follow_up_card_id)?;

    // Check 7: a card cannot be its own follow-up. Binding a card to itself
    // would record a disposition that resolves nothing and reads, in the
    // audit trail, as work moved somewhere when it moved nowhere.
    if follow_up_card_id == &record.card_id {
        return Err(HarnessError::Control {
            reason: format!(
                "card {} cannot be its own follow-up; split moves work to a different, already-existing card",
                record.card_id
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    }

    // Check 8: the follow-up card must be in the same cycle as the
    // original. Cross-cycle follow-up is a real workflow but a different
    // decision with different rules (#24, cross-cycle ownership); this card
    // does not open it. Both cycles are named plainly, so an operator is
    // told the constraint rather than left guessing which card, or which
    // cycle, is the odd one out.
    if follow_up_record.cycle_id != record.cycle_id {
        return Err(HarnessError::Control {
            reason: format!(
                "card {} is in cycle {}, but follow-up card {follow_up_card_id} is in cycle {}; split only binds a follow-up card in the same cycle as the original — cross-cycle follow-up is a different decision with its own rules (#24)",
                record.card_id, record.cycle_id, follow_up_record.cycle_id
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    }

    // Check 9: the follow-up card must not be terminal. Moving work onto a
    // closed or abandoned card silently discards it: nobody would ever
    // work that card again, so the "moved work" the recorded fact claims
    // happened would in fact go nowhere.
    if follow_up_state.state.is_terminal() {
        return Err(HarnessError::Control {
            reason: format!(
                "follow-up card {follow_up_card_id} is {}; a terminal card cannot receive work moved by a split",
                follow_up_state.state.name()
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    }

    // Check 10: the actor must be authorized, by the same
    // `final_authorization_policy.authorizer_actor_ids` path every sibling
    // disposition above resolves it through — see `require_renewable`'s own
    // long note for why this set is reused rather than given its own field
    // on `ConvergencePolicy`.
    let authorization = config.final_authorization_policy.as_ref().ok_or_else(|| {
        HarnessError::ControlWithRecovery {
            reason: "final authorization is not configured for this project; explicitly configure final_authorization_policy before authorizing a convergence split".to_owned(),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_AUTHORIZATION_POLICY_NOT_CONFIGURED_RECOVERY,
        }
    })?;
    if !authorization.authorizes(actor) {
        return Err(HarnessError::ControlWithRecovery {
            reason: format!("actor {actor} is not configured to authorize a convergence split"),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_AUTHORIZATION_ACTOR_NOT_AUTHORIZED_RECOVERY,
        });
    }

    // Check 11: a disposition with no declared reason is not a decision.
    if rationale.trim().is_empty() {
        return Err(HarnessError::Control {
            reason: "disposition split requires a non-blank --rationale; a split with no declared reason cannot be recorded".to_owned(),
            code: ErrorCode::UsageInvalidArguments,
        });
    }

    policy.digest()
}

/// Reports what `disposition split` would bind, without binding it.
///
/// Runs every check the real command makes, through the same
/// [`require_splittable`], so a caller can never be told a split would
/// succeed (or told why it would fail) and then have the real command
/// disagree.
fn preview_split(
    args: &SplitArgs,
    card_id: &CardId,
    dimension: CardDimension,
    follow_up_card_id: &CardId,
) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.common.control)?;
    let (record, _state) = load_card(&control, card_id)?;
    let config = control.project()?;
    let policy_digest = require_splittable(
        &control,
        &config,
        &record,
        dimension,
        follow_up_card_id,
        &args.common.actor,
        &args.rationale,
    )?;
    Ok(CommandOutcome::new(
        "disposition.split",
        format!(
            "Dry run: would split card {card_id}'s {} work to follow-up card {follow_up_card_id}; nothing was changed",
            dimension_wire_name(dimension)
        ),
        serde_json::json!({
            "dry_run": true,
            "card_id": card_id.to_string(),
            "dimension": dimension_wire_name(dimension),
            "follow_up_card_id": follow_up_card_id.to_string(),
            "policy_digest": policy_digest.as_str(),
        }),
    ))
}

fn run_split(args: &SplitArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;
    let dimension: CardDimension = args.dimension.into();
    let follow_up_card_id: CardId = args.follow_up_card_id.parse()?;

    if args.dry_run {
        return preview_split(args, &card_id, dimension, &follow_up_card_id);
    }

    with_transaction(
        &args.common.control,
        "disposition.split",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let (record, state) = load_card(control, &card_id)?;
            let config = control.project()?;
            let policy_digest = require_splittable(
                control,
                &config,
                &record,
                dimension,
                &follow_up_card_id,
                &args.common.actor,
                &args.rationale,
            )?;

            // `head` binds to the current revision's own `base_sha` — the
            // same binding every sibling disposition above uses, and for
            // the same reason: it is the only exact commit SHA an
            // escalated card is guaranteed to carry in any lifecycle
            // state. No `store_card_state` call and no `.transition(...)`
            // on this event, and nothing at all is written to the
            // follow-up card: a split moves no card state, for either
            // card. The follow-up card is named, in `follow_up_card_id`
            // below, not mutated — the binding lives entirely in this one
            // record, because mutating a second card inside a disposition
            // would put card lifecycle changes in two places instead of
            // one.
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
                    .meta("disposition", serde_json::to_value(DispositionKind::Split)?)
                    .meta("dimension", serde_json::to_value(dimension)?)
                    .meta(
                        "follow_up_card_id",
                        serde_json::json!(follow_up_card_id.to_string()),
                    )
                    .meta("rationale", serde_json::json!(args.rationale))
                    .meta("authorized_by", serde_json::json!(args.common.actor))
                    .meta("policy_digest", serde_json::json!(policy_digest.as_str())),
                clock,
            )?;

            control.commit(
                expected,
                &format!("disposition: split {card_id} -> {follow_up_card_id}"),
            )?;

            Ok(CommandOutcome::new(
                "disposition.split",
                format!(
                    "Split card {card_id}'s {} work to follow-up card {follow_up_card_id}\nrationale: {}\nauthorized by: {}",
                    dimension_wire_name(dimension),
                    args.rationale,
                    args.common.actor
                ),
                serde_json::json!({
                    "card_id": card_id.to_string(),
                    "dimension": dimension_wire_name(dimension),
                    "follow_up_card_id": follow_up_card_id.to_string(),
                    "event_id": event.event_id.to_string(),
                    "policy_digest": policy_digest.as_str(),
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}

/// Runs every check `disposition redesign` must satisfy before it writes
/// anything, in the fixed order #74-8's contract requires, and returns the
/// configured convergence policy's digest on success — the exact value the
/// recorded fact must bind to, matching every sibling disposition's own
/// check function.
///
/// Shared between the real command and its `--dry-run` preview, so neither
/// can promise or refuse something the other disagrees with.
///
/// # Errors
///
/// Returns [`ErrorCode::PolicyInvalidTransition`] when there is no
/// convergence policy configured, when the card is not currently escalated,
/// when its current lifecycle state does not permit the transition to
/// `CardState::Abandoned` — whatever [`CardState::check_transition`] itself
/// returns for that — when the replacement card is the card being
/// redesigned, or when the replacement card is terminal. Returns whatever
/// [`load_card`] returns when the replacement card cannot be loaded —
/// ordinarily [`ErrorCode::PreconditionNotFound`] for one that was never
/// activated. Returns [`ErrorCode::PolicyNotAccepted`] when no
/// final-authorization policy is configured, or when the acting actor is
/// not in its authorized set. Returns [`ErrorCode::UsageInvalidArguments`]
/// when `rationale` is blank. Propagates a control-read or projection
/// failure unchanged.
fn require_redesignable(
    control: &ControlRepository,
    config: &ProjectConfig,
    record: &CardRecord,
    current_state: CardState,
    replacement_card_id: &CardId,
    actor: &str,
    rationale: &str,
) -> Result<Digest, HarnessError> {
    // Check 1: a convergence policy must be configured at all. With none, no
    // card carries a budget in the first place (`assess_card` answers
    // `LegacyUnassessed` for every card), so no card can ever be escalated,
    // and there is no digest to bind the recorded fact to. Identical to
    // `require_abandonable`'s own check 1: a redesign, like an abandon,
    // terminates the card outright, so both need exactly the same escape
    // hatch when there is no budget to have escalated it in the first
    // place.
    let Some(policy) = config.convergence_policy.as_ref() else {
        return Err(HarnessError::Control {
            reason: format!(
                "card {} cannot be redesigned: no convergence policy is configured for this project, so no card can be escalated",
                record.card_id
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    };

    // Check 2: the card must be escalated *right now* — read exactly the
    // way `require_abandonable`'s own check 2 reads it: a fresh
    // `EventStore::new(control).for_cycle(...)` projected and assessed, so
    // this command can never disagree with the check it exists to release a
    // card from. `disposition redesign` is an authorized *escalation exit*,
    // like `disposition abandon`, not a general-purpose way to end a card —
    // a card that is `Within` is refused here and pointed at `card abandon`,
    // the ordinary route that already covers it, so an operator refused
    // here is never left with nowhere to go.
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
                "card {} is not currently escalated; `disposition redesign` only ends an active escalation — to abandon a card that is not escalated, use `card abandon` instead",
                record.card_id
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    };

    // Check 3: the card's lifecycle state must permit the transition to
    // `Abandoned`. This is where "a repeated disposition refuses" comes
    // from, exactly as it does for `abandon`, and it comes for free:
    // `Abandoned` has no successors, so a second `disposition redesign`
    // against the same card reaches this exact check and is refused by it.
    // An already-redesigned card's recorded facts do not disappear, so it
    // still assesses as `Escalated` and still passes check 2 above every
    // time — it is this check, not check 2, that stops a repeat.
    current_state.check_transition(CardState::Abandoned)?;

    // Check 4: the replacement card must exist — loaded through the very
    // same `load_card` every sibling disposition uses to load the card
    // being acted on, and the way `require_splittable`'s own check 6 loads
    // its follow-up card, so a missing or never-activated replacement is
    // refused exactly the way a missing or never-activated card being
    // renewed, abandoned, or split to would be, rather than through a
    // second, hand-rolled existence check. The loaded record is discarded
    // here: this function returns only the policy digest, matching every
    // sibling disposition's own check function, so `replacement_cycle_id`
    // — needed only for the fact `run_redesign` is about to write, not for
    // any check — is read again there, fresh, once every check below has
    // passed.
    let (_replacement_record, replacement_state) = load_card(control, replacement_card_id)?;

    // Check 5: a card cannot replace itself. Naming a card as its own
    // replacement would record a decision that resolves nothing and reads,
    // in the audit trail, as work continuing somewhere when it moved
    // nowhere — the same reasoning `require_splittable`'s own check 7
    // applies to a follow-up card.
    if replacement_card_id == &record.card_id {
        return Err(HarnessError::Control {
            reason: format!(
                "card {} cannot be its own replacement; redesign points at a different, already-existing card",
                record.card_id
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    }

    // Check 6: the replacement card must not be terminal. Naming a closed
    // or abandoned card as the successor records a continuation that
    // cannot continue — the same reasoning `require_splittable`'s own
    // check 9 applies to a follow-up card, and for the same reason: nobody
    // would ever work a terminal card again.
    if replacement_state.state.is_terminal() {
        return Err(HarnessError::Control {
            reason: format!(
                "replacement card {replacement_card_id} is {}; a terminal card cannot be named as a redesign's replacement",
                replacement_state.state.name()
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    }

    // Check 7: the actor must be authorized, by the same
    // `final_authorization_policy.authorizer_actor_ids` path every sibling
    // disposition above resolves it through — see `require_renewable`'s
    // own long note for why this set is reused rather than given its own
    // field on `ConvergencePolicy`.
    let authorization = config.final_authorization_policy.as_ref().ok_or_else(|| {
        HarnessError::ControlWithRecovery {
            reason: "final authorization is not configured for this project; explicitly configure final_authorization_policy before authorizing a convergence redesign".to_owned(),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_AUTHORIZATION_POLICY_NOT_CONFIGURED_RECOVERY,
        }
    })?;
    if !authorization.authorizes(actor) {
        return Err(HarnessError::ControlWithRecovery {
            reason: format!("actor {actor} is not configured to authorize a convergence redesign"),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_AUTHORIZATION_ACTOR_NOT_AUTHORIZED_RECOVERY,
        });
    }

    // Check 8: a disposition with no declared reason is not a decision.
    if rationale.trim().is_empty() {
        return Err(HarnessError::Control {
            reason: "disposition redesign requires a non-blank --rationale; a redesign with no declared reason cannot be recorded".to_owned(),
            code: ErrorCode::UsageInvalidArguments,
        });
    }

    policy.digest()
}

/// Reports what `disposition redesign` would bind, without binding it.
///
/// Runs every check the real command makes, through the same
/// [`require_redesignable`], so a caller can never be told a redesign
/// would succeed (or told why it would fail) and then have the real
/// command disagree.
fn preview_redesign(
    args: &RedesignArgs,
    card_id: &CardId,
    replacement_card_id: &CardId,
) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.common.control)?;
    let (record, state) = load_card(&control, card_id)?;
    let config = control.project()?;
    let policy_digest = require_redesignable(
        &control,
        &config,
        &record,
        state.state,
        replacement_card_id,
        &args.common.actor,
        &args.rationale,
    )?;
    // Read once more, purely to report the replacement's cycle in the
    // preview — see `require_redesignable`'s own note on check 4 for why
    // this is a fresh read rather than data threaded out of that function.
    let (replacement_record, _replacement_state) = load_card(&control, replacement_card_id)?;
    Ok(CommandOutcome::new(
        "disposition.redesign",
        format!(
            "Dry run: would redesign card {card_id}, naming replacement {replacement_card_id}; nothing was changed"
        ),
        serde_json::json!({
            "dry_run": true,
            "card_id": card_id.to_string(),
            "replacement_card_id": replacement_card_id.to_string(),
            "replacement_cycle_id": replacement_record.cycle_id.to_string(),
            "policy_digest": policy_digest.as_str(),
        }),
    ))
}

fn run_redesign(args: &RedesignArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;
    let replacement_card_id: CardId = args.replacement_card_id.parse()?;

    if args.dry_run {
        return preview_redesign(args, &card_id, &replacement_card_id);
    }

    with_transaction(
        &args.common.control,
        "disposition.redesign",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let (record, state) = load_card(control, &card_id)?;
            let previous = state.state;
            let config = control.project()?;
            let policy_digest = require_redesignable(
                control,
                &config,
                &record,
                previous,
                &replacement_card_id,
                &args.common.actor,
                &args.rationale,
            )?;

            // Loaded again, now that every check has passed: see
            // `require_redesignable`'s own note on check 4 for why
            // `replacement_cycle_id` — needed only for the fact below, not
            // for any check — is read here rather than threaded out of
            // that function. Safe to re-read: nothing else touches the
            // control repository between the two reads inside this one
            // transaction.
            let (replacement_record, _replacement_state) =
                load_card(control, &replacement_card_id)?;

            store_card_state(control, &record, &state, CardState::Abandoned)?;

            // `head` binds to the current revision's own `base_sha` — the
            // same binding every sibling disposition above uses, and for
            // the same reason: it is the only exact commit SHA an
            // escalated card is guaranteed to carry in any lifecycle
            // state. The replacement card is named, in
            // `replacement_card_id` and `replacement_cycle_id` below, not
            // mutated — the same posture `split` takes toward its own
            // follow-up card, for the same reason: mutating a second card
            // inside a disposition would put card lifecycle changes in two
            // places instead of one.
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
                        serde_json::to_value(DispositionKind::Redesign)?,
                    )
                    .meta(
                        "replacement_card_id",
                        serde_json::json!(replacement_card_id.to_string()),
                    )
                    .meta(
                        "replacement_cycle_id",
                        serde_json::json!(replacement_record.cycle_id.to_string()),
                    )
                    .meta("rationale", serde_json::json!(args.rationale))
                    .meta("authorized_by", serde_json::json!(args.common.actor))
                    .meta("policy_digest", serde_json::json!(policy_digest.as_str())),
                clock,
            )?;

            control.commit(
                expected,
                &format!("disposition: redesign {card_id} -> {replacement_card_id}"),
            )?;

            Ok(CommandOutcome::new(
                "disposition.redesign",
                format!(
                    "Redesigned card {card_id}, replaced by {replacement_card_id}\nrationale: {}\nauthorized by: {}",
                    args.rationale, args.common.actor
                ),
                serde_json::json!({
                    "card_id": card_id.to_string(),
                    "state": CardState::Abandoned.name(),
                    "replacement_card_id": replacement_card_id.to_string(),
                    "replacement_cycle_id": replacement_record.cycle_id.to_string(),
                    "event_id": event.event_id.to_string(),
                    "policy_digest": policy_digest.as_str(),
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}
