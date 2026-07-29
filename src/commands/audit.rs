//! Reconstructing what happened in a cycle, including where the record
//! disagrees with itself.
//!
//! The report's value is entirely in the discrepancies. A summary of records
//! that all agree tells a reader nothing they could not get by listing files;
//! what they cannot get any other way is the answer to "does the evidence still
//! describe the objects it names". So a digest that no longer matches, or a
//! commit a receipt refers to that no longer exists, is a *finding* — never a
//! line quietly left out because it could not be resolved.

use clap::{Args, Subcommand};

use crate::{
    cli::output::CommandOutcome,
    commands::CONTROL_ENV,
    commands::{gate::receipts_for, review::reviews_for},
    control::{event_store::EventStore, repository::ControlRepository},
    domain::{
        card::CardRecord,
        clock::Clock,
        cycle::CycleRecord,
        ids::{CardId, CycleId},
    },
    error::{ErrorCode, HarnessError},
    git::{command::GitScope, inspect},
};

/// Subcommands under `audit`.
#[derive(Debug, Subcommand)]
pub enum AuditCommand {
    /// Reconstruct a cycle from control state and cross-check its evidence.
    Cycle(CycleArgs),
}

/// Arguments accepted by `audit cycle`.
#[derive(Debug, Args)]
pub struct CycleArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
    pub control: std::path::PathBuf,
    /// The cycle to report on.
    #[arg(long)]
    pub cycle_id: String,
}

/// Something the record says that the objects do not bear out.
#[derive(Clone, Debug, serde::Serialize)]
struct Discrepancy {
    /// The record that made the claim.
    subject: String,
    /// What the record says.
    claim: String,
    /// What was found instead.
    found: String,
}

/// Executes an `audit` subcommand.
///
/// # Errors
///
/// Returns a precondition error when the cycle does not exist, or a policy
/// error when the report found discrepancies.
pub fn execute(command: &AuditCommand, _clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    match command {
        AuditCommand::Cycle(args) => run_cycle(args),
    }
}

/// Checks that a commit named by the record still exists.
fn commit_exists(repository: &std::path::Path, sha: &str) -> bool {
    inspect::resolve_commit(&GitScope::work_tree(repository), sha).is_ok()
}

/// Cross-checks every receipt a card holds.
fn check_receipts(
    control: &ControlRepository,
    config: &crate::config::ProjectConfig,
    card_id: &CardId,
    found: &mut Vec<Discrepancy>,
) -> Result<usize, HarnessError> {
    let receipts = receipts_for(control, card_id)?;
    for receipt in &receipts {
        if !commit_exists(&config.repository, &receipt.evaluated_sha) {
            found.push(Discrepancy {
                subject: format!("receipt {}", receipt.receipt_id),
                claim: format!("evaluated commit {}", receipt.evaluated_sha),
                found: "the commit is not in the candidate repository".to_owned(),
            });
        }
        // The log location is reported, never its contents. Section 14.3 keeps
        // gate output outside control history precisely because it is captured
        // third-party text that nobody has vetted for secrets.
        if !receipt.log_location.exists() {
            found.push(Discrepancy {
                subject: format!("receipt {}", receipt.receipt_id),
                claim: format!("logs at {}", receipt.log_location.display()),
                found: "the log directory is gone; retention may have removed it".to_owned(),
            });
        }
    }
    Ok(receipts.len())
}

/// Cross-checks every review a card holds against the card it names.
fn check_reviews(
    control: &ControlRepository,
    config: &crate::config::ProjectConfig,
    card_id: &CardId,
    found: &mut Vec<Discrepancy>,
) -> Result<usize, HarnessError> {
    let reviews = reviews_for(control, card_id)?;
    for review in &reviews {
        if !commit_exists(&config.repository, &review.candidate_sha) {
            found.push(Discrepancy {
                subject: format!("review {}", review.review_id),
                claim: format!("reviewed candidate {}", review.candidate_sha),
                found: "the commit is not in the candidate repository".to_owned(),
            });
        }
        // The card revision the review was bound to must still digest to what
        // the review recorded, or the review describes a card that no longer
        // exists in that form.
        let relative = CardRecord::relative_path(card_id, review.card_revision);
        match control.read(&relative) {
            Ok(raw) => match serde_json::from_str::<CardRecord>(&raw) {
                Ok(record) => {
                    let actual = record.digest()?;
                    if actual != review.card_digest {
                        found.push(Discrepancy {
                            subject: format!("review {}", review.review_id),
                            claim: format!("card digest {}", review.card_digest),
                            found: format!(
                                "revision {} now digests to {actual}",
                                review.card_revision
                            ),
                        });
                    }
                }
                Err(source) => found.push(Discrepancy {
                    subject: format!("review {}", review.review_id),
                    claim: format!("card {card_id} revision {}", review.card_revision),
                    found: format!("the stored revision is malformed: {source}"),
                }),
            },
            Err(_) => found.push(Discrepancy {
                subject: format!("review {}", review.review_id),
                claim: format!("card {card_id} revision {}", review.card_revision),
                found: "the revision record is missing".to_owned(),
            }),
        }
    }
    Ok(reviews.len())
}

