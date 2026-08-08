//! Bounded, explicitly declared `JUnit` report parsing.
//!
//! Reports are evidence only when the trusted gate definition names them and
//! the file can be shown to have changed during this exact attempt. Nothing in
//! this module reads stdout/stderr or searches the worktree for unrequested
//! files.

use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Read as _,
    path::{Path, PathBuf},
    time::SystemTime,
};

use quick_xml::{Reader, events::Event};

use crate::{
    domain::{digest::Digest, gate::GateDefinition},
    error::{ErrorCode, HarnessError},
    runner::receipt::{TestResultStatus, TestResultSummary},
};

/// Maximum XML payload retained for one declared report.
pub const MAX_JUNIT_BYTES: usize = 16 * 1024 * 1024;
/// Maximum test cases counted across one report.
pub const MAX_JUNIT_CASES: u64 = 10_000_000;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Counts {
    total: u64,
    failed: u64,
    errors: u64,
    skipped: u64,
}

impl Counts {
    const ZERO: Self = Self {
        total: 0,
        failed: 0,
        errors: 0,
        skipped: 0,
    };

    fn add_status(&mut self, status: CaseStatus) -> Result<(), &'static str> {
        self.total = self.total.checked_add(1).ok_or("test count overflow")?;
        match status {
            CaseStatus::Passed => {}
            CaseStatus::Failed => {
                self.failed = self.failed.checked_add(1).ok_or("test count overflow")?;
            }
            CaseStatus::Error => {
                self.errors = self.errors.checked_add(1).ok_or("test count overflow")?;
            }
            CaseStatus::Skipped => {
                self.skipped = self.skipped.checked_add(1).ok_or("test count overflow")?;
            }
        }
        if self.total > MAX_JUNIT_CASES {
            return Err("JUnit report exceeds the test-case limit");
        }
        Ok(())
    }

    fn add_counts(&mut self, other: &Self) -> Result<(), &'static str> {
        self.total = self
            .total
            .checked_add(other.total)
            .ok_or("test count overflow")?;
        self.failed = self
            .failed
            .checked_add(other.failed)
            .ok_or("test count overflow")?;
        self.errors = self
            .errors
            .checked_add(other.errors)
            .ok_or("test count overflow")?;
        self.skipped = self
            .skipped
            .checked_add(other.skipped)
            .ok_or("test count overflow")?;
        if self.total > MAX_JUNIT_CASES {
            return Err("JUnit reports exceed the test-case limit");
        }
        Ok(())
    }

    fn summary(&self) -> Result<TestResultSummary, &'static str> {
        let classified = self
            .failed
            .checked_add(self.errors)
            .and_then(|value| value.checked_add(self.skipped))
            .ok_or("test count overflow")?;
        let passed = self
            .total
            .checked_sub(classified)
            .ok_or("JUnit counts are internally inconsistent")?;
        let summary = TestResultSummary {
            total: self.total,
            passed,
            failed: self.failed,
            errors: self.errors,
            skipped: self.skipped,
            status: TestResultStatus::Reported,
        };
        summary
            .validate()
            .map_err(|_| "JUnit counts are internally inconsistent")?;
        Ok(summary)
    }
}

#[derive(Clone, Copy, Debug)]
enum CaseStatus {
    Passed,
    Failed,
    Error,
    Skipped,
}

