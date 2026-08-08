//! Pure durable-record projections used by the snapshot collector.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::{
    ActiveCardSnapshot, GateMetrics, IntegrationBlocker, IntegrationSummary, OpenIntegration,
    ReviewMetrics, SilentLeaseSnapshot,
};
use crate::{
    domain::{
        card::{CardRecord, CardState},
        clock::Timestamp,
        cycle::{CycleRecord, CycleStatus, status_from_events},
        digest::Digest,
        handoff::{HandoffRecord, HandoffStatus},
        integration::{IntegrationRecord, IntegrationStatus},
        lease::LeaseRecord,
        review::{Decision, ReviewRecord},
    },
    error::{ErrorCode, HarnessError},
    git::command::{GitScope, run},
    runner::receipt::{Receipt, Termination, TestResultStatus, TestResultSummary},
};

const CYCLE_STATE_NAMES: &[&str] = &[
    "draft",
    "active",
    "sealed",
    "integrating",
    "accepted",
    "landed",
    "closed",
    "blocked",
    "abandoned",
];
const CARD_STATE_NAMES: &[&str] = &[
    "draft",
    "ready",
    "leased",
    "active",
    "handed_off",
    "review_pending",
    "changes_requested",
    "approved",
    "integrating",
    "accepted",
    "landed",
    "closed",
    "blocked",
    "abandoned",
];

#[derive(Clone, Debug, Deserialize)]
pub(super) struct StoredCardState {
    #[allow(dead_code)]
    pub(super) schema: String,
    pub(super) card_id: crate::domain::ids::CardId,
    pub(super) state: CardState,
    pub(super) current_revision: u32,
    #[allow(dead_code)]
    pub(super) current_digest: Digest,
    #[allow(dead_code)]
    pub(super) canonical_algorithm: String,
}

pub(super) fn card_counts(states: &[StoredCardState]) -> BTreeMap<String, u64> {
    let mut counts = zeroed_counts(CARD_STATE_NAMES);
    for state in states {
        *counts.entry(state.state.name().to_owned()).or_default() += 1;
    }
    counts
}

pub(super) fn cycle_counts(
    cycles: &[CycleRecord],
    events: &[crate::control::event_store::Event],
) -> (BTreeMap<String, u64>, Vec<String>) {
    let mut counts = zeroed_counts(CYCLE_STATE_NAMES);
    let mut diagnostics = Vec::new();
    for cycle in cycles {
        let event_states = events
            .iter()
            .filter(|event| {
                event.cycle_id.as_ref() == Some(&cycle.cycle_id)
                    && event.card_id.is_none()
                    && event.event_type.starts_with("cycle.")
            })
            .filter_map(|event| event.next_state.as_deref());
        let derived = status_from_events(event_states);
        if derived != cycle.status {
            diagnostics.push("cycle_stored_state_disagrees_with_events".to_owned());
        }
        *counts.entry(derived.name().to_owned()).or_default() += 1;
    }
    (counts, diagnostics)
}

pub(super) fn active_cards(
    cards: &[CardRecord],
    states: &[StoredCardState],
    events: &[crate::control::event_store::Event],
    leases: &[LeaseRecord],
    now: Timestamp,
) -> Vec<ActiveCardSnapshot> {
    let card_records: BTreeMap<_, _> = cards
        .iter()
        .map(|card| ((card.card_id.clone(), card.revision), card))
        .collect();
    let mut snapshots = Vec::new();
    for state in states {
        if matches!(
            state.state,
            CardState::Draft | CardState::Ready | CardState::Closed | CardState::Abandoned
        ) {
            continue;
        }
        let Some(card) = card_records.get(&(state.card_id.clone(), state.current_revision)) else {
            continue;
        };
        let lease = leases
            .iter()
            .find(|lease| lease.card_id == state.card_id && lease.is_held());
        let latest_event = events
            .iter()
            .filter(|event| event.card_id.as_ref() == Some(&state.card_id))
            .max_by_key(|event| event.occurred_at);
        let lease_activity = lease.map(LeaseRecord::last_activity_at);
        let event_activity = latest_event.map(|event| event.occurred_at);
        let last_activity_at = [lease_activity, event_activity].into_iter().flatten().max();
        let actor_id = lease
            .map(|lease| lease.actor_id.clone())
            .or_else(|| latest_event.map(|event| event.actor_id.clone()));
        let phase_started_at = events
            .iter()
            .filter(|event| {
                event.card_id.as_ref() == Some(&state.card_id)
                    && event.next_state.as_deref() == Some(state.state.name())
            })
            .max_by_key(|event| event.occurred_at)
            .map(|event| event.occurred_at)
            .or(last_activity_at)
            .or(Some(card.created_at));
        snapshots.push(ActiveCardSnapshot {
            card_id: state.card_id.to_string(),
            cycle_id: card.cycle_id.to_string(),
            phase: state.state.name().to_owned(),
            actor_id,
            age_seconds: phase_started_at.map_or(0, |at| age_seconds(now, at)),
            last_activity_at,
        });
    }
    snapshots.sort_by(|left, right| left.card_id.cmp(&right.card_id));
    snapshots
}

