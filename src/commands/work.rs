//! Work allocation commands: leases, branches, and worktrees.
//!
//! Section 13.2 fixes the order of `work start`. The order is the safety
//! property: every check that can refuse runs before anything is created, and
//! each created thing is journaled before it exists, so an interruption is
//! attributable to a boundary rather than inferred from wreckage.

use std::{fmt::Write as _, fs, path::PathBuf};

use clap::{Args, Subcommand};

use crate::{
    cli::output::CommandOutcome,
    commands::{
        card::{CardStateRecord, load_card, store_card_state},
        transaction::with_transaction,
    },
    control::{event_store::EventDraft, repository::ControlRepository},
    domain::{
        card::{CardRecord, CardState},
        clock::Clock,
        ids::{CardId, LeaseId},
        lease::{
            LEASE_DIR, LEASE_SCHEMA, LeaseRecord, LeaseStatus, ProgressNote, WORKTREE_LINK_SCHEMA,
            WorktreeLink,
        },
    },
    error::{ErrorCode, HarnessError},
    git::{command::GitScope, diff::diff_commits, inspect, worktree},
    policy::verification::{CandidateFacts, verify},
};

/// Subcommands under `work`.
#[derive(Debug, Subcommand)]
pub enum WorkCommand {
    /// Allocate a branch and worktree for a ready card.
    Start(StartArgs),
    /// Report a card's allocation.
    Status(CardArgs),
    /// Record a progress note against the current lease.
    Checkpoint(CheckpointArgs),
    /// Verify a worktree still matches authoritative control state.
    Resume(CardArgs),
    /// Verify the candidate stays inside its card.
    Verify(CardArgs),
    /// Mark work halted pending a decision.
    Block(BlockArgs),
}

/// Arguments shared by every work subcommand.
#[derive(Debug, Args)]
pub struct CommonArgs {
    /// Path to the control repository.
    #[arg(long)]
    pub control: PathBuf,
    /// Identifies the acting party. Declared, not proven; see D-013.
    #[arg(long, default_value = "operator")]
    pub actor: String,
}

/// Arguments accepted by `work start`.
#[derive(Debug, Args)]
pub struct StartArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The card to allocate.
    #[arg(long)]
    pub card_id: String,
    /// Report planned mutations without performing them.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments naming only a card.
#[derive(Debug, Args)]
pub struct CardArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The card to act on.
    #[arg(long)]
    pub card_id: String,
}

/// Arguments accepted by `work checkpoint`.
#[derive(Debug, Args)]
pub struct CheckpointArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The card to record progress against.
    #[arg(long)]
    pub card_id: String,
    /// What to record.
    #[arg(long)]
    pub note: String,
    /// Report planned mutations without performing them.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `work block`.
#[derive(Debug, Args)]
pub struct BlockArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The card to block.
    #[arg(long)]
    pub card_id: String,
    /// Why work cannot proceed.
    #[arg(long)]
    pub reason: String,
    /// Report planned mutations without performing them.
    #[arg(long)]
    pub dry_run: bool,
}

/// Executes a `work` subcommand.
///
/// # Errors
///
/// Returns a policy, precondition, or configuration error as appropriate.
pub fn execute(command: &WorkCommand, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    match command {
        WorkCommand::Start(args) => run_start(args, clock),
        WorkCommand::Status(args) => run_status(args),
        WorkCommand::Checkpoint(args) => run_checkpoint(args, clock),
        WorkCommand::Resume(args) => run_resume(args),
        WorkCommand::Verify(args) => run_verify(args),
        WorkCommand::Block(args) => run_block(args, clock),
    }
}

/// Allocates the next lease identifier.
fn next_lease_id(control: &ControlRepository) -> Result<LeaseId, HarnessError> {
    let directory = control.path(LEASE_DIR);
    let highest = if directory.exists() {
        fs::read_dir(&directory)
            .map_err(|source| HarnessError::ControlIo {
                path: directory.clone(),
                source,
            })?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                entry
                    .path()
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .and_then(|stem| stem.strip_prefix("L-"))
                    .and_then(|digits| digits.parse::<u64>().ok())
            })
            .max()
            .unwrap_or(0)
    } else {
        0
    };
    format!("L-{:06}", highest + 1).parse()
}

