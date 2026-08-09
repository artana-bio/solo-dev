use std::sync::atomic::{AtomicU64, Ordering};

use crate::{
    domain::assurance::{ProbeKind, ProbeResult},
    error::HarnessError,
};

static NEXT_RUN_ID: AtomicU64 = AtomicU64::new(1);

pub(super) fn next_run_id(kind: ProbeKind) -> String {
    format!(
        "{}-{}-{}",
        kind.name(),
        std::process::id(),
        NEXT_RUN_ID.fetch_add(1, Ordering::Relaxed)
    )
}

pub(super) fn error_code_from_text(text: &str) -> Option<String> {
    let start = text.find("CH-")?;
    let tail = &text[start..];
    Some(
        tail.split(|character: char| !character.is_ascii_uppercase() && character != '-')
            .next()?
            .to_owned(),
    )
}

pub(super) fn failed_probe(kind: ProbeKind, error: &HarnessError) -> ProbeResult {
    let detail = error.to_string();
    ProbeResult {
        run_id: next_run_id(kind),
        probe_id: kind.name().to_owned(),
        probe: kind.name().to_owned(),
        oracle: probe_command(kind).to_owned(),
        expected_error_code: Some(probe_expected(kind).to_owned()),
        observed_error_code: error_code_from_text(&detail),
        command_path: probe_command(kind).to_owned(),
        refused: false,
        classification: "executed_failed".to_owned(),
        network_declared: None,
        network_enforced: None,
        state_change_evidence: "probe setup or command failed before a valid refusal oracle"
            .to_owned(),
        cleanup_completed: true,
        detail,
    }
}

pub(super) fn probe_expected(kind: ProbeKind) -> &'static str {
    match kind {
        ProbeKind::OutOfScopeWrite => "CH-POLICY-CANDIDATE-OUT-OF-SCOPE",
        ProbeKind::StaleSha => "CH-POLICY-DELIVERED-SHA-MISMATCH",
        ProbeKind::SelfReview => "CH-POLICY-SELF-REVIEW",
        ProbeKind::SameSessionReview => "CH-POLICY-SAME-ACTOR",
        ProbeKind::MissingMutationReceipt => "CH-POLICY-INCOMPLETE-REVIEW",
        ProbeKind::MissingHumanAttestation => "CH-POLICY-RISK-REVIEW",
        ProbeKind::DeniedNetwork => "not_tested",
    }
}

pub(super) fn probe_command(kind: ProbeKind) -> &'static str {
    match kind {
        ProbeKind::OutOfScopeWrite | ProbeKind::StaleSha => "handoff.create",
        ProbeKind::SelfReview | ProbeKind::SameSessionReview => "review.begin",
        ProbeKind::MissingMutationReceipt | ProbeKind::MissingHumanAttestation => "review.record",
        ProbeKind::DeniedNetwork => "gate.run",
    }
}

pub(super) fn probe_error_code(output: &std::process::Output) -> Option<String> {
    serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .ok()
        .and_then(|value| value["error"]["code"].as_str().map(ToOwned::to_owned))
}
