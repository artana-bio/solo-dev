//! Project configuration and control-state commands.

use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use clap::{Args, Subcommand};

use crate::{
    cli::output::CommandOutcome,
    commands::CONTROL_ENV,
    commands::{integration::ResumeOutcome, transaction::with_transaction},
    config::{
        ConvergencePolicy, DEFAULT_AUTHORITY_REMOTE, FieldError, FinalAuthorizationPolicy,
        HostPolicy, PROJECT_SCHEMA, ProjectConfig, ValidationPolicy,
        validate::{Mode, validate, validate_in_mode},
    },
    control::{
        event_store::{EVENT_DIR, EventDraft, EventStore},
        journal::{JOURNAL_DIR, Journal, OperationState},
        lock::{LOCK_FILE, LockDiagnosis, ProjectLock},
        repository::{ControlRepository, PROJECT_FILE, write_project},
    },
    domain::{
        acceptance::ACCEPTANCE_DIR,
        archive::ARCHIVE_DIR,
        card::CARD_DIR,
        clock::Clock,
        cycle::{CYCLE_DIR, CycleRecord, CycleStatus},
        digest::Digest,
        gate::GATE_DIR,
        handoff::HANDOFF_DIR,
        ids::ProjectId,
        integration::{INTEGRATION_DIR, VERIFICATION_DIR},
        lease::LEASE_DIR,
        review::REVIEW_DIR,
    },
    error::{ErrorCode, HarnessError},
    git::{
        authority::{
            initialize as initialize_authority, inspect_authority, stage_objects, unstage_objects,
        },
        command::{GitScope, run, run_ok},
        inspect,
    },
    policy::convergence::ATTEMPT_RECORDED_EVENT,
    runner::receipt::{LOG_DIR, RECEIPT_DIR},
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
    /// Install a convergence policy on an already-initialized project.
    SetConvergencePolicy(SetConvergencePolicyArgs),
    /// Install a final-authorization policy on an already-initialized
    /// project, making its declared `exception_triggers` reachable.
    SetFinalAuthorizationPolicy(SetFinalAuthorizationPolicyArgs),
}

impl ProjectCommand {
    /// Its dotted command path, as the result envelope reports it.
    ///
    /// The error envelope used to carry only the group — `project` — while a
    /// success carried the full path, so a consumer matching on `command` got a
    /// different granularity depending on whether the command worked.
    #[must_use]
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Init(..) => "project.init",
            Self::Validate(..) => "project.validate",
            Self::Status(..) => "project.status",
            Self::Recover(..) => "project.recover",
            Self::SetConvergencePolicy(..) => "project.set-convergence-policy",
            Self::SetFinalAuthorizationPolicy(..) => "project.set-final-authorization-policy",
        }
    }
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
    /// Declared actor permitted to make the final authorization of a sealed
    /// cycle. Repeat for more than one declared authorizer.
    #[arg(long = "final-authorizer-actor-id")]
    pub final_authorizer_actor_ids: Vec<String>,
    /// Path to a JSON convergence policy to install at creation, in the
    /// shape `ConvergencePolicy` deserializes. Absent means no policy: the
    /// project reports `legacy_unassessed`, never a budget nobody declared.
    #[arg(long)]
    pub convergence_policy: Option<PathBuf>,
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

/// Arguments accepted by `project set-convergence-policy`.
#[derive(Debug, Args)]
pub struct SetConvergencePolicyArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
    pub control: PathBuf,
    /// Path to a JSON convergence policy to install, in the shape
    /// `ConvergencePolicy` deserializes.
    #[arg(long)]
    pub policy: PathBuf,
    /// Who is installing the policy.
    #[arg(long, default_value = "operator")]
    pub actor: String,
    /// Validate and report planned mutations without performing them.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `project set-final-authorization-policy`.
#[derive(Debug, Args)]
pub struct SetFinalAuthorizationPolicyArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
    pub control: PathBuf,
    /// Path to a JSON final-authorization policy to install, in the shape
    /// `FinalAuthorizationPolicy` deserializes.
    #[arg(long)]
    pub policy: PathBuf,
    /// Who is installing the policy.
    #[arg(long, default_value = "operator")]
    pub actor: String,
    /// Validate and report planned mutations without performing them.
    #[arg(long)]
    pub dry_run: bool,
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
        ProjectCommand::SetConvergencePolicy(args) => run_set_convergence_policy(args, clock),
        ProjectCommand::SetFinalAuthorizationPolicy(args) => {
            run_set_final_authorization_policy(args, clock)
        }
    }
}

