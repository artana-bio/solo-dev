//! Reconstructing what happened in a cycle, including where the record
//! disagrees with itself.
//!
//! The report's value is entirely in the discrepancies. A summary of records
//! that all agree tells a reader nothing they could not get by listing files;
//! what they cannot get any other way is the answer to "does the evidence still
//! describe the objects it names". So a digest that no longer matches, or a
//! commit a receipt refers to that no longer exists, is a *finding* — never a
//! line quietly left out because it could not be resolved.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};

use crate::{
    cli::output::CommandOutcome,
    commands::CONTROL_ENV,
    commands::{
        gate::{
            load_compatibility_request, read_integration_compatibility_request, receipts_for,
            receipts_for_integration_verification,
        },
        review::reviews_for,
    },
    control::{event_store::EventStore, repository::ControlRepository},
    domain::{
        card::CardRecord,
        clock::Clock,
        cycle::CycleRecord,
        ids::{CardId, CycleId},
    },
    error::{ErrorCode, HarnessError},
    git::{
        authority::inspect_authority,
        command::{GitScope, run_ok},
        inspect, landing,
    },
    policy::receipt_compatibility::{IntegrationCompatibilityRequestV1, evaluate},
    runner::receipt::ProvenanceSubject,
};

/// Subcommands under `audit`.
#[derive(Debug, Subcommand)]
pub enum AuditCommand {
    /// Reconstruct a cycle from control state and cross-check its evidence.
    Cycle(CycleArgs),
    /// Verify that every control head a landing commit anchored is still an
    /// ancestor of the control record.
    Anchors(AnchorsArgs),
}

impl AuditCommand {
    /// Its dotted command path, as the result envelope reports it.
    ///
    /// The error envelope used to carry only the group — `audit` — while a
    /// success carried the full path, so a consumer matching on `command` got a
    /// different granularity depending on whether the command worked.
    #[must_use]
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Cycle(..) => "audit.cycle",
            Self::Anchors(..) => "audit.anchors",
        }
    }
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
    /// A frozen privacy-safe receipt-compatibility request to include in the
    /// audit. It is evaluated read-only and never changes workflow authority.
    #[arg(long)]
    pub compatibility_request: Option<PathBuf>,
}

/// Arguments accepted by `audit anchors`.
#[derive(Debug, Args)]
pub struct AnchorsArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
    pub control: std::path::PathBuf,
}

/// Something the record says that the objects do not bear out.
///
/// Crate-visible because `cycle replay` surfaces the same findings as
/// evidence flashes; the cross-check itself lives here so the two commands
/// can never disagree about what a discrepancy is.
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct Discrepancy {
    /// The record that made the claim.
    pub(crate) subject: String,
    /// What the record says.
    pub(crate) claim: String,
    /// What was found instead.
    pub(crate) found: String,
}

/// One card's evidence tally from a cross-check.
#[derive(Clone, Debug)]
pub(crate) struct CardEvidence {
    /// The card the tally belongs to.
    pub(crate) card_id: CardId,
    /// How many gate receipts it holds.
    pub(crate) receipts: usize,
    /// How many reviews it holds.
    pub(crate) reviews: usize,
}

/// Everything a cycle-wide evidence cross-check finds.
#[derive(Clone, Debug)]
pub(crate) struct CycleEvidence {
    /// Claims the objects did not bear out, in discovery order.
    pub(crate) discrepancies: Vec<Discrepancy>,
    /// Per-card evidence tallies, in the cycle's card order.
    pub(crate) cards: Vec<CardEvidence>,
}

/// Cross-checks a cycle's recorded evidence against the objects it names.
///
/// # Errors
///
/// Returns an error when a record cannot be read. A record that reads but
/// does not hold up is a [`Discrepancy`], not an error: the caller decides
/// whether finding one is a failure (`audit cycle`) or a warning (`cycle
/// replay`).
pub(crate) fn cross_check_cycle(
    control: &ControlRepository,
    config: &crate::config::ProjectConfig,
    cycle_id: &CycleId,
    cycle: &CycleRecord,
) -> Result<CycleEvidence, HarnessError> {
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
        let receipts = check_receipts(control, config, card_id, &mut discrepancies)?;
        let reviews = check_reviews(control, config, card_id, &mut discrepancies)?;
        cards.push(CardEvidence {
            card_id: card_id.clone(),
            receipts,
            reviews,
        });
    }
    Ok(CycleEvidence {
        discrepancies,
        cards,
    })
}

