//! Project configuration commands.

use std::{fs, path::PathBuf};

use clap::{Args, Subcommand};

use crate::{
    cli::output::CommandOutcome,
    config::{ProjectConfig, validate::validate},
    error::{ErrorCode, HarnessError},
};

/// Subcommands under `project`.
#[derive(Debug, Subcommand)]
pub enum ProjectCommand {
    /// Validate a project configuration without changing anything.
    Validate(ValidateArgs),
}

/// Arguments accepted by `project validate`.
#[derive(Debug, Args)]
pub struct ValidateArgs {
    /// Path to the project document to validate.
    #[arg(long)]
    pub config: PathBuf,
}

/// Executes a `project` subcommand.
///
/// # Errors
///
/// Returns a configuration error naming the exact offending field, or an
/// external-tool error when Git cannot be executed.
pub fn execute(command: &ProjectCommand) -> Result<CommandOutcome, HarnessError> {
    match command {
        ProjectCommand::Validate(args) => run_validate(args),
    }
}

fn run_validate(args: &ValidateArgs) -> Result<CommandOutcome, HarnessError> {
    let raw = fs::read_to_string(&args.config).map_err(|source| HarnessError::Config {
        field: "<file>".to_owned(),
        reason: format!("cannot read {}: {source}", args.config.display()),
        code: ErrorCode::ConfigMalformed,
    })?;

    let config = ProjectConfig::from_json(&raw)?;
    let report = validate(&config)?;

    let symlinked: Vec<&str> = report
        .paths
        .iter()
        .filter(|entry| entry.via_symlink)
        .map(|entry| entry.field.as_str())
        .collect();

    let mut text = format!(
        "Project {} is valid\ngit: {} (minimum {})\nhost: {}\nprotected branch: {} at {}",
        report.project_id,
        report.git_version,
        report.minimum_git_version,
        report.host_os,
        report.protected_branch,
        report.protected_branch_sha,
    );
    for entry in &report.paths {
        use std::fmt::Write as _;
        let _ = write!(text, "\n{}: {}", entry.field, entry.resolved.display());
    }

    let mut outcome = CommandOutcome::new("project.validate", text, serde_json::to_value(&report)?)
        .with_project(config.project_id.clone());

    if !symlinked.is_empty() {
        // Recorded rather than rejected: a symlinked path is legitimate, but a
        // later change in the resolved target must be visible, so the operator
        // is told which paths carry that risk.
        outcome = outcome.with_warning(format!(
            "these paths resolve through symlinks and must be revalidated if their targets change: {}",
            symlinked.join(", ")
        ));
    }

    Ok(outcome)
}
