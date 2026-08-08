//! Recording the decision that authorizes promotion.
//!
//! Section 10.9 makes acceptance the single gate on moving the protected
//! branch. Everything upstream establishes facts; this is where a named owner
//! takes responsibility for what those facts add up to.

use std::fs;

use clap::{Args, Subcommand};

use crate::{
    cli::output::CommandOutcome,
    commands::CONTROL_ENV,
    commands::{
        card::{load_card, store_card_state},
        integration::{
            load_cycle, load_integration, load_verification, member_implementers,
            require_cycle_convergence_budget, require_no_pending_exception, require_plan_binding,
            status_gate_refusal,
        },
        transaction::with_transaction,
    },
    control::{event_store::EventDraft, repository::ControlRepository},
    domain::{
        acceptance::{
            ACCEPTANCE_DIR, ACCEPTANCE_SCHEMA, ACCEPTANCE_V2_SCHEMA, AcceptanceDecision,
            AcceptanceRecord,
        },
        card::CardState,
        clock::Clock,
        digest::CANONICAL_ALGORITHM,
        ids::{AcceptanceId, IntegrationId},
        integration::{IntegrationRecord, IntegrationStatus},
    },
    error::{ErrorCode, HarnessError},
    policy::actors,
};

// ---------------------------------------------------------------------
// #179: `ErrorCode::PolicyNotAccepted`'s per-site recovery text.
//
// Every one of the sites #179 §2 counted shared one fallback string
// (`src/error.rs`'s `PolicyNotAccepted` table entry) whose advice —
// "Record an acceptance for this exact landing commit before
// promoting." — was the thing the reproducing operator had just
// attempted. #179 §7 asks for the situations to be established before
// any text is written: not one bespoke string per site, and not one
// string standing in for all of them (the original defect, just with
// more call sites).
//
// Reading every real construction site in this file, `disposition.rs`,
// and `integration.rs` (re-measured at 29, not the 36 the card's own §2
// counted — see the evidence report for where that count went wrong)
// sorts them into five situations, by what an operator would actually
// have to do differently, not by which file or which specific command
// triggered the refusal:
//
// 1. **No policy at all** (11 sites): `final_authorization_policy` was
//    never configured, or was configured once and is gone by the time a
//    later recheck runs. The fix is the same regardless of which
//    command tripped over the absence: install one.
// 2. **This actor is not on the list** (8 sites, including B2): a
//    policy exists, but the acting actor is not among
//    `authorizer_actor_ids`. #179 §8's own requirement for B2 — name
//    `final_authorization_policy.authorizer_actor_ids` and
//    `project example-final-authorization` — generalizes cleanly to
//    every site in this group, `disposition.rs`'s six included: the
//    field an operator needs to check does not change with the
//    operation being authorized.
// 3. **This integration's own binding is stale, before any decision is
//    recorded** (2 sites): the cycle was resealed after
//    `integration prepare --final` last captured its digest, so the
//    integration record itself — not any acceptance or exception — no
//    longer matches the cycle it claims to bind. Recording a decision
//    is exactly what is being refused here, so "record one" would be
//    circular; the fix is to re-prepare.
// 4. **An existing decision no longer covers the current state** (7
//    sites): an acceptance or exception was validly recorded once, but
//    the landing commit, the plan, or the policy has since moved, or
//    the recorded decision was itself a rejection. Unlike group 3,
//    something *was* already decided — the fix is a fresh decision
//    against the state as it is now, not a re-preparation.
// 5. **This specific trigger is not enabled** (1 site): a policy exists
//    and may authorize this actor for other things, but
//    `exception_triggers` does not list the one just named. A
//    different field than group 2's, so it needs its own text naming
//    it — reusing group 2's text here would send an operator to fix
//    the wrong list, the original defect with a smaller blast radius.
//
// One of the card's own candidate axes does not actually appear: "an
// acceptance genuinely does not exist yet" describes `check_promotion`'s
// own missing-acceptance check (`src/commands/integration.rs`) in
// spirit, but that site's code is `ErrorCode::PreconditionNotFound`, not
// `PolicyNotAccepted` — out of this card's scope, and the reason the
// shared fallback's own wording ("Record an acceptance...") never
// actually matched any `PolicyNotAccepted` site to begin with.
//
// Defined here, alongside `validate_final_authorization` and
// `validate_final_authorization_for_promotion` — the two functions that
// already own every rule these five situations describe — rather than
// once per file. `disposition.rs` and `integration.rs` import what they
// need, the same way `integration.rs` already imports
// `validate_final_authorization_for_promotion` itself from this module.
// ---------------------------------------------------------------------

