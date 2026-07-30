//! Handoff commands: create, inspect, and revoke.

use std::{fmt::Write as _, fs, path::PathBuf};

use clap::{Args, Subcommand};

use crate::{
    cli::output::CommandOutcome,
    commands::CONTROL_ENV,
    commands::{
        card::{load_card, store_card_state},
        gate::{load_gate, receipts_for},
        review::dependency_standings,
        transaction::with_transaction,
        work::held_lease,
    },
    control::{event_store::EventDraft, repository::ControlRepository},
    domain::{
        card::CardState,
        clock::Clock,
        cycle::CycleRecord,
        digest::CANONICAL_ALGORITHM,
        handoff::{
            ActorDeclaration, DependencyBinding, EvidenceEntry, HANDOFF_DIR, HANDOFF_SCHEMA,
            HandoffRecord, HandoffStatus, check_delivered_sha,
        },
        ids::CardId,
    },
    error::{ErrorCode, HarnessError},
    git::{command::GitScope, inspect},
    runner::receipt::evidence_is_acceptable,
};

/// Subcommands under `handoff`.
#[derive(Debug, Subcommand)]
pub enum HandoffCommand {
    /// Bind the current candidate to a reviewable record.
    Create(CreateArgs),
    /// Show a card's handoff and whether it still applies.
    Inspect(CardArgs),
    /// Withdraw a handoff, returning the card to work.
    Revoke(RevokeArgs),
}

impl HandoffCommand {
    /// Its dotted command path, as the result envelope reports it.
    ///
    /// The error envelope used to carry only the group — `handoff` — while a
    /// success carried the full path, so a consumer matching on `command` got a
    /// different granularity depending on whether the command worked.
    #[must_use]
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Create(..) => "handoff.create",
            Self::Inspect(..) => "handoff.inspect",
            Self::Revoke(..) => "handoff.revoke",
        }
    }
}

/// Arguments shared by handoff subcommands.
#[derive(Debug, Args)]
pub struct CommonArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
    pub control: PathBuf,
    /// Identifies the acting party. Declared, not proven; see D-013.
    #[arg(long, default_value = "operator")]
    pub actor: String,
}

/// Arguments accepted by `handoff create`.
#[derive(Debug, Args)]
pub struct CreateArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The card to hand off.
    #[arg(long)]
    pub card_id: String,
    /// Path to the actor's declaration, in YAML or JSON.
    #[arg(long)]
    pub declaration: PathBuf,
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

/// Arguments accepted by `handoff revoke`.
#[derive(Debug, Args)]
pub struct RevokeArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The card whose handoff is withdrawn.
    #[arg(long)]
    pub card_id: String,
    /// Why it is being withdrawn.
    #[arg(long)]
    pub reason: String,
    /// Report planned mutations without performing them.
    #[arg(long)]
    pub dry_run: bool,
}

/// Executes a `handoff` subcommand.
///
/// # Errors
///
/// Returns a policy, precondition, or gate error as appropriate.
pub fn execute(
    command: &HandoffCommand,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    match command {
        HandoffCommand::Create(args) => run_create(args, clock),
        HandoffCommand::Inspect(args) => run_inspect(args),
        HandoffCommand::Revoke(args) => run_revoke(args, clock),
    }
}

/// Allocates the next handoff identifier.
///
/// Monotonic rather than derived from the candidate SHA. A derived identifier
/// would be deterministic but unordered, and "the latest handoff" is a question
/// this code asks constantly: sorting SHA-prefixed names returns whichever
/// candidate happened to hash lower, not the most recent one.
fn next_handoff_id(control: &ControlRepository) -> Result<String, HarnessError> {
    let directory = control.path(HANDOFF_DIR);
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
                    .and_then(|stem| stem.strip_prefix("H-"))
                    .and_then(|digits| digits.parse::<u64>().ok())
            })
            .max()
            .unwrap_or(0)
    } else {
        0
    };
    Ok(format!("H-{:06}", highest + 1))
}

