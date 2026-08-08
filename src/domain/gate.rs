//! Named gates: the only executable commands the harness will run.
//!
//! Section 10.5 and D-008. A card names gates; it never defines them. That
//! separation is the entire security property of this module: a card is
//! authored by whoever is doing the work, and if a card could carry a command,
//! the actor being checked would be choosing the check.
//!
//! Gates are `argv` arrays, never strings. There is no code path here that
//! accepts a command line, so there is nothing for a shell to reinterpret.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    domain::digest::Digest,
    error::{ErrorCode, HarnessError},
};

/// Schema identifier for a gate definition.
pub const GATE_SCHEMA: &str = "harness.gate/v1";

/// Directory holding gate definitions, relative to the control repository.
pub const GATE_DIR: &str = "gates";

/// Longest timeout a gate may declare, in seconds.
///
/// A gate without an upper bound is a gate that can hang the workflow forever,
/// and "wait indefinitely" is never the right answer for an automated check.
pub const MAX_TIMEOUT_SECONDS: u64 = 3_600;

/// Maximum number of explicitly declared structured reports on one gate.
const MAX_JUNIT_REPORTS: usize = 32;

/// Environment variables never passed to a gate, whatever the allowlist says.
///
/// Section 14.1 requires the candidate process not to inherit production
/// credentials. The allowlist already excludes everything not named, but these
/// are denied even if named, so a careless registry entry cannot leak them.
const ALWAYS_DENIED: [&str; 8] = [
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "GITHUB_TOKEN",
    "GH_TOKEN",
    "NPM_TOKEN",
    "CARGO_REGISTRY_TOKEN",
    "SSH_AUTH_SOCK",
    "GPG_TTY",
];

/// What a gate is allowed to do with the network.
///
/// MVP enforcement is declarative: the policy is recorded and reported, not
/// imposed. Claiming enforcement without a sandbox would be a false security
/// claim, which Section 14.1 and `WP-710` explicitly forbid.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum NetworkPolicy {
    /// The gate declares it needs no network.
    Denied,
    /// The gate declares it needs network access.
    Allowed,
}

impl NetworkPolicy {
    /// Whether the harness imposes the declared policy on a running gate.
    ///
    /// Always false. Nothing in [`crate::runner`] restricts network access, so
    /// a gate declaring [`Self::Denied`] still reaches whatever its host can.
    /// `WP-710` is the package that can flip this; until it does, every
    /// surface that reports the policy has to say which of the two it means.
    pub const ENFORCED: bool = false;

    /// Renders the policy together with whether it is imposed.
    ///
    /// Reporting the bare variant was the defect. `gate show` printed
    /// `network: Denied` beside a timeout and an allowlist that are both real,
    /// so the one decorative field in the group read exactly like the enforced
    /// ones — and the receipt carried that reading into the evidence record.
    /// Section 14.1 and `WP-710` forbid describing a declaration as isolation,
    /// which an unqualified `Denied` does to every reader who has not read
    /// this file.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Denied => "denied (declared, not enforced)",
            Self::Allowed => "allowed (declared)",
        }
    }
}

/// How many times a gate may run before its result counts.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RetryPolicy {
    /// Total attempts permitted, including the first.
    pub max_attempts: u32,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        // Section 14.2: one attempt unless the gate declares otherwise. A silent
        // retry converts a flaky result into apparent evidence.
        Self { max_attempts: 1 }
    }
}

/// The environment a gate receives.
#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GateEnvironment {
    /// Variables passed through from the parent, if present.
    pub allow: Vec<String>,
    /// Variables set to fixed values.
    pub set: BTreeMap<String, String>,
}

/// One named, executable check.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GateDefinition {
    /// Always [`GATE_SCHEMA`].
    pub schema: String,
    /// Names the gate. Cards reference this, never a command.
    pub gate_id: String,
    /// Starts at 1 and increases by exactly one.
    pub revision: u32,
    /// The executable and its arguments. Never a shell string.
    pub argv: Vec<String>,
    /// Directory to run in, relative to the evaluation worktree.
    pub working_directory: String,
    /// How long the gate may run.
    pub timeout_seconds: u64,
    /// The environment it receives.
    pub environment: GateEnvironment,
    /// What it declares about network access.
    pub network_policy: NetworkPolicy,
    /// How many attempts its result may take.
    pub retry_policy: RetryPolicy,
    /// Files it produces that should be retained.
    pub artifacts: Vec<String>,
    /// `JUnit` XML reports produced by this gate, relative to its working
    /// directory. Empty is omitted from canonical serialization so existing
    /// gate digests remain stable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub junit_reports: Vec<String>,
}

