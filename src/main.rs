use std::{
    io::{self, IsTerminal as _, Write as _},
    process::ExitCode,
};

use clap::Parser;

use change_harness::{
    cli::{
        Cli, Command,
        exit::ExitCategory,
        output::{CommandErrorEnvelope, OutputFormat, render_text_error},
        resolve_output,
    },
    command_path,
    commands::{project::ProjectCommand, project_snapshot},
    error::{ErrorCode, HarnessError},
    execute, failure_format,
};

/// Renders a clap parse failure through the stable envelope when JSON was asked
/// for, and lets clap print its own help when it was not.
///
/// `Cli::parse` exits inside clap, so a caller that asked for `--output json`
/// received clap's usage text on stderr and nothing at all on stdout. An agent
/// driving this CLI — the interface the envelope exists for — cannot parse
/// that, and a malformed invocation is the failure it is most likely to hit.
///
/// Text mode keeps clap's output untouched. Its usage block is far better for a
/// person than a one-line envelope, and text is the default.
fn parse_or_report() -> Result<Cli, ExitCode> {
    let error = match Cli::try_parse() {
        Ok(cli) => return Ok(cli),
        Err(error) => error,
    };

    // `--help` and `--version` arrive here too: clap reports them as errors
    // that write to stdout and exit zero. They are output, not failure.
    if !error.use_stderr() {
        error.print().ok();
        return Err(ExitCode::from(ExitCategory::Success));
    }

    let raw: Vec<String> = std::env::args().skip(1).collect();
    if !asked_for_json(&raw) {
        error.print().ok();
        return Err(ExitCode::from(ExitCategory::Usage));
    }

    // The command was never parsed, so the path is recovered from what was
    // typed: the leading non-flag tokens, which is exactly what was attempted.
    let attempted: Vec<&str> = raw
        .iter()
        .take_while(|argument| !argument.starts_with('-'))
        .take(2)
        .map(String::as_str)
        .collect();
    let failure = HarnessError::Control {
        reason: error.render().to_string().trim().to_owned(),
        code: ErrorCode::UsageInvalidArguments,
    };
    // Nothing has executed yet — parsing itself is what failed — so `Usage`
    // below is the whole truth about this invocation. `println!`/`eprintln!`
    // panic on a broken pipe (exit 101), which would misreport a malformed
    // invocation as a harness crash to a caller that stopped reading.
    // Written through a locked handle with the write result discarded,
    // following the pattern #16 established for output after a command
    // succeeds (commit 661bc60): the exit code decided below must not
    // depend on whether anyone was still reading.
    match CommandErrorEnvelope::new(attempted.join("."), &failure).render() {
        Ok(rendered) => {
            let mut stdout = io::stdout().lock();
            let _ = writeln!(stdout, "{rendered}");
        }
        Err(nested) => {
            let mut stderr = io::stderr().lock();
            let _ = writeln!(stderr, "error: {nested}");
        }
    }
    Err(ExitCode::from(ExitCategory::Usage))
}

/// Whether the raw arguments requested JSON output.
///
/// Read from the raw arguments because parsing is what failed.
fn asked_for_json(raw: &[String]) -> bool {
    let mut arguments = raw.iter();
    while let Some(argument) = arguments.next() {
        let value = match argument.as_str() {
            "--output" | "--format" => arguments.next().map(String::as_str),
            other => other
                .strip_prefix("--output=")
                .or_else(|| other.strip_prefix("--format=")),
        };
        if value == Some("json") {
            return true;
        }
    }
    false
}

/// Runs the streaming form of `project snapshot`.
///
/// Streaming is kept at the process boundary because ordinary commands return
/// one rendered [`change_harness::Execution`]. The collector and renderer
/// remain in the command adapter, so every frame follows the same path as a
/// one-shot snapshot.
fn run_snapshot_watch(cli: Cli) -> ExitCode {
    let format = failure_format(&cli);
    let command = command_path(&cli);
    let output = cli.output;
    let Command::Project {
        command: ProjectCommand::Snapshot(args),
    } = cli.command
    else {
        unreachable!("watch dispatch is only selected for project snapshot");
    };
    let resolved = match resolve_output(output, None) {
        Ok(resolved) => resolved,
        Err(error) => return report_error(format, command, &error),
    };
    let stdout_is_terminal = io::stdout().is_terminal();
    let mut stdout = io::stdout().lock();
    match project_snapshot::run_watch(
        &args,
        resolved.format,
        &change_harness::domain::clock::SystemClock,
        &mut stdout,
        stdout_is_terminal,
    ) {
        Ok(_) => ExitCode::from(ExitCategory::Success),
        Err(error) => report_error(format, command, &error),
    }
}

/// Emits a command error without allowing a closed output stream to become a
/// panic or a misleading internal-error exit.
fn report_error(format: OutputFormat, command: &str, error: &HarnessError) -> ExitCode {
    match format {
        // The error envelope is a machine-readable result, not a diagnostic,
        // so it goes to stdout where a JSON consumer reads it.
        OutputFormat::Json => match CommandErrorEnvelope::new(command, error).render() {
            Ok(rendered) => {
                let mut stdout = io::stdout().lock();
                let _ = writeln!(stdout, "{rendered}");
            }
            Err(nested) => {
                let mut stderr = io::stderr().lock();
                let _ = writeln!(stderr, "error: {nested}");
            }
        },
        OutputFormat::Text => {
            let mut stderr = io::stderr().lock();
            let _ = writeln!(stderr, "error: {}", render_text_error(error));
        }
    }
    ExitCode::from(error.category())
}

fn main() -> ExitCode {
    let cli = match parse_or_report() {
        Ok(cli) => cli,
        Err(code) => return code,
    };
    let is_snapshot_watch = matches!(
        &cli.command,
        Command::Project {
            command: ProjectCommand::Snapshot(args),
        } if args.watch
    );
    if is_snapshot_watch {
        return run_snapshot_watch(cli);
    }
    let format = failure_format(&cli);
    let command = command_path(&cli);

    match execute(cli) {
        Ok(execution) => {
            // The command's state change is already committed when its result
            // is rendered. A downstream consumer may have closed stdout; do
            // not turn that completed operation into a panic (exit 101) that
            // invites an unsafe retry. The result is simply unavailable to
            // that consumer, while the authoritative control state remains
            // queryable.
            let mut stdout = io::stdout().lock();
            let _ = writeln!(stdout, "{}", execution.stdout);
            // Advisories go to stderr so a piped stdout carries only the
            // result, in text and JSON alike. In JSON mode they are also
            // inside the envelope.
            //
            // Written through a locked handle with the result discarded,
            // because `eprintln!` panics when the write fails. By this point
            // the command has already succeeded and its state change is
            // committed, so a reader that closed the pipe — `2>&-`, or a
            // `head` that has seen enough — would turn an advisory into exit
            // 101 over work that actually landed. An advisory that can change
            // the exit status is not an advisory. Dropping the line is the
            // right outcome: in JSON mode the envelope carries the warnings
            // too, which is where anything programmatic reads them.
            let mut stderr = io::stderr().lock();
            for warning in &execution.warnings {
                let _ = writeln!(stderr, "warning: {warning}");
            }
            ExitCode::from(ExitCategory::Success)
        }
        Err(error) => report_error(format, command, &error),
    }
}
