//! Preserving reachability, then cleaning up what is safe to remove.
//!
//! Cleanup and data loss look identical from the outside: a branch is gone
//! either way. The difference is whether the commits it held are still findable
//! afterwards, and that difference is what this package makes checkable rather
//! than hoped for.

use clap::{Args, Subcommand};

use crate::{
    cli::output::CommandOutcome,
    commands::CONTROL_ENV,
    commands::{
        card::{load_card, store_card_state},
        integration::load_integration,
        transaction::with_transaction,
        work::held_lease,
    },
    control::{event_store::EventDraft, repository::ControlRepository},
    domain::{
        archive::{ARCHIVE_SCHEMA, ArchiveRecord},
        card::CardState,
        clock::Clock,
        digest::CANONICAL_ALGORITHM,
        ids::IntegrationId,
        integration::{IntegrationRecord, IntegrationStatus},
    },
    error::{ErrorCode, HarnessError},
    git::{
        archive::{ArchivedRef, card_ref, create, integration_ref, unarchived_commits, verify},
        command::GitScope,
        integration_worktree::is_clean,
        landing,
        worktree::{delete_branch, remove_worktree, unlock_worktree},
    },
};

/// Subcommands under `archive`.
#[derive(Debug, Subcommand)]
pub enum ArchiveCommand {
    /// Create archive refs for a promoted integration and its cards.
    Create(CommonArgs),
    /// Check that every archived ref still resolves to its recorded commit.
    Verify(VerifyArgs),
    /// Remove worktrees and branches, closing the integration and its cards.
    Close(CloseArgs),
}

impl ArchiveCommand {
    /// Its dotted command path, as the result envelope reports it.
    ///
    /// The error envelope used to carry only the group — `archive` — while a
    /// success carried the full path, so a consumer matching on `command` got a
    /// different granularity depending on whether the command worked.
    #[must_use]
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Create(..) => "archive.create",
            Self::Verify(..) => "archive.verify",
            Self::Close(..) => "archive.close",
        }
    }
}

/// Arguments shared by every archive subcommand.
#[derive(Debug, Args)]
pub struct CommonArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
    pub control: std::path::PathBuf,
    /// The integration to act on.
    #[arg(long)]
    pub integration_id: String,
    /// Who is acting.
    #[arg(long, default_value = "operator")]
    pub actor_id: String,
    /// Report without changing anything.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `archive verify`.
///
/// No `--dry-run`: verification reads and reports, so a dry run of it would be
/// the same command. Advertising the flag would imply the plain form changes
/// something.
#[derive(Debug, Args)]
pub struct VerifyArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
    pub control: std::path::PathBuf,
    /// The integration to check.
    #[arg(long)]
    pub integration_id: String,
}

/// Arguments accepted by `archive close`.
#[derive(Debug, Args)]
pub struct CloseArgs {
    #[command(flatten)]
    pub common: CommonArgs,
}

/// Executes an `archive` subcommand.
///
/// # Errors
///
/// Returns a precondition or policy error as appropriate.
pub fn execute(
    command: &ArchiveCommand,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    match command {
        ArchiveCommand::Create(args) => run_create(args, clock),
        ArchiveCommand::Verify(args) => run_verify(args),
        ArchiveCommand::Close(args) => run_close(&args.common, clock),
    }
}

/// The refs a promoted integration must archive.
///
/// One per member candidate and one for the landing commit. The candidates
/// matter as much as the landing: a squashed or rebased landing tree does not
/// contain the individual candidate commits, so deleting the card branches
/// without archiving them would lose the history of how the change was made.
fn refs_for(record: &IntegrationRecord) -> Result<Vec<ArchivedRef>, HarnessError> {
    let Some(landing_sha) = record.landing_sha.clone() else {
        return Err(HarnessError::Control {
            reason: format!(
                "integration {} has no landing commit",
                record.integration_id
            ),
            code: ErrorCode::PreconditionNotFound,
        });
    };

    let mut refs = vec![ArchivedRef {
        reference: integration_ref(record.integration_id.as_str()),
        sha: landing_sha,
    }];
    for member in &record.members {
        refs.push(ArchivedRef {
            reference: card_ref(member.card_id.as_str()),
            sha: member.candidate_sha.clone(),
        });
    }
    Ok(refs)
}

/// Refuses to archive an integration that has not been promoted.
fn require_promoted(record: &IntegrationRecord) -> Result<(), HarnessError> {
    if record.status == IntegrationStatus::Promoted {
        return Ok(());
    }
    Err(HarnessError::Control {
        reason: format!(
            "integration {} is `{}`; archiving follows promotion",
            record.integration_id,
            record.status.name()
        ),
        code: ErrorCode::PolicyInvalidTransition,
    })
}

