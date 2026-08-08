//! Exact control-object loading and integrity validation for snapshots.

use std::collections::BTreeSet;

use serde::de::DeserializeOwned;

use super::{
    ConsistencyDiagnostics, PROJECT_SNAPSHOT_SCHEMA, ProjectSnapshot,
    project_snapshot_metrics::{self, StoredCardState},
    project_snapshot_observation,
};
use crate::{
    config::ProjectConfig,
    control::{
        event_store::{EVENT_DIR, Event},
        journal::Journal,
        lock::ProjectLock,
        repository::ControlRepository,
    },
    domain::{
        card::{CARD_DIR, CardRecord},
        clock::Clock,
        cycle::{CYCLE_DIR, CycleRecord},
        digest::Digest,
        handoff::{HANDOFF_DIR, HandoffRecord},
        integration::{INTEGRATION_DIR, IntegrationRecord},
        lease::{LEASE_DIR, LeaseRecord},
        review::{REVIEW_DIR, ReviewRecord},
    },
    error::{ErrorCode, HarnessError},
    runner::receipt::{RECEIPT_DIR, Receipt},
};

pub(super) fn collect(
    control: &ControlRepository,
    clock: &dyn Clock,
) -> Result<ProjectSnapshot, HarnessError> {
    let head = control.head()?.ok_or_else(|| HarnessError::Control {
        reason: "control repository has no commit to snapshot".to_owned(),
        code: ErrorCode::InternalControlCorrupt,
    })?;
    collect_at_head(control, &head, clock)
}

pub(super) fn collect_at_head(
    control: &ControlRepository,
    control_head: &str,
    clock: &dyn Clock,
) -> Result<ProjectSnapshot, HarnessError> {
    let config: ProjectConfig = read_json_at(control, control_head, "project/project.json")?;
    let captured_at = clock.now();
    let mut diagnostics = Vec::new();

    let project_head = project_snapshot_observation::candidate_head(&config, &mut diagnostics);
    let authority_head = project_snapshot_observation::authority_head(&config, &mut diagnostics);
    let cycles: Vec<CycleRecord> = read_json_files(control, control_head, CYCLE_DIR)?;
    let events: Vec<Event> = read_json_files(control, control_head, EVENT_DIR)?;
    let (cards, card_states) = read_cards(control, control_head)?;
    let raw_receipts = read_receipts(control, control_head)?;
    let reviews: Vec<ReviewRecord> = read_json_files(control, control_head, REVIEW_DIR)?;
    let handoffs: Vec<HandoffRecord> = read_json_files(control, control_head, HANDOFF_DIR)?;
    let integrations: Vec<IntegrationRecord> =
        read_json_files(control, control_head, INTEGRATION_DIR)?;
    let leases: Vec<LeaseRecord> = read_json_files(control, control_head, LEASE_DIR)?;

    let records = SnapshotRecords {
        cycles: &cycles,
        cards: &cards,
        states: &card_states,
        events: &events,
        receipts: &raw_receipts,
        reviews: &reviews,
        handoffs: &handoffs,
        integrations: &integrations,
        leases: &leases,
    };
    validate_subjects(&config, &records, &mut diagnostics);
    let receipts = validate_receipts(&config, &records)?;

    let (cycle_state_counts, cycle_status_diagnostics) =
        project_snapshot_metrics::cycle_counts(&cycles, &events);
    diagnostics.extend(cycle_status_diagnostics);
    let card_state_counts = project_snapshot_metrics::card_counts(&card_states);
    let active_cards =
        project_snapshot_metrics::active_cards(&cards, &card_states, &events, &leases, captured_at);
    let gate_metrics = project_snapshot_metrics::gate_metrics(&receipts);
    let review_metrics = project_snapshot_metrics::review_metrics(&events);
    let integration = project_snapshot_metrics::integration_summary(
        &cycles,
        &cards,
        &card_states,
        &integrations,
        &handoffs,
        &reviews,
        &leases,
    );
    let silent_leases = project_snapshot_metrics::silent_leases(&leases, captured_at);

    let control_worktree_clean = control.is_clean()?;
    if !control_worktree_clean {
        diagnostics.push("control_worktree_dirty_authoritative_head_used".to_owned());
    }
    let unresolved_journal_operations = Journal::new(control).unresolved()?.len() as u64;
    if unresolved_journal_operations > 0 {
        diagnostics.push("unresolved_journal_operations_are_ephemeral".to_owned());
    }
    let lock_state =
        project_snapshot_observation::lock_state(&ProjectLock::diagnose(control.root()));
    if lock_state != "free" {
        diagnostics.push("ephemeral_lock_observed".to_owned());
    }

    let current_head = control.head()?;
    if current_head.as_deref() != Some(control_head) {
        return Err(HarnessError::Control {
            reason: "control head moved while collecting project snapshot".to_owned(),
            code: ErrorCode::ConflictControlHeadMoved,
        });
    }

    Ok(ProjectSnapshot {
        schema: PROJECT_SNAPSHOT_SCHEMA.to_owned(),
        project_id: config.project_id.to_string(),
        project_head,
        authority_head,
        control_head: control_head.to_owned(),
        captured_at,
        cycle_state_counts,
        card_state_counts,
        active_cards,
        gate_metrics,
        review_metrics,
        integration,
        silent_leases,
        consistency: ConsistencyDiagnostics {
            authoritative_source: "control_git_object".to_owned(),
            ephemeral_source: "control_worktree_overlay".to_owned(),
            control_worktree_clean,
            control_head_unchanged: true,
            lock_state,
            unresolved_journal_operations,
            diagnostics,
        },
    })
}