/// Group 1: no `final_authorization_policy` is configured for this
/// project — never installed, or installed once and gone by the time a
/// later recheck runs. 11 sites: this file's `validate_final_authorization`
/// and `validate_final_authorization_for_promotion` (one each), every one
/// of `disposition.rs`'s six authorization checks, and three in
/// `integration.rs`'s exception handling (`exceptions_for`,
/// `exception_bindings`, `validate_exception_authorizer`).
pub(crate) const FINAL_AUTHORIZATION_POLICY_NOT_CONFIGURED_RECOVERY: &str = "`final_authorization_policy` is not configured for this project (or was removed since an earlier check relied on it); run `project example-final-authorization` for a complete, valid document, then install one with `project set-final-authorization-policy`.";

/// Group 2: a policy exists, but this actor is not among its
/// `authorizer_actor_ids`. 8 sites, including B2 (#179 §8) — this file's
/// own `validate_final_authorization`, every one of `disposition.rs`'s
/// six authorization checks, and `integration.rs`'s
/// `validate_exception_authorizer`. Names
/// `final_authorization_policy.authorizer_actor_ids` and
/// `project example-final-authorization`: #179 §8's requirement for B2
/// specifically, generalized to the seven siblings that ask the
/// identical question about a different actor doing a different thing.
pub(crate) const FINAL_AUTHORIZATION_ACTOR_NOT_AUTHORIZED_RECOVERY: &str = "This actor is not among `final_authorization_policy.authorizer_actor_ids`; run `project example-final-authorization` to see a configured policy's shape, then retry as one of the listed actors or add this one with `project set-final-authorization-policy`.";

/// Group 3: this final integration's own `sealed_cycle_digest` no longer
/// matches its cycle — the cycle was resealed after
/// `integration prepare --final` last captured it. 2 sites: this file's
/// `validate_final_authorization` and `integration.rs`'s
/// `exception_bindings`, both reached *before* any acceptance or
/// exception is recorded, which is why the fix is to re-prepare rather
/// than to retry recording (recording is exactly what is being refused).
pub(crate) const FINAL_INTEGRATION_SEAL_STALE_RECOVERY: &str = "This final integration's sealed-cycle binding no longer matches the cycle; run `integration prepare --final` again for this cycle, then retry.";

/// Group 4: an acceptance or exception was validly recorded once, but
/// the landing commit, the plan, or the policy has since moved — or the
/// recorded decision was itself a rejection. 7 sites: four checks in
/// this file's `validate_final_authorization_for_promotion`, plus
/// `integration.rs`'s `exceptions_for` (one check) and `check_promotion`
/// (two checks). Unlike group 3, a decision already exists; the fix is
/// a fresh one against the state as it stands now, not a re-preparation.
pub(crate) const FINAL_AUTHORIZATION_STALE_RECOVERY: &str = "The reason above names what changed. What was recorded no longer covers the current landing commit, plan, or policy — record a fresh decision against the current state with `acceptance record`, or, for an exception, `integration exception raise`.";