/// Everything a control-anchor audit finds.
pub(crate) struct AnchorEvidence {
    /// How many landing commits reachable from the protected branch carried
    /// at least one control anchor.
    landing_commits_examined: usize,
    /// How many anchored control heads were checked across them.
    anchors_checked: usize,
    /// Claims the objects did not bear out, in discovery order.
    ///
    /// Crate-visible for the same reason `CycleEvidence::discrepancies` is:
    /// `#89` calls this same check at the promotion boundary and reuses
    /// these findings verbatim in its refusal, rather than risking a second
    /// copy of the discrepancy text drifting from this one.
    pub(crate) discrepancies: Vec<Discrepancy>,
}

/// Cross-checks every control head a landing commit has anchored against the
/// control repository's own history.
///
/// #87 anchors the trailer on every landing commit, `run_anchors` decides
/// what finding one means; this only reports.
///
/// Crate-visible because `#89` calls this exact check at the promotion
/// boundary in `integration.rs`, the same reason `cross_check_cycle` is
/// crate-visible for `cycle replay`: two callers must never be able to
/// disagree about what counts as a discrepancy here.
///
/// # Errors
///
/// Returns an error when the authority or control repository cannot be read.
/// An anchor that fails to hold up — orphaned by a rewrite, or altogether
/// missing — is a [`Discrepancy`], never an error.
pub(crate) fn check_control_anchors(
    control: &ControlRepository,
    config: &crate::config::ProjectConfig,
) -> Result<AnchorEvidence, HarnessError> {
    let authority = inspect_authority(&config.authority_repository, &config.protected_branch)?;
    let Some(protected_sha) = authority.protected_sha else {
        // Nothing has ever landed, so there is nothing to anchor — not a
        // discrepancy, just an empty result.
        return Ok(AnchorEvidence {
            landing_commits_examined: 0,
            anchors_checked: 0,
            discrepancies: Vec::new(),
        });
    };
    // Read once, not once per anchor: unlike the anchors themselves, which
    // can each individually turn out to be missing or orphaned, the current
    // control head is a single fact every one of them is checked against.
    let control_head = control.head()?;

    let mut discrepancies = Vec::new();
    let mut landing_commits_examined = 0usize;
    let mut anchors_checked = 0usize;
    for landing_sha in landing_commits(&config.authority_repository, &protected_sha)? {
        let object = landing::inspect(&config.authority_repository, &landing_sha)?;
        let anchors: Vec<&str> = object.trailer_values(landing::TRAILER_CONTROL).collect();
        if anchors.is_empty() {
            // Most commits on the protected branch are not landing commits at
            // all — `project init`'s own bootstrap commit, or anything pushed
            // directly — and this is how they are told apart from the ones
            // this check cares about: not by classifying what a landing
            // commit is in general, but by whether this specific trailer is
            // present. A commit that carries it is exactly what this check
            // needs to examine, and nothing else is.
            continue;
        }
        landing_commits_examined += 1;

        let Some(head) = control_head.as_deref() else {
            // `landing_trailers` refuses to build this very trailer against
            // an unborn control history (see its own doc comment: reaching a
            // landing commit requires a chain of earlier control commits).
            // Finding one here anyway means that invariant broke elsewhere,
            // not that there is vacuously nothing to compare against, so this
            // refuses rather than silently passing every anchor as healthy.
            return Err(HarnessError::Control {
                reason: format!(
                    "landing commit {landing_sha} anchors control history, but the control repository has no commits at all"
                ),
                code: ErrorCode::InternalControlCorrupt,
            });
        };
        for anchored in anchors {
            anchors_checked += 1;
            check_anchor(control, head, &landing_sha, anchored, &mut discrepancies)?;
        }
    }
    Ok(AnchorEvidence {
        landing_commits_examined,
        anchors_checked,
        discrepancies,
    })
}