/// Reads, parses, and validates a convergence policy named by `--convergence-policy`.
///
/// Called from [`config_from_args`], which `run_init` invokes before it
/// touches the filesystem, so every rejection here — an unreadable file, a
/// document that will not deserialize, or a policy that fails its own
/// `validate` — happens before `project init` creates a control repository,
/// authority, or project document. Read and parse failures share the code
/// `--definition`, `--draft`, and `--verdict` already use for a path that
/// will not yield a document; a policy that parses but fails its own checks
/// carries whatever code `ConvergencePolicy::validate` already assigns.
fn read_convergence_policy(path: &PathBuf) -> Result<ConvergencePolicy, HarnessError> {
    let raw = fs::read_to_string(path).map_err(|source| HarnessError::Control {
        reason: format!(
            "cannot read convergence policy {}: {source}",
            path.display()
        ),
        code: ErrorCode::ConfigMalformed,
    })?;
    let policy: ConvergencePolicy =
        serde_json::from_str(&raw).map_err(|source| HarnessError::Control {
            reason: format!("convergence policy is malformed: {source}"),
            code: ErrorCode::ConfigMalformed,
        })?;
    policy.validate().map_err(FieldError::into_error)?;
    Ok(policy)
}

/// Reads, parses, and validates a final-authorization policy named by
/// `--policy`.
///
/// Mirrors [`read_convergence_policy`] exactly, for the same reason 79-1's
/// reader is reused by `set-convergence-policy`: an unreadable or
/// unparseable document is `ConfigMalformed`, and a document that parses but
/// fails its own checks carries whatever code
/// [`FinalAuthorizationPolicy::validate`] already assigns — this function
/// invents no new rejection of its own.
fn read_final_authorization_policy(
    path: &PathBuf,
) -> Result<FinalAuthorizationPolicy, HarnessError> {
    let raw = fs::read_to_string(path).map_err(|source| HarnessError::Control {
        reason: format!(
            "cannot read final authorization policy {}: {source}",
            path.display()
        ),
        code: ErrorCode::ConfigMalformed,
    })?;
    let policy: FinalAuthorizationPolicy =
        serde_json::from_str(&raw).map_err(|source| HarnessError::Control {
            reason: format!("final authorization policy is malformed: {source}"),
            code: ErrorCode::ConfigMalformed,
        })?;
    policy.validate().map_err(FieldError::into_error)?;
    Ok(policy)
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
    // Absent is not a default policy: an unassessed project stays
    // unassessed until an operator declares a budget.
    let convergence_policy = args
        .convergence_policy
        .as_ref()
        .map(read_convergence_policy)
        .transpose()?;

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
        validation_policy: ValidationPolicy::default(),
        final_authorization_policy: (!args.final_authorizer_actor_ids.is_empty()).then(|| {
            FinalAuthorizationPolicy {
                version: crate::config::FINAL_AUTHORIZATION_POLICY_V1.to_owned(),
                authorization_unit: "sealed_cycle".to_owned(),
                authorizer_actor_ids: args.final_authorizer_actor_ids.clone(),
                exception_triggers: vec![],
            }
        }),
        convergence_policy,
    })
}

/// Every entry `project init` may find in a directory it is about to adopt.
///
/// An interrupted `init` leaves the lock and the journal behind before Git is
/// even initialized, so those have to be admissible or a crash would wedge the
/// project harder than it already does. Everything else means the directory
/// belongs to someone.
const CONTROL_OWNED_ENTRIES: &[&str] = &[
    ".git",
    ".gitignore",
    ".DS_Store",
    LOCK_FILE,
    JOURNAL_DIR,
    "project",
    CARD_DIR,
    CYCLE_DIR,
    EVENT_DIR,
    GATE_DIR,
    HANDOFF_DIR,
    REVIEW_DIR,
    RECEIPT_DIR,
    LOG_DIR,
    LEASE_DIR,
    INTEGRATION_DIR,
    VERIFICATION_DIR,
    ACCEPTANCE_DIR,
    ARCHIVE_DIR,
];

/// Refuses to adopt a control directory holding anything the harness did not
/// put there.
///
/// Section 9.1 requires this and it was written only for the authority path.
/// Control adopted whatever was at `--control` and then overwrote its
/// `.gitignore` with the control one, reporting success — so pointing the flag
/// at a notes folder, a checkout, or a home directory destroyed a file and said
/// the project was initialized.
///
/// The test is what is present rather than emptiness, because an interrupted
/// `init` leaves a lock and a journal behind before Git exists, and refusing
/// those would turn a crash into an unrecoverable state.
///
/// # Errors
///
/// Returns a precondition error naming what it found.
fn refuse_occupied_control(path: &std::path::Path) -> Result<(), HarnessError> {
    let Ok(entries) = fs::read_dir(path) else {
        // Absent is the ordinary case, and unreadable is reported by the
        // creation that follows with a better message than this could give.
        return Ok(());
    };

    let mut foreign: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !CONTROL_OWNED_ENTRIES.contains(&name.as_str()))
        .collect();

    if foreign.is_empty() {
        return Ok(());
    }

    foreign.sort();
    foreign.truncate(5);
    Err(HarnessError::Control {
        reason: format!(
            "control path {} already holds {}; initialization will not adopt a directory whose contents nobody checked",
            path.display(),
            foreign.join(", ")
        ),
        code: ErrorCode::PreconditionOccupiedPath,
    })
}

