use std::{fs, path::PathBuf};

use clap::{Args, ValueEnum};
use serde::Serialize;

use crate::{error::HarnessError, git::GitClient};

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum OutputFormat {
    #[default]
    Text,
    Json,
}

#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Workspace or repository path to inspect.
    #[arg(long, default_value = ".")]
    pub workspace: PathBuf,

    /// Report encoding.
    #[arg(long, value_enum, default_value_t)]
    pub format: OutputFormat,
}

#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub schema: &'static str,
    pub workspace: PathBuf,
    pub git_version: String,
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
            schema: "harness.doctor/v1",
            workspace,
            git_version: git.version,
            repository_root: git.repository_root,
        })
    }

    /// Renders the report in the requested output format.
    ///
    /// # Errors
    ///
    /// Returns an error when JSON serialization fails.
    pub fn render(&self, format: OutputFormat) -> Result<String, HarnessError> {
        match format {
            OutputFormat::Text => {
                let repository = self.repository_root.as_ref().map_or_else(
                    || "not detected".to_owned(),
                    |path| path.display().to_string(),
                );

                Ok(format!(
                    "Change Harness doctor\nworkspace: {}\ngit: {}\nrepository: {repository}",
                    self.workspace.display(),
                    self.git_version
                ))
            }
            OutputFormat::Json => serde_json::to_string_pretty(self).map_err(HarnessError::from),
        }
    }
}
