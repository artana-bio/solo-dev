//! Read-only environment diagnostic.

use std::{fs, path::PathBuf};

use clap::Args;
use serde::{Deserialize, Serialize};

use crate::{
    cli::output::{CommandOutcome, OutputFormat},
    error::HarnessError,
    git::{GitClient, inspect::RepositoryClass},
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
///
/// The first four fields are frozen by `WP-100` compatibility. `WP-130` adds
/// the remainder; adding keys is compatible, renaming or removing one is not.
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
    /// The lowest Git version the harness supports.
    pub minimum_git_version: String,
    /// True when the installed Git satisfies the minimum.
    pub meets_minimum_git_version: bool,
    /// True when the installed Git supports the worktree subcommands.
    pub supports_worktrees: bool,
    /// What the inspected path turned out to be.
    pub repository: RepositoryClass,
    /// A short summary of the inspected path's role.
    pub workspace_role: String,
}

/// Describes the inspected path in one word, for the human rendering.
fn workspace_role(repository: &RepositoryClass) -> &'static str {
    match repository {
        RepositoryClass::Repository { bare: true, .. } => "bare repository",
        RepositoryClass::Repository {
            detached_head: true,
            ..
        } => "detached worktree",
        RepositoryClass::Repository {
            linked_worktree: true,
            ..
        } => "linked worktree",
        RepositoryClass::Repository { .. } => "main worktree",
        RepositoryClass::NotRepository => "not a repository",
        RepositoryClass::GitError { .. } => "Git refused inspection",
    }
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
            minimum_git_version: git.minimum_version.to_string(),
            meets_minimum_git_version: git.meets_minimum_version,
            supports_worktrees: git.supports_worktrees,
            workspace_role: workspace_role(&git.repository).to_owned(),
            repository: git.repository,
        })
    }

    /// Renders the human-readable body.
    #[must_use]
    pub fn to_text(&self) -> String {
        let repository = self.repository_root.as_ref().map_or_else(
            || "not detected".to_owned(),
            |path| path.display().to_string(),
        );
        let version_note = if self.meets_minimum_git_version {
            format!("meets minimum {}", self.minimum_git_version)
        } else {
            format!("BELOW minimum {}", self.minimum_git_version)
        };
        let diagnostic = match &self.repository {
            RepositoryClass::GitError { diagnostic } => format!("\ngit error: {diagnostic}"),
            RepositoryClass::Repository { .. } | RepositoryClass::NotRepository => String::new(),
        };
        format!(
            "Change Harness doctor\nworkspace: {}\ngit: {} ({version_note})\nworktree support: {}\nrole: {}\nrepository: {repository}{diagnostic}",
            self.workspace.display(),
            self.git_version,
            if self.supports_worktrees { "yes" } else { "no" },
            self.workspace_role,
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
        let repository = RepositoryClass::Repository {
            top_level: Some(PathBuf::from("/repo")),
            git_dir: PathBuf::from("/repo/.git"),
            common_dir: PathBuf::from("/repo/.git"),
            bare: false,
            linked_worktree: false,
            detached_head: false,
        };
        DoctorReport {
            schema: DOCTOR_SCHEMA.to_owned(),
            workspace: PathBuf::from("/repo"),
            git_version: "git version 2.50.1".to_owned(),
            repository_root: Some(PathBuf::from("/repo")),
            minimum_git_version: "2.50.0".to_owned(),
            meets_minimum_git_version: true,
            supports_worktrees: true,
            workspace_role: workspace_role(&repository).to_owned(),
            repository,
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
            repository: RepositoryClass::NotRepository,
            workspace_role: workspace_role(&RepositoryClass::NotRepository).to_owned(),
            ..sample()
        };
        assert!(report.to_text().contains("repository: not detected"));
        assert!(report.to_text().contains("role: not a repository"));
    }

    #[test]
    fn a_git_refusal_is_surfaced_and_never_reads_as_absence() {
        let repository = RepositoryClass::GitError {
            diagnostic: "detected dubious ownership".to_owned(),
        };
        let report = DoctorReport {
            repository_root: None,
            workspace_role: workspace_role(&repository).to_owned(),
            repository,
            ..sample()
        };
        let text = report.to_text();
        assert!(text.contains("role: Git refused inspection"));
        assert!(text.contains("git error: detected dubious ownership"));
    }

    #[test]
    fn workspace_role_distinguishes_every_repository_shape() {
        let base = |bare, linked, detached| RepositoryClass::Repository {
            top_level: Some(PathBuf::from("/repo")),
            git_dir: PathBuf::from("/repo/.git"),
            common_dir: PathBuf::from("/repo/.git"),
            bare,
            linked_worktree: linked,
            detached_head: detached,
        };
        assert_eq!(workspace_role(&base(false, false, false)), "main worktree");
        assert_eq!(workspace_role(&base(false, true, false)), "linked worktree");
        assert_eq!(
            workspace_role(&base(false, false, true)),
            "detached worktree"
        );
        assert_eq!(workspace_role(&base(true, false, false)), "bare repository");
    }

    #[test]
    fn text_reports_minimum_version_compliance_and_worktree_support() {
        let text = sample().to_text();
        assert!(text.contains("meets minimum 2.50.0"));
        assert!(text.contains("worktree support: yes"));

        let below = DoctorReport {
            meets_minimum_git_version: false,
            ..sample()
        };
        assert!(below.to_text().contains("BELOW minimum 2.50.0"));
    }

    #[test]
    fn report_round_trips_through_json() {
        let report = sample();
        let encoded = report.to_legacy_json().unwrap();
        let decoded: DoctorReport = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.to_legacy_json().unwrap(), encoded);
    }
}