/// Every lease for one card, oldest first.
fn leases_for(
    control: &ControlRepository,
    card_id: &CardId,
) -> Result<Vec<LeaseRecord>, HarnessError> {
    let directory = control.path(LEASE_DIR);
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut names: Vec<String> = fs::read_dir(&directory)
        .map_err(|source| HarnessError::ControlIo {
            path: directory,
            source,
        })?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension()? == "json")
                .then(|| path.file_stem()?.to_str().map(ToOwned::to_owned))?
        })
        .collect();
    names.sort();

    let mut leases = Vec::new();
    for name in names {
        let raw = control.read(&format!("{LEASE_DIR}/{name}.json"))?;
        let lease: LeaseRecord =
            serde_json::from_str(&raw).map_err(|source| HarnessError::Control {
                reason: format!("lease {name} is malformed: {source}"),
                code: ErrorCode::InternalControlCorrupt,
            })?;
        if lease.card_id == *card_id {
            leases.push(lease);
        }
    }
    Ok(leases)
}

/// The lease currently held for a card, if any.
///
/// # Errors
///
/// Returns an error when the lease store cannot be read.
pub fn held_lease(
    control: &ControlRepository,
    card_id: &CardId,
) -> Result<Option<LeaseRecord>, HarnessError> {
    Ok(leases_for(control, card_id)?
        .into_iter()
        .find(LeaseRecord::is_held))
}

/// Writes a lease record.
fn store_lease(control: &ControlRepository, lease: &LeaseRecord) -> Result<(), HarnessError> {
    control.write_atomic(
        &LeaseRecord::relative_path(&lease.lease_id),
        &format!("{}\n", serde_json::to_string_pretty(lease)?),
    )
}

/// The branch name a card's work uses.
fn branch_for(card_id: &CardId) -> String {
    format!("card/{card_id}")
}

/// The worktree path a card's work uses.
fn worktree_for(worktree_root: &std::path::Path, card_id: &CardId) -> PathBuf {
    worktree_root.join(card_id.as_str())
}

/// Creates the branch, worktree, lock, and exclude rule.
///
/// Ordered so the cheapest-to-undo mutation happens first: a branch with no
/// worktree is a smaller mess than a worktree with no branch.
fn allocate_worktree(
    scope: &GitScope,
    branch: &str,
    base: &str,
    path: &std::path::Path,
    card_id: &CardId,
) -> Result<(), HarnessError> {
    worktree::create_branch(scope, branch, base)?;
    worktree::add_worktree(scope, path, branch)?;
    worktree::lock_worktree(scope, path, &format!("allocated to card {card_id}"))?;
    worktree::install_agent_exclude(scope)?;
    Ok(())
}

/// Writes the ignored locator into the allocated worktree.
fn write_locator(
    control: &ControlRepository,
    config: &crate::config::ProjectConfig,
    path: &std::path::Path,
    card_id: &CardId,
    card_revision: u32,
    lease_id: &LeaseId,
) -> Result<(), HarnessError> {
    let link = WorktreeLink {
        schema: WORKTREE_LINK_SCHEMA.to_owned(),
        project_id: config.project_id.clone(),
        card_id: card_id.clone(),
        card_revision,
        control_repository: control.root().to_path_buf(),
        lease_id: lease_id.clone(),
    };
    let link_path = WorktreeLink::path_in(path);
    fs::create_dir_all(link_path.parent().unwrap_or(path)).map_err(|source| {
        HarnessError::ControlIo {
            path: path.to_path_buf(),
            source,
        }
    })?;
    fs::write(
        &link_path,
        format!("{}\n", serde_json::to_string_pretty(&link)?),
    )
    .map_err(|source| HarnessError::ControlIo {
        path: link_path,
        source,
    })
}