#[derive(Clone, Debug)]
struct SuiteFrame {
    declared: Option<Counts>,
    observed: Counts,
    has_child_suite: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct TestcaseFrame {
    status: Option<CaseStatus>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReportBefore {
    length: u64,
    modified: Option<SystemTime>,
    digest: Digest,
}

/// Parses the reports declared by `gate` after one exact attempt.
pub fn collect(
    gate: &GateDefinition,
    working_directory: &Path,
    worktree: &Path,
    before: &[Option<ReportBefore>],
    attempt: u32,
) -> Result<Option<TestResultSummary>, HarnessError> {
    if gate.junit_reports.is_empty() {
        return Ok(None);
    }
    if before.len() != gate.junit_reports.len() {
        return Err(report_error(
            gate,
            attempt,
            "report state did not match declaration",
        ));
    }

    let worktree_root = worktree.canonicalize().map_err(|error| {
        report_error(
            gate,
            attempt,
            &format!("evaluation worktree could not be canonicalized: {error}"),
        )
    })?;
    let mut canonical_paths = BTreeSet::new();
    let mut all = Counts::ZERO;
    let mut report_digests = BTreeSet::new();

    for (index, declared) in gate.junit_reports.iter().enumerate() {
        let path = checked_report_path(working_directory, &worktree_root, declared)
            .map_err(|reason| report_error(gate, attempt, &reason))?;
        let canonical = path.canonicalize().map_err(|error| {
            report_error(
                gate,
                attempt,
                &format!("declared report `{declared}` is missing: {error}"),
            )
        })?;
        if !canonical.starts_with(&worktree_root) {
            return Err(report_error(
                gate,
                attempt,
                &format!("declared report `{declared}` resolves outside the worktree"),
            ));
        }
        if !canonical_paths.insert(canonical) {
            return Err(report_error(
                gate,
                attempt,
                &format!("declared reports contain a duplicate file: `{declared}`"),
            ));
        }
        let metadata = fs::metadata(&path).map_err(|error| {
            report_error(
                gate,
                attempt,
                &format!("declared report `{declared}` cannot be read: {error}"),
            )
        })?;
        if !metadata.is_file() {
            return Err(report_error(
                gate,
                attempt,
                &format!("declared report `{declared}` is not a regular file"),
            ));
        }
        let (bytes, digest) = read_report(&path, declared, gate, attempt)?;
        let after = ReportBefore {
            length: metadata.len(),
            modified: metadata.modified().ok(),
            digest,
        };
        if before[index].as_ref() == Some(&after) {
            return Err(report_error(
                gate,
                attempt,
                &format!("declared report `{declared}` was not produced by this attempt"),
            ));
        }
        if !report_digests.insert(after.digest.clone()) {
            return Err(report_error(
                gate,
                attempt,
                "multiple declared reports have identical content",
            ));
        }
        let counts = parse_report(&bytes).map_err(|reason| {
            report_error(
                gate,
                attempt,
                &format!("JUnit report `{declared}`: {reason}"),
            )
        })?;
        all.add_counts(&counts)
            .map_err(|reason| report_error(gate, attempt, reason))?;
    }
    all.summary()
        .map(Some)
        .map_err(|reason| report_error(gate, attempt, reason))
}

/// Captures report state before a gate starts, without trusting an old report.
pub fn capture_before(
    gate: &GateDefinition,
    working_directory: &Path,
    worktree: &Path,
    attempt: u32,
) -> Result<Vec<Option<ReportBefore>>, HarnessError> {
    let worktree_root = worktree.canonicalize().map_err(|error| {
        report_error(
            gate,
            attempt,
            &format!("evaluation worktree could not be canonicalized: {error}"),
        )
    })?;
    gate.junit_reports
        .iter()
        .map(|declared| {
            let path = checked_report_path(working_directory, &worktree_root, declared)
                .map_err(|reason| report_error(gate, attempt, &reason))?;
            if !path.exists() {
                return Ok(None);
            }
            let metadata = fs::metadata(&path).map_err(|error| {
                report_error(
                    gate,
                    attempt,
                    &format!("declared report `{declared}` cannot be inspected: {error}"),
                )
            })?;
            if !metadata.is_file() {
                return Err(report_error(
                    gate,
                    attempt,
                    &format!("declared report `{declared}` is not a regular file"),
                ));
            }
            let digest = read_report(&path, declared, gate, attempt)?.1;
            Ok(Some(ReportBefore {
                length: metadata.len(),
                modified: metadata.modified().ok(),
                digest,
            }))
        })
        .collect()
}

fn checked_report_path(
    working_directory: &Path,
    worktree_root: &Path,
    declared: &str,
) -> Result<PathBuf, String> {
    if declared.is_empty()
        || declared.starts_with('/')
        || declared.contains('\0')
        || Path::new(declared)
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(format!(
            "declared report `{declared}` must remain relative to the gate working directory"
        ));
    }
    let path = working_directory.join(declared);
    reject_symlink_components(working_directory, declared)?;
    if let Ok(canonical) = path.canonicalize()
        && !canonical.starts_with(worktree_root)
    {
        return Err(format!(
            "declared report `{declared}` resolves outside the worktree"
        ));
    }
    Ok(path)
}

fn reject_symlink_components(root: &Path, declared: &str) -> Result<(), String> {
    let mut current = root.to_path_buf();
    for component in Path::new(declared).components() {
        if let std::path::Component::Normal(part) = component {
            current.push(part);
            if let Ok(metadata) = fs::symlink_metadata(&current)
                && metadata.file_type().is_symlink()
            {
                return Err(format!("declared report `{declared}` traverses a symlink"));
            }
        }
    }
    Ok(())
}

fn read_report(
    path: &Path,
    declared: &str,
    gate: &GateDefinition,
    attempt: u32,
) -> Result<(Vec<u8>, Digest), HarnessError> {
    let file = File::open(path).map_err(|error| {
        report_error(
            gate,
            attempt,
            &format!("declared report `{declared}` cannot be opened: {error}"),
        )
    })?;
    let mut bytes = Vec::new();
    file.take((MAX_JUNIT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            report_error(
                gate,
                attempt,
                &format!("declared report `{declared}` cannot be read: {error}"),
            )
        })?;
    if bytes.len() > MAX_JUNIT_BYTES {
        return Err(report_error(
            gate,
            attempt,
            &format!("declared report `{declared}` exceeds the {MAX_JUNIT_BYTES}-byte limit"),
        ));
    }
    let digest = Digest::of_bytes(&bytes);
    Ok((bytes, digest))
}

#[allow(clippy::too_many_lines)]
fn parse_report(bytes: &[u8]) -> Result<Counts, &'static str> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut elements: Vec<Vec<u8>> = Vec::new();
    let mut suites: Vec<SuiteFrame> = Vec::new();
    let mut testcase: Option<TestcaseFrame> = None;
    let mut actual = Counts::ZERO;
    let mut root_name: Option<Vec<u8>> = None;
    let mut root_declared: Option<Counts> = None;
    let mut leaf_declared = Counts::ZERO;
    let mut leaf_count = 0u64;

    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|_| "malformed XML")?
        {
            Event::Decl(_) | Event::PI(_) | Event::Comment(_) => {}
            Event::Text(_) | Event::CData(_) => {
                if testcase.is_none()
                    && elements
                        .last()
                        .is_none_or(|name| name.as_slice() != b"system-out")
                    && elements
                        .last()
                        .is_none_or(|name| name.as_slice() != b"system-err")
                {
                    // Whitespace is already trimmed. Non-whitespace outside a
                    // report payload is not a JUnit document we can interpret.
                    return Err("unexpected text outside a test result payload");
                }
            }
            Event::Start(start) => {
                let name = start.local_name().as_ref().to_vec();
                if elements.is_empty() {
                    if name.as_slice() != b"testsuite" && name.as_slice() != b"testsuites" {
                        return Err("root element must be testsuite or testsuites");
                    }
                    root_name = Some(name.clone());
                    root_declared = declared_counts(&start)?;
                } else if name.as_slice() == b"testcase" {
                    if testcase.is_some() {
                        return Err("nested testcase elements are not supported");
                    }
                    testcase = Some(TestcaseFrame::default());
                } else if let Some(current) = testcase.as_mut() {
                    if elements
                        .last()
                        .is_some_and(|parent| parent.as_slice() == b"testcase")
                    {
                        set_case_status(current, &name)?;
                    } else if !is_allowed_payload(&name) {
                        return Err("unknown element inside testcase");
                    }
                } else if name.as_slice() == b"testsuite" {
                    if let Some(parent) = suites.last_mut() {
                        parent.has_child_suite = true;
                    }
                    suites.push(SuiteFrame {
                        declared: declared_counts(&start)?,
                        observed: Counts::ZERO,
                        has_child_suite: false,
                    });
                } else if !is_allowed_payload(&name) && name.as_slice() != b"properties" {
                    return Err("unknown element outside test results");
                }
                elements.push(name);
            }
            Event::Empty(empty) => {
                let name = empty.local_name().as_ref().to_vec();
                if elements.is_empty() {
                    if name.as_slice() != b"testsuite" && name.as_slice() != b"testsuites" {
                        return Err("root element must be testsuite or testsuites");
                    }
                    root_name = Some(name.clone());
                    root_declared = declared_counts(&empty)?;
                } else if name.as_slice() == b"testcase" {
                    if testcase.is_some() {
                        return Err("nested testcase elements are not supported");
                    }
                    record_case(&mut actual, &mut suites, CaseStatus::Passed)?;
                } else if let Some(current) = testcase.as_mut() {
                    if elements
                        .last()
                        .is_some_and(|parent| parent.as_slice() == b"testcase")
                    {
                        set_case_status(current, &name)?;
                    } else if !is_allowed_payload(&name) {
                        return Err("unknown element inside testcase");
                    }
                } else if name.as_slice() == b"testsuite" {
                    let declared = declared_counts(&empty)?;
                    if let Some(declared) = declared {
                        leaf_declared.add_counts(&declared)?;
                        leaf_count = leaf_count.checked_add(1).ok_or("test count overflow")?;
                    }
                }
            }
            Event::End(end) => {
                let name = end.local_name().as_ref().to_vec();
                let Some(open) = elements.pop() else {
                    return Err("unexpected closing element");
                };
                if open.as_slice() != name.as_slice() {
                    return Err("mismatched XML elements");
                }
                if name.as_slice() == b"testcase" {
                    let case = testcase.take().ok_or("testcase state is invalid")?;
                    record_case(
                        &mut actual,
                        &mut suites,
                        case.status.unwrap_or(CaseStatus::Passed),
                    )?;
                } else if name.as_slice() == b"testsuite"
                    && let Some(suite) = suites.pop()
                    && let Some(declared) = suite.declared
                {
                    if suite.observed.total > 0 {
                        if suite.observed != declared {
                            return Err("testsuite counts disagree with testcase elements");
                        }
                    } else if !suite.has_child_suite {
                        leaf_declared.add_counts(&declared)?;
                        leaf_count = leaf_count.checked_add(1).ok_or("test count overflow")?;
                    }
                }
            }
            Event::DocType(_) => return Err("DOCTYPE declarations are not allowed"),
            Event::Eof => break,
        }
        buffer.clear();
    }
    if !elements.is_empty() || testcase.is_some() || !suites.is_empty() {
        return Err("incomplete XML document");
    }
    let Some(root) = root_name else {
        return Err("JUnit document has no root element");
    };

    if actual.total > 0 {
        if let Some(declared) = root_declared
            && actual != declared
        {
            return Err("root counts disagree with testcase elements");
        }
        actual.summary_counts()
    } else if let Some(declared) = root_declared {
        declared.summary_counts()
    } else if leaf_count > 0 {
        leaf_declared.summary_counts()
    } else {
        let _ = root;
        Err("JUnit document contains no test cases or complete counts")
    }
}