/// Checks one anchored control head, telling "orphaned" apart from "missing"
/// the way #88's work card requires: by asking the control repository
/// directly whether the object exists at all, rather than reading that fact
/// out of `merge-base --is-ancestor`'s exit code.
///
/// Both cases exit `merge-base --is-ancestor` non-zero. Per `git-merge-base(1)`
/// — and confirmed directly against Git 2.50.1 rather than assumed — exit 1
/// means "resolved fine, just not an ancestor"; anything else (128 in
/// practice, with a `fatal: Not a valid commit name` diagnostic on stderr)
/// means Git could not resolve one side at all. Leaning on that split would
/// mean trusting that a code answering a different question (did the process
/// succeed) forever stays a reliable proxy for this one (does the object
/// exist). Existence is answered first instead, independently, with the same
/// `rev-parse --verify` this file already uses for that question against the
/// candidate repository, in `commit_exists` above.
fn check_anchor(
    control: &ControlRepository,
    control_head: &str,
    landing_sha: &str,
    anchored: &str,
    found: &mut Vec<Discrepancy>,
) -> Result<(), HarnessError> {
    if inspect::resolve_commit(&control.scope(), anchored).is_err() {
        found.push(Discrepancy {
            subject: format!("landing commit {landing_sha}"),
            claim: format!("control anchor {anchored}"),
            found: format!("{anchored} is not present in the control repository at all"),
        });
        return Ok(());
    }

    if inspect::is_ancestor(&control.scope(), anchored, control_head)? {
        return Ok(());
    }

    found.push(Discrepancy {
        subject: format!("landing commit {landing_sha}"),
        claim: format!("control anchor {anchored}"),
        found: format!(
            "{anchored} exists in the control repository but is not an ancestor of control head {control_head}; control history was rewritten"
        ),
    });
    Ok(())
}

