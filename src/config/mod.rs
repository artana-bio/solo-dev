//! Project configuration: the `harness.project/v1` schema and its validation.
//!
//! Configuration names the three repositories whose separation the whole trust
//! model rests on. A configuration error is therefore refused before any
//! mutation, never repaired silently.

pub mod validate;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{
    domain::{card::Risk, ids::ProjectId},
    error::{ErrorCode, HarnessError},
};

/// Schema identifier for the project file.
pub const PROJECT_SCHEMA: &str = "harness.project/v1";

/// Default name of the Git remote pointing at the authority repository.
pub const DEFAULT_AUTHORITY_REMOTE: &str = "harness-authority";

/// The only progressive-validation policy this release understands.
pub const VALIDATION_POLICY_V1: &str = "harness.validation-policy/v1";

/// Project-wide rules that decide which risks require a declared proof map.
///
/// The policy is versioned because every activated card binds this project
/// document through its cycle digest. Later policy versions must not quietly
/// reinterpret an existing card.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ValidationPolicy {
    /// Stable identifier for the policy semantics.
    pub version: String,
    /// Risks that cannot activate or revise without a complete proof map.
    pub proof_map_required_for: Vec<Risk>,
}

impl Default for ValidationPolicy {
    fn default() -> Self {
        Self {
            version: VALIDATION_POLICY_V1.to_owned(),
            proof_map_required_for: vec![Risk::Medium, Risk::High, Risk::Critical],
        }
    }
}

impl ValidationPolicy {
    /// Rejects an unrecognized or ambiguous policy before it can authorize a
    /// card definition.
    ///
    /// # Errors
    ///
    /// Returns a field error when the version is unknown or the risk list is
    /// empty or duplicates a risk.
    pub fn validate(&self) -> Result<(), FieldError> {
        if self.version != VALIDATION_POLICY_V1 {
            return Err(FieldError::new(
                "validation_policy.version",
                format!(
                    "expected `{VALIDATION_POLICY_V1}`, found `{}`",
                    self.version
                ),
                ErrorCode::ConfigInvalidValue,
            ));
        }
        if self.proof_map_required_for.is_empty() {
            return Err(FieldError::new(
                "validation_policy.proof_map_required_for",
                "must name at least one risk; omit validation_policy to use the compatible default",
                ErrorCode::ConfigInvalidValue,
            ));
        }
        for risk in [Risk::Low, Risk::Medium, Risk::High, Risk::Critical] {
            let count = self
                .proof_map_required_for
                .iter()
                .filter(|configured| **configured == risk)
                .count();
            if count > 1 {
                return Err(FieldError::new(
                    "validation_policy.proof_map_required_for",
                    format!("names `{}` more than once", risk.name()),
                    ErrorCode::ConfigInvalidValue,
                ));
            }
        }
        Ok(())
    }

    /// Whether a card at `risk` must carry a complete immutable proof map.
    #[must_use]
    pub fn requires_proof_map(&self, risk: Risk) -> bool {
        self.proof_map_required_for.contains(&risk)
    }
}

/// Which configured field a failure refers to.
///
/// Carried so both text and JSON diagnostics can name the exact invalid field
/// rather than reporting that "configuration is invalid".
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldError {
    /// Dotted path to the field, such as `host_policy.minimum_git_version`.
    pub field: String,
    /// Why the value was rejected.
    pub reason: String,
    /// The stable code for this class of configuration failure.
    pub code: ErrorCode,
}

impl FieldError {
    /// Builds a field error.
    #[must_use]
    pub fn new(field: impl Into<String>, reason: impl Into<String>, code: ErrorCode) -> Self {
        Self {
            field: field.into(),
            reason: reason.into(),
            code,
        }
    }

    /// Converts to the harness error type.
    #[must_use]
    pub fn into_error(self) -> HarnessError {
        HarnessError::Config {
            field: self.field,
            reason: self.reason,
            code: self.code,
        }
    }
}

/// Host constraints a project declares.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HostPolicy {
    /// Operating systems this project supports, using Rust's `std::env::consts`
    /// naming.
    pub supported_os: Vec<String>,
    /// Lowest acceptable Git version, as `major.minor.patch`.
    pub minimum_git_version: String,
}

