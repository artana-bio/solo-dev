//! Project configuration and control-state commands.

use std::{fmt::Write as _, fs, path::PathBuf};

use clap::{Args, Subcommand};

use crate::{
    cli::output::CommandOutcome,
    commands::CONTROL_ENV,
    commands::{integration::ResumeOutcome, transaction::with_transaction},
    config::{
        DEFAULT_AUTHORITY_REMOTE, HostPolicy, PROJECT_SCHEMA, ProjectConfig,
        validate::{Mode, validate, validate_in_mode},
    },
    control::{
        journal::{Journal, OperationState},
        lock::{LockDiagnosis, ProjectLock},
        repository::{ControlRepository, write_project},
    },
    domain::{clock::Clock, ids::ProjectId},
    error::{ErrorCode, HarnessError},
    git::{
        authority::{
            initialize as initialize_authority, inspect_authority, stage_objects, unstage_objects,
        },
        command::{GitScope, run, run_ok},
        inspect,
    },
};

/// Subcommands under `project`.
#[derive(Debug, Subcommand)]
pub enum ProjectCommand {
    /// Create the control repository and record project configuration.
    Init(InitArgs),
    /// Validate a project configuration without changing anything.
    Validate(ValidateArgs),
    /// Report project and control state.
    Status(StatusArgs),
    /// Report interrupted operations and how to resolve them.
    Recover(RecoverArgs),
}

/// Arguments accepted by `project init`.
///
/// Section 9.1: every destructive or authority-related path is explicit. None
/// is inferred from the current directory, because inferring an authority path
/// is how a command destroys the wrong repository.
#[derive(Debug, Args)]
pub struct InitArgs {
    /// Identifier for the new project.
    #[arg(long)]
    pub project_id: String,
    /// Absolute path to the candidate repository.
    #[arg(long)]
    pub repository: PathBuf,
    /// Absolute path to the control repository.
    #[arg(long)]
    pub control: PathBuf,
    /// Absolute path to the bare authority repository.
    #[arg(long)]
    pub authority: PathBuf,
    /// Branch that promotion targets.
    #[arg(long, default_value = "main")]
    pub protected_branch: String,
    /// Root under which card worktrees are allocated.
    #[arg(long)]
    pub worktree_root: Option<PathBuf>,
    /// Validate and report planned mutations without performing them.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `project validate`.
#[derive(Debug, Args)]
pub struct ValidateArgs {
    /// Path to the project document to validate.
    #[arg(long)]
    pub config: PathBuf,
}

/// Arguments accepted by `project status`.
#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
    pub control: PathBuf,
}

/// Arguments accepted by `project recover`.
#[derive(Debug, Args)]
pub struct RecoverArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
    pub control: PathBuf,
    /// Finish what can be finished instead of only reporting it.
    ///
    /// Off by default. Reporting is always safe; resuming writes, and an
    /// operator investigating an interruption should not have their diagnostic
    /// command change state underneath them.
    #[arg(long)]
    pub resume: bool,
    /// Who is resuming.
    #[arg(long, default_value = "operator")]
    pub actor_id: String,
}

/// Executes a `project` subcommand.
///
/// # Errors
///
/// Returns a configuration, policy, or recovery error as appropriate.
pub fn execute(
    command: &ProjectCommand,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    match command {
        ProjectCommand::Init(args) => run_init(args, clock),
        ProjectCommand::Validate(args) => run_validate(args),
        ProjectCommand::Status(args) => run_status(args),
        ProjectCommand::Recover(args) => run_recover(args, clock),
    }
}

/// Builds the configuration an `init` invocation describes.
fn config_from_args(args: &InitArgs) -> Result<ProjectConfig, HarnessError> {
    let project_id: ProjectId = args.project_id.parse()?;
    let worktree_root = args.worktree_root.clone().unwrap_or_else(|| {
        args.control
            .parent()
            .unwrap_or(&args.control)
            .join(format!("{project_id}-worktrees"))
    });

    Ok(ProjectConfig {
        schema: PROJECT_SCHEMA.to_owned(),
        project_id,
        repository: args.repository.clone(),
        control_repository: args.control.clone(),
        authority_repository: args.authority.clone(),
        authority_remote: DEFAULT_AUTHORITY_REMOTE.to_owned(),
        protected_branch: args.protected_branch.clone(),
        worktree_root,
        default_output: "text".to_owned(),
        host_policy: HostPolicy::default(),
    })
}

