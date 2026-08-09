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
        handoff::{HANDOFF_DIR, HandoffRecord, HandoffStatus},
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
    let raw_cycles: Vec<Captured<CycleRecord>> = read_json_files(control, control_head, CYCLE_DIR)?;
    let raw_events: Vec<Captured<Event>> = read_json_files(control, control_head, EVENT_DIR)?;
    let (cards, card_states) = read_cards(control, control_head)?;
    let raw_receipts: Vec<Captured<Receipt>> = read_json_files(control, control_head, RECEIPT_DIR)?;
    let raw_reviews: Vec<Captured<ReviewRecord>> =
        read_json_files(control, control_head, REVIEW_DIR)?;
    let raw_handoffs: Vec<Captured<HandoffRecord>> =
        read_json_files(control, control_head, HANDOFF_DIR)?;
    let raw_integrations: Vec<Captured<IntegrationRecord>> =
        read_json_files(control, control_head, INTEGRATION_DIR)?;
    let raw_leases: Vec<Captured<LeaseRecord>> = read_json_files(control, control_head, LEASE_DIR)?;

    let records = SnapshotRecords {
        cycles: &raw_cycles,
        cards: &cards,
        states: &card_states,
        events: &raw_events,
        receipts: &raw_receipts,
        reviews: &raw_reviews,
        handoffs: &raw_handoffs,
        integrations: &raw_integrations,
        leases: &raw_leases,
    };
    let receipts = validate_records(control, control_head, &config, &records, &mut diagnostics)?;
    let cycles = into_records(raw_cycles);
    let events = into_records(raw_events);
    let cards = into_records(cards);
    let card_states = into_records(card_states);
    let reviews = into_records(raw_reviews);
    let handoffs = into_records(raw_handoffs);
    let integrations = into_records(raw_integrations);
    let leases = into_records(raw_leases);

    let (cycle_state_counts, cycle_status_diagnostics) =
        project_snapshot_metrics::cycle_counts(&cycles, &events);
    diagnostics.extend(cycle_status_diagnostics);
    let card_state_counts = project_snapshot_metrics::card_counts(&card_states);
    let active_cards =
        project_snapshot_metrics::active_cards(&cards, &card_states, &events, &leases, captured_at);
    let gate_metrics = project_snapshot_metrics::gate_metrics(&receipts);
    let test_metrics = project_snapshot_metrics::test_metrics(&receipts)?;
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
        test_metrics,
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
struct Captured<T> {
    relative_path: String,
    record: T,
    /// Digest of the canonical JSON object as captured from the control blob.
    ///
    /// This is intentionally separate from a typed record's re-serialized
    /// digest: deserialization can supply defaults for fields that were absent
    /// in a historical record, and those defaults must not rewrite its
    /// identity during a read-only snapshot.
    canonical_digest: Digest,
}

struct SnapshotRecords<'a> {
    cycles: &'a [Captured<CycleRecord>],
    cards: &'a [Captured<CardRecord>],
    states: &'a [Captured<StoredCardState>],
    events: &'a [Captured<Event>],
    receipts: &'a [Captured<Receipt>],
    reviews: &'a [Captured<ReviewRecord>],
    handoffs: &'a [Captured<HandoffRecord>],
    integrations: &'a [Captured<IntegrationRecord>],
    leases: &'a [Captured<LeaseRecord>],
}

fn read_json_at<T: DeserializeOwned>(
    control: &ControlRepository,
    head: &str,
    relative: &str,
) -> Result<T, HarnessError> {
    Ok(read_captured_json_at(control, head, relative)?.record)
}

fn read_captured_json_at<T: DeserializeOwned>(
    control: &ControlRepository,
    head: &str,
    relative: &str,
) -> Result<Captured<T>, HarnessError> {
    let object = format!("{head}:{relative}");
    let output = crate::git::command::run(&control.scope(), ["show", object.as_str()])?;
    if !output.success() {
        return Err(control_corrupt(
            "required control record is missing from the captured commit",
        ));
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout_bytes)
        .map_err(|_| control_corrupt("a control record in the captured commit is malformed"))?;
    let canonical_digest = Digest::of_canonical(&value)
        .map_err(|_| control_corrupt("a control record in the captured commit is malformed"))?;
    let record = serde_json::from_value(value)
        .map_err(|_| control_corrupt("a control record in the captured commit is malformed"))?;
    Ok(Captured {
        relative_path: relative.to_owned(),
        record,
        canonical_digest,
    })
}

