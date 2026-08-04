//! Project configuration: the `harness.project/v1` schema and its validation.
//!
//! Configuration names the three repositories whose separation the whole trust
//! model rests on. A configuration error is therefore refused before any
//! mutation, never repaired silently.

pub mod validate;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{
    domain::{card::Risk, digest::Digest, ids::ProjectId},
    error::{ErrorCode, HarnessError},
    policy::actors,
};

/// Schema identifier for the project file.
pub const PROJECT_SCHEMA: &str = "harness.project/v1";

/// Default name of the Git remote pointing at the authority repository.
pub const DEFAULT_AUTHORITY_REMOTE: &str = "harness-authority";

/// The only progressive-validation policy this release understands.
pub const VALIDATION_POLICY_V1: &str = "harness.validation-policy/v1";

/// The versioned policy that names the declared actor(s) permitted to make the
/// one final authorization of a sealed cycle. Identity remains declared rather
/// than provider-verified; this policy makes the declaration auditable.
pub const FINAL_AUTHORIZATION_POLICY_V1: &str = "harness.final-authorization-policy/v1";

/// The first shipped, explicitly bounded convergence-budget policy.
pub const CONVERGENCE_POLICY_V1: &str = "harness.convergence-policy/v1";

/// Limits for the attempts a single card may consume at one risk level.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CardConvergenceLimits {
    pub review_returns: u32,
    pub repair_attempts: u32,
    pub gate_failures: u32,
    pub material_scope_revisions: u32,
}

impl CardConvergenceLimits {
    fn validate(&self, field: &str) -> Result<(), FieldError> {
        for (name, value) in [
            ("review_returns", self.review_returns),
            ("repair_attempts", self.repair_attempts),
            ("gate_failures", self.gate_failures),
            ("material_scope_revisions", self.material_scope_revisions),
        ] {
            if value == 0 {
                return Err(FieldError::new(
                    format!("{field}.{name}"),
                    "must be greater than zero",
                    ErrorCode::ConfigInvalidValue,
                ));
            }
        }
        Ok(())
    }
}

/// The card limits selected for each declared risk level.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RiskConvergenceLimits {
    pub low: CardConvergenceLimits,
    pub medium: CardConvergenceLimits,
    pub high: CardConvergenceLimits,
    pub critical: CardConvergenceLimits,
}

impl RiskConvergenceLimits {
    fn validate(&self) -> Result<(), FieldError> {
        self.low.validate("convergence_policy.card_limits.low")?;
        self.medium
            .validate("convergence_policy.card_limits.medium")?;
        self.high.validate("convergence_policy.card_limits.high")?;
        self.critical
            .validate("convergence_policy.card_limits.critical")?;
        Ok(())
    }

    #[must_use]
    pub fn for_risk(&self, risk: Risk) -> &CardConvergenceLimits {
        match risk {
            Risk::Low => &self.low,
            Risk::Medium => &self.medium,
            Risk::High => &self.high,
            Risk::Critical => &self.critical,
        }
    }
}

/// Cycle-level convergence limits.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CycleConvergenceLimits {
    pub integration_failures: u32,
}

/// Versioned limits whose digest facts bind before they may be counted.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ConvergencePolicy {
    pub version: String,
    pub card_limits: RiskConvergenceLimits,
    pub cycle_limits: CycleConvergenceLimits,
}

impl ConvergencePolicy {
    /// Validates the complete policy; zero is never a hidden "unlimited" value.
    ///
    /// # Errors
    ///
    /// Returns a field error for an unsupported version or a zero limit.
    pub fn validate(&self) -> Result<(), FieldError> {
        if self.version != CONVERGENCE_POLICY_V1 {
            return Err(FieldError::new(
                "convergence_policy.version",
                format!("must name `{CONVERGENCE_POLICY_V1}`"),
                ErrorCode::ConfigInvalidValue,
            ));
        }
        self.card_limits.validate()?;
        if self.cycle_limits.integration_failures == 0 {
            return Err(FieldError::new(
                "convergence_policy.cycle_limits.integration_failures",
                "must be greater than zero",
                ErrorCode::ConfigInvalidValue,
            ));
        }
        Ok(())
    }

