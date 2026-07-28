//! Command-line surface.

pub mod exit;
pub mod output;

use clap::{Parser, Subcommand};

use crate::{
    commands::{cycle::CycleCommand, doctor::DoctorArgs, project::ProjectCommand},
    error::HarnessError,
};

use output::OutputFormat;

/// Coordinates bounded changes across local Git worktrees.
#[derive(Debug, Parser)]
#[command(
    name = "change-harness",
    version,
    about = "Coordinate bounded changes across local Git worktrees"
)]
pub struct Cli {
    /// Result encoding. This is the canonical output option.
    #[arg(long, value_enum, global = true)]
    pub output: Option<OutputFormat>,

    #[command(subcommand)]
    pub command: Command,
}

/// Every implemented command.
///
/// Section 12.3 lists the full intended surface. A command appears here only
/// when its owning work package implements it, so help output never advertises
/// a placeholder.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Inspect the local Git environment without changing it.
    Doctor(DoctorArgs),

    /// Inspect and validate project configuration.
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },

    /// Declare and manage bounded integration periods.
    Cycle {
        #[command(subcommand)]
        command: CycleCommand,
    },
}

/// How the caller asked for output, and whether they used the deprecated
/// per-command option to do it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedOutput {
    /// The format to render in.
    pub format: OutputFormat,
    /// True when the caller used `--format` rather than `--output`.
    pub used_legacy_option: bool,
}

/// Text emitted when a caller still uses the deprecated per-command option.
pub const LEGACY_FORMAT_WARNING: &str =
    "`--format` is deprecated and will be removed in a future release; use `--output` instead";

/// Resolves the effective output format from the global and legacy options.
///
/// Section 12.1 makes `--output` canonical while keeping `doctor --format`
/// accepted through the Single-repository MVP. Supplying both is a usage error
/// rather than a silent precedence rule, because a silent winner would make one
/// of the two options quietly meaningless.
///
/// # Errors
///
/// Returns an error when both options are supplied.
pub fn resolve_output(
    global: Option<OutputFormat>,
    legacy: Option<OutputFormat>,
) -> Result<ResolvedOutput, HarnessError> {
    match (global, legacy) {
        (Some(_), Some(_)) => Err(HarnessError::ConflictingOptions(
            "`--output` and `--format` cannot be combined; use `--output`".to_owned(),
        )),
        (Some(format), None) => Ok(ResolvedOutput {
            format,
            used_legacy_option: false,
        }),
        (None, Some(format)) => Ok(ResolvedOutput {
            format,
            used_legacy_option: true,
        }),
        (None, None) => Ok(ResolvedOutput {
            format: OutputFormat::Text,
            used_legacy_option: false,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_text_when_neither_option_is_given() {
        let resolved = resolve_output(None, None).unwrap();
        assert_eq!(resolved.format, OutputFormat::Text);
        assert!(!resolved.used_legacy_option);
    }

    #[test]
    fn global_option_is_not_flagged_as_legacy() {
        let resolved = resolve_output(Some(OutputFormat::Json), None).unwrap();
        assert_eq!(resolved.format, OutputFormat::Json);
        assert!(!resolved.used_legacy_option);
    }

    #[test]
    fn legacy_option_is_accepted_and_flagged() {
        let resolved = resolve_output(None, Some(OutputFormat::Json)).unwrap();
        assert_eq!(resolved.format, OutputFormat::Json);
        assert!(resolved.used_legacy_option);
    }

    #[test]
    fn combining_both_options_is_a_usage_error() {
        let error =
            resolve_output(Some(OutputFormat::Json), Some(OutputFormat::Text)).expect_err("reject");
        assert_eq!(error.category(), exit::ExitCategory::Usage);
        assert_eq!(error.code().as_string(), "CH-USAGE-CONFLICTING-OPTIONS");
    }
}
