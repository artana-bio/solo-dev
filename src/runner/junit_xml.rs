//! Bounded `JUnit` XML parsing.
//!
//! Only counts are retained. Test names, failure text, properties, and
//! runner-owned paths never enter a receipt or snapshot. The depth bound is
//! independent of the 16 MiB byte bound so a small adversarial document cannot
//! grow the parser stack without limit.

use quick_xml::{Reader, events::Event};

use crate::runner::receipt::{StructuredResultErrorCode, TestResultStatus, TestResultSummary};

use super::MAX_JUNIT_CASES;

/// Maximum nesting depth accepted in one `JUnit` document.
pub const MAX_JUNIT_DEPTH: usize = 128;

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

    fn add_status(&mut self, status: CaseStatus) -> Result<(), StructuredResultErrorCode> {
        self.total = self
            .total
            .checked_add(1)
            .ok_or(StructuredResultErrorCode::Inconsistent)?;
        match status {
            CaseStatus::Passed => {}
            CaseStatus::Failed => {
                self.failed = self
                    .failed
                    .checked_add(1)
                    .ok_or(StructuredResultErrorCode::Inconsistent)?;
            }
            CaseStatus::Error => {
                self.errors = self
                    .errors
                    .checked_add(1)
                    .ok_or(StructuredResultErrorCode::Inconsistent)?;
            }
            CaseStatus::Skipped => {
                self.skipped = self
                    .skipped
                    .checked_add(1)
                    .ok_or(StructuredResultErrorCode::Inconsistent)?;
            }
        }
        self.validate()
    }

    fn add_counts(&mut self, other: &Self) -> Result<(), StructuredResultErrorCode> {
        self.total = self
            .total
            .checked_add(other.total)
            .ok_or(StructuredResultErrorCode::Inconsistent)?;
        self.failed = self
            .failed
            .checked_add(other.failed)
            .ok_or(StructuredResultErrorCode::Inconsistent)?;
        self.errors = self
            .errors
            .checked_add(other.errors)
            .ok_or(StructuredResultErrorCode::Inconsistent)?;
        self.skipped = self
            .skipped
            .checked_add(other.skipped)
            .ok_or(StructuredResultErrorCode::Inconsistent)?;
        self.validate()
    }

    fn validate(&self) -> Result<(), StructuredResultErrorCode> {
        let classified = self
            .failed
            .checked_add(self.errors)
            .and_then(|value| value.checked_add(self.skipped))
            .ok_or(StructuredResultErrorCode::Inconsistent)?;
        if classified > self.total || self.total > MAX_JUNIT_CASES {
            return Err(StructuredResultErrorCode::Inconsistent);
        }
        Ok(())
    }

    fn summary(&self) -> Result<TestResultSummary, StructuredResultErrorCode> {
        self.validate()?;
        let classified = self
            .failed
            .checked_add(self.errors)
            .and_then(|value| value.checked_add(self.skipped))
            .ok_or(StructuredResultErrorCode::Inconsistent)?;
        let summary = TestResultSummary {
            total: self.total,
            passed: self
                .total
                .checked_sub(classified)
                .ok_or(StructuredResultErrorCode::Inconsistent)?,
            failed: self.failed,
            errors: self.errors,
            skipped: self.skipped,
            status: TestResultStatus::Reported,
            error_code: None,
        };
        summary
            .validate()
            .map_err(|_| StructuredResultErrorCode::Inconsistent)?;
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
    direct: Counts,
    children: Counts,
    child_count: usize,
}

#[derive(Clone, Copy, Debug, Default)]
struct TestcaseFrame {
    status: Option<CaseStatus>,
}

/// Parse one complete report into redacted counts.
pub(crate) fn parse_report(bytes: &[u8]) -> Result<TestResultSummary, StructuredResultErrorCode> {
    parse_counts(bytes)?.summary()
}