/// Every commit the protected branch has ever pointed at, walked back from
/// its current tip.
///
/// `--first-parent`, deliberately, not merely for speed. A landing commit's
/// *second* parent is the integration head, which carries the candidate
/// repository's own commit history — transferred into the authority wholesale
/// so the landing object it carries is complete — and no commit reachable
/// only that way was ever itself the tip of the protected branch. Walking it
/// too would grow this check's cost with the *candidate* repository's history
/// rather than with how many times something has landed, for no commit it
/// would find that the first-parent walk does not already find: every
/// landing's first parent is the previous tip (see `landing_trailers`), and a
/// directly pushed commit is single-parent and trivially its own
/// first-parent chain. So `--first-parent` already reaches everything that
/// was ever `<protected branch>` and nothing that was not.
///
/// # Errors
///
/// Returns an external-tool error when Git cannot be executed.
fn landing_commits(authority: &Path, protected_sha: &str) -> Result<Vec<String>, HarnessError> {
    let scope = GitScope::git_dir(authority);
    Ok(
        run_ok(&scope, ["rev-list", "--first-parent", protected_sha])?
            .trimmed_stdout()
            .lines()
            .map(ToOwned::to_owned)
            .collect(),
    )
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
        AuditCommand::Anchors(args) => run_anchors(args),
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
                "at": event["occurred_at"],
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

    let compatibility = args
        .compatibility_request
        .as_ref()
        .map(|path| audit_compatibility(&control, &cycle, path))
        .transpose()?;

    // Events carry their own order: identifiers are monotonic, and the store
    // returns them sorted. Sorting by timestamp instead would collapse two
    // events recorded in the same second into an arbitrary order.
    let store = EventStore::new(&control);
    let raw_events = store.for_cycle(&cycle_id)?;
    let events: Vec<serde_json::Value> = raw_events
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<_, _>>()?;

    let evidence = cross_check_cycle(&control, &config, &cycle_id, &cycle)?;
    let mut discrepancies = evidence.discrepancies;
    let cards: Vec<serde_json::Value> = evidence
        .cards
        .iter()
        .map(|card| {
            serde_json::json!({
                "card_id": card.card_id.to_string(),
                "receipts": card.receipts,
                "reviews": card.reviews,
            })
        })
        .collect();

    let transitions = promotions(&events);
    let exceptions = audit_exceptions(&control, &raw_events, &config, &mut discrepancies)?;
    let report = serde_json::json!({
        "cycle_id": cycle_id.to_string(),
        "objective": cycle.objective,
        "status": cycle.status.to_string(),
        "baseline_sha": cycle.baseline_sha,
        "events": events.len(),
        "timeline": events.iter().map(|event| serde_json::json!({
            "event_id": event["event_id"],
            "at": event["occurred_at"],
            "type": event["event_type"],
            "actor_id": event["actor_id"],
            "card_id": event["card_id"],
            "metadata": event["metadata"],
        })).collect::<Vec<_>>(),
        "cards": cards,
        "receipt_compatibility": compatibility,
        "protected_branch_transitions": transitions,
        "exceptions": exceptions,
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

/// Executes `audit anchors`.
///
/// Read-only by construction: nothing here calls `with_transaction`, writes
/// through `control`, or changes the authority. It only reads the protected
/// branch and reports. Refusing a promotion built on a bad anchor is a
/// separate card (#89); merging that into this one would make neither half
/// testable on its own.
///
/// # Errors
///
/// Returns a configuration error when the control or authority repository
/// cannot be read, or a policy error when the report found discrepancies.
fn run_anchors(args: &AnchorsArgs) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.control)?;
    let config = control.project()?;

    let AnchorEvidence {
        landing_commits_examined,
        anchors_checked,
        discrepancies,
    } = check_control_anchors(&control, &config)?;

    let report = serde_json::json!({
        "protected_branch": config.protected_branch,
        "landing_commits_examined": landing_commits_examined,
        "anchors_checked": anchors_checked,
        "discrepancies": discrepancies,
    });

    if discrepancies.is_empty() {
        return Ok(CommandOutcome::new(
            "audit.anchors",
            format!(
                "Audit of control anchors on `{}`\nlanding commits examined: {landing_commits_examined}\nanchors checked: {anchors_checked}\n\nevery anchored control head is still an ancestor of the control record",
                config.protected_branch,
            ),
            report,
        )
        .with_project(config.project_id));
    }

    // As in `run_cycle`, a report that found problems exits non-zero rather
    // than making a reader parse prose to learn the answer. Unlike
    // `run_cycle`'s message, this one folds each discrepancy's claim and
    // found text into the summary, not just its subject: the two facts §6.4
    // of the work card requires this report to name — the anchored SHA and
    // the landing commit that claimed it — live in different `Discrepancy`
    // fields, and only the joined `reason` string below reaches the error
    // envelope's `message` (`HarnessError::Control::details` only echoes the
    // same `reason`, and there is no companion "replay"-style command here to
    // surface the full structured list on a non-failing path the way `cycle
    // replay` does for `audit cycle`'s findings).
    Err(HarnessError::Control {
        reason: format!(
            "audit of control anchors on `{}` found {} discrepancy(ies): {}",
            config.protected_branch,
            discrepancies.len(),
            discrepancies
                .iter()
                .map(|entry| format!(
                    "{} claims {}, found {}",
                    entry.subject, entry.claim, entry.found
                ))
                .collect::<Vec<_>>()
                .join("; "),
        ),
        code: ErrorCode::PolicyAuditDiscrepancy,
    })
}

/// Projects exception events while treating malformed or policy-mismatched
/// events as audit findings.  The command never silently drops an event that
/// could otherwise make an authorization look clear when it was not.
#[allow(clippy::too_many_lines)]
fn audit_exceptions(
    control: &ControlRepository,
    events: &[crate::control::event_store::Event],
    config: &crate::config::ProjectConfig,
    found: &mut Vec<Discrepancy>,
) -> Result<Vec<serde_json::Value>, HarnessError> {
    let current_policy = config
        .final_authorization_policy
        .as_ref()
        .map(crate::config::FinalAuthorizationPolicy::digest)
        .transpose()?
        .map(|digest| digest.as_str().to_owned());
    let authorizes = |actor_id: &str| {
        config
            .final_authorization_policy
            .as_ref()
            .is_some_and(|policy| policy.authorizes(actor_id))
    };
    let raised_ids: std::collections::BTreeMap<String, String> = events
        .iter()
        .filter(|event| event.event_type == "integration.exception_raised")
        .filter_map(|event| {
            event
                .metadata
                .get("integration_id")
                .and_then(serde_json::Value::as_str)
                .map(|integration_id| (event.event_id.to_string(), integration_id.to_owned()))
        })
        .collect();
    let mut resolved_ids = std::collections::BTreeSet::new();
    let mut result = Vec::new();
    for event in events.iter().filter(|event| {
        matches!(
            event.event_type.as_str(),
            "integration.exception_raised" | "integration.exception_resolved"
        )
    }) {
        let metadata = &event.metadata;
        let integration_id = metadata
            .get("integration_id")
            .and_then(serde_json::Value::as_str);
        let malformed = match event.event_type.as_str() {
            "integration.exception_raised" => {
                let policy = metadata
                    .get("policy_digest")
                    .and_then(serde_json::Value::as_str);
                integration_id.is_none()
                    || metadata
                        .get("trigger")
                        .and_then(serde_json::Value::as_str)
                        .is_none()
                    || metadata
                        .get("evidence_refs")
                        .and_then(serde_json::Value::as_array)
                        .is_none()
                    || metadata
                        .get("integration_digest")
                        .and_then(serde_json::Value::as_str)
                        .is_none()
                    || metadata
                        .get("sealed_cycle_digest")
                        .and_then(serde_json::Value::as_str)
                        .is_none()
                    || policy != current_policy.as_deref()
                    || !exception_bindings_match(control, config, metadata)
            }
            "integration.exception_resolved" => {
                let target = metadata
                    .get("exception_event_id")
                    .and_then(serde_json::Value::as_str);
                let duplicate =
                    target.is_some_and(|target| !resolved_ids.insert(target.to_owned()));
                integration_id.is_none()
                    || metadata
                        .get("policy_digest")
                        .and_then(serde_json::Value::as_str)
                        != current_policy.as_deref()
                    || metadata
                        .get("integration_digest")
                        .and_then(serde_json::Value::as_str)
                        .is_none()
                    || metadata
                        .get("sealed_cycle_digest")
                        .and_then(serde_json::Value::as_str)
                        .is_none()
                    || !exception_bindings_match(control, config, metadata)
                    || metadata
                        .get("resolution")
                        .and_then(serde_json::Value::as_str)
                        != Some("continue")
                    || !authorizes(&event.actor_id)
                    || !target.is_some_and(|target| {
                        raised_ids.get(target).is_some_and(|raised_integration| {
                            Some(raised_integration.as_str()) == integration_id
                        })
                    })
                    || duplicate
            }
            _ => false,
        };
        if malformed {
            found.push(Discrepancy {
                subject: format!("exception event {}", event.event_id),
                claim: "well-formed exception fact under current final authorization policy"
                    .to_owned(),
                found: "missing required binding or policy digest mismatch".to_owned(),
            });
        }
        result.push(serde_json::json!({
            "event_id":event.event_id,
            "type":event.event_type,
            "actor_id":event.actor_id,
            "at":event.occurred_at,
            "integration_id":integration_id,
            "metadata":metadata,
            "valid":!malformed,
        }));
    }
    Ok(result)
}

/// Verifies the immutable event digests against the integration that the event
/// names.  Audit returns a discrepancy for any failure instead of trusting a
/// hand-edited event as workflow authority.
fn exception_bindings_match(
    control: &ControlRepository,
    config: &crate::config::ProjectConfig,
    metadata: &std::collections::BTreeMap<String, serde_json::Value>,
) -> bool {
    let Some(integration_id) = metadata
        .get("integration_id")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| value.parse().ok())
    else {
        return false;
    };
    let Ok(record) = crate::commands::integration::load_integration(control, &integration_id)
    else {
        return false;
    };
    let Some(policy) = config.final_authorization_policy.as_ref() else {
        return false;
    };
    let (Ok(policy_digest), Ok(integration_digest)) =
        (policy.digest(), record.substantive_digest())
    else {
        return false;
    };
    metadata
        .get("policy_digest")
        .and_then(serde_json::Value::as_str)
        == Some(policy_digest.as_str())
        && metadata
            .get("integration_digest")
            .and_then(serde_json::Value::as_str)
            == Some(integration_digest.as_str())
        && metadata
            .get("sealed_cycle_digest")
            .and_then(serde_json::Value::as_str)
            == record
                .sealed_cycle_digest
                .as_ref()
                .map(crate::domain::digest::Digest::as_str)
}