/// Clears an earlier `project.init` that never finished, so this one may run.
///
/// An interrupted `init` left an unresolved entry, so re-running it failed
/// `require_settled` with "run `project recover`" — and `recover` opens the
/// control repository, which needs the project document the interrupted run
/// never wrote, so it answered "run `project init` first". The project was
/// wedged with two commands each pointing at the other, and nothing else could
/// reach that state to break it.
///
/// Only `project.init` entries are superseded, and only because every step this
/// command journals is safe to repeat: `initialize_git` skips an existing
/// `.git`, `establish_authority` adopts an existing remote and authority
/// without overwriting either, and the project document is written wholesale.
/// Re-running is the recovery. An unresolved entry from any other command is
/// left alone and still blocks, because nothing here knows how to finish it.
///
/// # Errors
///
/// Returns an error when the journal cannot be written.
fn supersede_interrupted_init(journal: &Journal, clock: &dyn Clock) -> Result<(), HarnessError> {
    for mut record in journal.unresolved()? {
        if record.command != "project.init" {
            continue;
        }
        journal.finish(
            &mut record,
            OperationState::FailedClean,
            Some("superseded by a later `project init`, which repeats every step".to_owned()),
            clock,
        )?;
    }
    Ok(())
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
    supersede_interrupted_init(&journal, clock)?;
    journal.require_settled()?;
    refuse_occupied_control(&config.control_repository)?;

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

/// Reports a worktree locator that names a different control repository.
///
/// Section 9.3 keeps the locator advisory: it lives in a tree the actor can
/// edit, so it is never authoritative and this never refuses. But it does know
/// which project its worktree belongs to, and that is exactly the fact missing
/// when `CHANGE_HARNESS_CONTROL` is exported for one project and a command is
/// run for another — where the command succeeds, correctly, against the wrong
/// records. Reporting the disagreement costs nothing and is the only signal
/// available.
fn locator_disagreement(control: &ControlRepository) -> Option<String> {
    let mut directory = std::env::current_dir().ok()?;
    loop {
        let candidate = directory
            .join(crate::git::worktree::AGENT_DIR)
            .join("project.json");
        if candidate.exists() {
            let raw = fs::read_to_string(&candidate).ok()?;
            let link: crate::domain::lease::WorktreeLink = serde_json::from_str(&raw).ok()?;
            // Compared after canonicalization, so a symlinked or
            // relatively-expressed path does not read as a different project.
            let recorded = fs::canonicalize(&link.control_repository).ok()?;
            let in_use = fs::canonicalize(control.root()).ok()?;
            return (recorded != in_use).then(|| {
                format!(
                    "this worktree belongs to card {} of the project at {}, not the control repository in use at {}",
                    link.card_id,
                    recorded.display(),
                    in_use.display()
                )
            });
        }
        if !directory.pop() {
            return None;
        }
    }
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
    let disagreement = locator_disagreement(&control);
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

    let mut outcome = CommandOutcome::new(
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
            "locator_disagreement": disagreement,
        }),
    );
    if let Some(warning) = disagreement {
        outcome = outcome.with_warning(warning);
    }
    Ok(outcome.with_project(config.project_id.clone()))
}

/// Finishes what an interrupted operation left undone.
///
/// Only the promotion boundary is resumable, and deliberately so: it is the
/// only one where a command can die having already changed something outside
/// the control repository. Every other boundary either wrote nothing or wrote
/// only to control, where the next command's compare-and-swap sorts it out.
/// The one journaled command `--resume` knows how to finish.
///
/// `resume_promotion` re-derives its state from the authority branch rather
/// than from a journal step, which is what makes it safe to run again. Nothing
/// else has an equivalent, so nothing else may be settled by this command.
const PROMOTE_COMMAND: &str = "integration.promote";

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
                steps,
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
        // `resume_promotion` is the only recovery this command performs, so it
        // is the only kind of entry that may be settled here. Marking every
        // unresolved entry completed and then resuming only promotions closed
        // an interrupted allocation or merge as though its work had been done,
        // reported "nothing to resume", and destroyed the sole record that a
        // partial mutation happened — after which `project status` reported a
        // healthy project over a half-made one. Three reviewers found this
        // independently.
        let unresumable: Vec<&str> = unresolved
            .iter()
            .filter(|record| record.command != PROMOTE_COMMAND)
            .map(|record| record.command.as_str())
            .collect();
        if !unresumable.is_empty() {
            return Err(HarnessError::Control {
                reason: format!(
                    "cannot resume: {} interrupted operation(s) are not promotions ({}); `project recover` without `--resume` reports what each one left behind, and their disposition is an operator decision",
                    unresumable.len(),
                    unresumable.join(", ")
                ),
                code: ErrorCode::RecoveryIncomplete,
            });
        }

        // The journal is settled first so the resuming transaction can open its
        // own entry; the interrupted one is marked completed because its work
        // is about to be finished by `run_resume`, not abandoned.
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

