use std::{
    io::{self, Write as _},
    process::ExitCode,
};

use clap::Parser;

use change_harness::{
    cli::{Cli, exit::ExitCategory, output::CommandErrorEnvelope, output::OutputFormat},
    command_path,
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
    match CommandErrorEnvelope::new(attempted.join("."), &failure).render() {
        Ok(rendered) => println!("{rendered}"),
        Err(nested) => eprintln!("error: {nested}"),
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

fn main() -> ExitCode {
    let cli = match parse_or_report() {
        Ok(cli) => cli,
        Err(code) => return code,
    };
    let format = failure_format(&cli);
    let command = command_path(&cli);

    match execute(cli) {
        Ok(execution) => {
            println!("{}", execution.stdout);
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
        Err(error) => {
            match format {
                // The error envelope is a machine-readable result, not a
                // diagnostic, so it goes to stdout where a JSON consumer reads.
                OutputFormat::Json => match CommandErrorEnvelope::new(command, &error).render() {
                    Ok(rendered) => println!("{rendered}"),
                    Err(nested) => eprintln!("error: {nested}"),
                },
                OutputFormat::Text => eprintln!("error: {error}"),
            }
            ExitCode::from(error.category())
        }
    }
}
