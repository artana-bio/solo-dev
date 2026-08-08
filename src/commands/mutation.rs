//! Commands for durable executable mutation evidence.

use crate::{
    cli::output::CommandOutcome,
    commands::CONTROL_ENV,
    commands::transaction::with_transaction,
    control::repository::ControlRepository,
    domain::{
        clock::Clock,
        digest::Digest,
        mutation::{MUTATION_RECEIPT_SCHEMA, MutationReceipt},
    },
    error::{ErrorCode, HarnessError},
    git::{
        command::{GitScope, run},
        inspect,
    },
    runner,
};
use clap::{Args, Subcommand};
use std::{
    fs,
    path::{Path, PathBuf},
};

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
    pub reviewer_principal_id: Option<String>,
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

#[allow(clippy::too_many_lines)]
fn create(args: &CreateArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let receipt = MutationReceipt {
        schema: MUTATION_RECEIPT_SCHEMA.to_owned(),
        receipt_id: args.receipt_id.clone(),
        card_revision: args.card_revision.clone(),
        candidate_sha: args.candidate_sha.clone(),
        reviewer_actor_id: args.reviewer_actor_id.clone(),
        reviewer_principal_id: args.reviewer_principal_id.clone(),
        reviewer_session_id: args.reviewer_session_id.clone(),
        mutation_digest: args.mutation_digest.clone(),
        patch_digest: args.patch_digest.clone(),
        command: args.command.clone(),
        gate_oracle: args.gate_oracle.clone(),
        expected_failure: args.expected_failure.clone(),
        observed_result: args.observed_result.clone(),
        failed_at_oracle: args.failed_at_oracle,
        restoration_proof: args.restoration_proof.clone(),
        restoration_sha: None,
        created_at: clock.now(),
        exemption: None,
    };
    receipt.validate()?;
    with_transaction(
        &args.control,
        "mutation.create",
        clock,
        |control, _events, expected, steps| {
            steps.at("control-write")?;
            let config = control.project()?;
            inspect::resolve_commit(
                &GitScope::work_tree(&config.repository),
                &receipt.candidate_sha,
            )
            .map_err(|_| HarnessError::ControlWithRecovery {
                reason: format!(
                    "candidate SHA {} does not exist in the configured repository",
                    receipt.candidate_sha
                ),
                code: ErrorCode::GateEvidenceStale,
                recovery: "Use the exact committed candidate SHA reviewed by the reviewer.",
            })?;
            let relative = MutationReceipt::relative_path(&receipt.receipt_id);
            if control.path(&relative).exists() {
                return Err(HarnessError::Control {
                    reason: format!("mutation receipt {} already exists", receipt.receipt_id),
                    code: ErrorCode::PreconditionBranchExists,
                });
            }
            let scratch = tempfile::tempdir().map_err(|source| HarnessError::ControlIo {
                path: std::env::temp_dir(),
                source,
            })?;
            let worktree = scratch.path().join("candidate");
            let candidate_sha = receipt.candidate_sha.clone();
            steps.outside_control("mutation-worktree-add")?;
            let attempt = (|| {
                run(
                    &GitScope::work_tree(&config.repository),
                    [
                        "worktree",
                        "add",
                        "--detach",
                        worktree.to_string_lossy().as_ref(),
                        receipt.candidate_sha.as_str(),
                    ],
                )?
                .require_success()?;
                steps.at("mutation-worktree-added")?;
                let oracle = crate::commands::gate::load_gate(control, &receipt.gate_oracle)?;
                let baseline_oracle = runner::run_attempt(
                    &oracle,
                    &worktree,
                    scratch.path().join("logs-before").as_path(),
                    1,
                    clock,
                )?;
                if !baseline_oracle.passed() {
                    return Err(HarnessError::Control {
                        reason: "mutation oracle must pass on the exact candidate before mutation"
                            .to_owned(),
                        code: ErrorCode::PolicyIncompleteReview,
                    });
                }
                let mut mutation = std::process::Command::new(&receipt.command[0]);
                mutation.args(&receipt.command[1..]).current_dir(&worktree);
                let mutation_output =
                    mutation.output().map_err(|source| HarnessError::Control {
                        reason: format!("mutation command could not start: {source}"),
                        code: ErrorCode::PolicyIncompleteReview,
                    })?;
                if !mutation_output.status.success() {
                    return Err(HarnessError::Control {
                        reason: format!(
                            "mutation command failed with exit {:?}",
                            mutation_output.status.code()
                        ),
                        code: ErrorCode::PolicyIncompleteReview,
                    });
                }
                let oracle_outcome = runner::run_attempt(
                    &oracle,
                    &worktree,
                    scratch.path().join("logs").as_path(),
                    1,
                    clock,
                )?;
                if receipt.failed_at_oracle == oracle_outcome.passed() {
                    return Err(HarnessError::Control {
                        reason: format!(
                            "mutation oracle result contradicted failed_at_oracle: mutation exit {:?}, oracle passed {}",
                            mutation_output.status.code(),
                            oracle_outcome.passed()
                        ),
                        code: ErrorCode::PolicyIncompleteReview,
                    });
                }
                let mut observed_patch =
                    run(&GitScope::work_tree(&worktree), ["diff", "--binary"])?.stdout;
                if observed_patch.trim().is_empty() {
                    let status =
                        run(&GitScope::work_tree(&worktree), ["status", "--porcelain"])?.stdout;
                    if !status.trim().is_empty() {
                        observed_patch = status;
                    }
                }
                if observed_patch.trim().is_empty() {
                    return Err(HarnessError::Control {
                        reason: "mutation command produced no tracked tree change".to_owned(),
                        code: ErrorCode::PolicyIncompleteReview,
                    });
                }
                let mut receipt = receipt;
                receipt.mutation_digest = Digest::of_bytes(observed_patch.as_bytes());
                receipt.patch_digest = receipt.mutation_digest.clone();
                receipt.observed_result = format!(
                    "mutation_exit={:?}; oracle_exit={:?}; oracle_passed={}",
                    mutation_output.status.code(),
                    oracle_outcome.exit_code,
                    oracle_outcome.passed()
                );
                receipt.validate()?;
                Ok(receipt)
            })();
            let cleanup = cleanup_disposable_worktree(
                &config.repository,
                &worktree,
                &candidate_sha,
                scratch.path(),
            );
            let receipt = match (attempt, cleanup) {
                (Ok(mut receipt), Ok(restoration_sha)) => {
                    receipt.restoration_sha = Some(restoration_sha);
                    receipt.validate()?;
                    receipt
                }
                (Err(error), Ok(_)) => return Err(error),
                (attempt, Err(cleanup_error)) => {
                    let retained = scratch.keep();
                    let original = attempt.err().map_or_else(
                        || "mutation succeeded but cleanup failed".to_owned(),
                        |error| error.to_string(),
                    );
                    return Err(HarnessError::ControlWithRecovery {
                        reason: format!(
                            "{original}; disposable mutation cleanup failed: {cleanup_error}; evidence retained at {}",
                            retained.display()
                        ),
                        code: ErrorCode::RecoveryIncomplete,
                        recovery: "Run project recover after inspecting the retained disposable mutation evidence.",
                    });
                }
            };
            control.write_atomic(
                &relative,
                &format!("{}\n", serde_json::to_string_pretty(&receipt)?),
            )?;
            control.commit(
                expected,
                &format!("mutation: record {}", receipt.receipt_id),
            )?;
            Ok(CommandOutcome::new(
                "mutation.create",
                format!("Recorded mutation receipt {}", receipt.receipt_id),
                serde_json::to_value(&receipt)?,
            )
            .with_project(config.project_id))
        },
    )
}