/// Group 5: a policy exists and may authorize this actor for other
/// things, but `exception_triggers` does not list the one just named. 1
/// site: `integration.rs`'s `run_exception_raise`. Kept apart from
/// group 2 because the field to check is different — reusing group 2's
/// text would send an operator to fix `authorizer_actor_ids` for a
/// problem that lives in `exception_triggers`.
pub(crate) const EXCEPTION_TRIGGER_NOT_ENABLED_RECOVERY: &str = "This trigger is not among `final_authorization_policy.exception_triggers`; add it with `project set-final-authorization-policy`, or raise a trigger the policy already declares.";

/// Subcommands under `acceptance`.
#[derive(Debug, Subcommand)]
pub enum AcceptanceCommand {
    /// Record an acceptance decision over a reviewed integration.
    Record(RecordArgs),
    /// Report a recorded acceptance.
    Inspect(InspectArgs),
}

impl AcceptanceCommand {
    /// Its dotted command path, as the result envelope reports it.
    ///
    /// The error envelope used to carry only the group — `acceptance` — while a
    /// success carried the full path, so a consumer matching on `command` got a
    /// different granularity depending on whether the command worked.
    #[must_use]
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Record(..) => "acceptance.record",
            Self::Inspect(..) => "acceptance.inspect",
        }
    }
}

/// Arguments accepted by `acceptance record`.
#[derive(Debug, Args)]
pub struct RecordArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
    pub control: std::path::PathBuf,
    /// The integration being decided.
    #[arg(long)]
    pub integration_id: String,
    /// Declared configured authorizer for a final sealed cycle. Identity is
    /// declared, not proven; see D-013.
    #[arg(long, conflicts_with = "acceptance_owner")]
    pub authorizer_actor_id: Option<String>,
    /// Compatible legacy spelling for `--authorizer-actor-id`.
    #[arg(long)]
    pub acceptance_owner: Option<String>,
    /// Refuse the integration instead of accepting it.
    #[arg(long, conflicts_with = "accept")]
    pub reject: bool,
    /// Accept the integration. The default when neither flag is given.
    #[arg(long)]
    pub accept: bool,
    /// How to undo this change if it turns out badly.
    #[arg(long)]
    pub rollback_reference: Option<String>,
    /// A risk accepted along with the change. Repeatable.
    #[arg(long = "residual-risk")]
    pub residual_risks: Vec<String>,
    /// Report the decision without recording it.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `acceptance inspect`.
#[derive(Debug, Args)]
pub struct InspectArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
    pub control: std::path::PathBuf,
    /// The integration whose acceptance to report.
    #[arg(long)]
    pub integration_id: String,
}

/// Executes an `acceptance` subcommand.
///
/// # Errors
///
/// Returns a precondition or policy error as appropriate.
pub fn execute(
    command: &AcceptanceCommand,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    match command {
        AcceptanceCommand::Record(args) => run_record(args, clock),
        AcceptanceCommand::Inspect(args) => run_inspect(args),
    }
}

/// Allocates the next acceptance identifier.
fn next_acceptance_id(control: &ControlRepository) -> Result<AcceptanceId, HarnessError> {
    let directory = control.path(ACCEPTANCE_DIR);
    let highest = if directory.exists() {
        fs::read_dir(&directory)
            .map_err(|source| HarnessError::ControlIo {
                path: directory,
                source,
            })?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                entry
                    .path()
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .and_then(|stem| stem.strip_prefix("ACC-"))
                    .and_then(|digits| digits.parse::<u64>().ok())
            })
            .max()
            .unwrap_or(0)
    } else {
        0
    };
    format!("ACC-{:06}", highest + 1).parse()
}