impl Default for HostPolicy {
    fn default() -> Self {
        Self {
            supported_os: vec!["macos".to_owned()],
            minimum_git_version: "2.50.0".to_owned(),
        }
    }
}

/// The authoritative project file.
///
/// `deny_unknown_fields` is deliberate. A typo in an authoritative document
/// must fail loudly rather than be silently ignored, because a silently
/// ignored field looks configured to the operator and is not.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    /// Always [`PROJECT_SCHEMA`].
    pub schema: String,
    /// Identifies this project.
    pub project_id: ProjectId,
    /// The candidate repository where feature work happens.
    pub repository: PathBuf,
    /// The control repository holding authoritative records.
    pub control_repository: PathBuf,
    /// The bare authority repository owning the protected ref.
    pub authority_repository: PathBuf,
    /// Name of the remote pointing at the authority repository.
    pub authority_remote: String,
    /// The protected branch promotion targets.
    pub protected_branch: String,
    /// Root directory under which card worktrees are allocated.
    pub worktree_root: PathBuf,
    /// Output format used when the caller does not choose one.
    pub default_output: String,
    /// Host constraints.
    pub host_policy: HostPolicy,
    /// Progressive-validation proof requirements. Omission preserves legacy
    /// project documents with the shipped v1 policy.
    #[serde(default)]
    pub validation_policy: ValidationPolicy,
}

impl ProjectConfig {
    /// Parses a project document, rejecting unknown fields.
    ///
    /// # Errors
    ///
    /// Returns a configuration error naming the offending field when the
    /// document is malformed or carries an undefined field.
    pub fn from_json(raw: &str) -> Result<Self, HarnessError> {
        let config: Self = serde_json::from_str(raw).map_err(|source| {
            let message = source.to_string();
            // serde reports an unknown field distinctly from a syntax error,
            // and the operator fix differs, so the codes differ too.
            let (field, code) = if let Some(name) = unknown_field_name(&message) {
                (name, ErrorCode::ConfigUnknownField)
            } else {
                (String::from("<document>"), ErrorCode::ConfigMalformed)
            };
            HarnessError::Config {
                field,
                reason: message,
                code,
            }
        })?;

        if config.schema != PROJECT_SCHEMA {
            return Err(FieldError::new(
                "schema",
                format!("expected `{PROJECT_SCHEMA}`, found `{}`", config.schema),
                ErrorCode::ConfigInvalidValue,
            )
            .into_error());
        }
        config
            .validation_policy
            .validate()
            .map_err(FieldError::into_error)?;
        Ok(config)
    }

    /// Renders the project document as pretty JSON.
    ///
    /// # Errors
    ///
    /// Returns an error when serialization fails.
    pub fn to_json(&self) -> Result<String, HarnessError> {
        serde_json::to_string_pretty(self).map_err(HarnessError::from)
    }

    /// Every configured path with the field name that supplied it.
    #[must_use]
    pub fn labeled_paths(&self) -> Vec<(&'static str, &PathBuf)> {
        vec![
            ("repository", &self.repository),
            ("control_repository", &self.control_repository),
            ("authority_repository", &self.authority_repository),
            ("worktree_root", &self.worktree_root),
        ]
    }
}

