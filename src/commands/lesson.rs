//! Commands for proposing, authorizing, inspecting, and matching governed lessons.

use std::{fmt::Write as _, fs, path::PathBuf};

use clap::{Args, Subcommand};

use crate::{
    cli::output::CommandOutcome,
    commands::{
        CONTROL_ENV,
        acceptance::{
            FINAL_AUTHORIZATION_ACTOR_NOT_AUTHORIZED_RECOVERY,
            FINAL_AUTHORIZATION_POLICY_NOT_CONFIGURED_RECOVERY,
        },
        card::load_card,
        transaction::with_transaction,
    },
    config::ProjectConfig,
    control::{event_store::EventDraft, repository::ControlRepository},
    domain::{
        clock::Clock,
        digest::CANONICAL_ALGORITHM,
        ids::{CardId, LessonId},
        lesson::{
            LESSON_DIR, LESSON_SCHEMA, LessonDraft, LessonEnforcement, LessonObligations,
            LessonProvenance, LessonRecord, LessonSelectors, LessonStatus,
        },
    },
    error::{ErrorCode, HarnessError},
    policy::lessons::build_manifest,
};

/// Subcommands under `lesson`.
#[derive(Debug, Subcommand)]
pub enum LessonCommand {
    /// Propose a lesson from a YAML or JSON definition.
    Propose(ProposeArgs),
    /// Authorize a proposed lesson for future matching.
    Activate(StateArgs),
    /// Retire a lesson without deleting its history.
    Retire(StateArgs),
    /// List the latest revision of every lesson.
    List(CommonArgs),
    /// Show one lesson and its digest.
    Show(ShowArgs),
    /// Compute the exact lesson manifest for one activated card.
    Match(MatchArgs),
    /// Print a complete, valid lesson example.
    Example(ExampleArgs),
}

impl LessonCommand {
    /// Dotted command path used by the result envelope.
    #[must_use]
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Propose(..) => "lesson.propose",
            Self::Activate(..) => "lesson.activate",
            Self::Retire(..) => "lesson.retire",
            Self::List(..) => "lesson.list",
            Self::Show(..) => "lesson.show",
            Self::Match(..) => "lesson.match",
            Self::Example(..) => "lesson.example",
        }
    }
}

#[derive(Debug, Args)]
pub struct CommonArgs {
    #[arg(long, env = CONTROL_ENV)]
    pub control: PathBuf,
}

#[derive(Debug, Args)]
pub struct ProposeArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    #[arg(long)]
    pub definition: PathBuf,
    #[arg(long, default_value = "operator")]
    pub actor: String,
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct StateArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    #[arg(long)]
    pub lesson_id: String,
    #[arg(long, default_value = "operator")]
    pub actor: String,
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct ShowArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    #[arg(long)]
    pub lesson_id: String,
}

#[derive(Debug, Args)]
pub struct MatchArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    #[arg(long)]
    pub card_id: String,
}

#[derive(Debug, Args)]
pub struct ExampleArgs {}

/// # Errors
///
/// Returns a policy, precondition, or control-repository error.
pub fn execute(command: &LessonCommand, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    match command {
        LessonCommand::Propose(args) => run_propose(args, clock),
        LessonCommand::Activate(args) => run_state(args, LessonStatus::Active, clock),
        LessonCommand::Retire(args) => run_state(args, LessonStatus::Retired, clock),
        LessonCommand::List(args) => run_list(args),
        LessonCommand::Show(args) => run_show(args),
        LessonCommand::Match(args) => run_match(args),
        LessonCommand::Example(..) => run_example(),
    }
}

fn read_draft(path: &PathBuf) -> Result<LessonDraft, HarnessError> {
    let raw = fs::read_to_string(path).map_err(|source| HarnessError::ControlWithRecovery {
        reason: format!("cannot read lesson definition {}: {source}", path.display()),
        code: ErrorCode::ConfigMalformed,
        recovery: "Confirm the lesson definition path exists and compare it with `lesson example`.",
    })?;
    serde_yaml_ng::from_str(&raw).map_err(|source| HarnessError::ControlWithRecovery {
        reason: format!("lesson definition is malformed: {source}"),
        code: ErrorCode::ConfigMalformed,
        recovery: "Compare the definition with `lesson example`; selectors, provenance, and obligations are required.",
    })
}

fn corrupt_registry(reason: impl Into<String>) -> HarnessError {
    HarnessError::Control {
        reason: reason.into(),
        code: ErrorCode::InternalControlCorrupt,
    }
}

