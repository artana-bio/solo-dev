//! Commands for durable executable mutation evidence.

use crate::{
    cli::output::CommandOutcome,
    commands::CONTROL_ENV,
    control::repository::ControlRepository,
    domain::{
        clock::Clock,
        digest::Digest,
        mutation::{MUTATION_RECEIPT_SCHEMA, MutationReceipt},
    },
    error::{ErrorCode, HarnessError},
};
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum MutationCommand {
    Create(Box<CreateArgs>),
    Inspect(InspectArgs),
}

impl MutationCommand {
    #[must_use]
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Create(..) => "mutation.create",
            Self::Inspect(..) => "mutation.inspect",
        }
    }
}

#[derive(Debug, Args)]
pub struct CreateArgs {
    #[arg(long, env = CONTROL_ENV)]
    pub control: PathBuf,
    #[arg(long)]
    pub receipt_id: String,
    #[arg(long)]
    pub card_revision: String,
    #[arg(long)]
    pub candidate_sha: String,
    #[arg(long)]
    pub reviewer_actor_id: String,
    #[arg(long)]
    pub reviewer_session_id: Option<String>,
    #[arg(long)]
    pub mutation_digest: Digest,
    #[arg(long)]
    pub patch_digest: Digest,
    #[arg(long = "command", required = true)]
    pub command: Vec<String>,
    #[arg(long)]
    pub gate_oracle: String,
    #[arg(long)]
    pub expected_failure: String,
    #[arg(long)]
    pub observed_result: String,
    #[arg(long)]
    pub failed_at_oracle: bool,
    #[arg(long)]
    pub restoration_proof: String,
}

#[derive(Debug, Args)]
pub struct InspectArgs {
    #[arg(long, env = CONTROL_ENV)]
    pub control: PathBuf,
    #[arg(long)]
    pub receipt_id: String,
}

/// Executes a mutation-evidence command.
///
/// # Errors
///
/// Returns a policy, control-access, or serialization error.
pub fn execute(
    command: &MutationCommand,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    match command {
        MutationCommand::Create(args) => create(args, clock),
        MutationCommand::Inspect(args) => inspect(args),
    }
}

fn create(args: &CreateArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.control)?;
    let receipt = MutationReceipt {
        schema: MUTATION_RECEIPT_SCHEMA.to_owned(),
        receipt_id: args.receipt_id.clone(),
        card_revision: args.card_revision.clone(),
        candidate_sha: args.candidate_sha.clone(),
        reviewer_actor_id: args.reviewer_actor_id.clone(),
        reviewer_session_id: args.reviewer_session_id.clone(),
        mutation_digest: args.mutation_digest.clone(),
        patch_digest: args.patch_digest.clone(),
        command: args.command.clone(),
        gate_oracle: args.gate_oracle.clone(),
        expected_failure: args.expected_failure.clone(),
        observed_result: args.observed_result.clone(),
        failed_at_oracle: args.failed_at_oracle,
        restoration_proof: args.restoration_proof.clone(),
        created_at: clock.now(),
        exemption: None,
    };
    receipt.validate()?;
    let relative = MutationReceipt::relative_path(&receipt.receipt_id);
    if control.path(&relative).exists() {
        return Err(HarnessError::Control {
            reason: format!("mutation receipt {} already exists", receipt.receipt_id),
            code: ErrorCode::PreconditionBranchExists,
        });
    }
    control.write_atomic(
        &relative,
        &format!("{}\n", serde_json::to_string_pretty(&receipt)?),
    )?;
    let expected = control.head()?;
    control.commit(
        expected.as_deref(),
        &format!("mutation: record {}", receipt.receipt_id),
    )?;
    Ok(CommandOutcome::new(
        "mutation.create",
        format!("Recorded mutation receipt {}", receipt.receipt_id),
        serde_json::to_value(&receipt)?,
    )
    .with_project(control.project()?.project_id))
}

fn inspect(args: &InspectArgs) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.control)?;
    let receipt: MutationReceipt =
        serde_json::from_str(&control.read(&MutationReceipt::relative_path(&args.receipt_id))?)
            .map_err(|source| HarnessError::Control {
                reason: format!("mutation receipt is malformed: {source}"),
                code: ErrorCode::InternalControlCorrupt,
            })?;
    receipt.validate()?;
    Ok(CommandOutcome::new(
        "mutation.inspect",
        format!("Mutation receipt {} is valid", receipt.receipt_id),
        serde_json::to_value(receipt)?,
    )
    .with_project(control.project()?.project_id))
}