#[allow(clippy::too_many_lines)]
fn parse_counts(bytes: &[u8]) -> Result<Counts, StructuredResultErrorCode> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut elements: Vec<Vec<u8>> = Vec::new();
    let mut suites: Vec<SuiteFrame> = Vec::new();
    let mut testcase: Option<TestcaseFrame> = None;
    let mut root_name: Option<Vec<u8>> = None;
    let mut root_declared: Option<Counts> = None;
    let mut aggregate = Counts::ZERO;
    let mut completed_suite_count = 0usize;

    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|_| StructuredResultErrorCode::Malformed)?
        {
            Event::Decl(_) | Event::PI(_) | Event::Comment(_) => {}
            Event::Text(_) | Event::CData(_) => {
                let payload = elements
                    .last()
                    .is_some_and(|name| is_payload(name.as_slice()));
                if testcase.is_none() && !payload {
                    return Err(StructuredResultErrorCode::Malformed);
                }
            }
            Event::Start(start) => {
                if elements.len() >= MAX_JUNIT_DEPTH {
                    return Err(StructuredResultErrorCode::DepthExceeded);
                }
                let name = start.local_name().as_ref().to_vec();
                if elements.is_empty() {
                    if name.as_slice() != b"testsuite" && name.as_slice() != b"testsuites" {
                        return Err(StructuredResultErrorCode::Malformed);
                    }
                    root_name = Some(name.clone());
                    root_declared = declared_counts(&start)?;
                    if name.as_slice() == b"testsuite" {
                        suites.push(SuiteFrame {
                            declared: root_declared.clone(),
                            direct: Counts::ZERO,
                            children: Counts::ZERO,
                            child_count: 0,
                        });
                    }
                } else if name.as_slice() == b"testcase" {
                    if testcase.is_some() || suites.is_empty() {
                        return Err(StructuredResultErrorCode::Malformed);
                    }
                    testcase = Some(TestcaseFrame::default());
                } else if let Some(current) = testcase.as_mut() {
                    if elements.last().is_some_and(|parent| parent == b"testcase") {
                        set_case_status(current, &name)?;
                    } else if !is_payload(&name) {
                        return Err(StructuredResultErrorCode::Malformed);
                    }
                } else if name.as_slice() == b"testsuite" {
                    if let Some(parent) = suites.last_mut() {
                        parent.child_count += 1;
                    }
                    suites.push(SuiteFrame {
                        declared: declared_counts(&start)?,
                        direct: Counts::ZERO,
                        children: Counts::ZERO,
                        child_count: 0,
                    });
                } else if !is_payload(&name) && name.as_slice() != b"properties" {
                    return Err(StructuredResultErrorCode::Malformed);
                }
                elements.push(name);
            }
            Event::Empty(empty) => {
                if elements.len() >= MAX_JUNIT_DEPTH {
                    return Err(StructuredResultErrorCode::DepthExceeded);
                }
                let name = empty.local_name().as_ref().to_vec();
                if elements.is_empty() {
                    if name.as_slice() != b"testsuite" && name.as_slice() != b"testsuites" {
                        return Err(StructuredResultErrorCode::Malformed);
                    }
                    root_name = Some(name.clone());
                    root_declared = declared_counts(&empty)?;
                    if name.as_slice() == b"testsuite" {
                        let suite = SuiteFrame {
                            declared: root_declared.clone(),
                            direct: Counts::ZERO,
                            children: Counts::ZERO,
                            child_count: 0,
                        };
                        aggregate.add_counts(&finish_suite(suite)?)?;
                        completed_suite_count += 1;
                    }
                } else if name.as_slice() == b"testcase" {
                    if testcase.is_some() || suites.is_empty() {
                        return Err(StructuredResultErrorCode::Malformed);
                    }
                    record_case(&mut suites, CaseStatus::Passed)?;
                } else if let Some(current) = testcase.as_mut() {
                    if elements.last().is_some_and(|parent| parent == b"testcase") {
                        set_case_status(current, &name)?;
                    } else if !is_payload(&name) {
                        return Err(StructuredResultErrorCode::Malformed);
                    }
                } else if name.as_slice() == b"testsuite" {
                    if let Some(parent) = suites.last_mut() {
                        parent.child_count += 1;
                    }
                    let effective = finish_suite(SuiteFrame {
                        declared: declared_counts(&empty)?,
                        direct: Counts::ZERO,
                        children: Counts::ZERO,
                        child_count: 0,
                    })?;
                    if let Some(parent) = suites.last_mut() {
                        parent.children.add_counts(&effective)?;
                    } else {
                        aggregate.add_counts(&effective)?;
                    }
                    completed_suite_count += 1;
                }
            }
            Event::End(end) => {
                let name = end.local_name().as_ref().to_vec();
                let Some(open) = elements.pop() else {
                    return Err(StructuredResultErrorCode::Malformed);
                };
                if open != name {
                    return Err(StructuredResultErrorCode::Malformed);
                }
                if name.as_slice() == b"testcase" {
                    let case = testcase
                        .take()
                        .ok_or(StructuredResultErrorCode::Malformed)?;
                    record_case(&mut suites, case.status.unwrap_or(CaseStatus::Passed))?;
                } else if name.as_slice() == b"testsuite" {
                    let suite = suites.pop().ok_or(StructuredResultErrorCode::Malformed)?;
                    let effective = finish_suite(suite)?;
                    if let Some(parent) = suites.last_mut() {
                        parent.children.add_counts(&effective)?;
                    } else {
                        aggregate.add_counts(&effective)?;
                    }
                    completed_suite_count += 1;
                }
            }
            Event::DocType(_) => return Err(StructuredResultErrorCode::Malformed),
            Event::Eof => break,
        }
        buffer.clear();
    }
    if !elements.is_empty() || testcase.is_some() || !suites.is_empty() {
        return Err(StructuredResultErrorCode::Malformed);
    }
    let Some(root) = root_name else {
        return Err(StructuredResultErrorCode::Malformed);
    };
    if root.as_slice() == b"testsuites"
        && completed_suite_count == 0
        && let Some(declared) = root_declared
    {
        return declared.validate().map(|()| declared);
    }
    if completed_suite_count == 0 {
        return Err(StructuredResultErrorCode::Malformed);
    }
    if let Some(declared) = root_declared
        && declared != aggregate
    {
        return Err(StructuredResultErrorCode::Inconsistent);
    }
    aggregate.validate().map(|()| aggregate)
}