/// The acceptance recorded for one integration, when there is one.
///
/// # Errors
///
/// Returns an error when the store cannot be read or a record is malformed.
pub fn acceptance_for(
    control: &ControlRepository,
    integration_id: &IntegrationId,
) -> Result<Option<AcceptanceRecord>, HarnessError> {
    let directory = control.path(ACCEPTANCE_DIR);
    if !directory.exists() {
        return Ok(None);
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
    // Identifiers are zero-padded and monotonic, so the last is the newest.
    names.sort();

    for name in names.iter().rev() {
        let raw = control.read(&format!("{ACCEPTANCE_DIR}/{name}.json"))?;
        let record: AcceptanceRecord =
            serde_json::from_str(&raw).map_err(|source| HarnessError::Control {
                reason: format!("acceptance {name} is malformed: {source}"),
                code: ErrorCode::InternalControlCorrupt,
            })?;
        if record.integration_id == *integration_id {
            return Ok(Some(record));
        }
    }
    Ok(None)
}

/// The decision the flags describe.
///
/// Accepting is the default because `acceptance record` is reached only after
/// a reviewed integration, but rejecting stays explicit and available: an owner
/// who wants to refuse must be able to say so in the record rather than by
/// walking away.
const fn decision_of(args: &RecordArgs) -> AcceptanceDecision {
    if args.reject {
        AcceptanceDecision::Rejected
    } else {
        AcceptanceDecision::Accepted
    }
}

fn authorizer(args: &RecordArgs) -> Result<&str, HarnessError> {
    args.authorizer_actor_id
        .as_deref()
        .or(args.acceptance_owner.as_deref())
        .or(Some("owner").filter(|_| args.authorizer_actor_id.is_none() && args.acceptance_owner.is_none()))
        .ok_or_else(|| HarnessError::Control {
            reason: "acceptance record requires --authorizer-actor-id (or compatible --acceptance-owner)".to_owned(),
            code: ErrorCode::UsageInvalidArguments,
        })
}

/// Validates the decision against stored state and reports it, recording
/// nothing.
fn preview_record(
    args: &RecordArgs,
    integration_id: &IntegrationId,
    decision: AcceptanceDecision,
) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.control)?;
    let config = control.project()?;
    let record = load_integration(&control, integration_id)?;
    require_plan_binding(&control, &record)?;
    // 73-2: the first check able to refuse, before anything else — acceptance
    // is the single gate that authorizes moving the protected branch (see
    // this module's own doc comment), so it is squarely on the path this
    // card exists to close. A preview must never promise an acceptance the
    // real command would refuse for an escalated cycle. See
    // `require_cycle_convergence_budget`.
    require_cycle_convergence_budget(&control, &config, &record.cycle_id)?;
    require_reviewed(&record)?;
    require_no_pending_exception(&control, &config, &record)?;
    refuse_existing_acceptance(&control, integration_id)?;
    let authorizer = authorizer(args)?;
    refuse_author_accepting(&control, &record, authorizer)?;
    validate_final_authorization(&control, &config, &record, authorizer)?;

    Ok(CommandOutcome::new(
        "acceptance.record",
        format!(
            "Dry run: would record `{}` for {integration_id} by {}\nnothing was changed",
            decision.name(),
            authorizer
        ),
        serde_json::json!({
            "dry_run": true,
            "integration_id": integration_id.to_string(),
            "decision": decision.name(),
            "acceptance_owner": authorizer,
            "authorizer_actor_id": authorizer,
        }),
    )
    .with_project(config.project_id))
}