fn read_json_files<T: DeserializeOwned>(
    control: &ControlRepository,
    head: &str,
    prefix: &str,
) -> Result<Vec<Captured<T>>, HarnessError> {
    list_files(control, head, prefix)?
        .iter()
        .filter(|name| is_json(name))
        .map(|relative_path| read_captured_json_at(control, head, relative_path))
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

fn read_cards(control: &ControlRepository, head: &str) -> Result<CardRecords, HarnessError> {
    let mut cards = Vec::new();
    let mut states = Vec::new();
    for relative in list_files(control, head, CARD_DIR)? {
        if !is_json(&relative) {
            continue;
        }
        if relative.ends_with("/state.json") {
            states.push(read_captured_json_at(control, head, &relative)?);
        } else if relative.contains("/r") {
            cards.push(read_captured_json_at(control, head, &relative)?);
        }
    }
    Ok((cards, states))
}

type CardRecords = (Vec<Captured<CardRecord>>, Vec<Captured<StoredCardState>>);

fn into_records<T>(records: Vec<Captured<T>>) -> Vec<T> {
    records
        .into_iter()
        .map(|captured| captured.record)
        .collect()
}

fn is_json(relative: &str) -> bool {
    std::path::Path::new(relative)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}

fn validate_records(
    control: &ControlRepository,
    control_head: &str,
    config: &ProjectConfig,
    records: &SnapshotRecords<'_>,
    diagnostics: &mut Vec<String>,
) -> Result<Vec<Receipt>, HarnessError> {
    validate_unique_paths(
        records.cycles,
        "cycle",
        |captured| captured.record.cycle_id.clone(),
        |cycle| CycleRecord::relative_path(&cycle.cycle_id),
    )?;
    validate_unique_paths(
        records.cards,
        "card_revision",
        |captured| (captured.record.card_id.clone(), captured.record.revision),
        |card| CardRecord::relative_path(&card.card_id, card.revision),
    )?;
    validate_unique_paths(
        records.states,
        "card_state",
        |captured| captured.record.card_id.clone(),
        |state| format!("{CARD_DIR}/{}/state.json", state.card_id),
    )?;
    validate_unique_paths(
        records.events,
        "event",
        |captured| captured.record.event_id.clone(),
        |event| Event::relative_path(&event.event_id),
    )?;
    validate_unique_paths(
        records.leases,
        "lease",
        |captured| captured.record.lease_id.clone(),
        |lease| LeaseRecord::relative_path(&lease.lease_id),
    )?;
    validate_unique_paths(
        records.handoffs,
        "handoff",
        |captured| captured.record.handoff_id.clone(),
        |handoff| HandoffRecord::relative_path(&handoff.handoff_id),
    )?;
    validate_unique_paths(
        records.reviews,
        "review",
        |captured| captured.record.review_id.clone(),
        |review| ReviewRecord::relative_path(&review.review_id),
    )?;
    validate_unique_paths(
        records.integrations,
        "integration",
        |captured| captured.record.integration_id.clone(),
        |integration| IntegrationRecord::relative_path(&integration.integration_id),
    )?;

    let receipts = validate_receipts(config, records)?;
    validate_subjects(control, control_head, config, records, diagnostics)?;
    Ok(receipts)
}

fn validate_unique_paths<T, K: Ord>(
    records: &[Captured<T>],
    family: &str,
    identity: impl Fn(&Captured<T>) -> K,
    expected_path: impl Fn(&T) -> String,
) -> Result<(), HarnessError> {
    let mut identities = BTreeSet::new();
    for captured in records {
        if !identities.insert(identity(captured)) {
            return Err(control_corrupt(format!("duplicate_{family}_identity")));
        }
    }
    for captured in records {
        if captured.relative_path != expected_path(&captured.record) {
            return Err(control_corrupt(format!("{family}_file_name_mismatch")));
        }
    }
    Ok(())
}

fn validate_receipts(
    config: &ProjectConfig,
    records: &SnapshotRecords<'_>,
) -> Result<Vec<Receipt>, HarnessError> {
    let mut receipt_ids = BTreeSet::new();
    for captured in records.receipts {
        let receipt_id = captured.record.receipt_id.to_string();
        if !receipt_ids.insert(receipt_id) {
            return Err(receipt_corrupt("duplicate_receipt_id"));
        }
    }

    let mut valid = Vec::with_capacity(records.receipts.len());
    for captured in records.receipts {
        let receipt = &captured.record;
        let expected_path = format!("{RECEIPT_DIR}/{}.json", receipt.receipt_id);
        if captured.relative_path != expected_path {
            return Err(receipt_corrupt("receipt_file_name_mismatch"));
        }
        if receipt.project_id != config.project_id {
            return Err(receipt_corrupt("receipt_project_mismatch"));
        }
        if receipt
            .test_results
            .as_ref()
            .is_some_and(|results| results.validate().is_err())
        {
            return Err(receipt_corrupt("test_result_summary_invalid"));
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
            _ => return Err(receipt_corrupt("receipt_subject_invalid")),
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
        .find(|cycle| cycle.record.cycle_id == receipt.cycle_id)
    else {
        return Err(receipt_corrupt("receipt_cycle_reference_missing"));
    };
    if !cycle.record.card_ids.contains(card_id) {
        return Err(receipt_corrupt("receipt_card_cycle_mismatch"));
    }
    let card_matches = records.cards.iter().any(|card| {
        card.record.card_id == *card_id
            && card.record.cycle_id == receipt.cycle_id
            && card.record.digest().ok().as_ref() == Some(card_digest)
    });
    if !card_matches {
        return Err(receipt_corrupt("receipt_card_reference_invalid"));
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
        .find(|cycle| cycle.record.cycle_id == receipt.cycle_id)
    else {
        return Err(receipt_corrupt("receipt_cycle_reference_missing"));
    };
    let Some(integration) = records
        .integrations
        .iter()
        .find(|integration| integration.record.integration_id == *integration_id)
    else {
        return Err(receipt_corrupt("receipt_integration_reference_invalid"));
    };
    if integration.record.cycle_id != receipt.cycle_id {
        return Err(receipt_corrupt("receipt_integration_cycle_mismatch"));
    }
    for member in &integration.record.members {
        if !cycle.record.card_ids.contains(&member.card_id)
            || !records.cards.iter().any(|card| {
                card.record.card_id == member.card_id
                    && card.record.cycle_id == receipt.cycle_id
                    && card.record.digest().ok().as_ref() == Some(&member.card_digest)
            })
        {
            return Err(receipt_corrupt("receipt_integration_member_invalid"));
        }
    }
    Ok(())
}

fn validate_subjects(
    control: &ControlRepository,
    control_head: &str,
    config: &ProjectConfig,
    records: &SnapshotRecords<'_>,
    diagnostics: &mut Vec<String>,
) -> Result<(), HarnessError> {
    if records.cycles.iter().any(|cycle| {
        cycle.record.project_revision
            != Digest::of_canonical(config)
                .unwrap_or_else(|_| cycle.record.project_revision.clone())
    }) {
        diagnostics.push("cycle_project_revision_mismatch".to_owned());
    }
    validate_cycle_card_refs(records)?;
    validate_card_cycle_refs(records)?;
    validate_card_states(records)?;
    validate_events(config, records)?;
    validate_reviews(control, control_head, records)?;
    validate_handoffs(records)?;
    validate_integrations(records)?;
    validate_leases(records)?;
    validate_review_supersedes(records)?;
    Ok(())
}

fn validate_cycle_card_refs(records: &SnapshotRecords<'_>) -> Result<(), HarnessError> {
    for cycle in records.cycles {
        for card_id in &cycle.record.card_ids {
            if !records.cards.iter().any(|card| {
                card.record.card_id == *card_id && card.record.cycle_id == cycle.record.cycle_id
            }) {
                return Err(control_corrupt("cycle_card_reference_invalid"));
            }
        }
    }
    Ok(())
}

fn validate_card_cycle_refs(records: &SnapshotRecords<'_>) -> Result<(), HarnessError> {
    let cycle_ids: BTreeSet<_> = records
        .cycles
        .iter()
        .map(|cycle| cycle.record.cycle_id.clone())
        .collect();
    for card in records.cards {
        if !cycle_ids.contains(&card.record.cycle_id)
            || !records.cycles.iter().any(|cycle| {
                cycle.record.cycle_id == card.record.cycle_id
                    && cycle.record.card_ids.contains(&card.record.card_id)
            })
        {
            return Err(control_corrupt("card_cycle_reference_invalid"));
        }
    }
    Ok(())
}

fn validate_card_states(records: &SnapshotRecords<'_>) -> Result<(), HarnessError> {
    for state in records.states {
        if !records.cards.iter().any(|card| {
            card.record.card_id == state.record.card_id
                && card.record.revision == state.record.current_revision
                && card.record.digest().ok().as_ref() == Some(&state.record.current_digest)
        }) {
            return Err(control_corrupt("card_state_reference_invalid"));
        }
    }
    Ok(())
}

fn validate_events(
    config: &ProjectConfig,
    records: &SnapshotRecords<'_>,
) -> Result<(), HarnessError> {
    let cycle_ids: BTreeSet<_> = records
        .cycles
        .iter()
        .map(|cycle| cycle.record.cycle_id.clone())
        .collect();
    let card_ids: BTreeSet<_> = records
        .cards
        .iter()
        .map(|card| card.record.card_id.clone())
        .collect();
    for event in records.events {
        let event = &event.record;
        if event.project_id != config.project_id {
            return Err(control_corrupt("event_project_mismatch"));
        }
        if event
            .cycle_id
            .as_ref()
            .is_some_and(|id| !cycle_ids.contains(id))
        {
            return Err(control_corrupt("event_cycle_reference_invalid"));
        }
        if event
            .card_id
            .as_ref()
            .is_some_and(|id| !card_ids.contains(id))
        {
            return Err(control_corrupt("event_card_reference_invalid"));
        }
        match (&event.card_id, &event.card_revision, &event.card_digest) {
            (None, None, None) => {}
            (Some(card_id), Some(revision), Some(digest)) => {
                let Some(cycle_id) = event.cycle_id.as_ref() else {
                    return Err(control_corrupt("event_card_cycle_reference_invalid"));
                };
                if !records.cards.iter().any(|card| {
                    card.record.card_id == *card_id
                        && card.record.cycle_id == *cycle_id
                        && card.record.revision == *revision
                        && card.record.digest().ok().as_ref() == Some(digest)
                }) {
                    return Err(control_corrupt("event_card_revision_reference_invalid"));
                }
            }
            _ => return Err(control_corrupt("event_card_subject_invalid")),
        }
    }
    Ok(())
}

fn validate_reviews(
    control: &ControlRepository,
    control_head: &str,
    records: &SnapshotRecords<'_>,
) -> Result<(), HarnessError> {
    for review in records.reviews {
        let review = &review.record;
        if !records.cards.iter().any(|card| {
            card.record.card_id == review.card_id
                && card.record.cycle_id == review.cycle_id
                && card.record.revision == review.card_revision
                && card.record.digest().ok().as_ref() == Some(&review.card_digest)
        }) {
            return Err(control_corrupt("review_card_reference_invalid"));
        }
        let Some(handoff) = records
            .handoffs
            .iter()
            .find(|handoff| handoff.record.handoff_id == review.handoff_id)
        else {
            return Err(control_corrupt("review_handoff_reference_invalid"));
        };
        if handoff.record.card_id != review.card_id
            || handoff.record.cycle_id != review.cycle_id
            || handoff.record.card_digest != review.card_digest
            || !handoff_binding_matches(control, control_head, handoff, &review.handoff_digest)?
        {
            return Err(control_corrupt("review_handoff_binding_invalid"));
        }
    }
    Ok(())
}

/// Allows a review to keep naming the exact handoff it saw when that handoff
/// was later revoked through the normal delivery-side transition.
///
/// The current blob must either carry the review digest, or be a revoked
/// version whose exact prior blob at a commit reachable from `control_head`
/// carries it. The historical record must deserialize and match the current
/// record in every field after normalizing only `active` to the current
/// `revoked` status. This keeps revocation queryable without turning a changed
/// handoff into a trusted review binding.
fn handoff_binding_matches(
    control: &ControlRepository,
    control_head: &str,
    handoff: &Captured<HandoffRecord>,
    review_digest: &Digest,
) -> Result<bool, HarnessError> {
    if handoff.canonical_digest == *review_digest {
        return Ok(true);
    }
    if handoff.record.status != HandoffStatus::Revoked {
        return Ok(false);
    }

    let output = crate::git::command::run(
        &control.scope(),
        [
            "log",
            "--format=%H",
            control_head,
            "--",
            handoff.relative_path.as_str(),
        ],
    )?;
    if !output.success() {
        return Err(control_corrupt(
            "could not inspect the captured handoff history",
        ));
    }

    for historical_head in output.trimmed_stdout().lines() {
        if historical_head == control_head {
            continue;
        }
        let historical: Captured<HandoffRecord> =
            read_captured_json_at(control, historical_head, &handoff.relative_path)?;
        if historical.canonical_digest != *review_digest {
            continue;
        }
        if historical.record.status != HandoffStatus::Active {
            return Ok(false);
        }
        let mut current = handoff.record.clone();
        current.status = HandoffStatus::Active;
        return Ok(current == historical.record);
    }
    Ok(false)
}

fn validate_handoffs(records: &SnapshotRecords<'_>) -> Result<(), HarnessError> {
    for handoff in records.handoffs {
        let handoff = &handoff.record;
        if !records.cards.iter().any(|card| {
            card.record.card_id == handoff.card_id
                && card.record.cycle_id == handoff.cycle_id
                && card.record.revision == handoff.card_revision
                && card.record.digest().ok().as_ref() == Some(&handoff.card_digest)
        }) {
            return Err(control_corrupt("handoff_card_reference_invalid"));
        }
        for evidence in &handoff.receipts {
            if !records
                .receipts
                .iter()
                .any(|receipt| receipt.record.receipt_id.to_string() == evidence.receipt_id)
            {
                return Err(control_corrupt("handoff_receipt_reference_invalid"));
            }
        }
    }
    Ok(())
}

fn validate_integrations(records: &SnapshotRecords<'_>) -> Result<(), HarnessError> {
    for integration in records.integrations {
        let integration = &integration.record;
        let Some(cycle) = records
            .cycles
            .iter()
            .find(|cycle| cycle.record.cycle_id == integration.cycle_id)
        else {
            return Err(control_corrupt("integration_cycle_reference_invalid"));
        };
        let mut member_ids = BTreeSet::new();
        for member in &integration.members {
            if !member_ids.insert(member.card_id.clone())
                || !cycle.record.card_ids.contains(&member.card_id)
                || !records.cards.iter().any(|card| {
                    card.record.card_id == member.card_id
                        && card.record.cycle_id == integration.cycle_id
                        && card.record.digest().ok().as_ref() == Some(&member.card_digest)
                })
            {
                return Err(control_corrupt("integration_member_reference_invalid"));
            }
        }
        for card_id in &integration.abandoned_card_ids {
            if !cycle.record.card_ids.contains(card_id) {
                return Err(control_corrupt(
                    "integration_abandoned_card_reference_invalid",
                ));
            }
        }
    }
    Ok(())
}

fn validate_leases(records: &SnapshotRecords<'_>) -> Result<(), HarnessError> {
    for lease in records.leases {
        let lease = &lease.record;
        if !records.cards.iter().any(|card| {
            card.record.card_id == lease.card_id && card.record.revision == lease.card_revision
        }) {
            return Err(control_corrupt("lease_card_reference_invalid"));
        }
    }
    Ok(())
}

fn validate_review_supersedes(records: &SnapshotRecords<'_>) -> Result<(), HarnessError> {
    for review in records.reviews {
        if let Some(superseded) = &review.record.supersedes
            && !records.reviews.iter().any(|candidate| {
                candidate.record.review_id == *superseded
                    && candidate.record.card_id == review.record.card_id
                    && candidate.record.cycle_id == review.record.cycle_id
            })
        {
            return Err(control_corrupt("review_supersedes_reference_invalid"));
        }
    }
    Ok(())
}

fn control_corrupt(reason: impl Into<String>) -> HarnessError {
    HarnessError::Control {
        reason: format!(
            "project snapshot durable-record integrity: {}",
            reason.into()
        ),
        code: ErrorCode::InternalControlCorrupt,
    }
}

fn receipt_corrupt(reason: &str) -> HarnessError {
    HarnessError::Control {
        reason: format!("project snapshot receipt integrity: {reason}"),
        code: ErrorCode::InternalControlCorrupt,
    }
}