/// #142: no `audit` or `gate` subcommand emits a compatibility-request
/// example (see `COMPATIBILITY_REQUEST_READ_RECOVERY` in
/// `src/commands/gate.rs`, which reads the same document kind through a
/// separate, unshared implementation — `gate::load_json_request` is not
/// reused here; #142 §8 forbids fixing that duplication as part of this
/// card, so it is reported rather than merged).
const COMPATIBILITY_REQUEST_READ_RECOVERY: &str = "This is a read failure, not a syntax problem: the compatibility request file above could not be opened. Confirm the path exists, is spelled correctly, and is readable by this process.";

/// See `MUTATION_CAMPAIGN_SCHEMA_RECOVERY` in `src/commands/gate.rs` for why
/// `is_data()` splits this from its `_SYNTAX_RECOVERY` sibling.
const COMPATIBILITY_REQUEST_SCHEMA_RECOVERY: &str = "This compatibility request is valid JSON but does not match the schema; the message above names the missing or invalid field. There is no generated example for a compatibility request document; re-check the field the message names.";

/// The syntax-failure sibling of [`COMPATIBILITY_REQUEST_SCHEMA_RECOVERY`].
const COMPATIBILITY_REQUEST_SYNTAX_RECOVERY: &str = "This compatibility request is not valid JSON; the message above names the exact line and column to fix.";

