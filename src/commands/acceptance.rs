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
        integration::{load_integration, load_verification, member_implementers},
        transaction::with_transaction,
    },
    control::{event_store::EventDraft, repository::ControlRepository},
    domain::{
        acceptance::{ACCEPTANCE_DIR, ACCEPTANCE_SCHEMA, AcceptanceDecision, AcceptanceRecord},
        card::CardState,
        clock::Clock,
        digest::CANONICAL_ALGORITHM,
        ids::{AcceptanceId, IntegrationId},
        integration::{IntegrationRecord, IntegrationStatus},
    },
    error::{ErrorCode, HarnessError},
    policy::actors,
};

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
    /// Who is taking responsibility. Declared, not proven; see D-013.
    #[arg(long)]
    pub acceptance_owner: String,
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
    require_reviewed(&record)?;
    refuse_author_accepting(&control, &record, &args.acceptance_owner)?;

    Ok(CommandOutcome::new(
        "acceptance.record",
        format!(
            "Dry run: would record `{}` for {integration_id} by {}\nnothing was changed",
            decision.name(),
            args.acceptance_owner
        ),
        serde_json::json!({
            "dry_run": true,
            "integration_id": integration_id.to_string(),
            "decision": decision.name(),
            "acceptance_owner": args.acceptance_owner,
        }),
    )
    .with_project(config.project_id))
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
    refuse_author_accepting(control, record, &args.acceptance_owner)?;

    let Some(landing_sha) = record.landing_sha.clone() else {
        return Err(HarnessError::Control {
            reason: format!(
                "integration {} has no landing commit",
                record.integration_id
            ),
            code: ErrorCode::PreconditionNotFound,
        });
    };

    let acceptance = AcceptanceRecord {
        schema: ACCEPTANCE_SCHEMA.to_owned(),
        acceptance_id: next_acceptance_id(control)?,
        integration_id: record.integration_id.clone(),
        landing_sha: landing_sha.clone(),
        integration_record_digest: record.substantive_digest()?,
        receipt_ids: verification.receipt_ids.clone(),
        acceptance_owner: args.acceptance_owner.clone(),
        decision,
        residual_risks: args.residual_risks.clone(),
        rollback_reference: args.rollback_reference.clone().unwrap_or_else(|| {
            format!("revert landing commit {landing_sha} on the protected branch")
        }),
        accepted_at: clock.now(),
        canonical_algorithm: CANONICAL_ALGORITHM.to_owned(),
    };
    acceptance.validate()?;
    Ok(acceptance)
}

fn run_record(args: &RecordArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let integration_id: IntegrationId = args.integration_id.parse()?;
    let decision = decision_of(args);

    if args.dry_run {
        return preview_record(args, &integration_id, decision);
    }

    with_transaction(
        &args.control,
        "acceptance.record",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let config = control.project()?;
            let mut record = load_integration(control, &integration_id)?;
            require_reviewed(&record)?;
            if let Some(existing) = acceptance_for(control, &integration_id)? {
                return Err(HarnessError::Control {
                    reason: format!(
                        "integration {integration_id} already has acceptance {} (`{}`)",
                        existing.acceptance_id,
                        existing.decision.name()
                    ),
                    code: ErrorCode::PolicyInvalidTransition,
                });
            }

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
                EventDraft::new("acceptance.recorded", &args.acceptance_owner)
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
fn require_reviewed(record: &IntegrationRecord) -> Result<(), HarnessError> {
    if record.status == IntegrationStatus::Reviewed {
        return Ok(());
    }
    Err(HarnessError::Control {
        reason: format!(
            "integration {} is `{}`; acceptance follows an integration review",
            record.integration_id,
            record.status.name()
        ),
        code: ErrorCode::PolicyInvalidTransition,
    })
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