#[derive(Clone)]
struct CapturedReceipt {
    relative_path: String,
    receipt: Receipt,
}

struct SnapshotRecords<'a> {
    cycles: &'a [CycleRecord],
    cards: &'a [CardRecord],
    states: &'a [StoredCardState],
    events: &'a [Event],
    receipts: &'a [CapturedReceipt],
    reviews: &'a [ReviewRecord],
    handoffs: &'a [HandoffRecord],
    integrations: &'a [IntegrationRecord],
    leases: &'a [LeaseRecord],
}

fn read_json_at<T: DeserializeOwned>(
    control: &ControlRepository,
    head: &str,
    relative: &str,
) -> Result<T, HarnessError> {
    let object = format!("{head}:{relative}");
    let output = crate::git::command::run(&control.scope(), ["show", object.as_str()])?;
    if !output.success() {
        return Err(control_corrupt(
            "required control record is missing from the captured commit",
        ));
    }
    serde_json::from_slice(&output.stdout_bytes)
        .map_err(|_| control_corrupt("a control record in the captured commit is malformed"))
}

fn read_json_files<T: DeserializeOwned>(
    control: &ControlRepository,
    head: &str,
    prefix: &str,
) -> Result<Vec<T>, HarnessError> {
    list_files(control, head, prefix)?
        .iter()
        .filter(|name| is_json(name))
        .map(|name| read_json_at(control, head, name))
        .collect()
}

fn read_receipts(
    control: &ControlRepository,
    head: &str,
) -> Result<Vec<CapturedReceipt>, HarnessError> {
    list_files(control, head, RECEIPT_DIR)?
        .iter()
        .filter(|name| is_json(name))
        .map(|relative_path| {
            Ok(CapturedReceipt {
                relative_path: relative_path.clone(),
                receipt: read_json_at(control, head, relative_path)?,
            })
        })
        .collect()
}

fn list_files(
    control: &ControlRepository,
    head: &str,
    prefix: &str,
) -> Result<Vec<String>, HarnessError> {
    let output = crate::git::command::run(
        &control.scope(),
        ["ls-tree", "-r", "--name-only", head, "--", prefix],
    )?;
    if !output.success() {
        return Err(control_corrupt(
            "could not enumerate the captured control commit",
        ));
    }
    Ok(output.trimmed_stdout().lines().map(str::to_owned).collect())
}

fn read_cards(
    control: &ControlRepository,
    head: &str,
) -> Result<(Vec<CardRecord>, Vec<StoredCardState>), HarnessError> {
    let mut cards = Vec::new();
    let mut states = Vec::new();
    for relative in list_files(control, head, CARD_DIR)? {
        if !is_json(&relative) {
            continue;
        }
        if relative.ends_with("/state.json") {
            states.push(read_json_at(control, head, &relative)?);
        } else if relative.contains("/r") {
            cards.push(read_json_at(control, head, &relative)?);
        }
    }
    Ok((cards, states))
}

fn is_json(relative: &str) -> bool {
    std::path::Path::new(relative)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}

fn validate_receipts(
    config: &ProjectConfig,
    records: &SnapshotRecords<'_>,
) -> Result<Vec<Receipt>, HarnessError> {
    let mut receipt_ids = BTreeSet::new();
    for captured in records.receipts {
        let receipt_id = captured.receipt.receipt_id.to_string();
        if !receipt_ids.insert(receipt_id) {
            return Err(control_corrupt("duplicate_receipt_id"));
        }
    }

    let mut valid = Vec::with_capacity(records.receipts.len());
    for captured in records.receipts {
        let receipt = &captured.receipt;
        let expected_path = format!("{RECEIPT_DIR}/{}.json", receipt.receipt_id);
        if captured.relative_path != expected_path {
            return Err(control_corrupt("receipt_file_name_mismatch"));
        }
        if receipt.project_id != config.project_id {
            return Err(control_corrupt("receipt_project_mismatch"));
        }

        match (
            receipt.card_id.as_ref(),
            receipt.card_digest.as_ref(),
            receipt.integration_id.as_ref(),
        ) {
            (Some(card_id), Some(card_digest), None) => {
                validate_card_subject(records, receipt, card_id, card_digest)?;
            }
            (None, None, Some(integration_id)) => {
                validate_integration_subject(records, receipt, integration_id)?;
            }
            _ => return Err(control_corrupt("receipt_subject_invalid")),
        }
        valid.push(receipt.clone());
    }
    Ok(valid)
}