/// The `serde_json::from_value` conversion below reparses an already-parsed
/// `serde_json::Value`, so every error it can produce is
/// `Category::Data` (`serde_json::Error::classify()`) — there is no raw text
/// left for a syntax error to come from. Unlike the split pair above, this
/// is unambiguously schema-shaped, always.
const INTEGRATION_COMPATIBILITY_REQUEST_SCHEMA_RECOVERY: &str = "This integration compatibility request is valid JSON but does not match the schema; the message above names the missing or invalid field. There is no generated example for an integration compatibility request document; re-check the field the message names.";

/// Projects the same exact decision as `gate status` in the durable cycle
/// audit.  The request must name a card in this cycle; it is never inferred
/// from the receipt it evaluates, because that would erase incompatible
/// environment or fixture context.
fn audit_compatibility(
    control: &ControlRepository,
    cycle: &CycleRecord,
    path: &PathBuf,
) -> Result<serde_json::Value, HarnessError> {
    let raw =
        std::fs::read_to_string(path).map_err(|source| HarnessError::ControlWithRecovery {
            reason: format!(
                "cannot read receipt compatibility request {}: {source}",
                path.display()
            ),
            code: ErrorCode::ConfigMalformed,
            recovery: COMPATIBILITY_REQUEST_READ_RECOVERY,
        })?;
    let envelope: serde_json::Value = serde_json::from_str(&raw).map_err(|source| {
        let recovery = if source.is_data() {
            COMPATIBILITY_REQUEST_SCHEMA_RECOVERY
        } else {
            COMPATIBILITY_REQUEST_SYNTAX_RECOVERY
        };
        HarnessError::ControlWithRecovery {
            reason: format!(
                "receipt compatibility request {} is malformed: {source}",
                path.display()
            ),
            code: ErrorCode::ConfigMalformed,
            recovery,
        }
    })?;
    if envelope["expected"]["subject"]["kind"] == "integration" {
        let request: IntegrationCompatibilityRequestV1 =
            serde_json::from_value(envelope).map_err(|source| {
                HarnessError::ControlWithRecovery {
                    reason: format!(
                        "integration compatibility request {} is malformed: {source}",
                        path.display()
                    ),
                    code: ErrorCode::ConfigMalformed,
                    recovery: INTEGRATION_COMPATIBILITY_REQUEST_SCHEMA_RECOVERY,
                }
            })?;
        if request.context.cycle_id != cycle.cycle_id {
            return Err(HarnessError::Control {
                reason: "integration compatibility request names a different cycle".to_owned(),
                code: ErrorCode::ConfigInvalidValue,
            });
        }
        let record = crate::commands::integration::load_integration(
            control,
            &request.context.integration_id,
        )?;
        let (verification, receipts) =
            receipts_for_integration_verification(control, &request.context.integration_id)?;
        return serde_json::to_value(read_integration_compatibility_request(
            control,
            path,
            &record,
            &verification,
            &receipts,
        )?)
        .map_err(Into::into);
    }
    let request = load_compatibility_request(path)?;
    let ProvenanceSubject::Card { card_id, .. } = &request.expected.subject else {
        return Err(HarnessError::Control {
            reason: "receipt compatibility request for audit must name a card subject".to_owned(),
            code: ErrorCode::ConfigInvalidValue,
        });
    };
    if !cycle.card_ids.contains(card_id) {
        return Err(HarnessError::Control {
            reason: format!(
                "receipt compatibility request names card {card_id}, which is not in cycle {}",
                cycle.cycle_id
            ),
            code: ErrorCode::ConfigInvalidValue,
        });
    }
    serde_json::to_value(evaluate(&request, &receipts_for(control, card_id)?)).map_err(Into::into)
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