/// Confirms the allocation actually produced what was asked for.
///
/// Checked rather than assumed: an allocation that half-succeeded must not be
/// reported as success, because the next command would build on it.
fn verify_allocation(
    scope: &GitScope,
    path: &std::path::Path,
    base: &str,
) -> Result<(), HarnessError> {
    let corrupt = |reason: String| HarnessError::Control {
        reason,
        code: ErrorCode::InternalControlCorrupt,
    };

    if !worktree::is_registered(scope, path)? {
        return Err(corrupt(format!(
            "worktree {} was not registered by Git",
            path.display()
        )));
    }
    let worktree_scope = GitScope::work_tree(path);
    if inspect::resolve_commit(&worktree_scope, "HEAD")? != base {
        return Err(corrupt(format!(
            "worktree {} did not check out {base}",
            path.display()
        )));
    }
    if !inspect::worktree_state(&worktree_scope)?.clean {
        return Err(corrupt(format!(
            "worktree {} is not clean immediately after allocation",
            path.display()
        )));
    }
    Ok(())
}

/// Reports what `work start` would do, without doing any of it.
fn preview_start(args: &StartArgs, card_id: &CardId) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.common.control)?;
    let config = control.project()?;
    let (record, state) = load_card(&control, card_id)?;
    state.state.check_transition(CardState::Leased)?;
    let branch = branch_for(card_id);
    let path = worktree_for(&config.worktree_root, card_id);
    Ok(CommandOutcome::new(
        "work.start",
        format!(
            "Dry run: would allocate card {card_id}\nbranch: {branch} at {}\nworktree: {}\nnothing was changed",
            record.base_sha,
            path.display()
        ),
        serde_json::json!({
            "dry_run": true,
            "card_id": card_id.to_string(),
            "branch": branch,
            "base_sha": record.base_sha,
            "worktree_path": path,
        }),
    ))
}

/// Runs every check that can refuse, before anything is created.
///
/// Returns the resolved base commit, branch name, and worktree path.
fn preflight_start(
    control: &ControlRepository,
    config: &crate::config::ProjectConfig,
    card_id: &CardId,
    record: &CardRecord,
    state: &CardStateRecord,
) -> Result<(String, String, PathBuf), HarnessError> {
    // Lease availability is checked before the state transition on purpose. A
    // card that already holds a lease will also fail the transition check, but
    // "you hold lease L-000001 at <path>, resume it" tells the operator what to
    // do next; "cannot move from active to leased" only tells them they were
    // refused.
    if let Some(existing) = held_lease(control, card_id)? {
        return Err(HarnessError::Control {
            reason: format!(
                "card {card_id} already holds lease {} at {}; resume it or release it",
                existing.lease_id,
                existing.worktree_path.display()
            ),
            code: ErrorCode::PolicyLeaseHeld,
        });
    }
    state.state.check_transition(CardState::Leased)?;

    let scope = GitScope::work_tree(&config.repository);
    let branch = branch_for(card_id);
    let path = worktree_for(&config.worktree_root, card_id);

    if worktree::branch_exists(&scope, &branch)? {
        return Err(HarnessError::Control {
            reason: format!("branch `{branch}` already exists"),
            code: ErrorCode::PreconditionBranchExists,
        });
    }
    if path.exists() {
        return Err(HarnessError::Control {
            reason: format!("worktree path already exists: {}", path.display()),
            code: ErrorCode::PreconditionWorktreeExists,
        });
    }
    let base =
        inspect::resolve_commit(&scope, &record.base_sha).map_err(|_| HarnessError::Control {
            reason: format!(
                "card {card_id} declares base {} which does not name a commit in {}",
                record.base_sha,
                config.repository.display()
            ),
            code: ErrorCode::PreconditionBaseMissing,
        })?;
    Ok((base, branch, path))
}