fn run_init(args: &InitArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let config = config_from_args(args)?;
    // Validation runs before anything is created, so an invalid configuration
    // leaves the filesystem untouched, as Section 9.1 requires.
    validate_in_mode(&config, Mode::Initializing)?;

    let control = ControlRepository::at(&config.control_repository);

    if control.is_initialized() {
        return reinitialize(&control, &config, args.dry_run);
    }

    if args.dry_run {
        return Ok(CommandOutcome::new(
            "project.init",
            format!(
                "Dry run: project {} would be initialized\nwould create control repository: {}\nwould write: project/project.json\nwould commit control state\nnothing was changed",
                config.project_id,
                config.control_repository.display()
            ),
            serde_json::json!({
                "dry_run": true,
                "project_id": config.project_id.to_string(),
                "planned_mutations": [
                    format!("create control repository at {}", config.control_repository.display()),
                    "write project/project.json".to_owned(),
                    "commit control state".to_owned(),
                ],
            }),
        )
        .with_project(config.project_id.clone()));
    }

    fs::create_dir_all(&config.control_repository).map_err(|source| HarnessError::ControlIo {
        path: config.control_repository.clone(),
        source,
    })?;

    let _lock = ProjectLock::acquire(control.root(), "project.init", clock)?;
    let journal = Journal::new(&control);
    journal.require_settled()?;

    let expected_head = control.head()?;
    let mut operation = journal.begin("project.init", expected_head.clone(), clock)?;

    // Each step is journaled before the mutation it names, so an interruption
    // is attributable to a boundary rather than guessed at.
    let outcome = (|| -> Result<Option<String>, HarnessError> {
        journal.step(&mut operation, "control-git-initialized")?;
        control.initialize_git()?;
        journal.step(&mut operation, "authority-initialized")?;
        establish_authority(&config)?;
        journal.step(&mut operation, "project-document-written")?;
        write_project(&control, &config, expected_head.as_deref(), clock)
    })();

    match outcome {
        Ok(head) => {
            journal.step(&mut operation, "control-committed")?;
            journal.finish(&mut operation, OperationState::Completed, None, clock)?;
            Ok(CommandOutcome::new(
                "project.init",
                format!(
                    "Initialized project {}\ncontrol repository: {}\ncontrol head: {}",
                    config.project_id,
                    config.control_repository.display(),
                    head.as_deref().unwrap_or("unborn")
                ),
                serde_json::json!({
                    "project_id": config.project_id.to_string(),
                    "control_repository": config.control_repository,
                    "control_head": head,
                    "created": true,
                }),
            )
            .with_project(config.project_id.clone())
            .with_operation(operation.operation_id.clone()))
        }
        Err(error) => {
            journal.finish(
                &mut operation,
                OperationState::FailedPartial,
                Some(error.to_string()),
                clock,
            )?;
            Err(error)
        }
    }
}

/// Handles `init` against a control repository that already exists.
///
/// Identical configuration is a success with no change, which makes the command
/// safely repeatable. Different configuration fails without altering anything,
/// because silently rebinding a project's authority paths would repoint the
/// trust boundary the whole model rests on.
fn reinitialize(
    control: &ControlRepository,
    config: &ProjectConfig,
    dry_run: bool,
) -> Result<CommandOutcome, HarnessError> {
    let existing = control.project()?;
    if existing != *config {
        return Err(HarnessError::Control {
            reason: format!(
                "control repository at {} is already bound to a different configuration; refusing to rebind",
                control.root().display()
            ),
            code: ErrorCode::ConfigControlIncompatible,
        });
    }
    Ok(CommandOutcome::new(
        "project.init",
        format!(
            "Project {} is already initialized with identical configuration; nothing to do{}",
            config.project_id,
            if dry_run { " (dry run)" } else { "" }
        ),
        serde_json::json!({
            "project_id": config.project_id.to_string(),
            "control_repository": config.control_repository,
            "created": false,
            "dry_run": dry_run,
        }),
    )
    .with_project(config.project_id.clone()))
}

