//! Change Harness: a project-neutral change-control engine for local Git
//! repositories.

pub mod cli;
pub mod commands;
pub mod config;
pub mod control;
pub mod domain;
pub mod error;
pub mod git;
pub mod policy;
pub mod runner;

use cli::{
    Cli, Command, LEGACY_FORMAT_WARNING,
    output::{CommandOutcome, OutputFormat},
    resolve_output,
    tty::SystemEnvironment,
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
        Command::Demo(_)
        | Command::Project { .. }
        | Command::Cycle { .. }
        | Command::Card { .. }
        | Command::Work { .. }
        | Command::Gate { .. }
        | Command::Handoff { .. }
        | Command::Review { .. }
        | Command::Disposition { .. }
        | Command::Integration { .. }
        | Command::Acceptance { .. }
        | Command::Archive { .. }
        | Command::Backup { .. }
        | Command::Audit { .. } => None,
    };
    cli.output.or(legacy).unwrap_or_default()
}

/// The dotted command path of a parsed invocation, for the error envelope.
#[must_use]
pub fn command_path(cli: &Cli) -> &'static str {
    match &cli.command {
        Command::Doctor(_) => "doctor",
        Command::Demo(_) => "demo",
        Command::Project { command } => command.path(),
        Command::Cycle { command } => command.path(),
        Command::Card { command } => command.path(),
        Command::Work { command } => command.path(),
        Command::Gate { command } => command.path(),
        Command::Handoff { command } => command.path(),
        Command::Review { command } => command.path(),
        Command::Disposition { command } => command.path(),
        Command::Integration { command } => command.path(),
        Command::Acceptance { command } => command.path(),
        Command::Archive { command } => command.path(),
        Command::Backup { command } => command.path(),
        Command::Audit { command } => command.path(),
    }
}

/// Runs one subcommand and renders its outcome.
///
/// Every subcommand does the same three things — resolve the output format,
/// execute against the system clock, render — so they say it once here. Only
/// `doctor` differs, because `WP-100` froze its pre-envelope payload.
fn dispatch<F>(output: Option<OutputFormat>, run: F) -> Result<Execution, HarnessError>
where
    F: FnOnce(&dyn domain::clock::Clock) -> Result<CommandOutcome, HarnessError>,
{
    let resolved = resolve_output(output, None)?;
    let outcome = run(&domain::clock::SystemClock)?;
    Ok(Execution {
        stdout: outcome.render(resolved.format)?,
        warnings: outcome.warnings().to_vec(),
        format: resolved.format,
    })
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
        Command::Demo(args) => {
            // Not run through `dispatch`: the animation must know the
            // resolved format before it decides whether to play at all (JSON
            // output skips it), and `dispatch`'s closure is never given that.
            let resolved = resolve_output(cli.output, None)?;
            let outcome = commands::demo::execute(&args, resolved.format, &SystemEnvironment)?;
            Ok(Execution {
                stdout: outcome.render(resolved.format)?,
                warnings: outcome.warnings().to_vec(),
                format: resolved.format,
            })
        }
        Command::Project { command } => dispatch(cli.output, |clock| {
            commands::project::execute(&command, clock)
        }),
        // `cycle replay` bypasses `dispatch` for the same reason `demo`
        // does: whether the animation plays depends on the resolved output
        // format, which `dispatch`'s closure is never given.
        Command::Cycle {
            command: commands::cycle::CycleCommand::Replay(args),
        } => {
            let resolved = resolve_output(cli.output, None)?;
            let outcome =
                commands::cycle::execute_replay(&args, resolved.format, &SystemEnvironment)?;
            Ok(Execution {
                stdout: outcome.render(resolved.format)?,
                warnings: outcome.warnings().to_vec(),
                format: resolved.format,
            })
        }
        Command::Cycle { command } => dispatch(cli.output, |clock| {
            commands::cycle::execute(&command, clock)
        }),
        Command::Card { command } => {
            dispatch(cli.output, |clock| commands::card::execute(&command, clock))
        }
        Command::Work { command } => {
            dispatch(cli.output, |clock| commands::work::execute(&command, clock))
        }
        Command::Gate { command } => {
            dispatch(cli.output, |clock| commands::gate::execute(&command, clock))
        }
        Command::Handoff { command } => dispatch(cli.output, |clock| {
            commands::handoff::execute(&command, clock)
        }),
        Command::Review { command } => dispatch(cli.output, |clock| {
            commands::review::execute(&command, clock)
        }),
        Command::Disposition { command } => dispatch(cli.output, |clock| {
            commands::disposition::execute(&command, clock)
        }),
        Command::Integration { command } => dispatch(cli.output, |clock| {
            commands::integration::execute(&command, clock)
        }),
        Command::Acceptance { command } => dispatch(cli.output, |clock| {
            commands::acceptance::execute(&command, clock)
        }),
        Command::Archive { command } => dispatch(cli.output, |clock| {
            commands::archive::execute(&command, clock)
        }),
        Command::Backup { command } => dispatch(cli.output, |clock| {
            commands::backup::execute(&command, clock)
        }),
        Command::Audit { command } => dispatch(cli.output, |clock| {
            commands::audit::execute(&command, clock)
        }),
    }
}