    /// Returns the canonical digest facts must name before projection.
    ///
    /// # Errors
    ///
    /// Returns an error when canonical serialization fails.
    pub fn digest(&self) -> Result<Digest, HarnessError> {
        Digest::of_canonical(self)
    }
}

/// A human decision that can halt a reviewed final integration.
///
/// These names are deliberately closed.  The harness never infers that an
/// exception exists from a failed check or a conversation; a configured actor
/// must raise one of these declared reasons with evidence.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ExceptionTrigger {
    PolicyChange,
    ExternalEffect,
    CriticalResidualRisk,
    AmbiguousRollback,
    ConvergenceBudgetExhausted,
}

impl ExceptionTrigger {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::PolicyChange => "policy_change",
            Self::ExternalEffect => "external_effect",
            Self::CriticalResidualRisk => "critical_residual_risk",
            Self::AmbiguousRollback => "ambiguous_rollback",
            Self::ConvergenceBudgetExhausted => "convergence_budget_exhausted",
        }
    }

    /// # Errors
    ///
    /// Returns a configuration error when `value` is not a shipped trigger.
    pub fn parse(value: &str) -> Result<Self, HarnessError> {
        match value {
            "policy_change" => Ok(Self::PolicyChange),
            "external_effect" => Ok(Self::ExternalEffect),
            "critical_residual_risk" => Ok(Self::CriticalResidualRisk),
            "ambiguous_rollback" => Ok(Self::AmbiguousRollback),
            "convergence_budget_exhausted" => Ok(Self::ConvergenceBudgetExhausted),
            _ => Err(HarnessError::Config {
                field: "final_authorization_policy.exception_triggers".to_owned(),
                reason: format!("unsupported exception trigger `{value}`"),
                code: ErrorCode::ConfigInvalidValue,
            }),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FinalAuthorizationPolicy {
    pub version: String,
    pub authorization_unit: String,
    pub authorizer_actor_ids: Vec<String>,
    /// Explicitly delegated reasons that may halt a final integration.  Empty
    /// preserves the v1 behaviour: no exception command is enabled.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exception_triggers: Vec<ExceptionTrigger>,
}

impl Default for FinalAuthorizationPolicy {
    fn default() -> Self {
        Self {
            version: FINAL_AUTHORIZATION_POLICY_V1.to_owned(),
            authorization_unit: "sealed_cycle".to_owned(),
            authorizer_actor_ids: vec!["owner".to_owned()],
            exception_triggers: vec![],
        }
    }
}

impl FinalAuthorizationPolicy {
    /// Validates the policy values this release can enforce.
    ///
    /// # Errors
    ///
    /// Returns a field error when the policy is unsupported or ambiguous.
    pub fn validate(&self) -> Result<(), FieldError> {
        if self.version != FINAL_AUTHORIZATION_POLICY_V1 {
            return Err(FieldError::new(
                "final_authorization_policy.version",
                "must name the shipped v1 final-authorization policy",
                ErrorCode::ConfigInvalidValue,
            ));
        }
        if self.authorization_unit != "sealed_cycle" {
            return Err(FieldError::new(
                "final_authorization_policy.authorization_unit",
                "must be `sealed_cycle`",
                ErrorCode::ConfigInvalidValue,
            ));
        }
        if self.authorizer_actor_ids.is_empty()
            || self
                .authorizer_actor_ids
                .iter()
                .any(|id| id.trim().is_empty())
        {
            return Err(FieldError::new(
                "final_authorization_policy.authorizer_actor_ids",
                "must name at least one non-empty declared actor",
                ErrorCode::ConfigInvalidValue,
            ));
        }
        for actor_id in &self.authorizer_actor_ids {
            actors::refuse_unusable("final authorization policy actor", actor_id).map_err(
                |error| {
                    FieldError::new(
                        "final_authorization_policy.authorizer_actor_ids",
                        error.to_string(),
                        ErrorCode::ConfigInvalidValue,
                    )
                },
            )?;
        }
        for (index, actor_id) in self.authorizer_actor_ids.iter().enumerate() {
            if self.authorizer_actor_ids[..index]
                .iter()
                .any(|prior| actors::same(prior, actor_id))
            {
                return Err(FieldError::new(
                    "final_authorization_policy.authorizer_actor_ids",
                    "must not contain duplicate actor identifiers after normalization",
                    ErrorCode::ConfigInvalidValue,
                ));
            }
        }
        for (index, trigger) in self.exception_triggers.iter().enumerate() {
            if self.exception_triggers[..index].contains(trigger) {
                return Err(FieldError::new(
                    "final_authorization_policy.exception_triggers",
                    "must not contain duplicate exception triggers",
                    ErrorCode::ConfigInvalidValue,
                ));
            }
        }
        Ok(())
    }

    /// Returns the canonical digest a v2 authorization pins.
    ///
    /// # Errors
    ///
    /// Returns an error when canonical serialization fails.
    pub fn digest(&self) -> Result<Digest, HarnessError> {
        Digest::of_canonical(self)
    }

    #[must_use]
    pub fn authorizes(&self, actor_id: &str) -> bool {
        self.authorizer_actor_ids
            .iter()
            .any(|configured| configured == actor_id)
    }

    #[must_use]
    pub fn enables_exception(&self, trigger: ExceptionTrigger) -> bool {
        self.exception_triggers.contains(&trigger)
    }
}

/// An ordered point in the progressive-validation ladder.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStage {
    /// Cheap, card-local checks before handoff.
    Narrow,
    /// Checks required to make a handoff reviewable.
    Handoff,
    /// Checks only meaningful on the integrated result.
    FinalIntegration,
}

impl ValidationStage {
    /// Stable stage name for text output and policy diagnostics.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Narrow => "narrow",
            Self::Handoff => "handoff",
            Self::FinalIntegration => "final_integration",
        }
    }

    const fn order(self) -> u8 {
        match self {
            Self::Narrow => 0,
            Self::Handoff => 1,
            Self::FinalIntegration => 2,
        }
    }
}