fn run_start(args: &StartArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;

    if args.dry_run {
        return preview_start(args, &card_id);
    }

    with_transaction(
        &args.common.control,
        "work.start",
        clock,
        |control, events, expected| {
            let config = control.project()?;
            let (record, state) = load_card(control, &card_id)?;
            let (base, branch, path) =
                preflight_start(control, &config, &card_id, &record, &state)?;
            let scope = GitScope::work_tree(&config.repository);

            let lease_id = next_lease_id(control)?;

            // From here the operation mutates.
            allocate_worktree(&scope, &branch, &base, &path, &card_id)?;
            write_locator(
                control,
                &config,
                &path,
                &card_id,
                state.current_revision,
                &lease_id,
            )?;
            verify_allocation(&scope, &path, &base)?;

            let lease = LeaseRecord {
                schema: LEASE_SCHEMA.to_owned(),
                lease_id: lease_id.clone(),
                card_id: card_id.clone(),
                card_revision: state.current_revision,
                actor_id: args.common.actor.clone(),
                branch: branch.clone(),
                worktree_path: path.clone(),
                base_sha: base.clone(),
                status: LeaseStatus::Held,
                granted_at: clock.now(),
                released_at: None,
                progress: Vec::new(),
            };
            store_lease(control, &lease)?;
            store_card_state(control, &record, &state, CardState::Active)?;

            events.append(
                &config.project_id,
                EventDraft::new("work.started", &args.common.actor)
                    .cycle(record.cycle_id.clone())
                    .card(
                        card_id.clone(),
                        state.current_revision,
                        state.current_digest.clone(),
                    )
                    .transition(Some(state.state.name()), CardState::Active.name())
                    .head(base.clone())
                    .meta("lease_id", serde_json::json!(lease_id.to_string()))
                    .meta("branch", serde_json::json!(branch))
                    .meta("worktree_path", serde_json::json!(path)),
                clock,
            )?;
            control.commit(expected, &format!("work: start {card_id} on {branch}"))?;

            Ok(CommandOutcome::new(
                "work.start",
                format!(
                    "Allocated card {card_id}\nlease: {lease_id}\nbranch: {branch} at {base}\nworktree: {}",
                    path.display()
                ),
                serde_json::json!({
                    "card_id": card_id.to_string(),
                    "lease_id": lease_id.to_string(),
                    "branch": branch,
                    "base_sha": base,
                    "worktree_path": path,
                    "state": CardState::Active.name(),
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}

/// Loads the card, its state, and its held lease, or explains what is missing.
fn allocation(
    control: &ControlRepository,
    card_id: &CardId,
) -> Result<(CardRecord, CardStateRecord, LeaseRecord), HarnessError> {
    let (record, state) = load_card(control, card_id)?;
    let lease = held_lease(control, card_id)?.ok_or_else(|| HarnessError::Control {
        reason: format!("card {card_id} holds no lease; run `work start` first"),
        code: ErrorCode::PreconditionNotFound,
    })?;
    Ok((record, state, lease))
}

fn run_status(args: &CardArgs) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;
    let control = ControlRepository::open(&args.common.control)?;
    let config = control.project()?;
    let (_record, state) = load_card(&control, &card_id)?;
    let leases = leases_for(&control, &card_id)?;
    let held = leases.iter().find(|lease| lease.is_held());

    let mut text = format!(
        "Card {card_id}\nstate: {}\nrevision: {}\nleases: {}",
        state.state,
        state.current_revision,
        leases.len()
    );
    if let Some(lease) = held {
        let _ = write!(
            text,
            "\nheld lease: {}\n  actor: {}\n  branch: {} at {}\n  worktree: {}\n  progress notes: {}",
            lease.lease_id,
            lease.actor_id,
            lease.branch,
            lease.base_sha,
            lease.worktree_path.display(),
            lease.progress.len()
        );
        for note in &lease.progress {
            let _ = write!(text, "\n    {} {}", note.recorded_at, note.note);
        }
    } else {
        text.push_str("\nheld lease: none");
    }

    Ok(CommandOutcome::new(
        "work.status",
        text,
        serde_json::json!({
            "card_id": card_id.to_string(),
            "state": state.state.name(),
            "revision": state.current_revision,
            "lease_count": leases.len(),
            "held_lease": held,
        }),
    )
    .with_project(config.project_id.clone()))
}

fn run_checkpoint(
    args: &CheckpointArgs,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;

    if args.dry_run {
        let control = ControlRepository::open(&args.common.control)?;
        let (_record, _state, lease) = allocation(&control, &card_id)?;
        return Ok(CommandOutcome::new(
            "work.checkpoint",
            format!(
                "Dry run: would record a progress note against lease {}; nothing was changed",
                lease.lease_id
            ),
            serde_json::json!({ "dry_run": true, "card_id": card_id.to_string() }),
        ));
    }

    with_transaction(
        &args.common.control,
        "work.checkpoint",
        clock,
        |control, events, expected| {
            let config = control.project()?;
            let (record, state, mut lease) = allocation(control, &card_id)?;
            // A checkpoint does not move the card, per Section 11.4, so only
            // states that represent live work may record one.
            if !matches!(state.state, CardState::Active | CardState::Blocked) {
                return Err(HarnessError::Control {
                    reason: format!(
                        "card {card_id} is `{}`; only active or blocked work records progress",
                        state.state
                    ),
                    code: ErrorCode::PolicyInvalidTransition,
                });
            }

            let scope = GitScope::work_tree(&lease.worktree_path);
            let head = inspect::resolve_commit(&scope, "HEAD").ok();
            lease.progress.push(ProgressNote {
                recorded_at: clock.now(),
                note: args.note.clone(),
                head_sha: head.clone(),
            });
            store_lease(control, &lease)?;

            let mut draft = EventDraft::new("work.checkpoint", &args.common.actor)
                .cycle(record.cycle_id.clone())
                .card(
                    card_id.clone(),
                    state.current_revision,
                    state.current_digest.clone(),
                )
                .meta("note", serde_json::json!(args.note))
                .meta("lease_id", serde_json::json!(lease.lease_id.to_string()));
            if let Some(sha) = &head {
                draft = draft.head(sha.clone());
            }
            events.append(&config.project_id, draft, clock)?;
            control.commit(expected, &format!("work: checkpoint {card_id}"))?;

            Ok(CommandOutcome::new(
                "work.checkpoint",
                format!(
                    "Recorded progress on card {card_id}\nnote: {}\nhead: {}",
                    args.note,
                    head.as_deref().unwrap_or("no commits yet")
                ),
                serde_json::json!({
                    "card_id": card_id.to_string(),
                    "lease_id": lease.lease_id.to_string(),
                    "note": args.note,
                    "head_sha": head,
                    "state": state.state.name(),
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}

fn run_resume(args: &CardArgs) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;
    let control = ControlRepository::open(&args.common.control)?;
    let config = control.project()?;
    let (record, state, lease) = allocation(&control, &card_id)?;

    // The locator is read only to be checked against control state. Section 9.3
    // is explicit that it is a locator, not a source of truth: it lives inside a
    // tree the actor can edit.
    let link_path = WorktreeLink::path_in(&lease.worktree_path);
    let raw = fs::read_to_string(&link_path).map_err(|source| HarnessError::ControlIo {
        path: link_path.clone(),
        source,
    })?;
    let link: WorktreeLink =
        serde_json::from_str(&raw).map_err(|source| HarnessError::Control {
            reason: format!(
                "worktree locator at {} is malformed: {source}",
                link_path.display()
            ),
            code: ErrorCode::PolicyLocatorMismatch,
        })?;

    if let Some(disagreement) = link.disagreement(&lease, &config.project_id) {
        return Err(HarnessError::Control {
            reason: format!(
                "worktree locator at {} disagrees with control state: {disagreement}",
                link_path.display()
            ),
            code: ErrorCode::PolicyLocatorMismatch,
        });
    }

    let scope = GitScope::work_tree(&lease.worktree_path);
    let head = inspect::resolve_commit(&scope, "HEAD")?;
    let worktree_state = inspect::worktree_state(&scope)?;

    Ok(CommandOutcome::new(
        "work.resume",
        format!(
            "Card {card_id} is allocated and its worktree matches control state\nlease: {}\nbranch: {} at {}\nworktree: {}\nhead: {head}\nclean: {}\ncard revision: {}",
            lease.lease_id,
            lease.branch,
            lease.base_sha,
            lease.worktree_path.display(),
            worktree_state.clean,
            state.current_revision
        ),
        serde_json::json!({
            "card_id": card_id.to_string(),
            "lease_id": lease.lease_id.to_string(),
            "branch": lease.branch,
            "base_sha": lease.base_sha,
            "worktree_path": lease.worktree_path,
            "head_sha": head,
            "clean": worktree_state.clean,
            "dirty_paths": worktree_state.dirty_paths,
            "card_revision": state.current_revision,
            "cycle_id": record.cycle_id.to_string(),
            "locator_matches": true,
        }),
    )
    .with_project(config.project_id.clone()))
}

/// Collects the facts verification needs, from Git objects only.
///
/// Nothing here reads the worktree's copy of the card. Section 13.3 requires
/// verification to compare Git objects, because an actor can edit anything
/// inside their own worktree, including a cached card.
fn collect_facts(record: &CardRecord, lease: &LeaseRecord) -> Result<CandidateFacts, HarnessError> {
    let scope = GitScope::work_tree(&lease.worktree_path);
    let candidate_sha = inspect::resolve_commit(&scope, "HEAD")?;
    let declared_base = inspect::resolve_commit(&scope, &record.base_sha)?;
    let actual_base = inspect::merge_base(&scope, &declared_base, &candidate_sha)?;
    let diff = diff_commits(&scope, &declared_base, &candidate_sha)?;

    let subjects = inspect::raw(
        &scope,
        [
            "log",
            "--format=%s",
            &format!("{declared_base}..{candidate_sha}"),
        ],
    )?;
    let commit_subjects: Vec<String> = subjects
        .trimmed_stdout()
        .lines()
        .map(ToOwned::to_owned)
        .collect();

    let state = inspect::worktree_state(&scope)?;
    Ok(CandidateFacts {
        declared_base,
        actual_base,
        candidate_sha,
        diff,
        commit_subjects,
        worktree_clean: state.clean,
        dirty_paths: state.dirty_paths,
    })
}

fn run_verify(args: &CardArgs) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;
    let control = ControlRepository::open(&args.common.control)?;
    let config = control.project()?;
    let (record, state, lease) = allocation(&control, &card_id)?;

    let facts = collect_facts(&record, &lease)?;
    let report = verify(&record, state.current_digest.as_str(), &facts);

    let mut text = format!(
        "Card {card_id} candidate {}\nbase: {}\nchanged paths: {}\nverdict: {}",
        report.candidate_sha,
        report.base_sha,
        report.changed_paths.len(),
        if report.passed { "PASS" } else { "FAIL" }
    );
    for finding in &report.findings {
        let _ = write!(
            text,
            "\n  [{}] {} {}",
            match finding.severity {
                crate::policy::verification::Severity::Blocking => "blocking",
                crate::policy::verification::Severity::Advisory => "advisory",
            },
            finding.kind,
            finding.detail
        );
    }

    let outcome = CommandOutcome::new("work.verify", text, serde_json::to_value(&report)?)
        .with_project(config.project_id.clone());

    if report.passed {
        Ok(outcome)
    } else {
        // A failed verification is a policy refusal, not a report. Returning
        // success with `passed: false` would let a caller pipe it onward and
        // treat an out-of-scope candidate as ready.
        Err(HarnessError::Control {
            reason: format!(
                "candidate {} is outside card {card_id}: {}",
                report.candidate_sha,
                report
                    .blocking()
                    .iter()
                    .map(|finding| finding.detail.clone())
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
            code: ErrorCode::PolicyCandidateOutOfScope,
        })
    }
}

fn run_block(args: &BlockArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;

    if args.dry_run {
        let control = ControlRepository::open(&args.common.control)?;
        let (_record, state) = load_card(&control, &card_id)?;
        state.state.check_transition(CardState::Blocked)?;
        return Ok(CommandOutcome::new(
            "work.block",
            format!("Dry run: would block card {card_id}; nothing was changed"),
            serde_json::json!({ "dry_run": true, "card_id": card_id.to_string() }),
        ));
    }

    with_transaction(
        &args.common.control,
        "work.block",
        clock,
        |control, events, expected| {
            let config = control.project()?;
            let (record, state) = load_card(control, &card_id)?;
            state.state.check_transition(CardState::Blocked)?;
            store_card_state(control, &record, &state, CardState::Blocked)?;

            events.append(
                &config.project_id,
                EventDraft::new("work.blocked", &args.common.actor)
                    .cycle(record.cycle_id.clone())
                    .card(
                        card_id.clone(),
                        state.current_revision,
                        state.current_digest.clone(),
                    )
                    .transition(Some(state.state.name()), CardState::Blocked.name())
                    .meta("reason", serde_json::json!(args.reason)),
                clock,
            )?;
            control.commit(expected, &format!("work: block {card_id}"))?;

            Ok(CommandOutcome::new(
                "work.block",
                format!("Blocked card {card_id}\nreason: {}", args.reason),
                serde_json::json!({
                    "card_id": card_id.to_string(),
                    "state": CardState::Blocked.name(),
                    "reason": args.reason,
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}