pub(super) fn gate_metrics(receipts: &[Receipt]) -> GateMetrics {
    let mut metrics = GateMetrics::default();
    for receipt in receipts {
        metrics.attempts += 1;
        metrics.duration_ms = metrics.duration_ms.saturating_add(receipt.duration_ms);
        if !receipt.passed {
            metrics.failures += 1;
        }
        if receipt.termination == Termination::Timeout {
            metrics.timeouts += 1;
        }
        let gate = metrics.by_gate.entry(receipt.gate_id.clone()).or_default();
        gate.attempts += 1;
        gate.duration_ms = gate.duration_ms.saturating_add(receipt.duration_ms);
        if !receipt.passed {
            gate.failures += 1;
        }
        if receipt.termination == Termination::Timeout {
            gate.timeouts += 1;
        }
    }
    metrics
}

pub(super) fn test_metrics(receipts: &[Receipt]) -> Result<TestResultSummary, HarnessError> {
    let mut summary = TestResultSummary::not_reported();
    for receipt in receipts {
        let Some(test_results) = receipt.test_results.as_ref() else {
            continue;
        };
        test_results
            .validate()
            .map_err(|reason| HarnessError::Control {
                reason: format!("project snapshot receipt integrity: {reason}"),
                code: ErrorCode::InternalControlCorrupt,
            })?;
        if test_results.status == TestResultStatus::NotReported {
            continue;
        }
        summary.status = TestResultStatus::Reported;
        summary.total = summary
            .total
            .checked_add(test_results.total)
            .ok_or_else(|| HarnessError::Control {
                reason: "project snapshot test metrics overflow".to_owned(),
                code: ErrorCode::InternalControlCorrupt,
            })?;
        summary.passed = summary
            .passed
            .checked_add(test_results.passed)
            .ok_or_else(|| HarnessError::Control {
                reason: "project snapshot test metrics overflow".to_owned(),
                code: ErrorCode::InternalControlCorrupt,
            })?;
        summary.failed = summary
            .failed
            .checked_add(test_results.failed)
            .ok_or_else(|| HarnessError::Control {
                reason: "project snapshot test metrics overflow".to_owned(),
                code: ErrorCode::InternalControlCorrupt,
            })?;
        summary.errors = summary
            .errors
            .checked_add(test_results.errors)
            .ok_or_else(|| HarnessError::Control {
                reason: "project snapshot test metrics overflow".to_owned(),
                code: ErrorCode::InternalControlCorrupt,
            })?;
        summary.skipped = summary
            .skipped
            .checked_add(test_results.skipped)
            .ok_or_else(|| HarnessError::Control {
                reason: "project snapshot test metrics overflow".to_owned(),
                code: ErrorCode::InternalControlCorrupt,
            })?;
    }
    summary.validate().map_err(|reason| HarnessError::Control {
        reason: format!("project snapshot receipt integrity: {reason}"),
        code: ErrorCode::InternalControlCorrupt,
    })?;
    Ok(summary)
}

pub(super) fn review_metrics(events: &[crate::control::event_store::Event]) -> ReviewMetrics {
    let mut metrics = ReviewMetrics::default();
    for event in events {
        let Some(card_id) = event.card_id.as_ref() else {
            continue;
        };
        let Some(kind) = event
            .metadata
            .get("attempt_kind")
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let card = metrics.by_card.entry(card_id.to_string()).or_default();
        match kind {
            "review_return" => {
                metrics.review_returns += 1;
                card.review_returns += 1;
            }
            "repair_attempt" => {
                metrics.repair_attempts += 1;
                card.repair_attempts += 1;
            }
            _ => {}
        }
    }
    metrics
}

pub(super) fn silent_leases(leases: &[LeaseRecord], now: Timestamp) -> Vec<SilentLeaseSnapshot> {
    leases
        .iter()
        .filter(|lease| lease.is_silent(now))
        .map(|lease| SilentLeaseSnapshot {
            lease_id: lease.lease_id.to_string(),
            card_id: lease.card_id.to_string(),
            actor_id: lease.actor_id.clone(),
            granted_at: lease.granted_at,
            last_activity_at: lease.last_activity_at(),
            age_seconds: age_seconds(now, lease.last_activity_at()),
        })
        .collect()
}

fn current_candidate(
    card_id: &crate::domain::ids::CardId,
    handoff: Option<&HandoffRecord>,
    leases: &[LeaseRecord],
) -> Option<String> {
    let lease = leases
        .iter()
        .find(|lease| lease.card_id == *card_id && lease.is_held());
    if let Some(lease) = lease {
        let output = run(
            &GitScope::work_tree(&lease.worktree_path),
            ["rev-parse", "--verify", "HEAD"],
        )
        .ok()?;
        return output.success().then(|| output.trimmed_stdout().to_owned());
    }
    handoff.map(|handoff| handoff.candidate_sha.clone())
}