fn run_validate(args: &ValidateArgs) -> Result<CommandOutcome, HarnessError> {
    let raw = fs::read_to_string(&args.config).map_err(|source| HarnessError::Config {
        field: "<file>".to_owned(),
        reason: format!("cannot read {}: {source}", args.config.display()),
        code: ErrorCode::ConfigMalformed,
    })?;

    let config = ProjectConfig::from_json(&raw)?;
    let report = validate(&config)?;

    let symlinked: Vec<&str> = report
        .paths
        .iter()
        .filter(|entry| entry.via_symlink)
        .map(|entry| entry.field.as_str())
        .collect();

    let mut text = format!(
        "Project {} is valid\ngit: {} (minimum {})\nhost: {}\nprotected branch: {} at {}",
        report.project_id,
        report.git_version,
        report.minimum_git_version,
        report.host_os,
        report.protected_branch,
        report.protected_branch_sha,
    );
    for entry in &report.paths {
        let _ = write!(text, "\n{}: {}", entry.field, entry.resolved.display());
    }

    let mut outcome = CommandOutcome::new("project.validate", text, serde_json::to_value(&report)?)
        .with_project(config.project_id.clone());

    if !symlinked.is_empty() {
        // Recorded rather than rejected: a symlinked path is legitimate, but a
        // later change in the resolved target must be visible, so the operator
        // is told which paths carry that risk.
        outcome = outcome.with_warning(format!(
            "these paths resolve through symlinks and must be revalidated if their targets change: {}",
            symlinked.join(", ")
        ));
    }

    Ok(outcome)
}

/// What `project status` can determine about the authority right now.
///
/// Every field is optional or boolean because this is a health report: an
/// authority that has been moved, emptied, or replaced must be *described*,
/// not turned into an error. A command that refuses to run because the thing
/// it is diagnosing is unhealthy is useless exactly when it is needed.
fn authority_health(config: &ProjectConfig) -> serde_json::Value {
    let (bare, protected_sha, diagnostic) =
        match inspect_authority(&config.authority_repository, &config.protected_branch) {
            Ok(state) => (state.bare, state.protected_sha, None),
            Err(error) => (false, None, Some(error.to_string())),
        };

    // The remote is read from the candidate, so a repointed remote shows up
    // here rather than silently sending the next promotion elsewhere.
    let remote_url = run(
        &GitScope::work_tree(&config.repository),
        ["remote", "get-url", &config.authority_remote],
    )
    .ok()
    .and_then(|output| output.success().then(|| output.trimmed_stdout().to_owned()));
    let remote_matches = remote_url
        .as_ref()
        .is_some_and(|url| std::path::Path::new(url) == config.authority_repository);

    serde_json::json!({
        "path": config.authority_repository,
        "bare": bare,
        "protected_branch": config.protected_branch,
        "protected_sha": protected_sha,
        "remote": config.authority_remote,
        "remote_url": remote_url,
        "remote_matches": remote_matches,
        "diagnostic": diagnostic,
    })
}