/// Loads and validates the complete immutable history for one lesson.
///
/// The directory layout is part of the registry contract. Silently skipping a
/// gap, a mismatched embedded identity, or a broken `supersedes` pointer would
/// let the latest file replace history while still looking like an ordinary
/// revision to every caller that asks only for the current lesson.
fn lesson_history(
    control: &ControlRepository,
    lesson_id: &LessonId,
) -> Result<Vec<LessonRecord>, HarnessError> {
    let relative_directory = format!("{LESSON_DIR}/{lesson_id}");
    let directory = control.path(&relative_directory);
    let mut revisions = Vec::new();
    for entry in fs::read_dir(&directory).map_err(|source| HarnessError::ControlIo {
        path: directory.clone(),
        source,
    })? {
        let entry = entry.map_err(|source| HarnessError::ControlIo {
            path: directory.clone(),
            source,
        })?;
        let path = entry.path();
        if !path.is_file() {
            return Err(corrupt_registry(format!(
                "lesson {lesson_id} registry contains unexpected entry {}",
                path.display()
            )));
        }
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            return Err(corrupt_registry(format!(
                "lesson {lesson_id} registry contains a non-UTF-8 revision name"
            )));
        };
        let Some(number) = name
            .strip_prefix('r')
            .and_then(|value| value.strip_suffix(".json"))
            .and_then(|value| value.parse::<u32>().ok())
        else {
            return Err(corrupt_registry(format!(
                "lesson {lesson_id} registry contains unexpected revision file `{name}`"
            )));
        };
        if name != format!("r{number}.json") {
            return Err(corrupt_registry(format!(
                "lesson {lesson_id} revision file `{name}` is not canonical"
            )));
        }
        let relative = LessonRecord::relative_path(lesson_id, number);
        let record: LessonRecord =
            serde_json::from_str(&control.read(&relative)?).map_err(|source| {
                corrupt_registry(format!(
                    "lesson {lesson_id} revision {number} is malformed: {source}"
                ))
            })?;
        record.validate()?;
        if record.lesson_id != *lesson_id || record.revision != number {
            return Err(corrupt_registry(format!(
                "lesson {lesson_id} revision file r{number}.json embeds lesson {} revision {}",
                record.lesson_id, record.revision
            )));
        }
        revisions.push(record);
    }
    revisions.sort_by_key(|record| record.revision);
    if revisions.is_empty() {
        return Err(corrupt_registry(format!(
            "lesson {lesson_id} registry contains no revisions"
        )));
    }
    for (index, record) in revisions.iter().enumerate() {
        let expected_revision = u32::try_from(index + 1)
            .map_err(|_| corrupt_registry(format!("lesson {lesson_id} has too many revisions")))?;
        let expected_supersedes = expected_revision.checked_sub(1).filter(|value| *value > 0);
        if record.revision != expected_revision || record.supersedes != expected_supersedes {
            return Err(corrupt_registry(format!(
                "lesson {lesson_id} revision chain is broken at r{}: expected revision {expected_revision} superseding {}, found revision {} superseding {}",
                record.revision,
                expected_supersedes
                    .map_or_else(|| "nothing".to_owned(), |value| format!("r{value}")),
                record.revision,
                record
                    .supersedes
                    .map_or_else(|| "nothing".to_owned(), |value| format!("r{value}")),
            )));
        }
    }
    if revisions[0].status != LessonStatus::Proposed {
        return Err(corrupt_registry(format!(
            "lesson {lesson_id} revision 1 must be proposed"
        )));
    }
    Ok(revisions)
}

pub(crate) fn all_lessons(control: &ControlRepository) -> Result<Vec<LessonRecord>, HarnessError> {
    let directory = control.path(LESSON_DIR);
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut latest = Vec::new();
    for entry in fs::read_dir(&directory).map_err(|source| HarnessError::ControlIo {
        path: directory.clone(),
        source,
    })? {
        let entry = entry.map_err(|source| HarnessError::ControlIo {
            path: directory.clone(),
            source,
        })?;
        if !entry.path().is_dir() {
            return Err(corrupt_registry(format!(
                "lesson registry contains unexpected entry {}",
                entry.path().display()
            )));
        }
        let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
            return Err(corrupt_registry(
                "lesson registry contains a non-UTF-8 lesson identifier",
            ));
        };
        let lesson_id: LessonId = name
            .parse()
            .map_err(|_| corrupt_registry(format!("invalid lesson registry id `{name}`")))?;
        let mut revisions = lesson_history(control, &lesson_id)?;
        latest.push(revisions.pop().expect("validated non-empty history"));
    }
    latest.sort_by(|left, right| left.lesson_id.cmp(&right.lesson_id));
    Ok(latest)
}

