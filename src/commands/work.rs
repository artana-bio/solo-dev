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
    commands::CONTROL_ENV,
    commands::{
        card::{CardStateRecord, load_card, require_convergence_budget, store_card_state},
        lesson::all_lessons,
        transaction::{Steps, with_transaction},
    },
    control::{event_store::EventDraft, repository::ControlRepository},
    domain::{
        card::{CardRecord, CardState},
        clock::{Clock, Timestamp},
        ids::{CardId, LeaseId},
        lease::{
            LEASE_DIR, LEASE_SCHEMA, LeaseRecord, LeaseStatus, ProgressNote, WORKTREE_LINK_SCHEMA,
            WorktreeLink,
        },
    },
    error::{ErrorCode, HarnessError},
    git::{command::GitScope, diff::diff_commits, inspect, worktree},
    policy::lessons::build_manifest,
    policy::verification::{CandidateFacts, verify},
};

/// Subcommands under `work`.
#[derive(Debug, Subcommand)]
pub enum WorkCommand {
    /// Allocate a branch and worktree for a ready card.
    Start(StartArgs),
    /// Emit the complete implementation packet, including governed lessons.
    Packet(CardArgs),
    /// Report a card's allocation.
    Status(CardArgs),
    /// Record a progress note against the current lease.
    Checkpoint(CheckpointArgs),
    /// Verify a worktree matches control state and take the card back up.
    Resume(ResumeArgs),
    /// Verify the candidate stays inside its card.
    Verify(CardArgs),
    /// Mark work halted pending a decision.
    Block(BlockArgs),
    /// Take over a card's lease from an actor who will not return.
    Reclaim(ReclaimArgs),
}

impl WorkCommand {
    /// Its dotted command path, as the result envelope reports it.
    ///
    /// The error envelope used to carry only the group — `work` — while a
    /// success carried the full path, so a consumer matching on `command` got a
    /// different granularity depending on whether the command worked.
    #[must_use]
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Start(..) => "work.start",
            Self::Packet(..) => "work.packet",
            Self::Status(..) => "work.status",
            Self::Checkpoint(..) => "work.checkpoint",
            Self::Resume(..) => "work.resume",
            Self::Verify(..) => "work.verify",
            Self::Block(..) => "work.block",
            Self::Reclaim(..) => "work.reclaim",
        }
    }
}

/// Arguments shared by every work subcommand.
#[derive(Debug, Args)]
pub struct CommonArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
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

/// Arguments accepted by `work resume`.
#[derive(Debug, Args)]
pub struct ResumeArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The card to take back up.
    #[arg(long)]
    pub card_id: String,
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
        WorkCommand::Packet(args) => run_packet(args),
        WorkCommand::Status(args) => run_status(args),
        WorkCommand::Checkpoint(args) => run_checkpoint(args, clock),
        WorkCommand::Resume(args) => run_resume(args, clock),
        WorkCommand::Verify(args) => run_verify(args),
        WorkCommand::Block(args) => run_block(args, clock),
        WorkCommand::Reclaim(args) => run_reclaim(args, clock),
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

/// Every lease record in the control repository, oldest id first.
///
/// Factored out of `leases_for` so the per-card filter and
/// [`silent_leases`]'s project-wide scan share one reader instead of two
/// definitions of "how leases are listed and parsed."
fn all_leases(control: &ControlRepository) -> Result<Vec<LeaseRecord>, HarnessError> {
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
        leases.push(lease);
    }
    Ok(leases)
}

/// Every lease for one card, oldest first.
fn leases_for(
    control: &ControlRepository,
    card_id: &CardId,
) -> Result<Vec<LeaseRecord>, HarnessError> {
    Ok(all_leases(control)?
        .into_iter()
        .filter(|lease| lease.card_id == *card_id)
        .collect())
}