/// The most recently written handoff for a card, if any.
///
/// # Errors
///
/// Returns an error when the store cannot be read.
pub fn latest_handoff(
    control: &ControlRepository,
    card_id: &CardId,
) -> Result<Option<HandoffRecord>, HarnessError> {
    let directory = control.path(HANDOFF_DIR);
    if !directory.exists() {
        return Ok(None);
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
    // Identifiers are zero-padded and monotonic, so lexical order is issue
    // order.
    names.sort();

    for name in names.iter().rev() {
        let raw = control.read(&HandoffRecord::relative_path(name))?;
        let record: HandoffRecord =
            serde_json::from_str(&raw).map_err(|source| HarnessError::Control {
                reason: format!("handoff {name} is malformed: {source}"),
                code: ErrorCode::InternalControlCorrupt,
            })?;
        if record.card_id == *card_id {
            return Ok(Some(record));
        }
    }
    Ok(None)
}

/// Every handoff written for a card, oldest first.
///
/// # Errors
///
/// Returns an error when the store cannot be read.
pub fn handoffs_for(
    control: &ControlRepository,
    card_id: &CardId,
) -> Result<Vec<HandoffRecord>, HarnessError> {
    let directory = control.path(HANDOFF_DIR);
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

    let mut records = Vec::new();
    for name in &names {
        let raw = control.read(&HandoffRecord::relative_path(name))?;
        let record: HandoffRecord =
            serde_json::from_str(&raw).map_err(|source| HarnessError::Control {
                reason: format!("handoff {name} is malformed: {source}"),
                code: ErrorCode::InternalControlCorrupt,
            })?;
        if record.card_id == *card_id {
            records.push(record);
        }
    }
    Ok(records)
}

/// Whether `ancestor` is in `descendant`'s history, or `None` if unanswerable.
///
/// An object Git no longer has is not an error here, and it is not an answer
/// either. A card's candidate can become unreachable once its branch is deleted
/// and its objects collected, and the two callers of this want opposite
/// defaults in that case — binding a dependency treats it as not incorporated,
/// checking one treats it as not superseded — so the ambiguity is returned
/// rather than resolved here.
///
/// # Errors
///
/// Returns an error when `is_ancestor` itself fails. Note that a missing object
/// cannot reach that path: `object_type` reports a Git execution failure the
/// same way it reports an absent object, so both collapse into `None` above.
/// That is the conservative direction for both callers — neither invalidates on
/// an unanswerable question — but it means a genuinely broken Git is reported as
/// "cannot say" rather than as a failure. A reviewer identified this; it is
/// recorded rather than papered over, because tightening it would mean
/// distinguishing the two inside `object_type`, which is a wider change than
/// this card owns.
pub(crate) fn ancestry(
    scope: &GitScope,
    ancestor: &str,
    descendant: &str,
) -> Result<Option<bool>, HarnessError> {
    if inspect::object_type(scope, ancestor).is_err()
        || inspect::object_type(scope, descendant).is_err()
    {
        return Ok(None);
    }
    inspect::is_ancestor(scope, ancestor, descendant).map(Some)
}

/// Binds each declared dependency to the commit of it this candidate holds.
///
/// Section 10.7's `dependency SHAs`. The question asked is deliberately "which
/// commit of the dependency is inside this candidate", not "which commit is the
/// dependency approved at". A dependent branched from the cycle baseline
/// incorporates nothing and binds `None`, and stays valid however often its
/// dependency is re-reviewed; a dependent branched from — or merged with — the
/// dependency's candidate binds that exact commit, and goes stale when the
/// dependency is re-approved somewhere else, because the candidate then carries
/// a superseded version of code that is about to land twice.
///
/// Handoffs are searched newest first so the binding is the most recent version
/// of the dependency the candidate actually has.
fn resolve_dependency_bindings(
    control: &ControlRepository,
    scope: &GitScope,
    depends_on: &[CardId],
    candidate_sha: &str,
) -> Result<Vec<DependencyBinding>, HarnessError> {
    let mut ordered: Vec<CardId> = depends_on.to_vec();
    ordered.sort();
    ordered.dedup();

    let mut bindings = Vec::with_capacity(ordered.len());
    for card_id in ordered {
        let mut incorporated_sha = None;
        for handoff in handoffs_for(control, &card_id)?.iter().rev() {
            if ancestry(scope, &handoff.candidate_sha, candidate_sha)?.unwrap_or(false) {
                incorporated_sha = Some(handoff.candidate_sha.clone());
                break;
            }
        }
        bindings.push(DependencyBinding {
            card_id,
            incorporated_sha,
        });
    }
    Ok(bindings)
}

/// Reads and parses an actor declaration.
fn read_declaration(path: &PathBuf) -> Result<ActorDeclaration, HarnessError> {
    let raw = fs::read_to_string(path).map_err(|source| HarnessError::Control {
        reason: format!("cannot read declaration {}: {source}", path.display()),
        code: ErrorCode::ConfigMalformed,
    })?;
    serde_yaml_ng::from_str(&raw).map_err(|source| HarnessError::Control {
        reason: format!("handoff declaration is malformed: {source}"),
        code: ErrorCode::ConfigMalformed,
    })
}

/// Collects the gate evidence in force for a candidate, refusing if it is not.
///
/// Section 10.7: handoff creation fails when required gates are stale or
/// missing. The check is here rather than at review time because a handoff is
/// supposed to be the package a reviewer can trust without re-deriving it.
fn collect_evidence(
    control: &ControlRepository,
    card_id: &CardId,
    gates: &[String],
    candidate_sha: &str,
) -> Result<Vec<EvidenceEntry>, HarnessError> {
    let receipts = receipts_for(control, card_id)?;
    let mut evidence = Vec::new();

    for gate_id in gates {
        let gate = load_gate(control, gate_id)?;
        let gate_digest = gate.digest()?;
        let current: Vec<_> = receipts
            .iter()
            .filter(|receipt| {
                receipt.gate_id == *gate_id && receipt.is_current_for(candidate_sha, &gate_digest)
            })
            .cloned()
            .collect();

        if !evidence_is_acceptable(&current, gate.retry_policy.max_attempts) {
            let reason = receipts
                .iter()
                .rfind(|receipt| receipt.gate_id == *gate_id)
                .and_then(|receipt| receipt.staleness(candidate_sha, &gate_digest))
                .unwrap_or_else(|| format!("no passing run of `{gate_id}` for this candidate"));
            return Err(HarnessError::Control {
                reason: format!("required gate `{gate_id}` is not satisfied: {reason}"),
                code: ErrorCode::GateEvidenceStale,
            });
        }

        for receipt in current.iter().filter(|receipt| receipt.passed) {
            evidence.push(EvidenceEntry {
                gate_id: receipt.gate_id.clone(),
                gate_digest: receipt.gate_digest.clone(),
                receipt_id: receipt.receipt_id.to_string(),
                passed: receipt.passed,
                evaluated_sha: receipt.evaluated_sha.clone(),
            });
        }
    }
    Ok(evidence)
}

/// Reports what `handoff create` would bind, without binding it.
fn preview_create(
    args: &CreateArgs,
    card_id: &CardId,
    declaration: &ActorDeclaration,
) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.common.control)?;
    let lease = held_lease(&control, card_id)?.ok_or_else(|| HarnessError::Control {
        reason: format!("card {card_id} holds no lease"),
        code: ErrorCode::PreconditionNotFound,
    })?;
    let head = inspect::resolve_commit(&GitScope::work_tree(&lease.worktree_path), "HEAD")?;
    check_delivered_sha(&declaration.delivered_sha, &head)?;
    Ok(CommandOutcome::new(
        "handoff.create",
        format!("Dry run: would hand off card {card_id} at {head}; nothing was changed"),
        serde_json::json!({
            "dry_run": true,
            "card_id": card_id.to_string(),
            "candidate_sha": head,
        }),
    ))
}