/// The digest a project's currently configured convergence policy is frozen
/// at, or `None` for a `legacy_unassessed` project that has never had one.
fn existing_policy_digest(config: &ProjectConfig) -> Result<Option<Digest>, HarnessError> {
    config
        .convergence_policy
        .as_ref()
        .map(ConvergencePolicy::digest)
        .transpose()
}

/// The digest a project's currently configured final-authorization policy is
/// frozen at, or `None` for a project that has never declared one.
fn existing_final_authorization_policy_digest(
    config: &ProjectConfig,
) -> Result<Option<Digest>, HarnessError> {
    config
        .final_authorization_policy
        .as_ref()
        .map(FinalAuthorizationPolicy::digest)
        .transpose()
}

/// Whether a cycle carrying this status could still be compared against
/// `project_revision` at `src/commands/gate.rs:791`
/// (`gate::validation_progress`).
///
/// That function never inspects a cycle's status at all — it only requires
/// `load_card` to find a `CardRecord` and the cycle's `baseline_sha` to be
/// `Some`. So this is not "does the code check status", which it does not,
/// but "could a cycle legitimately at this status still hold a card whose
/// gate commands run the comparison". Established empirically, not guessed,
/// by reading every writer of a `CycleRecord` and every path that can put a
/// card under one:
///
/// - `src/commands/cycle.rs`'s `store` is the only function in this codebase
///   that persists a `CycleRecord`, and it has exactly five call sites:
///   create, activate, seal, declare-group, abandon. No command anywhere
///   sets a stored cycle's status to `integrating`, `accepted`, `landed`, or
///   `blocked` — those four are real, designed states with transition edges
///   already in `CycleStatus::successors`, but no shipped work package
///   produces them yet.
/// - `src/commands/card.rs`'s `run_activate` is the only path that creates a
///   `CardRecord`, gated by `cycle_accepting_cards` on
///   `CycleStatus::accepts_cards`, true only for `active`.
///
/// From those two facts:
///
/// - `draft` cannot block. No command frozen a baseline yet, and no stored
///   cycle ever returns to `draft` (`successors` has no edge back to it), so
///   a draft cycle can hold no card and the comparison is unreachable for
///   it — not merely unlikely.
/// - `closed` and `abandoned` cannot block. Both are
///   `CycleStatus::is_terminal`: the coordinator has already taken the cycle
///   out of play, and both are the exact remedy this command's own refusal
///   offers below. Refusing a `closed` or `abandoned` cycle would be the fix
///   and the fault at once — the canonical over-blocking mistake.
/// - Every other status blocks. `active` and `sealed` are real today (the
///   only other two statuses any command produces), and cards keep working
///   in both — `sealed`'s own doc comment says existing cards continue
///   through work and review. `integrating`, `accepted`, `landed`, and
///   `blocked` are not producible by any command in this codebase yet, but
///   each names a cycle still expected to move — `blocked` most explicitly,
///   since its own successors lead back to `active` or `integrating` — so a
///   card declared under any of them keeps exactly the frozen baseline
///   `validation_progress` reads with no regard to the label on it.
const fn blocks_convergence_policy_change(status: CycleStatus) -> bool {
    !matches!(
        status,
        CycleStatus::Draft | CycleStatus::Closed | CycleStatus::Abandoned
    )
}

