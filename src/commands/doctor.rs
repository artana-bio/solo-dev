//! Read-only environment diagnostic.

use std::{fs, path::PathBuf};

use clap::Args;
use serde::{Deserialize, Serialize};

use crate::{
    cli::output::{CommandOutcome, OutputFormat},
    error::HarnessError,
    git::GitClient,
};

/// Schema identifier for the doctor payload.
pub const DOCTOR_SCHEMA: &str = "harness.doctor/v1";

/// Arguments accepted by `doctor`.
#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Workspace or repository path to inspect.
    #[arg(long, default_value = ".")]
    pub workspace: PathBuf,

    /// Deprecated alias for the global `--output` option.
    #[arg(long, value_enum)]
    pub format: Option<OutputFormat>,
}

/// The diagnostic payload.
#[derive(Debug, Deserialize, Serialize)]
pub struct DoctorReport {
    /// Always [`DOCTOR_SCHEMA`].
    pub schema: String,
    /// The canonicalized workspace path.
    pub workspace: PathBuf,
    /// The installed Git version string.
    pub git_version: String,
    /// The repository containing the workspace, when one was detected.
    pub repository_root: Option<PathBuf>,
}

impl DoctorReport {
    /// Collects a read-only report for the requested workspace.
    ///
    /// # Errors
    ///
    /// Returns an error when the workspace is missing or inaccessible, or when
    /// Git cannot be executed.
    pub fn collect(args: &DoctorArgs) -> Result<Self, HarnessError> {
        if !args.workspace.exists() {
            return Err(HarnessError::WorkspaceNotFound(args.workspace.clone()));
        }

        let workspace =
            fs::canonicalize(&args.workspace).map_err(|source| HarnessError::WorkspaceAccess {
                path: args.workspace.clone(),
                source,
            })?;
        let git = GitClient::probe(&workspace)?;

        Ok(Self {
            schema: DOCTOR_SCHEMA.to_owned(),
            workspace,
            git_version: git.version,
            repository_root: git.repository_root,
        })
    }

    /// Renders the human-readable body.
    #[must_use]
    pub fn to_text(&self) -> String {
        let repository = self.repository_root.as_ref().map_or_else(
            || "not detected".to_owned(),
            |path| path.display().to_string(),
        );
        format!(
            "Change Harness doctor\nworkspace: {}\ngit: {}\nrepository: {repository}",
            self.workspace.display(),
            self.git_version
        )
    }

    /// Renders the pre-envelope payload emitted by the deprecated `--format`
    /// option.
    ///
    /// `WP-100` requires existing `doctor` behavior to remain compatible, so
    /// this shape is frozen: top-level `schema`, `workspace`, `git_version`,
    /// and `repository_root`. The global `--output` option emits the Section
    /// 12.4 envelope instead.
    ///
    /// # Errors
    ///
    /// Returns an error when serialization fails.
    pub fn to_legacy_json(&self) -> Result<String, HarnessError> {
        serde_json::to_string_pretty(self).map_err(HarnessError::from)
    }

    /// Builds the envelope-ready outcome.
    ///
    /// # Errors
    ///
    /// Returns an error when the payload cannot be serialized.
    pub fn to_outcome(&self) -> Result<CommandOutcome, HarnessError> {
        Ok(CommandOutcome::new(
            "doctor",
            self.to_text(),
            serde_json::to_value(self)?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> DoctorReport {
        DoctorReport {
            schema: DOCTOR_SCHEMA.to_owned(),
            workspace: PathBuf::from("/repo"),
            git_version: "git version 2.50.1".to_owned(),
            repository_root: Some(PathBuf::from("/repo")),
        }
    }

    #[test]
    fn legacy_payload_keeps_its_frozen_top_level_shape() {
        let value: serde_json::Value =
            serde_json::from_str(&sample().to_legacy_json().unwrap()).unwrap();
        assert_eq!(value["schema"], DOCTOR_SCHEMA);
        assert_eq!(value["repository_root"], "/repo");
        assert_eq!(value["git_version"], "git version 2.50.1");
    }

    #[test]
    fn envelope_payload_moves_the_report_under_data() {
        let rendered = sample()
            .to_outcome()
            .unwrap()
            .render(OutputFormat::Json)
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["schema"], "harness.command-result/v1");
        assert_eq!(value["command"], "doctor");
        assert_eq!(value["data"]["schema"], DOCTOR_SCHEMA);
        assert_eq!(value["data"]["repository_root"], "/repo");
    }

    #[test]
    fn text_body_is_identical_in_both_paths() {
        let report = sample();
        let outcome_text = report
            .to_outcome()
            .unwrap()
            .render(OutputFormat::Text)
            .unwrap();
        assert_eq!(outcome_text, report.to_text());
    }

    #[test]
    fn missing_repository_renders_as_not_detected() {
        let report = DoctorReport {
            repository_root: None,
            ..sample()
        };
        assert!(report.to_text().contains("repository: not detected"));
    }

    #[test]
    fn report_round_trips_through_json() {
        let report = sample();
        let encoded = report.to_legacy_json().unwrap();
        let decoded: DoctorReport = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.to_legacy_json().unwrap(), encoded);
    }
}