/// Verifies that every lesson revision embedded in a frozen manifest still
/// exists in the immutable registry and has not been rewritten.
///
/// The active-set comparison belongs to each lifecycle command because it
/// needs the card. This lower-level check catches a different failure: a
/// handoff or review packet whose lesson body, digest, or revision reference
/// was edited after it was issued.
pub(crate) fn validate_manifest_registry(
    control: &ControlRepository,
    manifest: &crate::domain::lesson::LessonManifest,
) -> Result<(), HarnessError> {
    let mut seen = std::collections::BTreeSet::new();
    for selected in &manifest.lessons {
        if !seen.insert((selected.lesson_id.clone(), selected.revision)) {
            return Err(HarnessError::Control {
                reason: format!(
                    "lesson manifest contains duplicate lesson {} revision {}",
                    selected.lesson_id, selected.revision
                ),
                code: ErrorCode::PolicyLessonManifestStale,
            });
        }
        let history = lesson_history(control, &selected.lesson_id).map_err(|error| {
            HarnessError::Control {
                reason: format!(
                    "lesson manifest references an invalid registry history for {}: {error}",
                    selected.lesson_id
                ),
                code: ErrorCode::PolicyLessonManifestStale,
            }
        })?;
        let record = history
            .iter()
            .find(|record| record.revision == selected.revision)
            .ok_or_else(|| HarnessError::Control {
                reason: format!(
                    "lesson manifest references missing lesson {} revision {}",
                    selected.lesson_id, selected.revision
                ),
                code: ErrorCode::PolicyLessonManifestStale,
            })?;
        if record.lesson_id != selected.lesson_id
            || record.revision != selected.revision
            || record.digest()? != selected.lesson_digest
            || record.enforcement != selected.enforcement
            || record.title != selected.title
            || record.rule != selected.rule
            || record.obligations != selected.obligations
        {
            return Err(HarnessError::Control {
                reason: format!(
                    "lesson manifest entry for {} revision {} does not match its immutable registry record",
                    selected.lesson_id, selected.revision
                ),
                code: ErrorCode::PolicyLessonManifestStale,
            });
        }
    }
    Ok(())
}

fn require_lesson_authorizer(
    config: &ProjectConfig,
    actor: &str,
    action: &str,
) -> Result<(), HarnessError> {
    let policy = config
        .final_authorization_policy
        .as_ref()
        .ok_or_else(|| HarnessError::ControlWithRecovery {
            reason: format!(
                "final authorization is not configured for this project; lesson {action} changes the policy applied to future cards"
            ),
            code: ErrorCode::PolicyNotAccepted,
            recovery: FINAL_AUTHORIZATION_POLICY_NOT_CONFIGURED_RECOVERY,
        })?;
    if policy.authorizes(actor) {
        return Ok(());
    }
    Err(HarnessError::ControlWithRecovery {
        reason: format!("actor {actor} is not configured to authorize lesson {action}"),
        code: ErrorCode::PolicyNotAccepted,
        recovery: FINAL_AUTHORIZATION_ACTOR_NOT_AUTHORIZED_RECOVERY,
    })
}

fn require_lesson_transition(
    previous: LessonStatus,
    next: LessonStatus,
    lesson_id: &LessonId,
) -> Result<(), HarnessError> {
    let permitted = matches!(
        (previous, next),
        (LessonStatus::Proposed, LessonStatus::Active)
            | (LessonStatus::Active, LessonStatus::Retired)
    );
    if permitted {
        return Ok(());
    }
    Err(HarnessError::Control {
        reason: format!("lesson {lesson_id} cannot transition from {previous:?} to {next:?}"),
        code: ErrorCode::PolicyLessonInvalid,
    })
}

fn authorized_lesson_transition(
    control: &ControlRepository,
    lesson_id: &LessonId,
    status: LessonStatus,
    actor: &str,
) -> Result<(ProjectConfig, LessonRecord), HarnessError> {
    let config = control.project()?.clone();
    let previous = latest_lesson(control, lesson_id)?;
    require_lesson_authorizer(
        &config,
        actor,
        if status == LessonStatus::Active {
            "activation"
        } else {
            "retirement"
        },
    )?;
    require_lesson_transition(previous.status, status, lesson_id)?;
    Ok((config, previous))
}