/// Gathers the machine-computed half of a handoff from Git objects.
///
/// Separate from the actor's declaration on purpose: this half is derived and
/// trustworthy, that half is a claim. `SPIKE-001` finding F-5 showed a
/// declaration asserting behavior the code did not have.
fn derive_facts(
    scope: &GitScope,
    base_sha: &str,
    candidate_sha: &str,
) -> Result<(String, Vec<String>, Vec<crate::git::diff::ChangedPath>), HarnessError> {
    let baseline = inspect::resolve_commit(scope, base_sha)?;
    let diff = crate::git::diff::diff_commits(scope, &baseline, candidate_sha)?;
    let commits: Vec<String> = inspect::raw(
        scope,
        [
            "log",
            "--format=%H",
            "--reverse",
            &format!("{baseline}..{candidate_sha}"),
        ],
    )?
    .trimmed_stdout()
    .lines()
    .map(ToOwned::to_owned)
    .collect();
    Ok((baseline, commits, diff.paths))
}

/// Resolves the candidate a handoff would describe, refusing if it cannot stand.
///
/// The `delivered_sha` check runs before every other precondition, because a
/// branch that moved after delivery invalidates the whole exercise: everything
/// downstream would describe code the actor did not produce.
fn candidate_of(
    control: &ControlRepository,
    card_id: &CardId,
    delivered_sha: &str,
) -> Result<(crate::domain::lease::LeaseRecord, GitScope, String), HarnessError> {
    let lease = held_lease(control, card_id)?.ok_or_else(|| HarnessError::Control {
        reason: format!("card {card_id} holds no lease; run `work start` first"),
        code: ErrorCode::PreconditionNotFound,
    })?;
    let scope = GitScope::work_tree(&lease.worktree_path);
    let candidate_sha = inspect::resolve_commit(&scope, "HEAD")?;

    // SPIKE-001 F-1: before anything else, confirm the branch still holds what
    // the actor says they delivered.
    check_delivered_sha(delivered_sha, &candidate_sha)?;

    let worktree_state = inspect::worktree_state(&scope)?;
    if !worktree_state.clean {
        return Err(HarnessError::Control {
            reason: format!(
                "worktree {} has uncommitted or untracked content: {}",
                lease.worktree_path.display(),
                worktree_state.dirty_paths.join(", ")
            ),
            code: ErrorCode::PreconditionWorktreeDirty,
        });
    }
    Ok((lease, scope, candidate_sha))
}

