//! Project configuration and control-state commands.

use std::{fmt::Write as _, fs, path::PathBuf};

use clap::{Args, Subcommand};

use crate::{
    cli::output::CommandOutcome,
    config::{
        DEFAULT_AUTHORITY_REMOTE, HostPolicy, PROJECT_SCHEMA, ProjectConfig,
        validate::{Mode, validate, validate_in_mode},
    },
    control::{
        journal::{Journal, OperationState},
        lock::ProjectLock,
        repository::{ControlRepository, write_project},
    },
    domain::{clock::Clock, ids::ProjectId},
    error::{ErrorCode, HarnessError},
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
    #[arg(long)]
    pub control: PathBuf,
}

/// Arguments accepted by `project recover`.
#[derive(Debug, Args)]
pub struct RecoverArgs {
    /// Path to the control repository.
    #[arg(long)]
    pub control: PathBuf,
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
        ProjectCommand::Recover(args) => run_recover(args),
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
        if ProjectLock::is_held(control.root()) {
            "held"
        } else {
            "free"
        },
        unresolved.len()
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
            "unresolved_operations": unresolved,
        }),
    )
    .with_project(config.project_id.clone()))
}

fn run_recover(args: &RecoverArgs) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.control)?;
    let config = control.project()?;
    let journal = Journal::new(&control);
    let unresolved = journal.unresolved()?;

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
        "\n\nEach entry names the last boundary it reached. Inspect the control repository, \
         resolve the state by hand, then mark the entry completed or abandoned.",
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