fn run_status(args: &StatusArgs) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.control)?;
    let config = control.project()?;
    let journal = Journal::new(&control);
    let unresolved = journal.unresolved()?;
    let head = control.head()?;

    let mut text = format!(
        "Project {}\ncontrol repository: {}\ncontrol head: {}\ncontrol commits: {}\nlock: {}\nunresolved operations: {}",
        config.project_id,
        control.root().display(),
        head.as_deref().unwrap_or("unborn"),
        control.commit_count()?,
        match ProjectLock::diagnose(control.root()) {
            LockDiagnosis::Free => "free".to_owned(),
            LockDiagnosis::Held(owner) => format!("held by live process {}", owner.pid),
            LockDiagnosis::Stale { holder, reason } => {
                format!("STALE (left by process {}): {reason}", holder.pid)
            }
            LockDiagnosis::Ambiguous { reason, .. } => format!("AMBIGUOUS: {reason}"),
        },
        unresolved.len()
    );
    let authority = authority_health(&config);
    let _ = write!(
        text,
        "\nauthority: {} ({})\nprotected branch: {} at {}",
        config.authority_repository.display(),
        authority["diagnostic"]
            .as_str()
            .unwrap_or(if authority["remote_matches"] == true {
                "healthy"
            } else {
                "reachable, but the candidate's remote points elsewhere"
            }),
        config.protected_branch,
        authority["protected_sha"].as_str().unwrap_or("unborn"),
    );

    for record in &unresolved {
        let _ = write!(
            text,
            "\n  {} {} ({:?}) steps: {}",
            record.operation_id,
            record.command,
            record.state,
            if record.steps.is_empty() {
                "none".to_owned()
            } else {
                record.steps.join(", ")
            }
        );
    }

    Ok(CommandOutcome::new(
        "project.status",
        text,
        serde_json::json!({
            "project_id": config.project_id.to_string(),
            "control_repository": control.root(),
            "control_head": head,
            "control_commits": control.commit_count()?,
            "lock_held": ProjectLock::is_held(control.root()),
            "lock": match ProjectLock::diagnose(control.root()) {
                LockDiagnosis::Free => serde_json::json!({"state": "free"}),
                LockDiagnosis::Held(owner) => serde_json::json!({"state": "held", "holder": owner}),
                LockDiagnosis::Stale { holder, reason } => {
                    serde_json::json!({"state": "stale", "holder": holder, "reason": reason})
                }
                LockDiagnosis::Ambiguous { holder, reason } => {
                    serde_json::json!({"state": "ambiguous", "holder": holder, "reason": reason})
                }
            },
            "authority": authority,
            "unresolved_operations": unresolved,
        }),
    )
    .with_project(config.project_id.clone()))
}

/// Finishes what an interrupted operation left undone.
///
/// Only the promotion boundary is resumable, and deliberately so: it is the
/// only one where a command can die having already changed something outside
/// the control repository. Every other boundary either wrote nothing or wrote
/// only to control, where the next command's compare-and-swap sorts it out.
fn run_resume(args: &RecoverArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    with_transaction(
        &args.control,
        "project.recover",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let config = control.project()?;
            match crate::commands::integration::resume_promotion(
                control,
                events,
                &args.actor_id,
                expected,
                clock,
            )? {
                ResumeOutcome::Completed(outcome) => Ok(*outcome),
                ResumeOutcome::NothingHappened => Ok(CommandOutcome::new(
                    "project.recover",
                    format!(
                        "Nothing to resume for project {}: no promotion reached the authority",
                        config.project_id
                    ),
                    serde_json::json!({
                        "project_id": config.project_id.to_string(),
                        "resumed": false,
                    }),
                )
                .with_project(config.project_id.clone())),
            }
        },
    )
}

