//! Stable command output envelopes.
//!
//! Section 12.4 fixes these shapes. Machine consumers depend on them, so keys
//! are always emitted, including as `null`, rather than omitted when empty. A
//! key that appears only sometimes forces every consumer to branch.

use serde::{Deserialize, Serialize};

use crate::{
    domain::ids::{OperationId, ProjectId},
    error::HarnessError,
    policy::credential_shape::redact_github_tokens,
};

/// Schema identifier for a successful command result.
pub const RESULT_SCHEMA: &str = "harness.command-result/v1";
/// Schema identifier for a failed command result.
pub const ERROR_SCHEMA: &str = "harness.command-error/v1";

/// Renders one parsed-command error for text mode.
#[must_use]
pub fn render_text_error(error: &HarnessError) -> String {
    redact_github_tokens(&error.to_string())
}

/// How a command renders its result.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, clap::ValueEnum)]
pub enum OutputFormat {
    /// Human-readable text.
    #[default]
    Text,
    /// The stable JSON envelope.
    Json,
}

/// The successful-result envelope.
#[derive(Debug, Deserialize, Serialize)]
pub struct CommandResult {
    /// Always [`RESULT_SCHEMA`].
    pub schema: String,
    /// Dotted command path, such as `work.start`.
    pub command: String,
    /// Always `"success"`.
    pub status: String,
    /// The project this command acted on, when it had one.
    pub project_id: Option<ProjectId>,
    /// The journaled operation, when the command mutated state.
    pub operation_id: Option<OperationId>,
    /// Command-specific payload.
    pub data: serde_json::Value,
    /// Non-fatal advisories.
    pub warnings: Vec<String>,
}

/// The error payload carried inside [`CommandErrorEnvelope`].
#[derive(Debug, Deserialize, Serialize)]
pub struct CommandErrorBody {
    /// Stable machine-readable code, such as `CH-USAGE-INVALID-ID`.
    pub code: String,
    /// Human-readable description.
    pub message: String,
    /// Structured detail fields.
    pub details: serde_json::Value,
    /// Operator guidance.
    pub recovery: String,
}

/// The failed-result envelope.
#[derive(Debug, Deserialize, Serialize)]
pub struct CommandErrorEnvelope {
    /// Always [`ERROR_SCHEMA`].
    pub schema: String,
    /// Dotted command path, such as `work.start`.
    pub command: String,
    /// Always `"error"`.
    pub status: String,
    /// The failure.
    pub error: CommandErrorBody,
}

impl CommandErrorEnvelope {
    /// Builds an envelope for one failure.
    #[must_use]
    pub fn new(command: impl Into<String>, error: &HarnessError) -> Self {
        let mut details = error.details();
        redact_github_tokens_in_json(&mut details);
        Self {
            schema: ERROR_SCHEMA.to_owned(),
            command: command.into(),
            status: "error".to_owned(),
            error: CommandErrorBody {
                code: error.code().as_string(),
                message: redact_github_tokens(&error.to_string()),
                details,
                recovery: error.code().recovery().to_owned(),
            },
        }
    }

    /// Renders the envelope as pretty JSON.
    ///
    /// # Errors
    ///
    /// Returns an error when serialization fails.
    pub fn render(&self) -> Result<String, HarnessError> {
        serde_json::to_string_pretty(self).map_err(HarnessError::from)
    }
}