fn finish_suite(suite: SuiteFrame) -> Result<Counts, StructuredResultErrorCode> {
    let mut effective = suite.direct;
    effective.add_counts(&suite.children)?;
    if effective.total == 0 {
        effective = suite
            .declared
            .clone()
            .ok_or(StructuredResultErrorCode::Malformed)?;
    }
    if let Some(declared) = suite.declared
        && declared != effective
    {
        return Err(StructuredResultErrorCode::Inconsistent);
    }
    effective.validate().map(|()| effective)
}

fn record_case(
    suites: &mut [SuiteFrame],
    status: CaseStatus,
) -> Result<(), StructuredResultErrorCode> {
    suites
        .last_mut()
        .ok_or(StructuredResultErrorCode::Malformed)?
        .direct
        .add_status(status)
}

fn set_case_status(case: &mut TestcaseFrame, name: &[u8]) -> Result<(), StructuredResultErrorCode> {
    let status = match name {
        b"failure" => CaseStatus::Failed,
        b"error" => CaseStatus::Error,
        b"skipped" => CaseStatus::Skipped,
        _ if is_payload(name) => return Ok(()),
        _ => return Err(StructuredResultErrorCode::Malformed),
    };
    if case.status.replace(status).is_some() {
        return Err(StructuredResultErrorCode::Inconsistent);
    }
    Ok(())
}