/// Required validation stages by card risk.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RiskStageRequirements {
    /// Low-risk cards still need a narrow proof and final integration check.
    pub low: Vec<ValidationStage>,
    /// Medium-risk work also needs a handoff-stage check.
    pub medium: Vec<ValidationStage>,
    /// High-risk work receives the full declared ladder.
    pub high: Vec<ValidationStage>,
    /// Critical work receives the full declared ladder; human assurance is
    /// separately governed by the review policy.
    pub critical: Vec<ValidationStage>,
}

impl Default for RiskStageRequirements {
    fn default() -> Self {
        Self {
            low: vec![ValidationStage::Narrow, ValidationStage::FinalIntegration],
            medium: vec![
                ValidationStage::Narrow,
                ValidationStage::Handoff,
                ValidationStage::FinalIntegration,
            ],
            high: vec![
                ValidationStage::Narrow,
                ValidationStage::Handoff,
                ValidationStage::FinalIntegration,
            ],
            critical: vec![
                ValidationStage::Narrow,
                ValidationStage::Handoff,
                ValidationStage::FinalIntegration,
            ],
        }
    }
}

impl RiskStageRequirements {
    /// Required stages for one risk tier.
    #[must_use]
    pub fn for_risk(&self, risk: Risk) -> &[ValidationStage] {
        match risk {
            Risk::Low => &self.low,
            Risk::Medium => &self.medium,
            Risk::High => &self.high,
            Risk::Critical => &self.critical,
        }
    }

    fn validate(&self) -> Result<(), FieldError> {
        for (field, stages) in [
            ("low", &self.low),
            ("medium", &self.medium),
            ("high", &self.high),
            ("critical", &self.critical),
        ] {
            if stages.is_empty() {
                return Err(FieldError::new(
                    format!("validation_policy.stage_requirements.{field}"),
                    "must contain at least one ordered validation stage",
                    ErrorCode::ConfigInvalidValue,
                ));
            }
            if stages
                .windows(2)
                .any(|pair| pair[0].order() >= pair[1].order())
            {
                return Err(FieldError::new(
                    format!("validation_policy.stage_requirements.{field}"),
                    "must list each stage at most once in narrow, handoff, final_integration order",
                    ErrorCode::ConfigInvalidValue,
                ));
            }
        }
        Ok(())
    }
}

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
    /// Ordered stages each risk tier must declare. Omission preserves legacy
    /// project records with the shipped v1 ladder.
    #[serde(default)]
    pub stage_requirements: RiskStageRequirements,
}