/// Refuses a second decision for one integration in both preview and commit
/// paths. A dry run that reports success for an already decided integration is
/// not a preview of the real command.
fn refuse_existing_acceptance(
    control: &ControlRepository,
    integration_id: &IntegrationId,
) -> Result<(), HarnessError> {
    if let Some(existing) = acceptance_for(control, integration_id)? {
        return Err(HarnessError::Control {
            reason: format!(
                "integration {integration_id} already has acceptance {} (`{}`)",
                existing.acceptance_id,
                existing.decision.name()
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    }
    Ok(())
}

/// Returns the final-cycle bindings a v2 acceptance must pin. Ordinary v1
/// integrations deliberately stay on their legacy acceptance path.
fn validate_final_authorization(
    control: &ControlRepository,
    config: &crate::config::ProjectConfig,
    record: &IntegrationRecord,
    authorizer_actor_id: &str,
) -> Result<Option<(crate::domain::digest::Digest, crate::domain::digest::Digest)>, HarnessError> {
    if !record.final_for_cycle {
        return Ok(None);
    }
    let default_policy = crate::config::FinalAuthorizationPolicy::default();
    let policy = config.final_authorization_policy.as_ref().or_else(|| (config.final_authorization_mode.as_deref() == Some("installed_default")).then_some(&default_policy)).ok_or_else(|| HarnessError::ControlWithRecovery {
        reason: "final authorization is not configured for this project; explicitly configure final_authorization_policy before authorizing a sealed cycle".to_owned(),
        code: ErrorCode::PolicyNotAccepted,
        recovery: FINAL_AUTHORIZATION_POLICY_NOT_CONFIGURED_RECOVERY,
    })?;
    if !policy.authorizes(authorizer_actor_id) {
        return Err(HarnessError::ControlWithRecovery {
            reason: format!(
                "actor {authorizer_actor_id} is not configured to authorize final sealed cycles"
            ),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_AUTHORIZATION_ACTOR_NOT_AUTHORIZED_RECOVERY,
        });
    }
    let cycle = load_cycle(control, &record.cycle_id)?;
    let sealed = record
        .sealed_cycle_digest
        .as_ref()
        .ok_or_else(|| HarnessError::Control {
            reason: format!(
                "final integration {} has no sealed cycle digest",
                record.integration_id
            ),
            code: ErrorCode::InternalControlCorrupt,
        })?;
    let actual = crate::domain::digest::Digest::of_canonical(&cycle)?;
    if cycle.status != crate::domain::cycle::CycleStatus::Sealed || &actual != sealed {
        return Err(HarnessError::ControlWithRecovery {
            reason: format!(
                "final integration {} no longer binds its sealed cycle",
                record.integration_id
            ),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_INTEGRATION_SEAL_STALE_RECOVERY,
        });
    }
    Ok(Some((policy.digest()?, sealed.clone())))
}

/// Rechecks the final-cycle authority bindings immediately before promotion.
/// This is intentionally separate from recording: a changed project policy or
/// resealed/tampered cycle must invalidate an older v2 authorization rather
/// than relying on the old record's existence.
pub(crate) fn validate_final_authorization_for_promotion(
    control: &ControlRepository,
    config: &crate::config::ProjectConfig,
    record: &IntegrationRecord,
    acceptance: &AcceptanceRecord,
) -> Result<(), HarnessError> {
    if !record.final_for_cycle {
        return Ok(());
    }
    // Historical v1 records remain promotable only under an old project that
    // has no v2 final-authorization policy. An explicit v2 policy cannot be
    // bypassed by rewriting a final record to look historical.
    if acceptance.schema == ACCEPTANCE_SCHEMA {
        if config.final_authorization_policy.is_none() {
            return Ok(());
        }
        return Err(HarnessError::ControlWithRecovery {
            reason: format!(
                "final integration {} has a v1 acceptance while an explicit final authorization policy requires v2",
                record.integration_id
            ),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_AUTHORIZATION_STALE_RECOVERY,
        });
    }
    if acceptance.schema != ACCEPTANCE_V2_SCHEMA {
        return Err(HarnessError::ControlWithRecovery {
            reason: format!(
                "final integration {} requires a v2 final authorization",
                record.integration_id
            ),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_AUTHORIZATION_STALE_RECOVERY,
        });
    }
    let Some(authorizer) = acceptance.authorizer_actor_id.as_deref() else {
        return Err(HarnessError::Control {
            reason: "v2 final authorization has no authorizer_actor_id".to_owned(),
            code: ErrorCode::InternalControlCorrupt,
        });
    };
    let Some(recorded_policy) = acceptance.final_authorization_policy_digest.as_ref() else {
        return Err(HarnessError::Control {
            reason: "v2 final authorization has no policy digest".to_owned(),
            code: ErrorCode::InternalControlCorrupt,
        });
    };
    let Some(recorded_seal) = acceptance.sealed_cycle_digest.as_ref() else {
        return Err(HarnessError::Control {
            reason: "v2 final authorization has no sealed cycle digest".to_owned(),
            code: ErrorCode::InternalControlCorrupt,
        });
    };
    let default_policy = crate::config::FinalAuthorizationPolicy::default();
    let policy = config
        .final_authorization_policy
        .as_ref()
        .or_else(|| {
            (config.final_authorization_mode.as_deref() == Some("installed_default"))
                .then_some(&default_policy)
        })
        .ok_or_else(|| HarnessError::ControlWithRecovery {
            reason: "final authorization policy is no longer configured".to_owned(),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_AUTHORIZATION_POLICY_NOT_CONFIGURED_RECOVERY,
        })?;
    if !policy.authorizes(authorizer) || &policy.digest()? != recorded_policy {
        return Err(HarnessError::ControlWithRecovery {
            reason: format!(
                "final authorization {} no longer matches the current project policy",
                acceptance.acceptance_id
            ),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_AUTHORIZATION_STALE_RECOVERY,
        });
    }
    let cycle = load_cycle(control, &record.cycle_id)?;
    let current_seal = crate::domain::digest::Digest::of_canonical(&cycle)?;
    if cycle.status != crate::domain::cycle::CycleStatus::Sealed
        || record.sealed_cycle_digest.as_ref() != Some(recorded_seal)
        || &current_seal != recorded_seal
    {
        return Err(HarnessError::ControlWithRecovery {
            reason: format!(
                "final authorization {} no longer matches its sealed cycle",
                acceptance.acceptance_id
            ),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_AUTHORIZATION_STALE_RECOVERY,
        });
    }
    Ok(())
}

/// Refuses an acceptance recorded by someone who implemented a member card.
///
/// Acceptance is the only thing that authorizes moving the protected branch,
/// and it was the one step in the lifecycle with no separation check at all —
/// both reviews had one, and the step they lead to did not.
///
/// A free function rather than a step inside the record builder, because the
/// dry run has to reach it too. When it lived in the builder, a preview
/// succeeded for an implementer whose real acceptance was refused, which
/// contradicts the documented promise that a dry run gives the same error the
/// real command would.
fn refuse_author_accepting(
    control: &ControlRepository,
    record: &IntegrationRecord,
    acceptance_owner: &str,
) -> Result<(), HarnessError> {
    let implementers = member_implementers(control, record)?;
    actors::refuse_author_acting_as(
        "acceptance owner",
        acceptance_owner,
        &record.integration_id.to_string(),
        implementers
            .iter()
            .map(|(card, actor)| (card.as_str(), actor.as_str())),
    )
}

/// Builds the acceptance record a validated decision becomes.
fn build_acceptance(
    control: &ControlRepository,
    record: &IntegrationRecord,
    args: &RecordArgs,
    decision: AcceptanceDecision,
    clock: &dyn Clock,
) -> Result<AcceptanceRecord, HarnessError> {
    let verification = load_verification(control, &record.integration_id)?;
    let authorizer = authorizer(args)?;
    refuse_author_accepting(control, record, authorizer)?;
    let final_bindings =
        validate_final_authorization(control, &control.project()?, record, authorizer)?;

    let Some(landing_sha) = record.landing_sha.clone() else {
        return Err(HarnessError::Control {
            reason: format!(
                "integration {} has no landing commit",
                record.integration_id
            ),
            code: ErrorCode::PreconditionNotFound,
        });
    };

    Ok(AcceptanceRecord {
        schema: if final_bindings.is_some() {
            ACCEPTANCE_V2_SCHEMA.to_owned()
        } else {
            ACCEPTANCE_SCHEMA.to_owned()
        },
        acceptance_id: next_acceptance_id(control)?,
        integration_id: record.integration_id.clone(),
        landing_sha: landing_sha.clone(),
        integration_record_digest: record.substantive_digest()?,
        receipt_ids: verification.receipt_ids.clone(),
        acceptance_owner: authorizer.to_owned(),
        authorizer_actor_id: final_bindings.as_ref().map(|_| authorizer.to_owned()),
        final_authorization_policy_digest: final_bindings
            .as_ref()
            .map(|(policy, _)| policy.clone()),
        sealed_cycle_digest: final_bindings.map(|(_, sealed)| sealed),
        decision,
        residual_risks: args.residual_risks.clone(),
        rollback_reference: args.rollback_reference.clone().unwrap_or_else(|| {
            format!("revert landing commit {landing_sha} on the protected branch")
        }),
        accepted_at: clock.now(),
        canonical_algorithm: CANONICAL_ALGORITHM.to_owned(),
    })
}

fn run_record(args: &RecordArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let integration_id: IntegrationId = args.integration_id.parse()?;
    let decision = decision_of(args);

    if args.dry_run {
        return preview_record(args, &integration_id, decision);
    }

    {
        let control = ControlRepository::open(&args.control)?;
        let record = load_integration(&control, &integration_id)?;
        require_plan_binding(&control, &record)?;
    }

    with_transaction(
        &args.control,
        "acceptance.record",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let config = control.project()?;
            let mut record = load_integration(control, &integration_id)?;
            steps.recheck(require_plan_binding(control, &record))?;
            // 73-2: the first check able to refuse, before any write — see
            // `require_cycle_convergence_budget`.
            require_cycle_convergence_budget(control, &config, &record.cycle_id)?;
            require_reviewed(&record)?;
            require_no_pending_exception(control, &config, &record)?;
            refuse_existing_acceptance(control, &integration_id)?;

            let acceptance = build_acceptance(control, &record, args, decision, clock)?;
            let acceptance_id = acceptance.acceptance_id.clone();
            let landing_sha = acceptance.landing_sha.clone();
            let digest = acceptance.digest()?;
            control.write_atomic(
                &AcceptanceRecord::relative_path(&acceptance_id),
                &format!("{}\n", serde_json::to_string_pretty(&acceptance)?),
            )?;

            // A rejection is recorded but advances nothing: Section 11.3 has no
            // transition out of `reviewed` for a refusal, and inventing one
            // would erase the fact that the work reached review at all.
            if decision.authorizes_promotion() {
                record.status = IntegrationStatus::Accepted;
                control.write_atomic(
                    &IntegrationRecord::relative_path(&integration_id),
                    &format!("{}\n", serde_json::to_string_pretty(&record)?),
                )?;
                for member in &record.members {
                    let (card, state) = load_card(control, &member.card_id)?;
                    state.state.check_transition(CardState::Accepted)?;
                    store_card_state(control, &card, &state, CardState::Accepted)?;
                }
            }

            events.append(
                &config.project_id,
                EventDraft::new("acceptance.recorded", authorizer(args)?)
                    .cycle(record.cycle_id.clone())
                    .head(landing_sha.clone())
                    .transition(
                        Some(IntegrationStatus::Reviewed.name()),
                        if decision.authorizes_promotion() {
                            IntegrationStatus::Accepted.name()
                        } else {
                            IntegrationStatus::Reviewed.name()
                        },
                    )
                    .meta(
                        "integration_id",
                        serde_json::json!(integration_id.to_string()),
                    )
                    .meta(
                        "acceptance_id",
                        serde_json::json!(acceptance_id.to_string()),
                    )
                    .meta("acceptance_digest", serde_json::json!(digest.as_str()))
                    .meta("decision", serde_json::json!(decision.name()))
                    .meta("landing_sha", serde_json::json!(landing_sha)),
                clock,
            )?;
            control.commit(
                expected,
                &format!("acceptance: {} {integration_id}", decision.name()),
            )?;

            Ok(report_acceptance(
                "acceptance.record",
                &acceptance,
                &digest,
                &config.project_id,
            ))
        },
    )
}

/// Refuses an acceptance for an integration that has not been reviewed.
///
/// #112: the refusal carries its own recovery — see `status_gate_refusal`
/// in `commands/integration.rs` — exactly when the reason is a built
/// landing commit nothing has verified yet, the one non-`Reviewed` status
/// with an unambiguous next command. `acceptance record` on an
/// already-accepted integration reaches this same refusal from a later
/// status, where `integration verify` would be wrong advice since
/// verification already happened; that and every other non-`Reviewed`
/// status keep the plain code default rather than guessing.
fn require_reviewed(record: &IntegrationRecord) -> Result<(), HarnessError> {
    if record.status == IntegrationStatus::Reviewed {
        return Ok(());
    }
    let reason = format!(
        "integration {} is `{}`; acceptance follows an integration review",
        record.integration_id,
        record.status.name()
    );
    Err(status_gate_refusal(
        reason,
        record,
        "Run `integration verify` before recording acceptance.",
    ))
}

/// Turns an acceptance into the command's outcome.
fn report_acceptance(
    command: &str,
    acceptance: &AcceptanceRecord,
    digest: &crate::domain::digest::Digest,
    project_id: &crate::domain::ids::ProjectId,
) -> CommandOutcome {
    CommandOutcome::new(
        command,
        format!(
            "Recorded acceptance {} (`{}`)\nintegration: {}\nlanding commit: {}\nowner: {}\nrollback: {}\nresidual risks: {}",
            acceptance.acceptance_id,
            acceptance.decision.name(),
            acceptance.integration_id,
            acceptance.landing_sha,
            acceptance.acceptance_owner,
            acceptance.rollback_reference,
            if acceptance.residual_risks.is_empty() {
                "none declared".to_owned()
            } else {
                acceptance.residual_risks.join("; ")
            }
        ),
        serde_json::json!({
            "acceptance_id": acceptance.acceptance_id.to_string(),
            "acceptance_digest": digest.as_str(),
            "integration_id": acceptance.integration_id.to_string(),
            "landing_sha": acceptance.landing_sha,
            "decision": acceptance.decision.name(),
            "authorizes_promotion": acceptance.decision.authorizes_promotion(),
            "acceptance_owner": acceptance.acceptance_owner,
            "authorizer_actor_id": acceptance.authorizer_actor_id,
            "final_authorization_policy_digest": acceptance.final_authorization_policy_digest.as_ref().map(crate::domain::digest::Digest::as_str),
            "sealed_cycle_digest": acceptance.sealed_cycle_digest.as_ref().map(crate::domain::digest::Digest::as_str),
            "receipt_ids": acceptance.receipt_ids,
            "residual_risks": acceptance.residual_risks,
            "rollback_reference": acceptance.rollback_reference,
        }),
    )
    .with_project(project_id.clone())
}

fn run_inspect(args: &InspectArgs) -> Result<CommandOutcome, HarnessError> {
    let integration_id: IntegrationId = args.integration_id.parse()?;
    let control = ControlRepository::open(&args.control)?;
    let config = control.project()?;

    let acceptance =
        acceptance_for(&control, &integration_id)?.ok_or_else(|| HarnessError::Control {
            reason: format!("integration {integration_id} has no recorded acceptance"),
            code: ErrorCode::PreconditionNotFound,
        })?;
    let digest = acceptance.digest()?;

    let mut outcome = report_acceptance(
        "acceptance.inspect",
        &acceptance,
        &digest,
        &config.project_id,
    );
    outcome = outcome.with_warning(format!(
        "inspection only; promotion still requires `integration promote --integration-id {integration_id}`"
    ));
    Ok(outcome)
}