impl GateDefinition {
    /// Relative path of a gate inside the control repository.
    #[must_use]
    pub fn relative_path(gate_id: &str) -> String {
        format!("{GATE_DIR}/{gate_id}.json")
    }

    /// The gate's canonical digest.
    ///
    /// Receipts bind to this. A revision changes the digest, which is what makes
    /// an older receipt detectably stale rather than quietly reusable.
    ///
    /// # Errors
    ///
    /// Returns an error when the definition cannot be serialized.
    pub fn digest(&self) -> Result<Digest, HarnessError> {
        Digest::of_canonical(self)
    }

    /// Validates everything the registry requires.
    ///
    /// # Errors
    ///
    /// Returns a configuration error naming the first violated rule.
    pub fn validate(&self) -> Result<(), HarnessError> {
        let reject = |reason: String| HarnessError::Control {
            reason,
            code: ErrorCode::ConfigInvalidGate,
        };

        if self.schema != GATE_SCHEMA {
            return Err(reject(format!(
                "expected schema `{GATE_SCHEMA}`, found `{}`",
                self.schema
            )));
        }
        if self.gate_id.trim().is_empty() {
            return Err(reject("a gate must have an identifier".to_owned()));
        }
        if !self.gate_id.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-' || byte == b'_'
        }) {
            return Err(reject(format!(
                "gate identifier `{}` may contain only ASCII letters, digits, `.`, `-`, and `_`",
                self.gate_id
            )));
        }
        if self.revision == 0 {
            return Err(reject("gate revisions begin at 1".to_owned()));
        }

        let Some(program) = self.argv.first() else {
            return Err(reject(format!(
                "gate `{}` must declare a non-empty argv",
                self.gate_id
            )));
        };
        if program.trim().is_empty() {
            return Err(reject(format!(
                "gate `{}` names an empty executable",
                self.gate_id
            )));
        }
        // A single argv element containing shell syntax is the classic way a
        // "command array" smuggles a shell command back in. There is no shell in
        // the runner, so such an entry would simply fail to execute; rejecting
        // it here makes the mistake obvious at registration instead.
        if self.argv.len() == 1 && program.split_whitespace().count() > 1 {
            return Err(reject(format!(
                "gate `{}` declares a single argv entry containing spaces (`{program}`); argv is an array of arguments, not a command line",
                self.gate_id
            )));
        }
        for shell_marker in ['|', ';', '&', '>', '<', '`', '$'] {
            if program.contains(shell_marker) {
                return Err(reject(format!(
                    "gate `{}` executable contains shell metacharacter `{shell_marker}`; commands are never parsed by a shell",
                    self.gate_id
                )));
            }
        }

        if self.timeout_seconds == 0 {
            return Err(reject(format!(
                "gate `{}` must declare a non-zero timeout",
                self.gate_id
            )));
        }
        if self.timeout_seconds > MAX_TIMEOUT_SECONDS {
            return Err(reject(format!(
                "gate `{}` timeout {} exceeds the maximum {MAX_TIMEOUT_SECONDS}",
                self.gate_id, self.timeout_seconds
            )));
        }
        if self.retry_policy.max_attempts == 0 {
            return Err(reject(format!(
                "gate `{}` must permit at least one attempt",
                self.gate_id
            )));
        }

        validate_working_directory(&self.working_directory).map_err(reject)?;

        validate_junit_reports(&self.gate_id, &self.junit_reports).map_err(reject)?;

        for name in self
            .environment
            .allow
            .iter()
            .chain(self.environment.set.keys())
        {
            if ALWAYS_DENIED.contains(&name.as_str()) {
                return Err(reject(format!(
                    "gate `{}` names `{name}`, which is denied to gate processes regardless of the allowlist",
                    self.gate_id
                )));
            }
        }

        Ok(())
    }
}

fn validate_junit_reports(gate_id: &str, reports: &[String]) -> Result<(), String> {
    if reports.len() > MAX_JUNIT_REPORTS {
        return Err(format!(
            "gate `{gate_id}` declares more than {MAX_JUNIT_REPORTS} JUnit reports"
        ));
    }
    let mut report_paths = std::collections::BTreeSet::new();
    for report in reports {
        validate_report_path(report)
            .map_err(|reason| format!("gate `{gate_id}` JUnit report: {reason}"))?;
        if !report_paths.insert(report) {
            return Err(format!(
                "gate `{gate_id}` declares duplicate JUnit report `{report}`"
            ));
        }
    }
    Ok(())
}