fn run_recover(args: &RecoverArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.control)?;
    let config = control.project()?;
    let journal = Journal::new(&control);
    let unresolved = journal.unresolved()?;

    if args.resume {
        // Cleared before the journal is consulted, because a process killed
        // outright leaves a lock and no unresolved entry at all — so gating
        // this on the journal would leave the commonest stale lock permanent.
        // Only a *provably* stale lock is removed; `clear_stale` refuses
        // anything whose holder might still be running.
        let diagnosis = ProjectLock::diagnose(control.root());
        let cleared_lock = ProjectLock::clear_stale(control.root(), &diagnosis)?;

        if unresolved.is_empty() {
            return Ok(CommandOutcome::new(
                "project.recover",
                format!(
                    "Project {} has no interrupted operations{}",
                    config.project_id,
                    if cleared_lock {
                        "; cleared the stale lock its dead holder left behind"
                    } else {
                        "; nothing to resume"
                    }
                ),
                serde_json::json!({
                    "project_id": config.project_id.to_string(),
                    "resumed": false,
                    "cleared_stale_lock": cleared_lock,
                    "unresolved_operations": [],
                }),
            )
            .with_project(config.project_id.clone()));
        }
        // The journal is settled first so the resuming transaction can open its
        // own entry; the interrupted one is marked completed because its work
        // is about to be finished, not abandoned.
        let mut records = unresolved.clone();
        for record in &mut records {
            journal.finish(record, OperationState::Completed, None, clock)?;
        }
        return run_resume(args, clock);
    }

    if unresolved.is_empty() {
        return Ok(CommandOutcome::new(
            "project.recover",
            format!(
                "Project {} has no interrupted operations; nothing to recover",
                config.project_id
            ),
            serde_json::json!({
                "project_id": config.project_id.to_string(),
                "unresolved_operations": [],
                "recovery_required": false,
            }),
        )
        .with_project(config.project_id.clone()));
    }

    // This package reports rather than repairs. Automatic resumption across
    // every mutation boundary is WP-500; inventing it here would mean guessing
    // at boundaries that do not exist yet.
    let mut text = format!(
        "Project {} has {} interrupted operation(s)",
        config.project_id,
        unresolved.len()
    );
    for record in &unresolved {
        let _ = write!(
            text,
            "\n\n{} {} ({:?})\n  started: {}\n  completed steps: {}\n  failure: {}",
            record.operation_id,
            record.command,
            record.state,
            record.started_at,
            if record.steps.is_empty() {
                "none".to_owned()
            } else {
                record.steps.join(", ")
            },
            record.failure.as_deref().unwrap_or("none recorded")
        );
    }
    text.push_str(
        "\n\nEach entry names the last boundary it reached. Run `project recover --resume` \
         to finish a promotion whose authority update succeeded; anything else must be \
         inspected and resolved by hand.",
    );

    Ok(CommandOutcome::new(
        "project.recover",
        text,
        serde_json::json!({
            "project_id": config.project_id.to_string(),
            "unresolved_operations": unresolved,
            "recovery_required": true,
        }),
    )
    .with_project(config.project_id.clone()))
}

/// Creates the bare authority, registers its remote, and seeds the protected
/// branch.
///
/// Seeding matters: a cycle freezes its baseline from the *authority*, so an
/// authority with no protected branch would make the first cycle unactivatable.
/// The transfer is a staged push followed by a ref update, never a force push.
///
/// # Errors
///
/// Returns a configuration error when the authority path is unusable, or an
/// external-tool error when Git fails.
fn establish_authority(config: &ProjectConfig) -> Result<(), HarnessError> {
    initialize_authority(&config.authority_repository, &config.protected_branch)?;

    // The remote is added only when absent. Section 9.1: initialization never
    // overwrites a remote, because repointing someone's existing remote is how
    // a push ends up somewhere nobody expected.
    let candidate = GitScope::work_tree(&config.repository);
    let existing = run(&candidate, ["remote", "get-url", &config.authority_remote])?;
    if !existing.success() {
        run_ok(
            &candidate,
            [
                "remote".as_ref(),
                "add".as_ref(),
                config.authority_remote.as_ref(),
                config.authority_repository.as_os_str(),
            ],
        )?;
    }

    let state = inspect_authority(&config.authority_repository, &config.protected_branch)?;
    if state.protected_sha.is_some() {
        return Ok(());
    }

    // The authority is empty, so transfer the candidate's protected branch.
    let head = inspect::resolve_commit(
        &candidate,
        &format!("refs/heads/{}", config.protected_branch),
    )?;
    let incoming = stage_objects(
        &config.repository,
        &config.authority_repository,
        &head,
        "bootstrap",
    )?;
    run_ok(
        &GitScope::git_dir(&config.authority_repository),
        [
            "update-ref",
            &format!("refs/heads/{}", config.protected_branch),
            &head,
        ],
    )?;
    unstage_objects(&config.authority_repository, &incoming);
    Ok(())
}
