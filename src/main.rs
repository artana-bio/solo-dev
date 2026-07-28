use std::process::ExitCode;

use clap::Parser;

use change_harness::{cli::Cli, execute};

fn main() -> ExitCode {
    match execute(Cli::parse()) {
        Ok(report) => {
            println!("{report}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