/// Requires a report path to be a non-empty repository-relative path.
fn validate_report_path(value: &str) -> Result<(), String> {
    if value.is_empty() || value == "." {
        return Err("path must name a report file".to_owned());
    }
    if value.starts_with('/') || value.contains('\0') {
        return Err("path must be relative to the gate working directory".to_owned());
    }
    if std::path::Path::new(value)
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("path must not traverse outside the gate working directory".to_owned());
    }
    Ok(())
}

/// Requires a working directory to stay inside the evaluation worktree.
fn validate_working_directory(value: &str) -> Result<(), String> {
    if value.is_empty() || value == "." {
        return Ok(());
    }
    if value.starts_with('/') {
        return Err(format!(
            "working directory `{value}` must be relative to the evaluation worktree"
        ));
    }
    if value.split('/').any(|segment| segment == "..") {
        return Err(format!(
            "working directory `{value}` must not traverse outside the evaluation worktree"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate() -> GateDefinition {
        GateDefinition {
            schema: GATE_SCHEMA.to_owned(),
            gate_id: "gate.unit".to_owned(),
            revision: 1,
            argv: vec!["cargo".to_owned(), "test".to_owned()],
            working_directory: ".".to_owned(),
            timeout_seconds: 600,
            environment: GateEnvironment::default(),
            network_policy: NetworkPolicy::Denied,
            retry_policy: RetryPolicy::default(),
            artifacts: vec![],
            junit_reports: vec![],
        }
    }

    #[test]
    fn a_well_formed_gate_validates() {
        gate().validate().expect("this gate is well formed");
    }

    #[test]
    fn an_empty_argv_is_rejected() {
        let mut invalid = gate();
        invalid.argv = vec![];
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn a_command_line_masquerading_as_argv_is_rejected() {
        // The failure mode this catches: someone writes argv as one string,
        // expecting it to be split. Nothing splits it, so it would try to exec
        // a program literally named "cargo test --all".
        let mut invalid = gate();
        invalid.argv = vec!["cargo test --all".to_owned()];
        let error = invalid.validate().expect_err("must reject");
        assert!(error.to_string().contains("not a command line"));
    }

    #[test]
    fn shell_metacharacters_in_the_executable_are_rejected() {
        for bad in [
            "sh -c 'rm -rf /'",
            "cargo test | tee log",
            "cargo test; rm -rf /",
            "cargo test && echo done",
            "cargo test > out",
            "$(whoami)",
            "`whoami`",
        ] {
            let mut invalid = gate();
            invalid.argv = vec![bad.to_owned()];
            assert!(
                invalid.validate().is_err(),
                "`{bad}` must be rejected at registration"
            );
        }
    }

    #[test]
    fn a_multi_element_argv_may_contain_spaces_in_arguments() {
        // Only the executable is checked for shell syntax; an argument may
        // legitimately contain spaces, because nothing splits it.
        let mut valid = gate();
        valid.argv = vec![
            "cargo".to_owned(),
            "test".to_owned(),
            "--test-threads 1".to_owned(),
        ];
        valid.validate().expect("arguments may contain spaces");
    }

    #[test]
    fn a_zero_or_excessive_timeout_is_rejected() {
        let mut zero = gate();
        zero.timeout_seconds = 0;
        assert!(zero.validate().is_err());

        let mut excessive = gate();
        excessive.timeout_seconds = MAX_TIMEOUT_SECONDS + 1;
        assert!(excessive.validate().is_err());
    }

    #[test]
    fn zero_attempts_is_rejected() {
        let mut invalid = gate();
        invalid.retry_policy = RetryPolicy { max_attempts: 0 };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn the_default_retry_policy_is_a_single_attempt() {
        assert_eq!(RetryPolicy::default().max_attempts, 1);
    }

    #[test]
    fn a_working_directory_outside_the_worktree_is_rejected() {
        for bad in ["/etc", "../escape", "sub/../../escape"] {
            let mut invalid = gate();
            invalid.working_directory = bad.to_owned();
            assert!(invalid.validate().is_err(), "`{bad}` must be rejected");
        }
    }

    #[test]
    fn a_relative_working_directory_is_accepted() {
        for good in ["", ".", "crates/core", "sub/dir"] {
            let mut valid = gate();
            valid.working_directory = good.to_owned();
            valid.validate().unwrap_or_else(|error| {
                panic!("`{good}` should be accepted: {error}");
            });
        }
    }

    #[test]
    fn credential_variables_are_denied_even_when_explicitly_allowed() {
        let mut invalid = gate();
        invalid.environment.allow = vec!["GITHUB_TOKEN".to_owned()];
        let error = invalid.validate().expect_err("must reject");
        assert!(error.to_string().contains("GITHUB_TOKEN"));

        let mut also_invalid = gate();
        also_invalid
            .environment
            .set
            .insert("AWS_SECRET_ACCESS_KEY".to_owned(), "x".to_owned());
        assert!(also_invalid.validate().is_err());
    }

    #[test]
    fn an_ordinary_variable_may_be_allowed_or_set() {
        let mut valid = gate();
        valid.environment.allow = vec!["PATH".to_owned(), "HOME".to_owned()];
        valid
            .environment
            .set
            .insert("CI".to_owned(), "true".to_owned());
        valid.validate().expect("ordinary variables are fine");
    }

    #[test]
    fn an_invalid_gate_identifier_is_rejected() {
        for bad in ["", "has space", "has/slash", "has:colon"] {
            let mut invalid = gate();
            invalid.gate_id = bad.to_owned();
            assert!(invalid.validate().is_err(), "`{bad}` must be rejected");
        }
    }

    #[test]
    fn revision_zero_is_rejected() {
        let mut invalid = gate();
        invalid.revision = 0;
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn a_revision_change_moves_the_digest() {
        let first = gate().digest().unwrap();
        let mut revised = gate();
        revised.revision = 2;
        assert_ne!(
            first,
            revised.digest().unwrap(),
            "a revision must invalidate receipts bound to the old digest"
        );
    }

    #[test]
    fn any_material_change_moves_the_digest() {
        let base = gate().digest().unwrap();
        for mutate in [
            (|g: &mut GateDefinition| g.argv.push("--all".to_owned())) as fn(&mut GateDefinition),
            |g: &mut GateDefinition| g.timeout_seconds = 30,
            |g: &mut GateDefinition| g.network_policy = NetworkPolicy::Allowed,
            |g: &mut GateDefinition| g.retry_policy = RetryPolicy { max_attempts: 3 },
            |g: &mut GateDefinition| g.working_directory = "sub".to_owned(),
        ] {
            let mut changed = gate();
            mutate(&mut changed);
            assert_ne!(base, changed.digest().unwrap());
        }
    }

    #[test]
    fn an_empty_junit_declaration_preserves_legacy_gate_digest() {
        let original = gate();
        let mut legacy = serde_json::to_value(&original).unwrap();
        legacy.as_object_mut().unwrap().remove("junit_reports");
        let decoded: GateDefinition = serde_json::from_value(legacy).unwrap();
        assert_eq!(original.digest().unwrap(), decoded.digest().unwrap());

        let mut reported = original;
        reported.junit_reports = vec!["target/junit.xml".to_owned()];
        assert_ne!(decoded.digest().unwrap(), reported.digest().unwrap());
    }

    #[test]
    fn every_rendering_of_a_network_policy_states_its_enforcement_status() {
        // The point is not the wording, it is that neither variant can reach a
        // reader as a bare fact. Stated as a relationship rather than as
        // today's answer, so `WP-710` flipping `ENFORCED` fails here and sends
        // whoever flipped it to `describe`, instead of silently leaving a
        // caveat on a policy that by then actually holds.
        let expected = if NetworkPolicy::ENFORCED {
            "enforced"
        } else {
            "declared"
        };
        for policy in [NetworkPolicy::Denied, NetworkPolicy::Allowed] {
            assert!(
                policy.describe().contains(expected),
                "{policy:?} renders as `{}`, which never says `{expected}`",
                policy.describe()
            );
        }
        assert_eq!(
            NetworkPolicy::Denied.describe().contains("not enforced"),
            !NetworkPolicy::ENFORCED,
            "the variant a reader would mistake for isolation says outright whether it is one"
        );
    }

    #[test]
    fn a_definition_round_trips_and_rejects_unknown_fields() {
        let encoded = serde_json::to_string_pretty(&gate()).unwrap();
        let decoded: GateDefinition = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, gate());

        let mut value = serde_json::to_value(gate()).unwrap();
        value["surprise"] = serde_json::json!(1);
        assert!(serde_json::from_value::<GateDefinition>(value).is_err());
    }
}