impl Default for ValidationPolicy {
    fn default() -> Self {
        Self {
            version: VALIDATION_POLICY_V1.to_owned(),
            proof_map_required_for: vec![Risk::Medium, Risk::High, Risk::Critical],
            stage_requirements: RiskStageRequirements::default(),
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
        self.stage_requirements.validate()?;
        Ok(())
    }

    /// Whether a card at `risk` must carry a complete immutable proof map.
    #[must_use]
    pub fn requires_proof_map(&self, risk: Risk) -> bool {
        self.proof_map_required_for.contains(&risk)
    }

    /// The ordered stages a card at `risk` must declare.
    #[must_use]
    pub fn required_stages(&self, risk: Risk) -> &[ValidationStage] {
        self.stage_requirements.for_risk(risk)
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
    /// Declared final-cycle authorization policy. Omission preserves the
    /// compatible v1 default while every v2 acceptance pins its digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_authorization_policy: Option<FinalAuthorizationPolicy>,
    /// Omission is deliberately not a default: old projects are reported as
    /// `legacy_unassessed` rather than silently receiving new budgets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub convergence_policy: Option<ConvergencePolicy>,
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
        if let Some(policy) = &config.final_authorization_policy {
            policy.validate().map_err(FieldError::into_error)?;
        }
        if let Some(policy) = &config.convergence_policy {
            policy.validate().map_err(FieldError::into_error)?;
        }
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

        let unordered = valid_document().replace(
            "\"host_policy\": {",
            "\"validation_policy\": {\"version\": \"harness.validation-policy/v1\", \"proof_map_required_for\": [\"medium\"], \"stage_requirements\": {\"low\": [\"final_integration\", \"narrow\"], \"medium\": [\"narrow\"], \"high\": [\"narrow\"], \"critical\": [\"narrow\"]}},\n  \"host_policy\": {",
        );
        let error = ProjectConfig::from_json(&unordered).expect_err("unordered stages must refuse");
        assert_eq!(error.code(), ErrorCode::ConfigInvalidValue);
        assert_eq!(
            error.details()["field"],
            "validation_policy.stage_requirements.low"
        );

        // #29 may eventually offer a versioned learning suggestion, but it
        // cannot become a back door that changes #30's required stages. There
        // is deliberately no suggestion field in the executable policy.
        let suggestion = valid_document().replace(
            "\"host_policy\": {",
            "\"validation_policy\": {\"version\": \"harness.validation-policy/v1\", \"proof_map_required_for\": [\"medium\"], \"pattern_suggestion\": {\"required_stages\": [\"final_integration\"]}},\n  \"host_policy\": {",
        );
        assert!(
            ProjectConfig::from_json(&suggestion).is_err(),
            "a learning suggestion must not silently alter the executable policy"
        );
    }

    #[test]
    fn convergence_policy_refuses_unknown_missing_and_zero_limits() {
        let complete = r#"{"version":"harness.convergence-policy/v1","card_limits":{"low":{"review_returns":1,"repair_attempts":1,"gate_failures":1,"material_scope_revisions":1},"medium":{"review_returns":1,"repair_attempts":1,"gate_failures":1,"material_scope_revisions":1},"high":{"review_returns":1,"repair_attempts":1,"gate_failures":1,"material_scope_revisions":1},"critical":{"review_returns":1,"repair_attempts":1,"gate_failures":1,"material_scope_revisions":1}},"cycle_limits":{"integration_failures":1}}"#;
        let with_policy = |policy: &str| {
            valid_document().replace(
                "\"host_policy\": {",
                &format!("\"convergence_policy\": {policy},\n  \"host_policy\": {{"),
            )
        };
        let unknown = complete.replace("/v1", "/v99");
        assert!(ProjectConfig::from_json(&with_policy(&unknown)).is_err());
        let missing = complete.replace("\"high\":{\"review_returns\":1,\"repair_attempts\":1,\"gate_failures\":1,\"material_scope_revisions\":1},", "");
        assert!(ProjectConfig::from_json(&with_policy(&missing)).is_err());
        let zero = complete.replace("\"integration_failures\":1", "\"integration_failures\":0");
        assert!(ProjectConfig::from_json(&with_policy(&zero)).is_err());
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
