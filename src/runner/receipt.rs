//! Gate receipts: the structured result of one gate run against one commit.
//!
//! Section 10.6 and invariant 7.4. A receipt names the exact gate definition
//! and the exact evaluated commit, so reusing it after either changes is
//! detectable rather than convenient. Passing and failing attempts are both
//! recorded, because a gate that only passed on its third try is not the same
//! evidence as one that passed on its first.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::domain::{
    clock::Timestamp,
    digest::Digest,
    ids::{CardId, CycleId, ProjectId, ReceiptId},
};

/// Schema identifier for a receipt.
pub const RECEIPT_SCHEMA: &str = "harness.receipt/v1";

/// Directory holding receipts, relative to the control repository.
pub const RECEIPT_DIR: &str = "receipts";

/// Directory holding gate logs, relative to the control repository.
///
/// Logs live outside Git history: they are large, uninteresting when passing,
/// and Section 14.3 gives them retention windows rather than permanence. The
/// receipt records their location and digest, which is what invariant 7.4.2
/// requires.
pub const LOG_DIR: &str = "logs";

/// How a gate process ended.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Termination {
    /// The process exited on its own before the timeout.
    Completed,
    /// The process was still running at its deadline and was terminated.
    Timeout,
    /// The process was killed by a signal.
    Signal,
    /// The runner could not execute or supervise the process.
    RunnerError,
}

impl Termination {
    /// Its stable serialized name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Timeout => "timeout",
            Self::Signal => "signal",
            Self::RunnerError => "runner_error",
        }
    }
}

/// One recorded gate run.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    /// Always [`RECEIPT_SCHEMA`].
    pub schema: String,
    /// Identifies this receipt.
    pub receipt_id: ReceiptId,
    /// The project it belongs to.
    pub project_id: ProjectId,
    /// The cycle it belongs to.
    pub cycle_id: CycleId,
    /// The card whose gate ran.
    pub card_id: CardId,
    /// The card digest in force.
    pub card_digest: Digest,
    /// The exact commit the gate ran against.
    pub evaluated_sha: String,
    /// The gate that ran.
    pub gate_id: String,
    /// The exact gate definition that ran.
    pub gate_digest: Digest,
    /// The harness version that produced the receipt.
    pub harness_version: String,
    /// What the run environment was.
    pub environment_fingerprint: String,
    /// When the run began.
    pub started_at: Timestamp,
    /// When it ended.
    pub finished_at: Timestamp,
    /// How long it took.
    pub duration_ms: u64,
    /// The process exit code, absent when signalled.
    pub exit_code: Option<i32>,
    /// How the process ended.
    pub termination: Termination,
    /// Digest of captured standard output.
    pub stdout_digest: Digest,
    /// Digest of captured standard error.
    pub stderr_digest: Digest,
    /// Digests of declared artifacts that were produced.
    pub artifact_digests: BTreeMap<String, String>,
    /// Where the logs were written.
    pub log_location: PathBuf,
    /// Which attempt this was, starting at 1.
    pub attempt: u32,
    /// True when this attempt satisfied the gate.
    pub passed: bool,
}

impl Receipt {
    /// Relative path of a receipt inside the control repository.
    #[must_use]
    pub fn relative_path(receipt_id: &ReceiptId) -> String {
        format!("{RECEIPT_DIR}/{receipt_id}.json")
    }

    /// True when this receipt still describes the given gate and commit.
    ///
    /// Invariant 7.4.3: a receipt for an earlier SHA is stale and cannot be
    /// reused. The gate digest is checked too, because re-running the same
    /// commit under a changed definition is a different check.
    #[must_use]
    pub fn is_current_for(&self, evaluated_sha: &str, gate_digest: &Digest) -> bool {
        self.evaluated_sha == evaluated_sha && self.gate_digest == *gate_digest
    }

    /// Explains why this receipt does not apply, if it does not.
    #[must_use]
    pub fn staleness(&self, evaluated_sha: &str, gate_digest: &Digest) -> Option<String> {
        if self.evaluated_sha != evaluated_sha {
            return Some(format!(
                "receipt evaluated {} but the candidate is now {evaluated_sha}",
                self.evaluated_sha
            ));
        }
        if self.gate_digest != *gate_digest {
            return Some(format!(
                "receipt used gate definition {} but the registry now holds {gate_digest}",
                self.gate_digest
            ));
        }
        None
    }
}

/// Where one attempt's logs are written.
///
/// Attempt number is part of the path, so a retry never overwrites the evidence
/// of the attempt before it. Section 14.2 requires every attempt to be
/// recorded, and a log that was overwritten is not a record.
#[must_use]
pub fn attempt_log_paths(log_root: &Path, gate_id: &str, attempt: u32) -> (PathBuf, PathBuf) {
    let directory = log_root.join(gate_id).join(format!("attempt-{attempt}"));
    (directory.join("stdout.log"), directory.join("stderr.log"))
}