fn declared_counts(
    element: &quick_xml::events::BytesStart<'_>,
) -> Result<Option<Counts>, StructuredResultErrorCode> {
    let mut values: [Option<u64>; 4] = [None, None, None, None];
    for attribute in element.attributes().with_checks(true) {
        let attribute = attribute.map_err(|_| StructuredResultErrorCode::Malformed)?;
        let slot = match attribute.key.as_ref() {
            b"tests" => Some(&mut values[0]),
            b"failures" => Some(&mut values[1]),
            b"errors" => Some(&mut values[2]),
            b"skipped" => Some(&mut values[3]),
            _ => None,
        };
        if let Some(slot) = slot {
            if slot.is_some() {
                return Err(StructuredResultErrorCode::Malformed);
            }
            let value = attribute
                .unescape_value()
                .map_err(|_| StructuredResultErrorCode::Malformed)?
                .parse::<u64>()
                .map_err(|_| StructuredResultErrorCode::Malformed)?;
            *slot = Some(value);
        }
    }
    if values.iter().all(Option::is_none) {
        return Ok(None);
    }
    if values.iter().any(Option::is_none) {
        return Err(StructuredResultErrorCode::Inconsistent);
    }
    let counts = Counts {
        total: values[0].unwrap_or_default(),
        failed: values[1].unwrap_or_default(),
        errors: values[2].unwrap_or_default(),
        skipped: values[3].unwrap_or_default(),
    };
    counts.validate().map(|()| Some(counts))
}

fn is_payload(name: &[u8]) -> bool {
    matches!(
        name,
        b"failure"
            | b"error"
            | b"skipped"
            | b"system-out"
            | b"system-err"
            | b"properties"
            | b"property"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(xml: &str) -> TestResultSummary {
        parse_report(xml.as_bytes()).unwrap()
    }

    #[test]
    fn counts_statuses_and_redacts_payload() {
        let summary = parse(
            r#"<testsuite tests="4" failures="1" errors="1" skipped="1"><testcase/><testcase><failure>secret</failure></testcase><testcase><error>secret</error></testcase><testcase><skipped/></testcase></testsuite>"#,
        );
        assert_eq!(
            (
                summary.total,
                summary.passed,
                summary.failed,
                summary.errors,
                summary.skipped
            ),
            (4, 1, 1, 1, 1)
        );
        assert!(!serde_json::to_string(&summary).unwrap().contains("secret"));
    }

    #[test]
    fn nested_declared_hierarchy_is_reconciled() {
        let summary = parse(
            r#"<testsuites tests="2" failures="0" errors="0" skipped="0"><testsuite tests="2" failures="0" errors="0" skipped="0"><testsuite tests="2" failures="0" errors="0" skipped="0"/></testsuite></testsuites>"#,
        );
        assert_eq!(summary.total, 2);
        assert!(parse_report(b"<testsuites tests=\"1\" failures=\"0\" errors=\"0\" skipped=\"0\"><testsuite tests=\"2\" failures=\"0\" errors=\"0\" skipped=\"0\"/></testsuites>").is_err());
    }

    #[test]
    fn mixed_direct_and_child_counts_are_reconciled() {
        assert!(parse_report(b"<testsuite tests=\"1\" failures=\"0\" errors=\"0\" skipped=\"0\"><testcase/><testsuite tests=\"1\" failures=\"0\" errors=\"0\" skipped=\"0\"/></testsuite>").is_err());
        let summary = parse(
            r#"<testsuites><testsuite tests="1" failures="0" errors="0" skipped="0"/><testsuite><testcase/></testsuite></testsuites>"#,
        );
        assert_eq!(summary.total, 2);
    }

    #[test]
    fn depth_limit_is_independent_of_payload_size() {
        let mut xml =
            String::from("<testsuite tests=\"0\" failures=\"0\" errors=\"0\" skipped=\"0\">");
        for _ in 0..(MAX_JUNIT_DEPTH - 1) {
            xml.push_str("<properties>");
        }
        for _ in 0..(MAX_JUNIT_DEPTH - 1) {
            xml.push_str("</properties>");
        }
        xml.push_str("</testsuite>");
        assert!(parse_report(xml.as_bytes()).is_ok());
        let mut too_deep = String::from("<testsuite>");
        for _ in 0..MAX_JUNIT_DEPTH {
            too_deep.push_str("<properties>");
        }
        assert_eq!(
            parse_report(too_deep.as_bytes()),
            Err(StructuredResultErrorCode::DepthExceeded)
        );
    }
}