/// True when a card's own work is finished, so a lease still recorded
/// `held` against it is a bookkeeping leftover rather than a sign anyone is
/// still meant to be watching it.
///
/// #80 follow-up repair: nothing in this codebase ever writes
/// `LeaseStatus::Released` (that lifecycle question belongs to a future
/// card; see this repair's report), so `is_held()` alone never excludes a
/// finished card's lease, and every lease ever granted would read as
/// silent forever once it crossed the threshold. Card state is the axis
/// that actually distinguishes "quiet because nobody is home" from "quiet
/// because there is nothing left to do."
///
/// Exhaustively matched on purpose: a future `CardState` variant then
/// forces a decision here at compile time instead of silently defaulting
/// one way or the other.
///
/// `Approved`, `Integrating`, and `Accepted` are included as "over" even
/// though `CardState::successors` leaves a path back to `Active` from
/// `Approved` (an invalidated approval) and from `Blocked` reached out of
/// `Integrating` (Section 11.2's own `blocked -> active`): while a card
/// sits in one of these states, its lease is waiting on someone else's
/// process, not on its actor, and if a candidate change or a block does
/// send it back to `Active`, the actor's next `work checkpoint` or
/// `work resume` is itself a fresh sign of life that clears the report.
/// `Landed` is included on the strength of `work reclaim`'s own doc
/// comment (`src/commands/work.rs`), which already treats "landed or
/// abandoned" as the pair that makes a lease cleanup candidate via
/// `archive close`.
///
/// What this still misses: a card legitimately reactivated from `Approved`
/// or `Blocked` is invisible for one threshold window immediately after
/// the bounce-back, until enough time passes for its own silence to
/// re-cross the threshold from that later point — the same lag every
/// elapsed-time detector has at a state boundary, not something this
/// predicate could remove without watching transitions instead of state.
const fn card_work_is_over(state: CardState) -> bool {
    match state {
        CardState::Draft
        | CardState::Ready
        | CardState::Leased
        | CardState::Active
        | CardState::HandedOff
        | CardState::ReviewPending
        | CardState::ChangesRequested
        | CardState::Blocked => false,
        CardState::Approved
        | CardState::Integrating
        | CardState::Accepted
        | CardState::Landed
        | CardState::Closed
        | CardState::Abandoned => true,
    }
}