/// Extracts the field name from serde's unknown-field message.
fn unknown_field_name(message: &str) -> Option<String> {
    let rest = message.strip_prefix("unknown field `")?;
    let (name, _) = rest.split_once('`')?;
    Some(name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Section 9.2 example document.
    fn valid_document() -> String {
        format!(
            r#"{{
  "schema": "{PROJECT_SCHEMA}",
  "project_id": "example",
  "repository": "/abs/repository",
  "control_repository": "/abs/example-control",
  "authority_repository": "/abs/example-authority.git",
  "authority_remote": "{DEFAULT_AUTHORITY_REMOTE}",
  "protected_branch": "main",
  "worktree_root": "/abs/example-worktrees",
  "default_output": "text",
  "host_policy": {{
    "supported_os": ["macos"],
    "minimum_git_version": "2.50.0"
  }}
}}"#
        )
    }

    #[test]
    fn parses_the_documented_example() {
        let config = ProjectConfig::from_json(&valid_document()).unwrap();
        assert_eq!(config.project_id.as_str(), "example");
        assert_eq!(config.protected_branch, "main");
        assert_eq!(config.host_policy.supported_os, vec!["macos"]);
        assert_eq!(
            config.validation_policy,
            ValidationPolicy::default(),
            "omitting the additive field preserves legacy project records"
        );
    }

    #[test]
    fn validation_policy_refuses_unknown_versions_and_duplicate_risks() {
        let unknown = valid_document().replace(
            "\"host_policy\": {",
            "\"validation_policy\": {\"version\": \"harness.validation-policy/v99\", \"proof_map_required_for\": [\"medium\"]},\n  \"host_policy\": {",
        );
        let error = ProjectConfig::from_json(&unknown).expect_err("unknown policy must refuse");
        assert_eq!(error.code(), ErrorCode::ConfigInvalidValue);
        assert_eq!(error.details()["field"], "validation_policy.version");

        let duplicate = valid_document().replace(
            "\"host_policy\": {",
            "\"validation_policy\": {\"version\": \"harness.validation-policy/v1\", \"proof_map_required_for\": [\"medium\", \"medium\"]},\n  \"host_policy\": {",
        );
        let error = ProjectConfig::from_json(&duplicate).expect_err("duplicate risk must refuse");
        assert_eq!(error.code(), ErrorCode::ConfigInvalidValue);
        assert_eq!(
            error.details()["field"],
            "validation_policy.proof_map_required_for"
        );
    }

    #[test]
    fn round_trips_through_json() {
        let config = ProjectConfig::from_json(&valid_document()).unwrap();
        let reparsed = ProjectConfig::from_json(&config.to_json().unwrap()).unwrap();
        assert_eq!(config, reparsed);
    }

    #[test]
    fn an_unknown_field_fails_and_names_itself() {
        let document = valid_document().replace(
            r#""default_output": "text","#,
            r#""default_output": "text", "typo_field": 1,"#,
        );
        let error = ProjectConfig::from_json(&document).expect_err("must reject");
        assert_eq!(error.code(), ErrorCode::ConfigUnknownField);
        assert_eq!(error.details()["field"], "typo_field");
    }

    #[test]
    fn an_unknown_nested_field_fails() {
        let document = valid_document().replace(
            r#""minimum_git_version": "2.50.0""#,
            r#""minimum_git_version": "2.50.0", "extra": true"#,
        );
        let error = ProjectConfig::from_json(&document).expect_err("must reject");
        assert_eq!(error.code(), ErrorCode::ConfigUnknownField);
        assert_eq!(error.details()["field"], "extra");
    }

    #[test]
    fn a_missing_field_fails_as_malformed() {
        let document = valid_document().replace(
            r#"  "protected_branch": "main",
"#,
            "",
        );
        let error = ProjectConfig::from_json(&document).expect_err("must reject");
        assert_eq!(error.code(), ErrorCode::ConfigMalformed);
    }

    #[test]
    fn a_syntax_error_fails_as_malformed() {
        let error = ProjectConfig::from_json("{ not json").expect_err("must reject");
        assert_eq!(error.code(), ErrorCode::ConfigMalformed);
        assert_eq!(
            error.category(),
            crate::cli::exit::ExitCategory::Configuration
        );
    }

    #[test]
    fn a_wrong_schema_identifier_fails_and_names_the_field() {
        let document = valid_document().replace(PROJECT_SCHEMA, "harness.project/v99");
        let error = ProjectConfig::from_json(&document).expect_err("must reject");
        assert_eq!(error.code(), ErrorCode::ConfigInvalidValue);
        assert_eq!(error.details()["field"], "schema");
    }

    #[test]
    fn an_invalid_project_id_fails() {
        let document =
            valid_document().replace(r#""project_id": "example""#, r#""project_id": "Bad Id""#);
        assert!(ProjectConfig::from_json(&document).is_err());
    }

    #[test]
    fn labeled_paths_cover_every_configured_location() {
        let config = ProjectConfig::from_json(&valid_document()).unwrap();
        let fields: Vec<_> = config
            .labeled_paths()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(
            fields,
            vec![
                "repository",
                "control_repository",
                "authority_repository",
                "worktree_root"
            ]
        );
    }

    #[test]
    fn unknown_field_name_extraction_handles_serde_messages() {
        assert_eq!(
            unknown_field_name("unknown field `typo`, expected one of `a`, `b`"),
            Some("typo".to_owned())
        );
        assert_eq!(unknown_field_name("expected value at line 1"), None);
    }
}