fn approved_card_blocker(
    card_id: &crate::domain::ids::CardId,
    state: &StoredCardState,
    handoffs: &[HandoffRecord],
    reviews: &[ReviewRecord],
    leases: &[LeaseRecord],
) -> Option<&'static str> {
    let handoff = handoffs
        .iter()
        .filter(|handoff| handoff.card_id == *card_id && handoff.status == HandoffStatus::Active)
        .max_by_key(|handoff| handoff.created_at);
    let candidate = current_candidate(card_id, handoff, leases);
    let approval = reviews
        .iter()
        .filter(|review| {
            review.card_id == *card_id
                && review.decision == Decision::Approved
                && review.card_digest == state.current_digest
                && candidate
                    .as_deref()
                    .is_some_and(|sha| review.candidate_sha == sha)
        })
        .max_by_key(|review| review.reviewed_at);
    match (handoff, candidate, approval) {
        (Some(handoff), Some(candidate), Some(_)) if handoff.candidate_sha == candidate => None,
        (None, _, _) => Some("handoff_missing"),
        (Some(_), None, _) => Some("candidate_unavailable"),
        (Some(handoff), Some(candidate), _) if handoff.candidate_sha != candidate => {
            Some("candidate_changed_since_approval")
        }
        (Some(_), Some(_), _) => Some("approval_missing_or_stale"),
    }
}

pub(super) fn integration_summary(
    cycles: &[CycleRecord],
    cards: &[CardRecord],
    states: &[StoredCardState],
    integrations: &[IntegrationRecord],
    handoffs: &[HandoffRecord],
    reviews: &[ReviewRecord],
    leases: &[LeaseRecord],
) -> IntegrationSummary {
    let state_by_card: BTreeMap<_, _> =
        states.iter().map(|state| (&state.card_id, state)).collect();
    let card_by_id: BTreeMap<_, _> = cards.iter().map(|card| (&card.card_id, card)).collect();
    let mut summary = IntegrationSummary::default();
    for cycle in cycles {
        if cycle.status == CycleStatus::Blocked {
            summary.blockers.push(IntegrationBlocker {
                cycle_id: Some(cycle.cycle_id.to_string()),
                card_id: None,
                reason: "cycle_blocked".to_owned(),
            });
        }
        for card_id in &cycle.card_ids {
            let Some(state) = state_by_card.get(card_id) else {
                summary.blockers.push(IntegrationBlocker {
                    cycle_id: Some(cycle.cycle_id.to_string()),
                    card_id: Some(card_id.to_string()),
                    reason: "card_state_missing".to_owned(),
                });
                continue;
            };
            if state.state == CardState::Approved {
                if let Some(reason) =
                    approved_card_blocker(card_id, state, handoffs, reviews, leases)
                {
                    summary.blockers.push(IntegrationBlocker {
                        cycle_id: Some(cycle.cycle_id.to_string()),
                        card_id: Some(card_id.to_string()),
                        reason: reason.to_owned(),
                    });
                } else {
                    summary.ready_card_count += 1;
                }
            } else if !state.state.is_terminal() {
                summary.blockers.push(IntegrationBlocker {
                    cycle_id: Some(cycle.cycle_id.to_string()),
                    card_id: Some(card_id.to_string()),
                    reason: format!("card_state_{}", state.state.name()),
                });
            }
            if !card_by_id.contains_key(card_id) {
                summary.blockers.push(IntegrationBlocker {
                    cycle_id: Some(cycle.cycle_id.to_string()),
                    card_id: Some(card_id.to_string()),
                    reason: "card_record_missing".to_owned(),
                });
            }
        }
    }
    for integration in integrations {
        if integration.status.holds_lease() {
            if integration.status == IntegrationStatus::Blocked {
                summary.blockers.push(IntegrationBlocker {
                    cycle_id: Some(integration.cycle_id.to_string()),
                    card_id: None,
                    reason: "integration_blocked".to_owned(),
                });
            }
            summary.open_integrations.push(OpenIntegration {
                integration_id: integration.integration_id.to_string(),
                cycle_id: integration.cycle_id.to_string(),
                status: integration.status.name().to_owned(),
                member_count: integration.members.len() as u64,
            });
        }
    }
    summary.ready = summary.ready_card_count > 0;
    summary
}

fn zeroed_counts(names: &[&str]) -> BTreeMap<String, u64> {
    names.iter().map(|name| ((*name).to_owned(), 0)).collect()
}

fn age_seconds(now: Timestamp, then: Timestamp) -> i64 {
    now.unix_seconds()
        .saturating_sub(then.unix_seconds())
        .max(0)
}

#[allow(dead_code)]
fn _keep_cycle_record_used(_: &CycleRecord) {}