impl Counts {
    fn summary_counts(&self) -> Result<Self, &'static str> {
        let classified = self
            .failed
            .checked_add(self.errors)
            .and_then(|value| value.checked_add(self.skipped))
            .ok_or("test count overflow")?;
        if classified > self.total || self.total > MAX_JUNIT_CASES {
            return Err("JUnit counts are internally inconsistent");
        }
        Ok(self.clone())
    }
}

fn record_case(
    actual: &mut Counts,
    suites: &mut [SuiteFrame],
    status: CaseStatus,
) -> Result<(), &'static str> {
    actual.add_status(status)?;
    for suite in suites {
        suite.observed.add_status(status)?;
    }
    Ok(())
}

fn set_case_status(case: &mut TestcaseFrame, name: &[u8]) -> Result<(), &'static str> {
    let status = match name {
        b"failure" => CaseStatus::Failed,
        b"error" => CaseStatus::Error,
        b"skipped" => CaseStatus::Skipped,
        _ if is_allowed_payload(name) => return Ok(()),
        _ => return Err("unknown element inside testcase"),
    };
    if case.status.replace(status).is_some() {
        return Err("testcase has multiple result statuses");
    }
    Ok(())
}

fn declared_counts(
    element: &quick_xml::events::BytesStart<'_>,
) -> Result<Option<Counts>, &'static str> {
    let mut values: [Option<u64>; 4] = [None, None, None, None];
    for attribute in element.attributes().with_checks(true) {
        let attribute = attribute.map_err(|_| "malformed XML attribute")?;
        let slot = match attribute.key.as_ref() {
            b"tests" => Some(&mut values[0]),
            b"failures" => Some(&mut values[1]),
            b"errors" => Some(&mut values[2]),
            b"skipped" => Some(&mut values[3]),
            _ => None,
        };
        if let Some(slot) = slot {
            if slot.is_some() {
                return Err("duplicate JUnit count attribute");
            }
            let value = attribute
                .unescape_value()
                .map_err(|_| "malformed XML attribute")?
                .parse::<u64>()
                .map_err(|_| "JUnit counts must be nonnegative integers")?;
            *slot = Some(value);
        }
    }
    if values.iter().all(Option::is_none) {
        return Ok(None);
    }
    if values.iter().any(Option::is_none) {
        return Err("JUnit count attributes must include tests, failures, errors, and skipped");
    }
    let counts = Counts {
        total: values[0].unwrap(),
        failed: values[1].unwrap(),
        errors: values[2].unwrap(),
        skipped: values[3].unwrap(),
    };
    counts.summary_counts().map(Some)
}

