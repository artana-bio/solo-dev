//! Change Harness: a project-neutral change-control engine for local Git
//! repositories.

pub mod cli;
pub mod commands;
pub mod domain;
pub mod error;
pub mod git;

use cli::{
    Cli, Command, LEGACY_FORMAT_WARNING,
    output::{CommandOutcome, OutputFormat},
    resolve_output,
};
use commands::doctor::DoctorReport;
use error::HarnessError;

/// One command's rendered result, ready for the process to emit.
#[derive(Debug)]
pub struct Execution {
    /// Text destined for stdout.
    pub stdout: String,
    /// Advisories destined for stderr.
    pub warnings: Vec<String>,
    /// The format the caller asked for.
    pub format: OutputFormat,
}

/// Determines the output format a failing invocation should be rendered in.
///
/// The process needs this before it knows whether the command succeeded, so it
/// is resolved separately and never fails. A malformed option combination is
/// reported by [`execute`] and rendered under this best-effort format.
#[must_use]
pub fn failure_format(cli: &Cli) -> OutputFormat {
    let legacy = match &cli.command {
        Command::Doctor(args) => args.format,
    };
    cli.output.or(legacy).unwrap_or_default()
}

/// The dotted command path of a parsed invocation, for the error envelope.
#[must_use]
pub fn command_path(cli: &Cli) -> &'static str {
    match &cli.command {
        Command::Doctor(_) => "doctor",
    }
}

/// Executes one parsed command.
///
/// # Errors
///
/// Returns an error when the command's preconditions fail, when a required
/// external tool fails, or when the result cannot be rendered.
pub fn execute(cli: Cli) -> Result<Execution, HarnessError> {
    match cli.command {
        Command::Doctor(args) => {
            let resolved = resolve_output(cli.output, args.format)?;
            let report = DoctorReport::collect(&args)?;

            // The deprecated option keeps the pre-envelope payload so existing
            // callers are not broken; `--output` emits the Section 12.4
            // envelope.
            let stdout = match (resolved.format, resolved.used_legacy_option) {
                (OutputFormat::Json, true) => report.to_legacy_json()?,
                (format, _) => report.to_outcome()?.render(format)?,
            };

            let mut outcome = CommandOutcome::new("doctor", String::new(), serde_json::Value::Null);
            if resolved.used_legacy_option {
                outcome = outcome.with_warning(LEGACY_FORMAT_WARNING);
            }

            Ok(Execution {
                stdout,
                warnings: outcome.warnings().to_vec(),
                format: resolved.format,
            })
        }
    }
}
