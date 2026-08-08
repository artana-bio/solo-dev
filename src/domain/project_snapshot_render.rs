//! Concise rendering of the typed snapshot projection.

use std::fmt::Write as _;

use super::ProjectSnapshot;
use crate::domain::clock::Timestamp;

pub(super) fn render(snapshot: &ProjectSnapshot) -> String {
    let mut text = format!(
        "Project {} snapshot\ncontrol head: {}\nproject head: {}\nauthority head: {}",
        snapshot.project_id,
        snapshot.control_head,
        snapshot.project_head.as_deref().unwrap_or("unavailable"),
        snapshot.authority_head.as_deref().unwrap_or("unavailable"),
    );
    append_counts(&mut text, "cycles", &snapshot.cycle_state_counts);
    append_counts(&mut text, "cards", &snapshot.card_state_counts);
    let _ = write!(
        text,
        "\nactive cards: {}\ngates: {} attempts, {} failures, {} timeouts, {}ms\ntests: {} total, {} passed, {} failed, {} errors, {} skipped ({:?})\nreviews: {} returns, {} repair attempts\nintegration: {} ready card(s), {} blocker(s)\nsilent leases: {}",
        snapshot.active_cards.len(),
        snapshot.gate_metrics.attempts,
        snapshot.gate_metrics.failures,
        snapshot.gate_metrics.timeouts,
        snapshot.gate_metrics.duration_ms,
        snapshot.test_metrics.total,
        snapshot.test_metrics.passed,
        snapshot.test_metrics.failed,
        snapshot.test_metrics.errors,
        snapshot.test_metrics.skipped,
        snapshot.test_metrics.status.name(),
        snapshot.review_metrics.review_returns,
        snapshot.review_metrics.repair_attempts,
        snapshot.integration.ready_card_count,
        snapshot.integration.blockers.len(),
        snapshot.silent_leases.len(),
    );
    for card in &snapshot.active_cards {
        let _ = write!(
            text,
            "\n  {} {} actor={} age={}s last_activity={}",
            card.card_id,
            card.phase,
            card.actor_id.as_deref().unwrap_or("unassigned"),
            card.age_seconds,
            card.last_activity_at
                .as_ref()
                .map_or_else(|| "unavailable".to_owned(), Timestamp::to_rfc3339),
        );
    }
    let _ = write!(
        text,
        "\nconsistency: source={} ephemeral={} worktree={} lock={} journal={}",
        snapshot.consistency.authoritative_source,
        snapshot.consistency.ephemeral_source,
        if snapshot.consistency.control_worktree_clean {
            "clean"
        } else {
            "dirty"
        },
        snapshot.consistency.lock_state,
        snapshot.consistency.unresolved_journal_operations,
    );
    text
}

fn append_counts(text: &mut String, label: &str, counts: &std::collections::BTreeMap<String, u64>) {
    let rendered = counts
        .iter()
        .filter(|(_, count)| **count > 0)
        .map(|(name, count)| format!("{name}={count}"))
        .collect::<Vec<_>>()
        .join(", ");
    let _ = write!(
        text,
        "\n{label}: {}",
        if rendered.is_empty() {
            "none"
        } else {
            &rendered
        }
    );
}