/// Every cycle record in the control repository, sorted by identifier.
///
/// Mirrors the directory walk `commands::card::active_cycle_claims` already
/// uses to scan every cycle: `CYCLE_DIR` may not exist yet on a freshly
/// initialized project.
///
/// # Errors
///
/// Returns an error when the directory cannot be read or a record is
/// malformed.
fn all_cycles(control: &ControlRepository) -> Result<Vec<CycleRecord>, HarnessError> {
    let directory = control.path(CYCLE_DIR);
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut names: Vec<String> = fs::read_dir(&directory)
        .map_err(|source| HarnessError::ControlIo {
            path: directory.clone(),
            source,
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| {
            Path::new(name)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        })
        .collect();
    names.sort();

    names
        .into_iter()
        .map(|name| {
            let relative = format!("{CYCLE_DIR}/{name}");
            serde_json::from_str(&control.read(&relative)?).map_err(|source| {
                HarnessError::Control {
                    reason: format!("cycle record {relative} is malformed: {source}"),
                    code: ErrorCode::InternalControlCorrupt,
                }
            })
        })
        .collect()
}

/// Refuses when a cycle could still be compared against `project_revision`
/// and would start failing that comparison the moment this policy changes
/// the project's digest.
///
/// Today that comparison lives entirely inside `gate::validation_progress`
/// and fires only after the fact, as `CH-POLICY-INVALID-CYCLE` on whichever
/// gate command the cycle's cards happen to run next. This is the same
/// refusal, moved to the moment the policy change is proposed, while the
/// operator can still decide, instead of discovered later by accident.
///
/// Shared by `set-convergence-policy` and `set-final-authorization-policy`.
/// `Digest::of_canonical(&config)` covers the whole project document, so
/// either field moves the same `project_revision` a frozen cycle pins;
/// `blocks_convergence_policy_change` decides purely from cycle status, with
/// nothing in it specific to which field of the project changed, so one walk
/// serves both callers. `policy_name` only chooses the trailing clause's
/// wording, so each caller's refusal names the policy its own operator is
/// actually installing rather than the other command's.
///
/// # Errors
///
/// Returns [`ErrorCode::PolicyInvalidCycle`] naming every offending cycle and
/// its status when one exists.
fn refuse_blocking_cycles(
    control: &ControlRepository,
    policy_name: &str,
) -> Result<(), HarnessError> {
    let offenders: Vec<CycleRecord> = all_cycles(control)?
        .into_iter()
        .filter(|cycle| blocks_convergence_policy_change(cycle.status))
        .collect();
    if offenders.is_empty() {
        return Ok(());
    }
    let named = offenders
        .iter()
        .map(|cycle| format!("{} ({})", cycle.cycle_id, cycle.status))
        .collect::<Vec<_>>()
        .join(", ");
    Err(HarnessError::Control {
        reason: format!(
            "{} cycle(s) could still be compared against `project_revision` and would start failing that comparison once this policy changes the project's digest: {named}. Close or abandon them, or start a new cycle instead, before changing the {policy_name}.",
            offenders.len()
        ),
        code: ErrorCode::PolicyInvalidCycle,
    })
}

/// Refuses when convergence facts are already recorded and this policy would
/// change the digest they are bound to.
///
/// `policy::convergence::project` refuses its entire view — not just the
/// mismatched fact — the moment one recorded `convergence.attempt_recorded`
/// event names a digest other than the currently configured policy's, and
/// no rebinding ever makes that fact current again: nothing in this codebase
/// rewrites a recorded fact onto a new policy.
///
/// `disposition rebaseline` resolves that from the other side rather than by
/// rebinding. It retires the old digest, and the projection reads a fact
/// under a retired digest as a true record of what happened that no longer
/// measures today's budget — validated in full, but never counted. A digest
/// no rebaseline retired still refuses the whole view, so that narrowing is
/// only ever as wide as an authorized decision made it.
///
/// This command is not that decision: it installs a policy and nothing else.
/// So a policy that would orphan existing facts is still refused here, and
/// the refusal names the command that can make the change instead.
///
/// # Errors
///
/// Returns [`ErrorCode::ConfigControlIncompatible`] when facts are already
/// recorded and this digest would not match what they were recorded against.
fn refuse_orphaning_facts(
    control: &ControlRepository,
    existing_digest: Option<&Digest>,
    new_digest: &Digest,
) -> Result<(), HarnessError> {
    let events = EventStore::new(control);
    let has_facts = events
        .all()?
        .iter()
        .any(|event| event.event_type == ATTEMPT_RECORDED_EVENT);
    if !has_facts {
        return Ok(());
    }
    // The remedy is only `disposition rebaseline` when there is a digest to
    // retire. With none configured, that command refuses at its own check 2
    // ("no policy digest to retire"), so naming it here would send the
    // operator to a second refusal — the exact defect this message carried
    // for two releases when it named an issue instead of a command.
    //
    // No CLI path reaches that branch: every writer of an
    // `ATTEMPT_RECORDED_EVENT` is gated on `config.convergence_policy` being
    // `Some` (`review.rs`, both sites in `handoff.rs`, `card.rs`, and
    // `integration.rs`), and nothing removes a policy once installed, so a
    // fact cannot outlive the policy that admitted it. It is kept, and left
    // untested, as a diagnostic for a hand-edited project document — which
    // is why its text sends the operator to the control repository rather
    // than to a command that would only refuse again.
    let (currently, remedy) = existing_digest.map_or_else(
        || {
            (
                "no configured policy".to_owned(),
                "No digest is configured to retire, so no rebaseline applies either: these facts name a policy this project no longer accounts for, and the control repository has to be inspected before any policy is installed."
                    .to_owned(),
            )
        },
        |digest| {
            (
                format!("policy digest {digest}"),
                format!(
                    "This command installs a policy and nothing else. Run `disposition rebaseline` with `--policy` and `--rationale` to retire {digest} and install this one as a single authorized decision, re-pinning every non-terminal cycle; the facts already recorded stay in the record as history that no longer counts toward a budget."
                ),
            )
        },
    );
    Err(HarnessError::Control {
        reason: format!(
            "convergence facts are already recorded against {currently}; installing digest {new_digest} would name a foreign digest for every one of them, and the convergence projection refuses its entire view rather than read a fact it cannot bind. {remedy}"
        ),
        code: ErrorCode::ConfigControlIncompatible,
    })
}

/// Reports installing a policy that is already in force, unchanged.
///
/// Reinstalling a byte-identical policy is a success, not an error: nothing
/// about the project's digest moves, so it cannot possibly break a cycle's
/// frozen `project_revision` or orphan a recorded fact. Checked first, and
/// ahead of both refusals above and the project lock, because idempotent
/// success needs neither.
fn already_installed_outcome(
    config: &ProjectConfig,
    digest: &Digest,
    dry_run: bool,
) -> CommandOutcome {
    CommandOutcome::new(
        "project.set-convergence-policy",
        format!(
            "Project {} already has convergence policy digest {digest} installed; nothing to do{}",
            config.project_id,
            if dry_run { " (dry run)" } else { "" }
        ),
        serde_json::json!({
            "project_id": config.project_id.to_string(),
            "policy_digest": digest.as_str(),
            "changed": false,
            "dry_run": dry_run,
        }),
    )
    .with_project(config.project_id.clone())
}

/// Reports installing a final-authorization policy that is already in force,
/// unchanged.
///
/// Mirrors [`already_installed_outcome`] for the same reason: reinstalling a
/// byte-identical policy cannot move the project's digest, so it cannot break
/// a cycle's frozen `project_revision` either, and idempotent success needs
/// neither `refuse_blocking_cycles` nor the project lock.
fn already_installed_final_authorization_outcome(
    config: &ProjectConfig,
    digest: &Digest,
    dry_run: bool,
) -> CommandOutcome {
    CommandOutcome::new(
        "project.set-final-authorization-policy",
        format!(
            "Project {} already has final authorization policy digest {digest} installed; nothing to do{}",
            config.project_id,
            if dry_run { " (dry run)" } else { "" }
        ),
        serde_json::json!({
            "project_id": config.project_id.to_string(),
            "policy_digest": digest.as_str(),
            "changed": false,
            "dry_run": dry_run,
        }),
    )
    .with_project(config.project_id.clone())
}

/// Validates every check the real write makes, without taking the project
/// lock or opening a transaction: reporting rather than performing, the same
/// split `acceptance::preview_record` uses over `acceptance::run_record`'s
/// checks.
fn preview_set_convergence_policy(
    args: &SetConvergencePolicyArgs,
    new_digest: &Digest,
) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.control)?;
    let config = control.project()?;
    let existing_digest = existing_policy_digest(&config)?;

    if existing_digest.as_ref() == Some(new_digest) {
        return Ok(already_installed_outcome(&config, new_digest, true));
    }

    refuse_blocking_cycles(&control, "convergence policy")?;
    refuse_orphaning_facts(&control, existing_digest.as_ref(), new_digest)?;

    Ok(CommandOutcome::new(
        "project.set-convergence-policy",
        format!(
            "Dry run: project {} would install convergence policy digest {new_digest}\nwould write: project/project.json\nwould commit control state\nnothing was changed",
            config.project_id
        ),
        serde_json::json!({
            "dry_run": true,
            "project_id": config.project_id.to_string(),
            "policy_digest": new_digest.as_str(),
            "previous_policy_digest": existing_digest.as_ref().map(Digest::to_string),
            "planned_mutations": [
                "write project/project.json".to_owned(),
                "commit control state".to_owned(),
            ],
        }),
    )
    .with_project(config.project_id.clone()))
}

