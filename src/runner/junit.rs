//! Attempt-bound structured `JUnit` evidence.
//!
//! This module binds the trusted declaration to one attempt. Filesystem
//! custody and XML parsing live in focused sibling modules; neither ever reads
//! stdout/stderr or searches for undeclared reports.

#[path = "junit_fs.rs"]
mod junit_fs;
#[path = "junit_xml.rs"]
mod junit_xml;

use std::{collections::BTreeSet, path::Path};

use crate::{
    domain::gate::GateDefinition,
    runner::receipt::{StructuredResultErrorCode, TestResultStatus, TestResultSummary},
};

/// Maximum XML payload retained for one declared report.
pub const MAX_JUNIT_BYTES: usize = 16 * 1024 * 1024;
/// Maximum test cases counted across one report.
pub const MAX_JUNIT_CASES: u64 = 10_000_000;

#[derive(Clone, Debug)]
pub(crate) struct AttemptBindings {
    before: Vec<Option<junit_fs::ReportBefore>>,
    invalid: Option<StructuredResultErrorCode>,
}

/// Capture pre-attempt report state without making report custody a runner
/// error. An invalid declaration still gets a gate attempt and an auditable
/// receipt after the process finishes.
pub(crate) fn capture_before(
    gate: &GateDefinition,
    working_directory: &Path,
    worktree: &Path,
    _attempt: u32,
) -> AttemptBindings {
    let mut bindings = AttemptBindings {
        before: Vec::with_capacity(gate.junit_reports.len()),
        invalid: None,
    };
    for declared in &gate.junit_reports {
        match junit_fs::capture_before(working_directory, worktree, declared) {
            Ok(state) => bindings.before.push(state),
            Err(error) => {
                bindings.before.push(None);
                bindings.invalid.get_or_insert(error);
            }
        }
    }
    bindings
}

/// Read, validate, and aggregate only the reports declared for this attempt.
/// Any evidence problem becomes a typed invalid summary so the caller can
/// persist the failed attempt rather than losing the audit record.
pub(crate) fn collect(
    gate: &GateDefinition,
    working_directory: &Path,
    worktree: &Path,
    bindings: &AttemptBindings,
    _attempt: u32,
) -> Option<TestResultSummary> {
    if gate.junit_reports.is_empty() {
        return None;
    }
    if let Some(error) = bindings.invalid {
        return Some(TestResultSummary::invalid(error));
    }
    if bindings.before.len() != gate.junit_reports.len() {
        return Some(TestResultSummary::invalid(
            StructuredResultErrorCode::ReadError,
        ));
    }

    let mut digests = BTreeSet::new();
    let mut summary = TestResultSummary::not_reported();
    for (index, declared) in gate.junit_reports.iter().enumerate() {
        let report = match junit_fs::read_report(working_directory, worktree, declared) {
            Ok(report) => report,
            Err(error) => return Some(TestResultSummary::invalid(error)),
        };
        if bindings.before[index].as_ref() == Some(&report.state) {
            return Some(TestResultSummary::invalid(StructuredResultErrorCode::Stale));
        }
        if !digests.insert(report.state.digest.clone()) {
            return Some(TestResultSummary::invalid(
                StructuredResultErrorCode::Duplicate,
            ));
        }
        let parsed = match junit_xml::parse_report(&report.bytes) {
            Ok(parsed) => parsed,
            Err(error) => return Some(TestResultSummary::invalid(error)),
        };
        if merge_summary(&mut summary, &parsed).is_err() {
            return Some(TestResultSummary::invalid(
                StructuredResultErrorCode::Inconsistent,
            ));
        }
    }
    Some(summary)
}

fn merge_summary(
    destination: &mut TestResultSummary,
    source: &TestResultSummary,
) -> Result<(), ()> {
    if source.status != TestResultStatus::Reported || source.error_code.is_some() {
        return Err(());
    }
    destination.status = TestResultStatus::Reported;
    destination.total = destination.total.checked_add(source.total).ok_or(())?;
    destination.passed = destination.passed.checked_add(source.passed).ok_or(())?;
    destination.failed = destination.failed.checked_add(source.failed).ok_or(())?;
    destination.errors = destination.errors.checked_add(source.errors).ok_or(())?;
    destination.skipped = destination.skipped.checked_add(source.skipped).ok_or(())?;
    destination.validate().map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        clock::FixedClock,
        gate::{GATE_SCHEMA, GateEnvironment, NetworkPolicy, RetryPolicy},
    };

    #[test]
    fn a_report_from_a_prior_attempt_is_recorded_as_stale() {
        let temp = tempfile::tempdir().unwrap();
        let worktree = temp.path().join("worktree");
        let logs = temp.path().join("logs");
        std::fs::create_dir_all(&worktree).unwrap();
        let gate = GateDefinition {
            schema: GATE_SCHEMA.to_owned(),
            gate_id: "gate.junit".to_owned(),
            purpose: Some("JUnit retry test".to_owned()),
            semantics: Some("a stale report cannot satisfy a later attempt".to_owned()),
            migration: None,
            reuse_justification: None,
            revision: 1,
            argv: vec![
                "sh".to_owned(),
                "-c".to_owned(),
                "test -f report.xml || printf '%s' '<testsuite tests=\"1\" failures=\"0\" errors=\"0\" skipped=\"0\"><testcase/></testsuite>' > report.xml".to_owned(),
            ],
            working_directory: ".".to_owned(),
            timeout_seconds: 30,
            environment: GateEnvironment::default(),
            network_policy: NetworkPolicy::Denied,
            retry_policy: RetryPolicy { max_attempts: 2 },
            artifacts: vec![],
            junit_reports: vec!["report.xml".to_owned()],
        };
        let clock = FixedClock::at_unix_seconds(1_785_196_800).unwrap();
        let first = crate::runner::run_attempt(&gate, &worktree, &logs, 1, &clock).unwrap();
        assert_eq!(
            first.test_results.unwrap().status,
            TestResultStatus::Reported
        );
        let second = crate::runner::run_attempt(&gate, &worktree, &logs, 2, &clock).unwrap();
        assert_eq!(
            second.test_results.clone().unwrap().error_code,
            Some(StructuredResultErrorCode::Stale)
        );
        assert!(!second.passed());
    }
}