fn redact_github_tokens_in_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(text) => *text = redact_github_tokens(text),
        serde_json::Value::Array(values) => {
            for value in values {
                redact_github_tokens_in_json(value);
            }
        }
        serde_json::Value::Object(fields) => {
            for value in fields.values_mut() {
                redact_github_tokens_in_json(value);
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

/// One command's result, before it is rendered in a caller-chosen format.
///
/// Commands build this rather than printing, so the same result can be emitted
/// as text or as the JSON envelope without duplicating the payload.
#[derive(Debug)]
pub struct CommandOutcome {
    command: String,
    project_id: Option<ProjectId>,
    operation_id: Option<OperationId>,
    data: serde_json::Value,
    warnings: Vec<String>,
    text: String,
}

impl CommandOutcome {
    /// Builds an outcome from a command path, its human rendering, and its
    /// machine payload.
    #[must_use]
    pub fn new(
        command: impl Into<String>,
        text: impl Into<String>,
        data: serde_json::Value,
    ) -> Self {
        Self {
            command: command.into(),
            project_id: None,
            operation_id: None,
            data,
            warnings: Vec::new(),
            text: text.into(),
        }
    }

    /// Attaches the project this command acted on.
    #[must_use]
    pub fn with_project(mut self, project_id: ProjectId) -> Self {
        self.project_id = Some(project_id);
        self
    }

    /// Attaches the journaled operation this command recorded.
    #[must_use]
    pub fn with_operation(mut self, operation_id: OperationId) -> Self {
        self.operation_id = Some(operation_id);
        self
    }

    /// Adds a non-fatal advisory.
    #[must_use]
    pub fn with_warning(mut self, warning: impl Into<String>) -> Self {
        self.warnings.push(warning.into());
        self
    }

    /// The advisories attached so far.
    #[must_use]
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Renders the outcome in the requested format.
    ///
    /// Text mode prints only the human body; warnings are the caller's to route
    /// to stderr, so they never contaminate a piped stdout. JSON mode carries
    /// them inside the envelope.
    ///
    /// # Errors
    ///
    /// Returns an error when serialization fails.
    pub fn render(&self, format: OutputFormat) -> Result<String, HarnessError> {
        match format {
            OutputFormat::Text => Ok(self.text.clone()),
            OutputFormat::Json => {
                let envelope = CommandResult {
                    schema: RESULT_SCHEMA.to_owned(),
                    command: self.command.clone(),
                    status: "success".to_owned(),
                    project_id: self.project_id.clone(),
                    operation_id: self.operation_id.clone(),
                    data: self.data.clone(),
                    warnings: self.warnings.clone(),
                };
                serde_json::to_string_pretty(&envelope).map_err(HarnessError::from)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> CommandOutcome {
        CommandOutcome::new("work.start", "started", json!({"branch": "card/F-123"}))
    }

    #[test]
    fn json_envelope_carries_every_documented_key() {
        let rendered = sample().render(OutputFormat::Json).unwrap();
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        for key in [
            "schema",
            "command",
            "status",
            "project_id",
            "operation_id",
            "data",
            "warnings",
        ] {
            assert!(value.get(key).is_some(), "envelope is missing `{key}`");
        }
        assert_eq!(value["schema"], RESULT_SCHEMA);
        assert_eq!(value["status"], "success");
        assert_eq!(value["command"], "work.start");
        assert_eq!(value["data"]["branch"], "card/F-123");
    }

    #[test]
    fn absent_identifiers_are_null_rather_than_omitted() {
        let rendered = sample().render(OutputFormat::Json).unwrap();
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert!(value["project_id"].is_null());
        assert!(value["operation_id"].is_null());
    }

    #[test]
    fn identifiers_appear_when_attached() {
        let outcome = sample()
            .with_project("example".parse().unwrap())
            .with_operation("OP-000123".parse().unwrap());
        let rendered = outcome.render(OutputFormat::Json).unwrap();
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["project_id"], "example");
        assert_eq!(value["operation_id"], "OP-000123");
    }

    #[test]
    fn result_envelope_round_trips_through_the_fixture() {
        let rendered = sample()
            .with_project("example".parse().unwrap())
            .render(OutputFormat::Json)
            .unwrap();
        let decoded: CommandResult = serde_json::from_str(&rendered).unwrap();
        let reencoded = serde_json::to_string_pretty(&decoded).unwrap();
        assert_eq!(rendered, reencoded);
    }

    #[test]
    fn text_mode_emits_only_the_human_body() {
        let rendered = sample()
            .with_warning("deprecated option")
            .render(OutputFormat::Text)
            .unwrap();
        assert_eq!(rendered, "started");
        assert!(!rendered.contains('{'), "text mode must not emit JSON");
    }

    #[test]
    fn warnings_ride_inside_the_json_envelope() {
        let rendered = sample()
            .with_warning("deprecated option")
            .render(OutputFormat::Json)
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["warnings"][0], "deprecated option");
    }

    #[test]
    fn error_envelope_carries_code_details_and_recovery() {
        let error = HarnessError::invalid_id("F-1", "too short");
        let rendered = CommandErrorEnvelope::new("card.activate", &error)
            .render()
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["schema"], ERROR_SCHEMA);
        assert_eq!(value["status"], "error");
        assert_eq!(value["command"], "card.activate");
        assert_eq!(value["error"]["code"], "CH-USAGE-INVALID-ID");
        assert_eq!(value["error"]["details"]["value"], "F-1");
        assert!(!value["error"]["recovery"].as_str().unwrap().is_empty());
    }

    #[test]
    fn error_envelope_round_trips() {
        let error = HarnessError::GitCommand("boom".into());
        let rendered = CommandErrorEnvelope::new("doctor", &error)
            .render()
            .unwrap();
        let decoded: CommandErrorEnvelope = serde_json::from_str(&rendered).unwrap();
        assert_eq!(serde_json::to_string_pretty(&decoded).unwrap(), rendered);
    }
}