/// Validates every check the real write makes, without taking the project
/// lock or opening a transaction. Mirrors
/// [`preview_set_convergence_policy`]; the one structural difference is what
/// it does *not* call: unlike `ConvergencePolicy`, whose digest every
/// convergence fact pins, no recorded fact pins `FinalAuthorizationPolicy`'s
/// digest the same fail-forever way, so there is no `refuse_orphaning_facts`
/// equivalent to run here — see [`run_set_final_authorization_policy`] for
/// the full reasoning.
fn preview_set_final_authorization_policy(
    args: &SetFinalAuthorizationPolicyArgs,
    new_digest: &Digest,
) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.control)?;
    let config = control.project()?;
    let existing_digest = existing_final_authorization_policy_digest(&config)?;

    if existing_digest.as_ref() == Some(new_digest) {
        return Ok(already_installed_final_authorization_outcome(
            &config, new_digest, true,
        ));
    }

    refuse_blocking_cycles(&control, "final authorization policy")?;

    Ok(CommandOutcome::new(
        "project.set-final-authorization-policy",
        format!(
            "Dry run: project {} would install final authorization policy digest {new_digest}\nwould write: project/project.json\nwould commit control state\nnothing was changed",
            config.project_id
        ),
        serde_json::json!({
            "dry_run": true,
            "project_id": config.project_id.to_string(),
            "policy_digest": new_digest.as_str(),
            "previous_policy_digest": existing_digest.as_ref().map(Digest::to_string),
            "planned_mutations": [
                "write project/project.json".to_owned(),
                "commit control state".to_owned(),
            ],
        }),
    )
    .with_project(config.project_id.clone()))
}