fn latest_lesson(
    control: &ControlRepository,
    lesson_id: &LessonId,
) -> Result<LessonRecord, HarnessError> {
    all_lessons(control)?
        .into_iter()
        .find(|record| record.lesson_id == *lesson_id)
        .ok_or_else(|| HarnessError::Control {
            reason: format!("lesson {lesson_id} is not registered"),
            code: ErrorCode::PreconditionNotFound,
        })
}

fn next_lesson_id(control: &ControlRepository) -> Result<LessonId, HarnessError> {
    let highest = all_lessons(control)?
        .into_iter()
        .filter_map(|record| {
            record
                .lesson_id
                .as_str()
                .strip_prefix("LS-")?
                .parse::<u64>()
                .ok()
        })
        .max()
        .unwrap_or(0);
    format!("LS-{:06}", highest + 1).parse()
}

fn record_from_draft(
    draft: &LessonDraft,
    lesson_id: LessonId,
    actor: &str,
    clock: &dyn Clock,
) -> LessonRecord {
    LessonRecord {
        schema: LESSON_SCHEMA.to_owned(),
        lesson_id,
        revision: 1,
        status: LessonStatus::Proposed,
        title: draft.title.clone(),
        rule: draft.rule.clone(),
        rationale: draft.rationale.clone(),
        selectors: draft.selectors.clone(),
        enforcement: draft.enforcement,
        obligations: draft.obligations.clone(),
        provenance: draft.provenance.clone(),
        created_by: actor.to_owned(),
        created_at: clock.now(),
        supersedes: None,
        canonical_algorithm: CANONICAL_ALGORITHM.to_owned(),
    }
}

fn run_propose(args: &ProposeArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let draft = read_draft(&args.definition)?;
    draft.validate()?;
    if args.dry_run {
        let control = ControlRepository::open(&args.common.control)?;
        let id = next_lesson_id(&control)?;
        return Ok(CommandOutcome::new(
            "lesson.propose",
            format!("Dry run: would propose lesson {id}; nothing was changed"),
            serde_json::json!({"dry_run": true, "lesson_id": id.to_string()}),
        ));
    }
    with_transaction(
        &args.common.control,
        "lesson.propose",
        clock,
        |control, events, expected, steps| {
            steps.at("control-write")?;
            let config = control.project()?;
            let record = record_from_draft(&draft, next_lesson_id(control)?, &args.actor, clock);
            let digest = record.digest()?;
            control.write_atomic(
                &LessonRecord::relative_path(&record.lesson_id, record.revision),
                &format!("{}\n", serde_json::to_string_pretty(&record)?),
            )?;
            events.append(
                &config.project_id,
                EventDraft::new("lesson.proposed", &args.actor)
                    .meta("lesson_id", serde_json::json!(record.lesson_id.to_string()))
                    .meta("lesson_digest", serde_json::json!(digest.as_str())),
                clock,
            )?;
            control.commit(expected, &format!("lesson: propose {}", record.lesson_id))?;
            Ok(CommandOutcome::new(
                "lesson.propose",
                format!(
                    "Proposed lesson {} revision {}\ndigest: {digest}",
                    record.lesson_id, record.revision
                ),
                serde_json::json!({"lesson": record, "digest": digest.as_str()}),
            )
            .with_project(config.project_id.clone()))
        },
    )
}