/// Held leases with no recorded sign of life for at least
/// [`crate::domain::lease::SILENT_LEASE_THRESHOLD_SECONDS`], as of `now`,
/// excluding any lease whose card's own work is already over (see
/// [`card_work_is_over`]).
///
/// Mirrors `stranded_execution_permits` (`src/commands/gate.rs`): the one
/// place that scans every lease to ask which are quiet too long, so
/// `project status` and `project recover` (`src/commands/project.rs`) share
/// one definition of "silent" instead of each reimplementing "held, and
/// idle past the threshold" on its own. See #80.
///
/// # Errors
///
/// Returns an error when the lease store, or a candidate's card, cannot be
/// read.
pub(crate) fn silent_leases(
    control: &ControlRepository,
    now: Timestamp,
) -> Result<Vec<LeaseRecord>, HarnessError> {
    let mut silent = Vec::new();
    for lease in all_leases(control)? {
        if !lease.is_silent(now) {
            continue;
        }
        let (_, state) = load_card(control, &lease.card_id)?;
        if card_work_is_over(state.state) {
            continue;
        }
        silent.push(lease);
    }
    Ok(silent)
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
    steps: &mut Steps<'_>,
) -> Result<(), HarnessError> {
    // Named individually because the recoveries differ: a branch with no
    // worktree is retryable once the branch is removed, while a worktree with
    // no lock is already usable and must not be discarded.
    steps.outside_control("branch-created")?;
    worktree::create_branch(scope, branch, base)?;
    steps.outside_control("worktree-added")?;
    worktree::add_worktree(scope, path, branch)?;
    steps.outside_control("worktree-locked")?;
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
    // `preflight_start` is by definition every check that can refuse, and it
    // resolves the base commit rather than echoing what the card asked for.
    // This preview reimplemented a subset: it checked the state transition and
    // not the lease, so a card that already held one was refused with "cannot
    // move from active to leased" instead of "you hold lease L-000001 at
    // <path>, resume it" — the distinction `preflight_start`'s own comment
    // exists to preserve. It also never checked whether the branch or worktree
    // was already there.
    let (base, branch, path) = preflight_start(&control, &config, card_id, &record, &state)?;
    Ok(CommandOutcome::new(
        "work.start",
        format!(
            "Dry run: would allocate card {card_id}\nbranch: {branch} at {base}\nworktree: {}\nnothing was changed",
            path.display()
        ),
        serde_json::json!({
            "dry_run": true,
            "card_id": card_id.to_string(),
            "branch": branch,
            "base_sha": base,
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
    // 72-3: the first check that can refuse, before anything else — an
    // escalated card cannot take a new assignment. `preview_start` and
    // `run_start`'s transaction both call this one function, so there is a
    // single place this has to be checked rather than one in each, and
    // neither can promise or perform a start the other would refuse. See
    // `require_convergence_budget`.
    require_convergence_budget(control, config, record)?;
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
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let config = control.project()?;
            let (record, state) = load_card(control, &card_id)?;
            let (base, branch, path) =
                preflight_start(control, &config, &card_id, &record, &state)?;
            let scope = GitScope::work_tree(&config.repository);

            let lease_id = next_lease_id(control)?;

            // From here the operation mutates.
            allocate_worktree(&scope, &branch, &base, &path, &card_id, steps)?;
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

/// Emits a bounded implementation packet that a fresh agent can use without
/// relying on the implementer's conversation history.
fn run_packet(args: &CardArgs) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;
    let control = ControlRepository::open(&args.common.control)?;
    let config = control.project()?;
    let (card, state, lease) = allocation(&control, &card_id)?;
    let lessons = all_lessons(&control)?;
    let manifest = build_manifest(&card, &lessons)?;
    let manifest_digest = manifest.digest()?;
    let lesson_records: Vec<_> = lessons
        .into_iter()
        .filter(|lesson| {
            manifest.lessons.iter().any(|entry| {
                entry.lesson_id == lesson.lesson_id && entry.revision == lesson.revision
            })
        })
        .collect();
    let required: Vec<_> = manifest.required().cloned().collect();
    let advisory: Vec<_> = manifest.advisory().cloned().collect();
    Ok(CommandOutcome::new(
        "work.packet",
        format!(
            "Implementation packet for card {card_id}\ncard digest: {}\nlease: {}\nlesson manifest: {}\nrequired lessons: {}\nadvisory lessons: {}\nThe agent must report each required lesson check and pass the exact digest back with `handoff create --lesson-manifest-digest`",
            state.current_digest,
            lease.lease_id,
            manifest_digest,
            required.len(),
            advisory.len()
        ),
        serde_json::json!({
            "packet_schema": "harness.implementation-packet/v1",
            "card": card,
            "card_digest": state.current_digest.as_str(),
            "lease": lease,
            "manifest": manifest,
            "manifest_digest": manifest_digest.as_str(),
            "lessons": lesson_records,
            "required_lessons": required,
            "advisory_lessons": advisory,
            "reporting_contract": {
                "required": "include exact lesson ids, check ids, status, and evidence in the review verdict",
                "binding": "pass this exact digest back as `handoff create --lesson-manifest-digest`; do not hand off if the manifest digest or required gate evidence changes",
                "handoff_argument": ["--lesson-manifest-digest", manifest_digest.as_str()]
            }
        }),
    ).with_project(config.project_id.clone()))
}

// 72-3: deliberately never gated by `require_convergence_budget`. #72's
// escalation blocks what advances a card, not what parks or looks at it — a
// checkpoint is a progress note, not a step forward, and an escalated card is
// exactly the one whose record of "why it is where it is" matters most. See
// the same line drawn at `run_block`, below, and at `card status`'s report in
// `card.rs`.
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
        |control, events, expected, steps| {
            steps.at("control-write")?;
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

/// Reads a worktree locator and confirms it agrees with control state.
///
/// The locator is never trusted, only checked. Section 9.3 is explicit that it
/// is a locator, not a source of truth: it lives inside a tree the actor can
/// edit.
fn verify_locator(
    control: &ControlRepository,
    lease: &LeaseRecord,
    project_id: &crate::domain::ids::ProjectId,
) -> Result<(), HarnessError> {
    let _ = control;
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

    if let Some(disagreement) = link.disagreement(lease, project_id) {
        return Err(HarnessError::Control {
            reason: format!(
                "worktree locator at {} disagrees with control state: {disagreement}",
                link_path.display()
            ),
            code: ErrorCode::PolicyLocatorMismatch,
        });
    }
    Ok(())
}

/// Takes a card back up after review requested changes or work was blocked.
///
/// Section 11.2 permits `changes_requested -> active` and `blocked -> active`,
/// but nothing performed either transition, so a card that received review
/// feedback could never be handed off again. Resuming is the actor's own signal
/// that they have picked the work back up, which makes it the honest trigger.
/// See D-037.
fn run_resume(args: &ResumeArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;

    if args.dry_run {
        let control = ControlRepository::open(&args.common.control)?;
        let (record, state, lease) = allocation(&control, &card_id)?;
        let config = control.project()?;
        // 72-3: checked only for `ready`, the one source state
        // `resumes_to_active` admits that is functionally equivalent to
        // `work start` rather than to continuing already-owned work — see
        // the doc comment below. Resuming from `changes_requested`,
        // `blocked`, or `review_pending` is deliberately left open: an actor
        // must be able to take a returned card back up to redeliver, or a
        // card could never accumulate the attempts its budget exists to
        // count in the first place, and #72-2 already refuses the delivery
        // and review attempts themselves once that budget is spent. See
        // `require_convergence_budget`.
        if state.state == CardState::Ready {
            require_convergence_budget(&control, &config, &record)?;
        }
        verify_locator(&control, &lease, &config.project_id)?;
        return Ok(CommandOutcome::new(
            "work.resume",
            format!(
                "Dry run: locator matches; card {card_id} is `{}`{}",
                state.state,
                if resumes_to_active(state.state) {
                    " and would move to active"
                } else {
                    " and would stay as it is"
                }
            ),
            serde_json::json!({ "dry_run": true, "card_id": card_id.to_string() }),
        ));
    }

    let control = ControlRepository::open(&args.common.control)?;
    let (_record, state, _lease) = allocation(&control, &card_id)?;

    if resumes_to_active(state.state) {
        return resume_to_active(args, &card_id, clock);
    }
    report_resume(args, &card_id)
}

/// True when resuming should move the card back to active work.
///
/// `Ready` is included for one specific situation: revising a card returns it
/// to `ready`, and if the card was already allocated the lease survives. That
/// combination strands it — `work start` refuses because a lease exists, and
/// resume would refuse because the state is not one it handles. The allocation
/// is right there and the actor is plainly picking the work back up, which is
/// exactly what resuming means.
///
/// `ReviewPending` is included for the same reason one stage later. If the
/// branch moves after the handoff, no verdict can be recorded — that refusal is
/// deliberate and correct — and without a way back the card had no exit but
/// abandonment. Taking the work back is the revocation `handed_off → active`
/// already allows.
const fn resumes_to_active(state: CardState) -> bool {
    matches!(
        state,
        CardState::ChangesRequested
            | CardState::Blocked
            | CardState::Ready
            | CardState::ReviewPending
    )
}

/// Performs the `changes_requested`/`blocked` to `active` transition.
fn resume_to_active(
    args: &ResumeArgs,
    card_id: &CardId,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    with_transaction(
        &args.common.control,
        "work.resume",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let config = control.project()?;
            let (record, state, lease) = allocation(control, card_id)?;
            // 72-3: the same narrow case the dry run checks, for the same
            // reason — see the note there. `resume_to_active` handles every
            // source state `resumes_to_active` admits, and only `ready` is
            // gated: it is the one case a lease survives a revision back to
            // `ready`, making this the alternate entry point to the exact
            // advance `work start` already refuses for a fresh assignment.
            // Re-read fresh here rather than trusted from the caller's
            // routing decision, matching how every other fact this
            // transaction commits on is read at commit time, not assumed
            // from before it began.
            if state.state == CardState::Ready {
                require_convergence_budget(control, &config, &record)?;
            }
            verify_locator(control, &lease, &config.project_id)?;
            // Section 11.2 routes `ready` to `active` through `leased`, which
            // is the same path `work start` takes. Stepping through it rather
            // than widening the state machine keeps one definition of what a
            // legal transition is.
            let mut state = state;
            if state.state == CardState::Ready {
                state.state.check_transition(CardState::Leased)?;
                store_card_state(control, &record, &state, CardState::Leased)?;
                state.state = CardState::Leased;
            }
            state.state.check_transition(CardState::Active)?;
            store_card_state(control, &record, &state, CardState::Active)?;

            events.append(
                &config.project_id,
                EventDraft::new("work.resumed", &args.common.actor)
                    .cycle(record.cycle_id.clone())
                    .card(
                        card_id.clone(),
                        state.current_revision,
                        state.current_digest.clone(),
                    )
                    .transition(Some(state.state.name()), CardState::Active.name())
                    .meta("lease_id", serde_json::json!(lease.lease_id.to_string())),
                clock,
            )?;
            control.commit(expected, &format!("work: resume {card_id}"))?;

            Ok(CommandOutcome::new(
                "work.resume",
                format!(
                    "Resumed card {card_id}
moved from `{}` to `active`
worktree: {}",
                    state.state,
                    lease.worktree_path.display()
                ),
                serde_json::json!({
                    "card_id": card_id.to_string(),
                    "state": CardState::Active.name(),
                    "previous_state": state.state.name(),
                    "locator_matches": true,
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}

fn report_resume(args: &ResumeArgs, card_id: &CardId) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.common.control)?;
    let config = control.project()?;
    let (record, state, lease) = allocation(&control, card_id)?;
    let card_id = card_id.clone();

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

// 72-3: deliberately never gated by `require_convergence_budget` either.
// Blocking halts work; it does not advance the card, and it is a legitimate
// exit on its own. Refusing to record a halt on the one card that most needs
// one recorded would destroy the reason the card is where it is, not protect
// anything.
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
        |control, events, expected, steps| {
            steps.at("control-write")?;
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

/// Arguments accepted by `work reclaim`.
#[derive(Debug, Args)]
pub struct ReclaimArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The card whose lease is being taken over.
    #[arg(long)]
    pub card_id: String,
    /// Who is taking it over.
    #[arg(long)]
    pub actor_id: String,
    /// Why the previous holder is not coming back.
    #[arg(long)]
    pub reason: String,
    /// Report the takeover without performing it.
    #[arg(long)]
    pub dry_run: bool,
}

/// Takes over a card's lease from an actor who will not return.
///
/// Nothing in the candidate repository is touched. The branch, the worktree,
/// and every commit on it survive: a lease says who is responsible for a card,
/// not what the work is worth, and an abandoned lease is a coordination
/// problem rather than a reason to destroy code. Cleanup, if it is wanted, is
/// `archive close` after the card has landed or been abandoned on its own
/// terms.
///
/// # Errors
///
/// Returns a precondition error when no lease is held, or a policy error when
/// the worktree no longer matches control state.
fn run_reclaim(args: &ReclaimArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;

    if args.dry_run {
        let control = ControlRepository::open(&args.common.control)?;
        let (_record, _state, lease) = allocation(&control, &card_id)?;
        return Ok(CommandOutcome::new(
            "work.reclaim",
            format!(
                "Dry run: would move lease {} for card {card_id} from `{}` to `{}`\nthe branch, worktree, and every commit are left untouched\nnothing was changed",
                lease.lease_id, lease.actor_id, args.actor_id
            ),
            serde_json::json!({
                "dry_run": true,
                "card_id": card_id.to_string(),
                "lease_id": lease.lease_id.to_string(),
                "from_actor": lease.actor_id,
                "to_actor": args.actor_id,
            }),
        ));
    }

    with_transaction(
        &args.common.control,
        "work.reclaim",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let config = control.project()?;
            let (record, state, mut lease) = allocation(control, &card_id)?;

            if lease.actor_id == args.actor_id {
                return Err(HarnessError::Control {
                    reason: format!(
                        "{} already holds lease {} for card {card_id}; resume it rather than reclaiming it",
                        args.actor_id, lease.lease_id
                    ),
                    code: ErrorCode::PolicyLeaseHeld,
                });
            }

            // The candidate head is recorded before and after so the claim
            // "reclaiming preserves candidate commits" is checkable from the
            // event log rather than taken on trust.
            let head =
                inspect::resolve_commit(&GitScope::work_tree(&lease.worktree_path), "HEAD").ok();

            let previous_actor = lease.actor_id.clone();
            lease.actor_id.clone_from(&args.actor_id);
            lease.progress.push(ProgressNote {
                recorded_at: clock.now(),
                note: format!(
                    "lease reclaimed from {previous_actor} by {}: {}",
                    args.actor_id, args.reason
                ),
                head_sha: head.clone(),
            });
            store_lease(control, &lease)?;
            write_locator(
                control,
                &config,
                &lease.worktree_path,
                &card_id,
                state.current_revision,
                &lease.lease_id,
            )?;

            events.append(
                &config.project_id,
                EventDraft::new("work.reclaimed", &args.actor_id)
                    .cycle(record.cycle_id.clone())
                    .card(
                        card_id.clone(),
                        state.current_revision,
                        state.current_digest.clone(),
                    )
                    .meta("lease_id", serde_json::json!(lease.lease_id.to_string()))
                    .meta("from_actor", serde_json::json!(previous_actor))
                    .meta("reason", serde_json::json!(args.reason))
                    .meta("preserved_head", serde_json::json!(head)),
                clock,
            )?;
            control.commit(expected, &format!("work: reclaim lease for {card_id}"))?;

            Ok(CommandOutcome::new(
                "work.reclaim",
                format!(
                    "Reclaimed lease {} for card {card_id}\nfrom: {previous_actor}\nto: {}\nreason: {}\ncandidate head preserved at: {}",
                    lease.lease_id,
                    args.actor_id,
                    args.reason,
                    head.as_deref().unwrap_or("no commits yet")
                ),
                serde_json::json!({
                    "card_id": card_id.to_string(),
                    "lease_id": lease.lease_id.to_string(),
                    "from_actor": previous_actor,
                    "to_actor": args.actor_id,
                    "reason": args.reason,
                    "preserved_head": head,
                    "worktree_path": lease.worktree_path,
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}