/// The protected-branch transitions the event log records.
fn promotions(events: &[serde_json::Value]) -> Vec<serde_json::Value> {
    events
        .iter()
        .filter(|event| event["event_type"] == "integration.promoted")
        .map(|event| {
            serde_json::json!({
                "at": event["recorded_at"],
                "actor_id": event["actor_id"],
                "from": event["metadata"]["previous_main_sha"],
                "to": event["metadata"]["landing_sha"],
                "integration_id": event["metadata"]["integration_id"],
                "acceptance_id": event["metadata"]["acceptance_id"],
            })
        })
        .collect()
}

fn run_cycle(args: &CycleArgs) -> Result<CommandOutcome, HarnessError> {
    let cycle_id: CycleId = args.cycle_id.parse()?;
    let control = ControlRepository::open(&args.control)?;
    let config = control.project()?;

    let relative = CycleRecord::relative_path(&cycle_id);
    if !control.path(&relative).exists() {
        return Err(HarnessError::Control {
            reason: format!("cycle {cycle_id} does not exist"),
            code: ErrorCode::PreconditionNotFound,
        });
    }
    let cycle: CycleRecord = serde_json::from_str(&control.read(&relative)?).map_err(|source| {
        HarnessError::Control {
            reason: format!("cycle {cycle_id} is malformed: {source}"),
            code: ErrorCode::InternalControlCorrupt,
        }
    })?;

    // Events carry their own order: identifiers are monotonic, and the store
    // returns them sorted. Sorting by timestamp instead would collapse two
    // events recorded in the same second into an arbitrary order.
    let store = EventStore::new(&control);
    let events: Vec<serde_json::Value> = store
        .for_cycle(&cycle_id)?
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<_, _>>()?;

    let mut discrepancies = Vec::new();
    if let Some(baseline) = &cycle.baseline_sha
        && !commit_exists(&config.repository, baseline)
    {
        discrepancies.push(Discrepancy {
            subject: format!("cycle {cycle_id}"),
            claim: format!("frozen baseline {baseline}"),
            found: "the commit is not in the candidate repository".to_owned(),
        });
    }

    let mut cards = Vec::new();
    for card_id in &cycle.card_ids {
        let receipts = check_receipts(&control, &config, card_id, &mut discrepancies)?;
        let reviews = check_reviews(&control, &config, card_id, &mut discrepancies)?;
        cards.push(serde_json::json!({
            "card_id": card_id.to_string(),
            "receipts": receipts,
            "reviews": reviews,
        }));
    }

    let transitions = promotions(&events);
    let report = serde_json::json!({
        "cycle_id": cycle_id.to_string(),
        "objective": cycle.objective,
        "status": cycle.status.to_string(),
        "baseline_sha": cycle.baseline_sha,
        "events": events.len(),
        "timeline": events.iter().map(|event| serde_json::json!({
            "event_id": event["event_id"],
            "at": event["recorded_at"],
            "type": event["event_type"],
            "actor_id": event["actor_id"],
            "card_id": event["card_id"],
        })).collect::<Vec<_>>(),
        "cards": cards,
        "protected_branch_transitions": transitions,
        "discrepancies": discrepancies,
    });

    let text = render(&cycle_id, &cycle, &events, &transitions, &discrepancies);
    if discrepancies.is_empty() {
        return Ok(CommandOutcome::new("audit.cycle", text, report).with_project(config.project_id));
    }

    // A report that found problems exits non-zero. A reader piping this into
    // something else must not have to parse prose to learn the answer.
    Err(HarnessError::Control {
        reason: format!(
            "audit of cycle {cycle_id} found {} discrepancy(ies): {}",
            discrepancies.len(),
            discrepancies
                .iter()
                .map(|entry| entry.subject.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        code: ErrorCode::PolicyAuditDiscrepancy,
    })
}

/// Renders the human-readable report.
fn render(
    cycle_id: &CycleId,
    cycle: &CycleRecord,
    events: &[serde_json::Value],
    transitions: &[serde_json::Value],
    discrepancies: &[Discrepancy],
) -> String {
    let mut text = format!(
        "Audit of cycle {cycle_id} ({})\nobjective: {}\nbaseline: {}\nevents: {}",
        cycle.status,
        cycle.objective,
        cycle.baseline_sha.as_deref().unwrap_or("not frozen"),
        events.len()
    );

    if transitions.is_empty() {
        text.push_str("\n\nno protected-branch transition was recorded");
    } else {
        text.push_str("\n\nprotected-branch transitions:");
        for transition in transitions {
            let _ = std::fmt::Write::write_fmt(
                &mut text,
                format_args!(
                    "\n  {} → {}\n    by {} for {} under {}",
                    transition["from"].as_str().unwrap_or("unknown"),
                    transition["to"].as_str().unwrap_or("unknown"),
                    transition["actor_id"].as_str().unwrap_or("unknown"),
                    transition["integration_id"].as_str().unwrap_or("unknown"),
                    transition["acceptance_id"].as_str().unwrap_or("unknown"),
                ),
            );
        }
    }

    if discrepancies.is_empty() {
        text.push_str("\n\nevery recorded digest and commit still resolves");
    } else {
        let _ = std::fmt::Write::write_fmt(
            &mut text,
            format_args!("\n\n{} discrepancy(ies):", discrepancies.len()),
        );
        for entry in discrepancies {
            let _ = std::fmt::Write::write_fmt(
                &mut text,
                format_args!(
                    "\n  {}\n    claims: {}\n    found:  {}",
                    entry.subject, entry.claim, entry.found
                ),
            );
        }
    }
    text
}