fn run_state(
    args: &StateArgs,
    status: LessonStatus,
    clock: &dyn Clock,
) -> Result<CommandOutcome, HarnessError> {
    let lesson_id: LessonId = args.lesson_id.parse()?;
    if args.dry_run {
        let control = ControlRepository::open(&args.common.control)?;
        let (_, current) = authorized_lesson_transition(&control, &lesson_id, status, &args.actor)?;
        return Ok(CommandOutcome::new(
            match status {
                LessonStatus::Active => "lesson.activate",
                LessonStatus::Retired => "lesson.retire",
                LessonStatus::Proposed => "lesson.state",
            },
            format!(
                "Dry run: would set lesson {lesson_id} revision {} to {:?}; nothing was changed",
                current.revision + 1,
                status
            ),
            serde_json::json!({"dry_run": true, "lesson_id": lesson_id.to_string(), "status": status}),
        ));
    }
    with_transaction(
        &args.common.control,
        if status == LessonStatus::Active {
            "lesson.activate"
        } else {
            "lesson.retire"
        },
        clock,
        |control, events, expected, steps| {
            let (config, previous) =
                authorized_lesson_transition(control, &lesson_id, status, &args.actor)?;
            steps.at("control-write")?;
            let mut next = previous.clone();
            next.revision += 1;
            next.status = status;
            next.supersedes = Some(previous.revision);
            next.created_by.clone_from(&args.actor);
            next.created_at = clock.now();
            next.validate()?;
            let digest = next.digest()?;
            control.write_atomic(
                &LessonRecord::relative_path(&lesson_id, next.revision),
                &format!("{}\n", serde_json::to_string_pretty(&next)?),
            )?;
            let event_name = if status == LessonStatus::Active {
                "lesson.activated"
            } else {
                "lesson.retired"
            };
            events.append(
                &config.project_id,
                EventDraft::new(event_name, &args.actor)
                    .meta("lesson_id", serde_json::json!(lesson_id.to_string()))
                    .meta("revision", serde_json::json!(next.revision))
                    .meta("lesson_digest", serde_json::json!(digest.as_str())),
                clock,
            )?;
            control.commit(
                expected,
                &format!(
                    "lesson: {} {}",
                    if status == LessonStatus::Active {
                        "activate"
                    } else {
                        "retire"
                    },
                    lesson_id
                ),
            )?;
            Ok(CommandOutcome::new(
                if status == LessonStatus::Active {
                    "lesson.activate"
                } else {
                    "lesson.retire"
                },
                format!(
                    "Lesson {lesson_id} is now {:?} at revision {}\ndigest: {digest}",
                    status, next.revision
                ),
                serde_json::json!({"lesson": next, "digest": digest.as_str()}),
            )
            .with_project(config.project_id.clone()))
        },
    )
}

fn run_list(args: &CommonArgs) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.control)?;
    let config = control.project()?;
    let lessons = all_lessons(&control)?;
    let mut text = format!("{} lesson(s)", lessons.len());
    for lesson in &lessons {
        let _ = write!(
            text,
            "\n{} r{} [{:?}] {}",
            lesson.lesson_id, lesson.revision, lesson.status, lesson.title
        );
    }
    Ok(
        CommandOutcome::new("lesson.list", text, serde_json::json!({"lessons": lessons}))
            .with_project(config.project_id.clone()),
    )
}

fn run_show(args: &ShowArgs) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.common.control)?;
    let config = control.project()?;
    let id: LessonId = args.lesson_id.parse()?;
    let lesson = latest_lesson(&control, &id)?;
    let digest = lesson.digest()?;
    Ok(CommandOutcome::new(
        "lesson.show",
        format!(
            "Lesson {id} revision {} [{:?}]\ndigest: {digest}\n{}",
            lesson.revision, lesson.status, lesson.rule
        ),
        serde_json::json!({"lesson": lesson, "digest": digest.as_str()}),
    )
    .with_project(config.project_id.clone()))
}

fn run_match(args: &MatchArgs) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.common.control)?;
    let config = control.project()?;
    let card_id: CardId = args.card_id.parse()?;
    let (card, state) = load_card(&control, &card_id)?;
    let manifest = build_manifest(&card, &all_lessons(&control)?)?;
    let digest = manifest.digest()?;
    Ok(CommandOutcome::new("lesson.match", format!("Card {card_id} revision {} matches {} lesson(s)\nmanifest: {digest}", state.current_revision, manifest.lessons.len()), serde_json::json!({"card": card_id.to_string(), "manifest": manifest, "manifest_digest": digest.as_str()})).with_project(config.project_id.clone()))
}

fn run_example() -> Result<CommandOutcome, HarnessError> {
    let draft = LessonDraft {
        title: "Carry exact review lessons forward".to_owned(),
        rule: "Read and disposition every applicable lesson before handoff".to_owned(),
        rationale: "Fresh agents otherwise repeat known mistakes".to_owned(),
        selectors: LessonSelectors {
            paths: vec!["src/**".to_owned()],
            ..LessonSelectors::default()
        },
        enforcement: LessonEnforcement::Required,
        obligations: LessonObligations {
            review_checks: vec!["lesson-read".to_owned()],
            ..LessonObligations::default()
        },
        provenance: LessonProvenance {
            source_kind: "review".to_owned(),
            source_id: "RV-000001".to_owned(),
            evidence: "Prior review found an omitted regression".to_owned(),
        },
    };
    let example = serde_yaml_ng::to_string(&draft).map_err(|source| HarnessError::Control {
        reason: format!("failed to render lesson example: {source}"),
        code: ErrorCode::InternalEncoding,
    })?;
    Ok(CommandOutcome::new(
        "lesson.example",
        example.clone(),
        serde_json::json!({"format": "yaml", "example": example}),
    ))
}