/// Decides whether a set of attempts constitutes acceptable evidence.
///
/// Section 14.2: a gate that passes only after an undeclared retry remains
/// failed for acceptance. The rule is deliberately strict, because the whole
/// point of a gate is to distinguish "this works" from "this worked once".
#[must_use]
pub fn evidence_is_acceptable(attempts: &[Receipt], max_attempts: u32) -> bool {
    let Some(passing) = attempts.iter().find(|receipt| receipt.passed) else {
        return false;
    };
    passing.attempt <= max_attempts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::clock::{Clock as _, FixedClock};

    fn stamp() -> Timestamp {
        FixedClock::at_unix_seconds(1_785_196_800).unwrap().now()
    }

    fn receipt(attempt: u32, passed: bool) -> Receipt {
        Receipt {
            schema: RECEIPT_SCHEMA.to_owned(),
            receipt_id: format!("R-{attempt:06}").parse().unwrap(),
            project_id: "example".parse().unwrap(),
            cycle_id: "C-001".parse().unwrap(),
            card_id: "F-001".parse().unwrap(),
            card_digest: Digest::of_bytes(b"card"),
            evaluated_sha: "a".repeat(40),
            gate_id: "gate.unit".to_owned(),
            gate_digest: Digest::of_bytes(b"gate"),
            harness_version: "0.1.0".to_owned(),
            environment_fingerprint: "os=macos".to_owned(),
            started_at: stamp(),
            finished_at: stamp(),
            duration_ms: 120,
            exit_code: Some(i32::from(!passed)),
            termination: Termination::Completed,
            stdout_digest: Digest::of_bytes(b""),
            stderr_digest: Digest::of_bytes(b""),
            artifact_digests: BTreeMap::new(),
            log_location: PathBuf::from("/logs/gate.unit/attempt-1"),
            attempt,
            passed,
        }
    }

    #[test]
    fn every_termination_has_a_distinct_name() {
        let names: Vec<&str> = [
            Termination::Completed,
            Termination::Timeout,
            Termination::Signal,
            Termination::RunnerError,
        ]
        .iter()
        .map(|termination| termination.name())
        .collect();
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "{names:?}");
    }

    #[test]
    fn a_receipt_applies_only_to_its_exact_commit_and_gate() {
        let record = receipt(1, true);
        let gate = Digest::of_bytes(b"gate");
        assert!(record.is_current_for(&"a".repeat(40), &gate));
        assert!(!record.is_current_for(&"b".repeat(40), &gate));
        assert!(!record.is_current_for(&"a".repeat(40), &Digest::of_bytes(b"other gate")));
    }

    #[test]
    fn staleness_explains_which_binding_broke() {
        let record = receipt(1, true);
        let gate = Digest::of_bytes(b"gate");

        assert!(record.staleness(&"a".repeat(40), &gate).is_none());

        let moved_candidate = record.staleness(&"b".repeat(40), &gate).unwrap();
        assert!(
            moved_candidate.contains("candidate is now"),
            "{moved_candidate}"
        );

        let moved_gate = record
            .staleness(&"a".repeat(40), &Digest::of_bytes(b"other gate"))
            .unwrap();
        assert!(moved_gate.contains("registry now holds"), "{moved_gate}");
    }

    #[test]
    fn attempt_logs_never_overwrite_each_other() {
        let root = Path::new("/logs");
        let (first_out, first_err) = attempt_log_paths(root, "gate.unit", 1);
        let (second_out, second_err) = attempt_log_paths(root, "gate.unit", 2);
        assert_ne!(first_out, second_out);
        assert_ne!(first_err, second_err);
        assert_ne!(first_out, first_err);
    }

    #[test]
    fn a_first_attempt_pass_is_acceptable_evidence() {
        assert!(evidence_is_acceptable(&[receipt(1, true)], 1));
    }

    #[test]
    fn no_passing_attempt_is_never_acceptable() {
        assert!(!evidence_is_acceptable(&[receipt(1, false)], 1));
        assert!(!evidence_is_acceptable(
            &[receipt(1, false), receipt(2, false)],
            3
        ));
        assert!(!evidence_is_acceptable(&[], 1));
    }

    #[test]
    fn a_pass_beyond_the_declared_attempts_is_not_acceptable() {
        // The gate declares one attempt. Passing on the second means something
        // ran twice that was only authorized to run once, and a flaky pass is
        // not the same evidence as a deterministic one.
        assert!(!evidence_is_acceptable(
            &[receipt(1, false), receipt(2, true)],
            1
        ));
    }

    #[test]
    fn a_pass_within_a_declared_retry_budget_is_acceptable() {
        assert!(evidence_is_acceptable(
            &[receipt(1, false), receipt(2, true)],
            2
        ));
    }

    #[test]
    fn a_receipt_round_trips_and_rejects_unknown_fields() {
        let record = receipt(1, true);
        let encoded = serde_json::to_string_pretty(&record).unwrap();
        let decoded: Receipt = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, record);

        let mut value = serde_json::to_value(&record).unwrap();
        value["surprise"] = serde_json::json!(1);
        assert!(serde_json::from_value::<Receipt>(value).is_err());
    }

    #[test]
    fn a_receipt_records_a_failing_attempt_too() {
        // Invariant 7.4.1: passing and failing attempts are both recorded.
        let failed = receipt(1, false);
        assert!(!failed.passed);
        assert_eq!(failed.exit_code, Some(1));
        let encoded = serde_json::to_string(&failed).unwrap();
        assert!(encoded.contains("\"passed\":false"));
    }
}
