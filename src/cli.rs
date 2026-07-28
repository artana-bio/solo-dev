use clap::{Parser, Subcommand};

use crate::commands::doctor::DoctorArgs;

#[derive(Debug, Parser)]
#[command(
    name = "change-harness",
    version,
    about = "Coordinate bounded changes across local Git worktrees"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Inspect the local Git environment without changing it.
    Doctor(DoctorArgs),
}
