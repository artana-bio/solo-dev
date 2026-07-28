pub mod cli;
pub mod commands;
pub mod error;
pub mod git;

use cli::{Cli, Command};
use commands::doctor::DoctorReport;
use error::HarnessError;

/// Executes one parsed command and returns its rendered output.
///
/// # Errors
///
/// Returns an error when the selected command cannot inspect or render its
/// result.
pub fn execute(cli: Cli) -> Result<String, HarnessError> {
    match cli.command {
        Command::Doctor(args) => DoctorReport::collect(&args)?.render(args.format),
    }
}