fn is_allowed_payload(name: &[u8]) -> bool {
    matches!(
        name,
        b"failure" | b"error" | b"skipped" | b"system-out" | b"system-err" | b"properties"
    )
}

fn report_error(gate: &GateDefinition, attempt: u32, reason: &str) -> HarnessError {
    HarnessError::Control {
        reason: format!(
            "gate `{}` JUnit report validation failed on attempt {attempt}: {reason}",
            gate.gate_id
        ),
        code: ErrorCode::GateRunnerError,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::gate::{GATE_SCHEMA, GateEnvironment, NetworkPolicy, RetryPolicy};

    fn parse(xml: &str) -> TestResultSummary {
        parse_report(xml.as_bytes()).unwrap().summary().unwrap()
    }

    #[test]
    fn counts_testcase_statuses_without_exposing_test_details() {
        let summary = parse(
            r#"<testsuite tests="4" failures="1" errors="1" skipped="1"><testcase/><testcase><failure>secret</failure></testcase><testcase><error>secret</error></testcase><testcase><skipped/></testcase></testsuite>"#,
        );
        assert_eq!(summary.total, 4);
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.errors, 1);
        assert_eq!(summary.skipped, 1);
        assert!(!serde_json::to_string(&summary).unwrap().contains("secret"));
    }

    #[test]
    fn malformed_and_inconsistent_counts_fail_closed() {
        assert!(
            parse_report(b"<testsuite tests=\"-1\" failures=\"0\" errors=\"0\" skipped=\"0\"/>")
                .is_err()
        );
        assert!(parse_report(b"<testsuite tests=\"1\" failures=\"0\" errors=\"0\" skipped=\"0\"><testcase/><testcase/></testsuite>").is_err());
    }

    #[test]
    fn a_report_from_a_prior_attempt_is_not_reused() {
        let temp = tempfile::tempdir().unwrap();
        let worktree = temp.path().join("worktree");
        let logs = temp.path().join("logs");
        fs::create_dir_all(&worktree).unwrap();
        let gate = GateDefinition {
            schema: GATE_SCHEMA.to_owned(),
            gate_id: "gate.junit".to_owned(),
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
        crate::runner::run_attempt(
            &gate,
            &worktree,
            &logs,
            1,
            &crate::domain::clock::FixedClock::at_unix_seconds(1_785_196_800).unwrap(),
        )
        .unwrap();
        assert!(
            crate::runner::run_attempt(
                &gate,
                &worktree,
                &logs,
                2,
                &crate::domain::clock::FixedClock::at_unix_seconds(1_785_196_800).unwrap()
            )
            .is_err()
        );
    }
}