fn run_create(args: &CreateArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;
    let declaration = read_declaration(&args.declaration)?;
    declaration.validate()?;

    if args.dry_run {
        return preview_create(args, &card_id, &declaration);
    }

    with_transaction(
        &args.common.control,
        "handoff.create",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let config = control.project()?;
            let (record, state) = load_card(control, &card_id)?;
            state.state.check_transition(CardState::HandedOff)?;

            let (lease, scope, candidate_sha) =
                candidate_of(control, &card_id, &declaration.delivered_sha)?;

            let cycle: CycleRecord =
                serde_json::from_str(&control.read(&CycleRecord::relative_path(&record.cycle_id))?)
                    .map_err(|source| HarnessError::Control {
                        reason: format!("cycle {} is malformed: {source}", record.cycle_id),
                        code: ErrorCode::InternalControlCorrupt,
                    })?;

            let (baseline, commits, changed_paths) =
                derive_facts(&scope, &record.base_sha, &candidate_sha)?;

            let receipts = collect_evidence(
                control,
                &card_id,
                &record.named_gates.feature,
                &candidate_sha,
            )?;
            let dependency_bindings =
                resolve_dependency_bindings(control, &scope, &record.depends_on, &candidate_sha)?;

            let id = next_handoff_id(control)?;
            let handoff = HandoffRecord {
                schema: HANDOFF_SCHEMA.to_owned(),
                handoff_id: id.clone(),
                card_id: card_id.clone(),
                card_revision: state.current_revision,
                card_digest: state.current_digest.clone(),
                cycle_id: record.cycle_id.clone(),
                baseline_sha: cycle.baseline_sha.clone().unwrap_or(baseline),
                branch: lease.branch.clone(),
                candidate_sha: candidate_sha.clone(),
                commits,
                dependency_bindings,
                changed_paths,
                receipts,
                worktree_clean: true,
                declaration: declaration.clone(),
                actor_id: args.common.actor.clone(),
                created_at: clock.now(),
                status: HandoffStatus::Active,
                canonical_algorithm: CANONICAL_ALGORITHM.to_owned(),
            };
            let digest = handoff.digest()?;

            control.write_atomic(
                &HandoffRecord::relative_path(&id),
                &format!("{}\n", serde_json::to_string_pretty(&handoff)?),
            )?;
            store_card_state(control, &record, &state, CardState::HandedOff)?;

            events.append(
                &config.project_id,
                EventDraft::new("handoff.created", &args.common.actor)
                    .cycle(record.cycle_id.clone())
                    .card(
                        card_id.clone(),
                        state.current_revision,
                        state.current_digest.clone(),
                    )
                    .transition(Some(state.state.name()), CardState::HandedOff.name())
                    .head(candidate_sha.clone())
                    .meta("handoff_id", serde_json::json!(id))
                    .meta("handoff_digest", serde_json::json!(digest.as_str()))
                    .meta(
                        "delivered_sha",
                        serde_json::json!(declaration.delivered_sha),
                    ),
                clock,
            )?;
            control.commit(expected, &format!("handoff: create {id}"))?;

            Ok(CommandOutcome::new(
                "handoff.create",
                format!(
                    "Handed off card {card_id}\nhandoff: {id}\ncandidate: {candidate_sha}\ndigest: {digest}\nchanged paths: {}\nevidence: {} receipt(s)",
                    handoff.changed_paths.len(),
                    handoff.receipts.len()
                ),
                serde_json::json!({
                    "handoff": handoff,
                    "handoff_digest": digest.as_str(),
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}

fn run_inspect(args: &CardArgs) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;
    let control = ControlRepository::open(&args.common.control)?;
    let config = control.project()?;
    let (record, state) = load_card(&control, &card_id)?;

    let handoff = latest_handoff(&control, &card_id)?.ok_or_else(|| HarnessError::Control {
        reason: format!("card {card_id} has no handoff"),
        code: ErrorCode::PreconditionNotFound,
    })?;

    let current_candidate = held_lease(&control, &card_id)?.and_then(|lease| {
        inspect::resolve_commit(&GitScope::work_tree(&lease.worktree_path), "HEAD").ok()
    });
    let standings = dependency_standings(
        &control,
        &GitScope::work_tree(&config.repository),
        &record.depends_on,
        &handoff.dependency_bindings,
    )?;
    let staleness = current_candidate
        .as_ref()
        .and_then(|sha| handoff.staleness(sha, &state.current_digest, &standings));

    let mut text = format!(
        "Handoff {} for card {card_id}\ncandidate: {}\nbranch: {}\ndigest: {}\ncommits: {}\nchanged paths: {}\nevidence: {}\nstatus: {}",
        handoff.handoff_id,
        handoff.candidate_sha,
        handoff.branch,
        handoff.digest()?,
        handoff.commits.len(),
        handoff.changed_paths.len(),
        handoff.receipts.len(),
        if staleness.is_none() {
            "current"
        } else {
            "stale"
        }
    );
    if let Some(reason) = &staleness {
        let _ = write!(text, "\nstale because: {reason}");
    }

    let mut outcome = CommandOutcome::new(
        "handoff.inspect",
        text,
        serde_json::json!({
            "handoff": handoff,
            "handoff_digest": handoff.digest()?.as_str(),
            "current_candidate_sha": current_candidate,
            "is_current": staleness.is_none(),
            "stale_reason": staleness,
        }),
    )
    .with_project(config.project_id.clone());

    if staleness.is_some() {
        outcome = outcome.with_warning(
            "this handoff no longer describes the branch; a review recorded against it would be reviewing different code".to_owned(),
        );
    }
    Ok(outcome)
}

fn run_revoke(args: &RevokeArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;

    if args.dry_run {
        let control = ControlRepository::open(&args.common.control)?;
        let handoff = latest_handoff(&control, &card_id)?.ok_or_else(|| HarnessError::Control {
            reason: format!("card {card_id} has no handoff"),
            code: ErrorCode::PreconditionNotFound,
        })?;
        return Ok(CommandOutcome::new(
            "handoff.revoke",
            format!(
                "Dry run: would revoke handoff {}; nothing was changed",
                handoff.handoff_id
            ),
            serde_json::json!({ "dry_run": true, "handoff_id": handoff.handoff_id }),
        ));
    }

    with_transaction(
        &args.common.control,
        "handoff.revoke",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let config = control.project()?;
            let (record, state) = load_card(control, &card_id)?;
            let mut handoff =
                latest_handoff(control, &card_id)?.ok_or_else(|| HarnessError::Control {
                    reason: format!("card {card_id} has no handoff to revoke"),
                    code: ErrorCode::PreconditionNotFound,
                })?;

            if handoff.status == HandoffStatus::Revoked {
                return Err(HarnessError::Control {
                    reason: format!("handoff {} is already revoked", handoff.handoff_id),
                    code: ErrorCode::PolicyInvalidTransition,
                });
            }
            state.state.check_transition(CardState::Active)?;

            handoff.status = HandoffStatus::Revoked;
            control.write_atomic(
                &HandoffRecord::relative_path(&handoff.handoff_id),
                &format!("{}\n", serde_json::to_string_pretty(&handoff)?),
            )?;
            store_card_state(control, &record, &state, CardState::Active)?;

            events.append(
                &config.project_id,
                EventDraft::new("handoff.revoked", &args.common.actor)
                    .cycle(record.cycle_id.clone())
                    .card(
                        card_id.clone(),
                        state.current_revision,
                        state.current_digest.clone(),
                    )
                    .transition(Some(state.state.name()), CardState::Active.name())
                    .meta("handoff_id", serde_json::json!(handoff.handoff_id))
                    .meta("reason", serde_json::json!(args.reason)),
                clock,
            )?;
            control.commit(expected, &format!("handoff: revoke {}", handoff.handoff_id))?;

            Ok(CommandOutcome::new(
                "handoff.revoke",
                format!(
                    "Revoked handoff {}\nreason: {}\ncard {card_id} returns to active work",
                    handoff.handoff_id, args.reason
                ),
                serde_json::json!({
                    "handoff_id": handoff.handoff_id,
                    "card_id": card_id.to_string(),
                    "state": CardState::Active.name(),
                    "reason": args.reason,
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}