fn validate_card_subject(
    records: &SnapshotRecords<'_>,
    receipt: &Receipt,
    card_id: &crate::domain::ids::CardId,
    card_digest: &Digest,
) -> Result<(), HarnessError> {
    let Some(cycle) = records
        .cycles
        .iter()
        .find(|cycle| cycle.cycle_id == receipt.cycle_id)
    else {
        return Err(control_corrupt("receipt_cycle_reference_missing"));
    };
    if !cycle.card_ids.contains(card_id) {
        return Err(control_corrupt("receipt_card_cycle_mismatch"));
    }
    let card_matches = records.cards.iter().any(|card| {
        card.card_id == *card_id
            && card.cycle_id == receipt.cycle_id
            && card.digest().ok().as_ref() == Some(card_digest)
    });
    if !card_matches {
        return Err(control_corrupt("receipt_card_reference_invalid"));
    }
    Ok(())
}

fn validate_integration_subject(
    records: &SnapshotRecords<'_>,
    receipt: &Receipt,
    integration_id: &crate::domain::ids::IntegrationId,
) -> Result<(), HarnessError> {
    let Some(cycle) = records
        .cycles
        .iter()
        .find(|cycle| cycle.cycle_id == receipt.cycle_id)
    else {
        return Err(control_corrupt("receipt_cycle_reference_missing"));
    };
    let Some(integration) = records
        .integrations
        .iter()
        .find(|integration| integration.integration_id == *integration_id)
    else {
        return Err(control_corrupt("receipt_integration_reference_invalid"));
    };
    if integration.cycle_id != receipt.cycle_id {
        return Err(control_corrupt("receipt_integration_cycle_mismatch"));
    }
    for member in &integration.members {
        if !cycle.card_ids.contains(&member.card_id)
            || !records.cards.iter().any(|card| {
                card.card_id == member.card_id
                    && card.cycle_id == receipt.cycle_id
                    && card.digest().ok().as_ref() == Some(&member.card_digest)
            })
        {
            return Err(control_corrupt("receipt_integration_member_invalid"));
        }
    }
    Ok(())
}

fn validate_subjects(
    config: &ProjectConfig,
    records: &SnapshotRecords<'_>,
    diagnostics: &mut Vec<String>,
) {
    let cycle_ids: BTreeSet<_> = records.cycles.iter().map(|cycle| &cycle.cycle_id).collect();
    let card_ids: BTreeSet<_> = records.cards.iter().map(|card| &card.card_id).collect();
    if records.cycles.iter().any(|cycle| {
        cycle.project_revision
            != Digest::of_canonical(config).unwrap_or_else(|_| cycle.project_revision.clone())
    }) {
        diagnostics.push("cycle_project_revision_mismatch".to_owned());
    }
    for event in records.events {
        if event.project_id != config.project_id {
            diagnostics.push("event_project_mismatch".to_owned());
        }
        if event
            .cycle_id
            .as_ref()
            .is_some_and(|id| !cycle_ids.contains(id))
        {
            diagnostics.push("event_cycle_missing".to_owned());
        }
        if event
            .card_id
            .as_ref()
            .is_some_and(|id| !card_ids.contains(id))
        {
            diagnostics.push("event_card_missing".to_owned());
        }
    }
    for review in records.reviews {
        if !card_ids.contains(&review.card_id) {
            diagnostics.push("review_card_missing".to_owned());
        }
    }
    for handoff in records.handoffs {
        if !card_ids.contains(&handoff.card_id) {
            diagnostics.push("handoff_card_missing".to_owned());
        }
    }
    for integration in records.integrations {
        if !cycle_ids.contains(&integration.cycle_id) {
            diagnostics.push("integration_cycle_missing".to_owned());
        }
    }
    for lease in records.leases {
        if !card_ids.contains(&lease.card_id) {
            diagnostics.push("lease_card_missing".to_owned());
        }
    }
    for state in records.states {
        if !card_ids.contains(&state.card_id) {
            diagnostics.push("card_state_record_missing".to_owned());
        }
    }
}

fn control_corrupt(reason: &str) -> HarnessError {
    HarnessError::Control {
        reason: format!("project snapshot receipt integrity: {reason}"),
        code: ErrorCode::InternalControlCorrupt,
    }
}