/// Installs a convergence policy on a project that already exists.
///
/// 79-1 gave the path for a policy declared at `project init`; this is the
/// path for a project that already has cards and cycles, where a careless
/// change silently breaks in two ways: a cycle already frozen under the old
/// digest starts failing `gate.rs:791`'s comparison on its next command, or a
/// convergence fact already recorded under the old digest makes
/// `policy::convergence::project` refuse its entire view forever. Both are
/// now refused here, before anything is written, instead of discovered later
/// by whichever command happens to run into them.
///
/// # Errors
///
/// Returns a configuration error when the policy file cannot be read or
/// fails its own validation, or a policy error naming the blocking cycles or
/// the facts already recorded, as established by [`refuse_blocking_cycles`]
/// and [`refuse_orphaning_facts`].
fn run_set_convergence_policy(
    args: &SetConvergencePolicyArgs,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    // Reuses 79-1's reader as-is: same reads, same rejections, same
    // validation.
    let new_policy = read_convergence_policy(&args.policy)?;
    let new_digest = new_policy.digest()?;

    if args.dry_run {
        return preview_set_convergence_policy(args, &new_digest);
    }

    with_transaction(
        &args.control,
        "project.set-convergence-policy",
        clock,
        move |control, events, expected, steps| {
            steps.at("control-write")?;
            // Re-read and re-check inside the lock rather than trust the
            // preview's reads: two operations racing to install the same
            // policy must both still see one consistent, idempotent outcome.
            let mut config = control.project()?;
            let existing_digest = existing_policy_digest(&config)?;
            if existing_digest.as_ref() == Some(&new_digest) {
                return Ok(already_installed_outcome(&config, &new_digest, false));
            }
            refuse_blocking_cycles(control, "convergence policy")?;
            refuse_orphaning_facts(control, existing_digest.as_ref(), &new_digest)?;

            config.convergence_policy = Some(new_policy);
            control.write_atomic(PROJECT_FILE, &format!("{}\n", config.to_json()?))?;

            events.append(
                &config.project_id,
                EventDraft::new("project.convergence_policy_installed", &args.actor)
                    .meta("policy_digest", serde_json::json!(new_digest.as_str()))
                    .meta(
                        "previous_policy_digest",
                        serde_json::json!(existing_digest.as_ref().map(Digest::to_string)),
                    ),
                clock,
            )?;
            control.commit(
                expected,
                &format!("project: set convergence policy {new_digest}"),
            )?;

            Ok(CommandOutcome::new(
                "project.set-convergence-policy",
                format!(
                    "Installed convergence policy digest {new_digest} on project {}",
                    config.project_id
                ),
                serde_json::json!({
                    "project_id": config.project_id.to_string(),
                    "policy_digest": new_digest.as_str(),
                    "previous_policy_digest": existing_digest.as_ref().map(Digest::to_string),
                    "changed": true,
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}

/// Installs a final-authorization policy on a project that already exists.
///
/// Mirrors [`run_set_convergence_policy`] step for step: read and validate
/// the policy, short-circuit to an idempotent success on a byte-identical
/// reinstall, refuse a cycle whose frozen `project_revision` would start
/// failing `gate.rs:792`'s comparison the moment this policy moves the
/// project's digest, then write, record one event, and commit.
///
/// Before this command existed, nothing could put a value into
/// `FinalAuthorizationPolicy.exception_triggers` except `project init`
/// (which always writes it empty) or a test hand-editing `project.json`
/// directly, bypassing every governed command — so the entire `integration
/// exception raise`/`resolve` subsystem was unreachable in any project built
/// through the normal commands. This is that field's other half, the way
/// `set-convergence-policy` is 79-1's other half for `ConvergencePolicy`: it
/// changes or first installs a policy on a project that may already have
/// cards, cycles, and accepted final integrations.
///
/// Three decisions this command deliberately does *not* make the way a
/// neighboring command does, recorded here so nobody "fixes" one of them by
/// accident:
///
/// - **No `authorizer_actor_ids` gate.** Matching `set-convergence-policy`,
///   `--actor` only names who to credit in the emitted event; it authorizes
///   nothing. D-013 already treats a declared actor as a claim rather than a
///   proof, so a gate here would add no real trust. And gating the edit of a
///   policy on that same policy's own `authorizer_actor_ids` is a bootstrap
///   trap: a project whose declared authorizers are wrong could never be
///   corrected, because the only command that could fix them would refuse to
///   run until they were already right.
/// - **No re-pinning.** [`refuse_blocking_cycles`] refuses a blocking cycle
///   outright rather than re-pinning its `project_revision`, the way
///   `disposition rebaseline` re-pins every non-terminal cycle when it
///   changes the convergence policy. Re-pinning changes the rules governing
///   an already-open cycle, which is why that path is an *authorized
///   disposition* requiring an authorizer — and this command has none (see
///   above), so it must not perform the equivalent mutation under a
///   different name. The reachable path stays: set the policy before
///   creating a cycle, or between cycles.
/// - **No `refuse_orphaning_facts` equivalent.** Verified, not assumed: a
///   `ConvergencePolicy`'s digest is written into every convergence fact as
///   `policy_digest`, and `policy::convergence::project` refuses its
///   *entire*, project-wide view forever the instant one fact's digest no
///   longer matches the configured policy — which is exactly what
///   `refuse_orphaning_facts` exists to prevent before it can happen.
///   `FinalAuthorizationPolicy`'s digest is also embedded in recorded facts,
///   but narrower and already self-healing where `ConvergencePolicy`'s is
///   not: `integration.exception_raised`/`exception_resolved` events each
///   pin the policy digest in force when they were written
///   (`exception_bindings`, checked back by `exceptions_for` in
///   `commands::integration`), and a v2 `AcceptanceRecord` pins
///   `final_authorization_policy_digest` at acceptance time, re-checked by
///   `validate_final_authorization_for_promotion` in `commands::acceptance`
///   before every promotion — by that function's own doc comment,
///   deliberately: "a changed project policy … must invalidate an older v2
///   authorization rather than relying on the old record's existence." Each
///   of those checks its own narrow digest against the *current* policy on
///   every read, scoped to one integration or one acceptance, not folded
///   across the whole project. A mismatch fails closed there
///   (`CH-POLICY-NOT-ACCEPTED`, "no longer matches the current project
///   policy" / "no longer binds final integration") and is recoverable by
///   re-authorizing under the new policy — an intentional, working
///   invalidate-and-redo path, not an unrecoverable, project-wide
///   corruption with no rebaseline mechanism. So this command leaves that
///   already-fail-closed re-check to do its job rather than duplicating it
///   as a second refusal at install time. `refuse_blocking_cycles` still
///   applies here for the same reason it applies to `set-convergence-policy`:
///   `Digest::of_canonical(&config)` covers the whole project document, so
///   this policy moves the same `project_revision` a frozen cycle pins,
///   regardless of which field changed.
///
/// Reusing `refuse_blocking_cycles` here (noted above) is in fact why no
/// orphaning check is needed at all, not merely why an orphan would be
/// recoverable if one occurred: it means one cannot occur.
/// `blocks_convergence_policy_change` refuses every cycle status except
/// `Draft`, `Closed`, and `Abandoned`. A final integration cannot even be
/// prepared until its cycle is sealed (`integration.rs:1002`,
/// `final_cycle_binding`), so no exception binding or v2 acceptance record
/// for one is ever written under a `Draft` cycle; and a sealed cycle's only
/// stored successor is `Abandoned` (`CycleStatus::successors`), which is
/// terminal (`CycleStatus::is_terminal`) and never promotes again, so a
/// digest pinned on its acceptance record is never re-checked again either.
/// So `refuse_blocking_cycles` already refuses this install in every cycle
/// status under which a live, re-checkable digest binding can exist — there
/// is nothing left to orphan.
///
/// That makes `blocks_convergence_policy_change`'s exact refusal list
/// load-bearing for this command too, not only for `set-convergence-policy`:
/// narrowing it to exempt any non-terminal status would silently reopen the
/// orphan this reasoning rules out. `Sealed` above all — it is literally
/// `FinalAuthorizationPolicy::authorization_unit`'s own value,
/// `"sealed_cycle"`, so it is exactly the status under which a live v2
/// acceptance record pins this digest.
///
/// # Errors
///
/// Returns a configuration error when the policy file cannot be read or
/// fails its own validation, or a policy error naming the blocking cycles, as
/// established by [`refuse_blocking_cycles`].
fn run_set_final_authorization_policy(
    args: &SetFinalAuthorizationPolicyArgs,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    let new_policy = read_final_authorization_policy(&args.policy)?;
    let new_digest = new_policy.digest()?;

    if args.dry_run {
        return preview_set_final_authorization_policy(args, &new_digest);
    }

    with_transaction(
        &args.control,
        "project.set-final-authorization-policy",
        clock,
        move |control, events, expected, steps| {
            steps.at("control-write")?;
            // Re-read and re-check inside the lock rather than trust the
            // preview's reads: two operations racing to install the same
            // policy must both still see one consistent, idempotent outcome.
            let mut config = control.project()?;
            let existing_digest = existing_final_authorization_policy_digest(&config)?;
            if existing_digest.as_ref() == Some(&new_digest) {
                return Ok(already_installed_final_authorization_outcome(
                    &config,
                    &new_digest,
                    false,
                ));
            }
            refuse_blocking_cycles(control, "final authorization policy")?;

            config.final_authorization_policy = Some(new_policy);
            control.write_atomic(PROJECT_FILE, &format!("{}\n", config.to_json()?))?;

            events.append(
                &config.project_id,
                EventDraft::new("project.final_authorization_policy_installed", &args.actor)
                    .meta("policy_digest", serde_json::json!(new_digest.as_str()))
                    .meta(
                        "previous_policy_digest",
                        serde_json::json!(existing_digest.as_ref().map(Digest::to_string)),
                    ),
                clock,
            )?;
            control.commit(
                expected,
                &format!("project: set final authorization policy {new_digest}"),
            )?;

            Ok(CommandOutcome::new(
                "project.set-final-authorization-policy",
                format!(
                    "Installed final authorization policy digest {new_digest} on project {}",
                    config.project_id
                ),
                serde_json::json!({
                    "project_id": config.project_id.to_string(),
                    "policy_digest": new_digest.as_str(),
                    "previous_policy_digest": existing_digest.as_ref().map(Digest::to_string),
                    "changed": true,
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
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