fn run_create(args: &CommonArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let integration_id: IntegrationId = args.integration_id.parse()?;

    if args.dry_run {
        let control = ControlRepository::open(&args.control)?;
        let config = control.project()?;
        let record = load_integration(&control, &integration_id)?;
        require_promoted(&record)?;
        let refs = refs_for(&record)?;
        return Ok(CommandOutcome::new(
            "archive.create",
            format!(
                "Dry run: would create {} archive ref(s) for {integration_id}\nnothing was changed",
                refs.len()
            ),
            serde_json::json!({
                "dry_run": true,
                "integration_id": integration_id.to_string(),
                "refs": refs,
            }),
        )
        .with_project(config.project_id));
    }

    with_transaction(
        &args.control,
        "archive.create",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let config = control.project()?;
            let record = load_integration(control, &integration_id)?;
            require_promoted(&record)?;
            let refs = refs_for(&record)?;

            for archived in &refs {
                create(&config.repository, &archived.reference, &archived.sha)?;
            }
            // The landing ref did its job — keeping the commit alive between
            // construction and promotion — and the archive ref now does it
            // durably, so the temporary one is released.
            landing::release(&config.repository, integration_id.as_str())?;

            let archive = ArchiveRecord {
                schema: ARCHIVE_SCHEMA.to_owned(),
                integration_id: integration_id.clone(),
                cycle_id: record.cycle_id.clone(),
                refs: refs.clone(),
                archived_by: args.actor_id.clone(),
                archived_at: clock.now(),
                canonical_algorithm: CANONICAL_ALGORITHM.to_owned(),
            };
            let digest = archive.digest()?;
            control.write_atomic(
                &ArchiveRecord::relative_path(&integration_id),
                &format!("{}\n", serde_json::to_string_pretty(&archive)?),
            )?;

            events.append(
                &config.project_id,
                EventDraft::new("archive.created", &args.actor_id)
                    .cycle(record.cycle_id.clone())
                    .meta(
                        "integration_id",
                        serde_json::json!(integration_id.to_string()),
                    )
                    .meta("archive_digest", serde_json::json!(digest.as_str()))
                    .meta("refs", serde_json::json!(refs.len())),
                clock,
            )?;
            control.commit(
                expected,
                &format!("archive: create refs for {integration_id}"),
            )?;

            Ok(CommandOutcome::new(
                "archive.create",
                format!(
                    "Archived {integration_id}\nrefs created: {}\nthe temporary landing ref was released",
                    refs.len()
                ),
                serde_json::json!({
                    "integration_id": integration_id.to_string(),
                    "archive_digest": digest.as_str(),
                    "refs": refs,
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}

/// Reads an integration's archive record.
fn load_archive(
    control: &ControlRepository,
    integration_id: &IntegrationId,
) -> Result<ArchiveRecord, HarnessError> {
    let relative = ArchiveRecord::relative_path(integration_id);
    if !control.path(&relative).exists() {
        return Err(HarnessError::Control {
            reason: format!(
                "integration {integration_id} has not been archived; run `archive create` first"
            ),
            code: ErrorCode::PreconditionNotFound,
        });
    }
    serde_json::from_str(&control.read(&relative)?).map_err(|source| HarnessError::Control {
        reason: format!("archive for {integration_id} is malformed: {source}"),
        code: ErrorCode::InternalControlCorrupt,
    })
}

fn run_verify(args: &VerifyArgs) -> Result<CommandOutcome, HarnessError> {
    let integration_id: IntegrationId = args.integration_id.parse()?;
    let control = ControlRepository::open(&args.control)?;
    let config = control.project()?;
    let archive = load_archive(&control, &integration_id)?;

    let mut defects = Vec::new();
    for archived in &archive.refs {
        if let Some(defect) = verify(&config.repository, archived)? {
            defects.push((archived.clone(), defect));
        }
    }

    let payload = serde_json::json!({
        "integration_id": integration_id.to_string(),
        "checked": archive.refs.len(),
        "intact": defects.is_empty(),
        "defects": defects.iter().map(|(archived, defect)| serde_json::json!({
            "reference": archived.reference,
            "expected_sha": archived.sha,
            "defect": defect,
        })).collect::<Vec<_>>(),
    });

    if defects.is_empty() {
        return Ok(CommandOutcome::new(
            "archive.verify",
            format!(
                "Every archived ref for {integration_id} is intact ({} checked)",
                archive.refs.len()
            ),
            payload,
        )
        .with_project(config.project_id));
    }

    Err(HarnessError::Control {
        reason: format!(
            "{} archived ref(s) for {integration_id} do not resolve as recorded: {}",
            defects.len(),
            defects
                .iter()
                .map(|(archived, _)| archived.reference.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        code: ErrorCode::PolicyArchiveBroken,
    })
}

/// Removes a card's worktree and branch, refusing anything unsafe.
///
/// Both removals are non-forcing. Invariant 7.2 forbids force-removing a
/// worktree or deleting an unmerged branch, and the whole point of archiving
/// first is that by the time this runs, refusing should be impossible for the
/// right reasons rather than merely unlikely.
fn clean_up_card(
    control: &ControlRepository,
    config: &crate::config::ProjectConfig,
    card_id: &crate::domain::ids::CardId,
) -> Result<Vec<String>, HarnessError> {
    let scope = GitScope::work_tree(&config.repository);
    let branch = format!("card/{card_id}");
    let reference = format!("refs/heads/{branch}");
    let mut removed = Vec::new();

    // A branch holding commits no other ref can reach must not be deleted,
    // whatever the card's state says.
    let outstanding = unarchived_commits(&config.repository, &reference)?;
    if !outstanding.is_empty() {
        return Err(HarnessError::Control {
            reason: format!(
                "branch `{branch}` holds {} commit(s) reachable from nowhere else; archive them before cleanup",
                outstanding.len()
            ),
            code: ErrorCode::PreconditionUnmergedWork,
        });
    }

    if let Some(lease) = held_lease(control, card_id)?
        && lease.worktree_path.exists()
    {
        // `work start` locks the worktree so `git worktree prune` cannot
        // reclaim it. Removal has to unlock first — but cleanliness is checked
        // here, before unlocking, so the refusal never depends on the lock.
        // Unlocking a worktree holding uncommitted work and then discovering
        // it is dirty would leave it unprotected on the failure path.
        if !is_clean(&lease.worktree_path)? {
            return Err(HarnessError::Control {
                reason: format!(
                    "worktree {} holds uncommitted or untracked work",
                    lease.worktree_path.display()
                ),
                code: ErrorCode::PreconditionWorktreeDirty,
            });
        }
        unlock_worktree(&scope, &lease.worktree_path)?;
        remove_worktree(&scope, &lease.worktree_path)?;
        removed.push(lease.worktree_path.display().to_string());
    }

    let exists = crate::git::worktree::branch_exists(&scope, &branch)?;
    if exists {
        delete_branch(&scope, &branch)?;
        removed.push(branch);
    }
    Ok(removed)
}

/// Reports what `archive close` would remove, without removing it.
///
/// `close` is the most destructive command here: it removes worktrees, deletes
/// branches, and closes cards. It accepted `--dry-run` and never read it — the
/// dispatch arm passed only `args.common` to a function with no access to the
/// flag — so asking to preview the destruction performed it, and reported
/// success as though it had previewed.
///
/// Every check the real close makes runs here too. A preview that skips checks
/// is worse than no preview, because it tells an operator a destructive command
/// will succeed when it will not.
fn preview_close(
    args: &CommonArgs,
    integration_id: &IntegrationId,
) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.control)?;
    let config = control.project()?;
    let record = load_integration(&control, integration_id)?;

    if record.status == IntegrationStatus::Archived {
        return Ok(CommandOutcome::new(
            "archive.close",
            format!("Dry run: integration {integration_id} is already closed; nothing to do"),
            serde_json::json!({
                "dry_run": true,
                "integration_id": integration_id.to_string(),
                "status": IntegrationStatus::Archived.name(),
                "would_remove": Vec::<String>::new(),
                "changed": false,
            }),
        )
        .with_project(config.project_id));
    }

    record
        .status
        .check_transition(IntegrationStatus::Archived)?;
    let archive = load_archive(&control, integration_id)?;
    for archived in &archive.refs {
        if let Some(defect) = verify(&config.repository, archived)? {
            return Err(HarnessError::Control {
                reason: format!(
                    "archived ref {} is {defect:?}; refusing to remove anything",
                    archived.reference
                ),
                code: ErrorCode::PolicyArchiveBroken,
            });
        }
    }

    // The same refusals `clean_up_card` makes, without the removals it makes
    // alongside them: a branch holding commits nothing else reaches, or a
    // worktree with uncommitted work.
    let scope = GitScope::work_tree(&config.repository);
    let mut would_remove = Vec::new();
    for member in &record.members {
        let card_id = &member.card_id;
        let branch = format!("card/{card_id}");
        let outstanding = unarchived_commits(&config.repository, &format!("refs/heads/{branch}"))?;
        if !outstanding.is_empty() {
            return Err(HarnessError::Control {
                reason: format!(
                    "branch `{branch}` holds {} commit(s) reachable from nowhere else; archive them before cleanup",
                    outstanding.len()
                ),
                code: ErrorCode::PreconditionUnmergedWork,
            });
        }
        if let Some(lease) = held_lease(&control, card_id)?
            && lease.worktree_path.exists()
        {
            if !is_clean(&lease.worktree_path)? {
                return Err(HarnessError::Control {
                    reason: format!(
                        "worktree {} holds uncommitted or untracked work",
                        lease.worktree_path.display()
                    ),
                    code: ErrorCode::PreconditionWorktreeDirty,
                });
            }
            would_remove.push(lease.worktree_path.display().to_string());
        }
        if crate::git::worktree::branch_exists(&scope, &branch)? {
            would_remove.push(branch);
        }
    }

    Ok(CommandOutcome::new(
        "archive.close",
        format!(
            "Dry run: would remove {} item(s) closing {integration_id}\nnothing was changed",
            would_remove.len()
        ),
        serde_json::json!({
            "dry_run": true,
            "integration_id": integration_id.to_string(),
            "would_remove": would_remove,
            "changed": false,
        }),
    )
    .with_project(config.project_id))
}

fn run_close(args: &CommonArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let integration_id: IntegrationId = args.integration_id.parse()?;

    if args.dry_run {
        return preview_close(args, &integration_id);
    }

    with_transaction(
        &args.control,
        "archive.close",
        clock,
        |control, events, expected, steps| {
            let config = control.project()?;
            let mut record = load_integration(control, &integration_id)?;

            // Repeated close is a no-op rather than an error: an operator who
            // is unsure whether cleanup finished should be able to ask again.
            if record.status == IntegrationStatus::Archived {
                return Ok(CommandOutcome::new(
                    "archive.close",
                    format!("Integration {integration_id} is already closed; nothing to do"),
                    serde_json::json!({
                        "integration_id": integration_id.to_string(),
                        "status": IntegrationStatus::Archived.name(),
                        "removed": Vec::<String>::new(),
                        "changed": false,
                    }),
                )
                .with_project(config.project_id.clone()));
            }

            record
                .status
                .check_transition(IntegrationStatus::Archived)?;
            let archive = load_archive(control, &integration_id)?;
            // Cleanup rests on the archive being intact, so it is re-checked
            // here rather than assumed from the record's existence.
            for archived in &archive.refs {
                if let Some(defect) = verify(&config.repository, archived)? {
                    return Err(HarnessError::Control {
                        reason: format!(
                            "archived ref {} is {defect:?}; refusing to remove anything",
                            archived.reference
                        ),
                        code: ErrorCode::PolicyArchiveBroken,
                    });
                }
            }

            steps.outside_control("cleanup-started")?;
            let mut removed = Vec::new();
            for member in &record.members {
                removed.extend(clean_up_card(control, &config, &member.card_id)?);
                let (card, state) = load_card(control, &member.card_id)?;
                state.state.check_transition(CardState::Closed)?;
                store_card_state(control, &card, &state, CardState::Closed)?;
            }

            steps.at("cleanup-complete")?;
            record.status = IntegrationStatus::Archived;
            control.write_atomic(
                &IntegrationRecord::relative_path(&integration_id),
                &format!("{}\n", serde_json::to_string_pretty(&record)?),
            )?;

            events.append(
                &config.project_id,
                EventDraft::new("archive.closed", &args.actor_id)
                    .cycle(record.cycle_id.clone())
                    .transition(
                        Some(IntegrationStatus::Promoted.name()),
                        IntegrationStatus::Archived.name(),
                    )
                    .meta(
                        "integration_id",
                        serde_json::json!(integration_id.to_string()),
                    )
                    .meta("removed", serde_json::json!(removed)),
                clock,
            )?;
            control.commit(expected, &format!("archive: close {integration_id}"))?;

            Ok(CommandOutcome::new(
                "archive.close",
                format!(
                    "Closed {integration_id}\nremoved: {}\ncards closed: {}",
                    if removed.is_empty() {
                        "nothing".to_owned()
                    } else {
                        removed.join(", ")
                    },
                    record.members.len()
                ),
                serde_json::json!({
                    "integration_id": integration_id.to_string(),
                    "status": IntegrationStatus::Archived.name(),
                    "removed": removed,
                    "changed": true,
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}