/// Restores a disposable mutation worktree without destructive Git shortcuts.
///
/// All entries other than Git's worktree metadata are first moved into a
/// quarantine directory inside the temporary allocation. The candidate tree
/// is then restored with bounded `git restore` and a non-forcing detached
/// checkout, after which ordinary `git worktree remove` can deregister it.
fn cleanup_disposable_worktree(
    repository: &Path,
    worktree: &Path,
    candidate_sha: &str,
    scratch: &Path,
) -> Result<String, HarnessError> {
    let quarantine = scratch.join("quarantine");
    fs::create_dir_all(&quarantine).map_err(|source| HarnessError::ControlIo {
        path: quarantine.clone(),
        source,
    })?;
    for entry in fs::read_dir(worktree).map_err(|source| HarnessError::ControlIo {
        path: worktree.to_owned(),
        source,
    })? {
        let entry = entry.map_err(|source| HarnessError::ControlIo {
            path: worktree.to_owned(),
            source,
        })?;
        if entry.file_name() == ".git" {
            continue;
        }
        let destination = quarantine.join(entry.file_name());
        fs::rename(entry.path(), &destination).map_err(|source| HarnessError::ControlIo {
            path: entry.path(),
            source,
        })?;
    }
    let scope = GitScope::work_tree(worktree);
    run(
        &scope,
        [
            "restore",
            "--source",
            candidate_sha,
            "--staged",
            "--worktree",
            "--",
            ".",
        ],
    )?
    .require_success()?;
    run(&scope, ["checkout", "--detach", candidate_sha])?.require_success()?;
    if !run(&scope, ["status", "--porcelain"])?
        .trimmed_stdout()
        .is_empty()
    {
        return Err(HarnessError::Control {
            reason: "disposable mutation cleanup left the worktree dirty".to_owned(),
            code: ErrorCode::RecoveryIncomplete,
        });
    }
    run(
        &GitScope::work_tree(repository),
        ["worktree", "remove", worktree.to_string_lossy().as_ref()],
    )?
    .require_success()?;
    Ok(candidate_sha.to_owned())
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
