use std::process::ExitCode;

use clap::Parser;

use change_harness::{
    cli::{Cli, exit::ExitCategory, output::CommandErrorEnvelope, output::OutputFormat},
    command_path, execute, failure_format,
};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let format = failure_format(&cli);
    let command = command_path(&cli);

    match execute(cli) {
        Ok(execution) => {
            println!("{}", execution.stdout);
            // Advisories go to stderr so a piped stdout carries only the
            // result, in text and JSON alike. In JSON mode they are also
            // inside the envelope.
            for warning in &execution.warnings {
                eprintln!("warning: {warning}");
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
