//! Card lifecycle commands.

use std::{fs, path::PathBuf};

use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

use crate::{
    cli::output::CommandOutcome,
    commands::transaction::with_transaction,
    control::{event_store::EventDraft, repository::ControlRepository},
    domain::{
        card::{CARD_DIR, CardDraft, CardRecord, CardState},
        clock::Clock,
        cycle::CycleRecord,
        digest::Digest,
        ids::CardId,
    },
    error::{ErrorCode, HarnessError},
};

/// Schema identifier for a card's mutable state file.
pub const CARD_STATE_SCHEMA: &str = "harness.card-state/v1";

/// The mutable pointer to a card's current immutable revision.
///
/// Kept separate from the revision records so the records themselves are never
/// rewritten. Section 7.3.3 makes an activated card immutable; a state field
/// living inside it would break that.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CardStateRecord {
    /// Always [`CARD_STATE_SCHEMA`].
    pub schema: String,
    /// Identifies the card.
    pub card_id: CardId,
    /// Where the card sits in its lifecycle.
    pub state: CardState,
    /// The revision currently in force.
    pub current_revision: u32,
    /// Digest of the revision currently in force.
    pub current_digest: Digest,
    /// The canonicalization algorithm the digest was computed under.
    pub canonical_algorithm: String,
}

impl CardStateRecord {
    /// Relative path of a card's state file.
    #[must_use]
    pub fn relative_path(card_id: &CardId) -> String {
        format!("{CARD_DIR}/{card_id}/state.json")
    }
}

/// Subcommands under `card`.
#[derive(Debug, Subcommand)]
pub enum CardCommand {
    /// Validate a draft without storing it.
    Validate(DraftArgs),
    /// Store a draft against a cycle.
    Create(CreateArgs),
    /// Freeze a draft into an immutable revision.
    Activate(ActivateArgs),
    /// Supersede the current revision with a new one.
    Revise(ReviseArgs),
    /// Report a card's state, revision, and digest.
    Status(StatusArgs),
}

/// Arguments shared by card subcommands that touch control state.
#[derive(Debug, Args)]
pub struct CommonArgs {
    /// Path to the control repository.
    #[arg(long)]
    pub control: PathBuf,
    /// Identifies the acting party. Declared, not proven; see D-013.
    #[arg(long, default_value = "operator")]
    pub actor: String,
}

/// Arguments accepted by `card validate`.
#[derive(Debug, Args)]
pub struct DraftArgs {
    /// Path to the draft, in YAML or JSON.
    #[arg(long)]
    pub draft: PathBuf,
}

/// Arguments accepted by `card create`.
#[derive(Debug, Args)]
pub struct CreateArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// Path to the draft, in YAML or JSON.
    #[arg(long)]
    pub draft: PathBuf,
    /// Report planned mutations without performing them.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `card activate`.
#[derive(Debug, Args)]
pub struct ActivateArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The card to activate.
    #[arg(long)]
    pub card_id: String,
    /// Report planned mutations without performing them.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `card revise`.
#[derive(Debug, Args)]
pub struct ReviseArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The card to revise.
    #[arg(long)]
    pub card_id: String,
    /// Path to the replacement draft.
    #[arg(long)]
    pub draft: PathBuf,
    /// Why the card is being revised.
    #[arg(long)]
    pub reason: String,
    /// Report planned mutations without performing them.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments accepted by `card status`.
#[derive(Debug, Args)]
pub struct StatusArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// The card to report on.
    #[arg(long)]
    pub card_id: String,
}

/// Executes a `card` subcommand.
///
/// # Errors
///
/// Returns a policy, precondition, or configuration error as appropriate.
pub fn execute(command: &CardCommand, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    match command {
        CardCommand::Validate(args) => run_validate(args),
        CardCommand::Create(args) => run_create(args, clock),
        CardCommand::Activate(args) => run_activate(args, clock),
        CardCommand::Revise(args) => run_revise(args, clock),
        CardCommand::Status(args) => run_status(args),
    }
}

/// Reads and parses a draft file.
fn read_draft(path: &PathBuf) -> Result<CardDraft, HarnessError> {
    let raw = fs::read_to_string(path).map_err(|source| HarnessError::Control {
        reason: format!("cannot read draft {}: {source}", path.display()),
        code: ErrorCode::ConfigMalformed,
    })?;
    CardDraft::parse(&raw)
}

/// Relative path of a stored draft.
fn draft_path(card_id: &CardId) -> String {
    format!("{CARD_DIR}/{card_id}/draft.json")
}

/// Reads the stored draft for a card.
fn stored_draft(control: &ControlRepository, card_id: &CardId) -> Result<CardDraft, HarnessError> {
    let relative = draft_path(card_id);
    if !control.path(&relative).exists() {
        return Err(HarnessError::Control {
            reason: format!("card {card_id} has no stored draft; run `card create` first"),
            code: ErrorCode::PreconditionNotFound,
        });
    }
    CardDraft::parse(&control.read(&relative)?)
}

/// Reads a card's state file.
fn state_of(
    control: &ControlRepository,
    card_id: &CardId,
) -> Result<Option<CardStateRecord>, HarnessError> {
    let relative = CardStateRecord::relative_path(card_id);
    if !control.path(&relative).exists() {
        return Ok(None);
    }
    let raw = control.read(&relative)?;
    serde_json::from_str(&raw)
        .map(Some)
        .map_err(|source| HarnessError::Control {
            reason: format!("card {card_id} state is malformed: {source}"),
            code: ErrorCode::InternalControlCorrupt,
        })
}

/// Loads the cycle a draft names, requiring it to accept cards.
fn cycle_accepting_cards(
    control: &ControlRepository,
    draft: &CardDraft,
) -> Result<CycleRecord, HarnessError> {
    let relative = CycleRecord::relative_path(&draft.cycle_id);
    if !control.path(&relative).exists() {
        return Err(HarnessError::Control {
            reason: format!("cycle {} does not exist", draft.cycle_id),
            code: ErrorCode::PreconditionNotFound,
        });
    }
    let cycle: CycleRecord = serde_json::from_str(&control.read(&relative)?).map_err(|source| {
        HarnessError::Control {
            reason: format!("cycle {} is malformed: {source}", draft.cycle_id),
            code: ErrorCode::InternalControlCorrupt,
        }
    })?;
    if !cycle.status.accepts_cards() {
        return Err(HarnessError::Control {
            reason: format!(
                "cycle {} is `{}` and cannot accept new cards",
                cycle.cycle_id, cycle.status
            ),
            code: ErrorCode::PolicyInvalidTransition,
        });
    }
    Ok(cycle)
}

fn run_validate(args: &DraftArgs) -> Result<CommandOutcome, HarnessError> {
    let draft = read_draft(&args.draft)?;
    draft.validate()?;
    Ok(CommandOutcome::new(
        "card.validate",
        format!(
            "Draft for card {} is valid\ncycle: {}\nrisk: {}\nwrite scope: {}",
            draft.card_id,
            draft.cycle_id,
            draft.risk.name(),
            draft.write_scope.include.join(", ")
        ),
        serde_json::json!({
            "card_id": draft.card_id.to_string(),
            "cycle_id": draft.cycle_id.to_string(),
            "risk": draft.risk.name(),
            "valid": true,
        }),
    ))
}

fn run_create(args: &CreateArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let draft = read_draft(&args.draft)?;
    draft.validate()?;

    if args.dry_run {
        let control = ControlRepository::open(&args.common.control)?;
        cycle_accepting_cards(&control, &draft)?;
        return Ok(CommandOutcome::new(
            "card.create",
            format!(
                "Dry run: would store a draft for card {} in cycle {}; nothing was changed",
                draft.card_id, draft.cycle_id
            ),
            serde_json::json!({ "dry_run": true, "card_id": draft.card_id.to_string() }),
        ));
    }

    with_transaction(
        &args.common.control,
        "card.create",
        clock,
        |control, events, expected| {
            let cycle = cycle_accepting_cards(control, &draft)?;
            if let Some(state) = state_of(control, &draft.card_id)? {
                return Err(HarnessError::Control {
                    reason: format!(
                        "card {} already exists at revision {} in state `{}`; use `card revise`",
                        draft.card_id, state.current_revision, state.state
                    ),
                    code: ErrorCode::PolicyInvalidCard,
                });
            }

            let config = control.project()?;
            control.write_atomic(
                &draft_path(&draft.card_id),
                &format!("{}\n", serde_json::to_string_pretty(&draft)?),
            )?;
            events.append(
                &config.project_id,
                EventDraft::new("card.created", &args.common.actor)
                    .cycle(cycle.cycle_id.clone())
                    .transition(None::<String>, CardState::Draft.name()),
                clock,
            )?;
            control.commit(expected, &format!("card: create {}", draft.card_id))?;

            Ok(CommandOutcome::new(
                "card.create",
                format!(
                    "Created draft for card {} in cycle {}",
                    draft.card_id, draft.cycle_id
                ),
                serde_json::json!({
                    "card_id": draft.card_id.to_string(),
                    "cycle_id": draft.cycle_id.to_string(),
                    "state": CardState::Draft.name(),
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}

fn run_activate(args: &ActivateArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;

    if args.dry_run {
        let control = ControlRepository::open(&args.common.control)?;
        let draft = stored_draft(&control, &card_id)?;
        draft.validate()?;
        let preview = CardRecord::activate(&draft, 1, &args.common.actor, clock.now())?;
        return Ok(CommandOutcome::new(
            "card.activate",
            format!(
                "Dry run: would activate card {card_id} at revision 1 with digest {}; nothing was changed",
                preview.digest()?
            ),
            serde_json::json!({
                "dry_run": true,
                "card_id": card_id.to_string(),
                "revision": 1,
                "digest": preview.digest()?.as_str(),
            }),
        ));
    }

    with_transaction(
        &args.common.control,
        "card.activate",
        clock,
        |control, events, expected| {
            if let Some(state) = state_of(control, &card_id)? {
                return Err(HarnessError::Control {
                    reason: format!(
                        "card {card_id} is already activated at revision {}; an activated card is immutable, use `card revise`",
                        state.current_revision
                    ),
                    code: ErrorCode::PolicyInvalidCard,
                });
            }
            let draft = stored_draft(control, &card_id)?;
            let cycle = cycle_accepting_cards(control, &draft)?;
            let config = control.project()?;

            let record = CardRecord::activate(&draft, 1, &args.common.actor, clock.now())?;
            let digest = record.digest()?;
            write_revision(control, &record, &digest, CardState::Ready)?;

            // The cycle records its membership, which is how overlap checks in
            // WP-220 find the cards to compare against.
            let mut updated = cycle;
            updated.declare_card(card_id.clone())?;
            control.write_atomic(
                &CycleRecord::relative_path(&updated.cycle_id),
                &format!("{}\n", serde_json::to_string_pretty(&updated)?),
            )?;

            events.append(
                &config.project_id,
                EventDraft::new("card.activated", &args.common.actor)
                    .cycle(updated.cycle_id.clone())
                    .card(card_id.clone(), 1, digest.clone())
                    .transition(Some(CardState::Draft.name()), CardState::Ready.name()),
                clock,
            )?;
            control.commit(expected, &format!("card: activate {card_id} r1"))?;

            Ok(CommandOutcome::new(
                "card.activate",
                format!(
                    "Activated card {card_id} at revision 1\ndigest: {digest}\nstate: {}",
                    CardState::Ready
                ),
                serde_json::json!({
                    "card_id": card_id.to_string(),
                    "revision": 1,
                    "digest": digest.as_str(),
                    "state": CardState::Ready.name(),
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}

/// Writes an immutable revision and updates the mutable state pointer.
fn write_revision(
    control: &ControlRepository,
    record: &CardRecord,
    digest: &Digest,
    state: CardState,
) -> Result<(), HarnessError> {
    let relative = CardRecord::relative_path(&record.card_id, record.revision);
    if control.path(&relative).exists() {
        return Err(HarnessError::Control {
            reason: format!(
                "revision {} of card {} already exists; an activated revision is never rewritten",
                record.revision, record.card_id
            ),
            code: ErrorCode::PolicyInvalidCard,
        });
    }
    control.write_atomic(
        &relative,
        &format!("{}\n", serde_json::to_string_pretty(record)?),
    )?;
    control.write_atomic(
        &CardStateRecord::relative_path(&record.card_id),
        &format!(
            "{}\n",
            serde_json::to_string_pretty(&CardStateRecord {
                schema: CARD_STATE_SCHEMA.to_owned(),
                card_id: record.card_id.clone(),
                state,
                current_revision: record.revision,
                current_digest: digest.clone(),
                canonical_algorithm: CardRecord::canonical_algorithm().to_owned(),
            })?
        ),
    )
}

fn run_revise(args: &ReviseArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;
    let draft = read_draft(&args.draft)?;
    draft.validate()?;

    if args.dry_run {
        let control = ControlRepository::open(&args.common.control)?;
        let state = state_of(&control, &card_id)?.ok_or_else(|| HarnessError::Control {
            reason: format!("card {card_id} is not activated"),
            code: ErrorCode::PreconditionNotFound,
        })?;
        return Ok(CommandOutcome::new(
            "card.revise",
            format!(
                "Dry run: would supersede card {card_id} revision {} with revision {}; nothing was changed",
                state.current_revision,
                state.current_revision + 1
            ),
            serde_json::json!({
                "dry_run": true,
                "card_id": card_id.to_string(),
                "superseded_revision": state.current_revision,
            }),
        ));
    }

    with_transaction(
        &args.common.control,
        "card.revise",
        clock,
        |control, events, expected| {
            let state = state_of(control, &card_id)?.ok_or_else(|| HarnessError::Control {
                reason: format!("card {card_id} is not activated; use `card activate`"),
                code: ErrorCode::PreconditionNotFound,
            })?;
            if state.state.is_terminal() {
                return Err(HarnessError::Control {
                    reason: format!("card {card_id} is `{}` and cannot be revised", state.state),
                    code: ErrorCode::PolicyInvalidTransition,
                });
            }

            let previous: CardRecord = serde_json::from_str(
                &control.read(&CardRecord::relative_path(&card_id, state.current_revision))?,
            )
            .map_err(|source| HarnessError::Control {
                reason: format!("card {card_id} revision record is malformed: {source}"),
                code: ErrorCode::InternalControlCorrupt,
            })?;

            let config = control.project()?;
            let record = previous.revise(&draft, &args.common.actor, clock.now())?;
            let digest = record.digest()?;
            // A revision returns the card to `ready`: its definition changed, so
            // any work, handoff, or review bound to the old digest no longer
            // describes it. Invariant 7.3.4.
            write_revision(control, &record, &digest, CardState::Ready)?;

            events.append(
                &config.project_id,
                EventDraft::new("card.revised", &args.common.actor)
                    .cycle(record.cycle_id.clone())
                    .card(card_id.clone(), record.revision, digest.clone())
                    .transition(Some(state.state.name()), CardState::Ready.name())
                    .meta("reason", serde_json::json!(args.reason))
                    .meta(
                        "invalidated_revision",
                        serde_json::json!(state.current_revision),
                    )
                    .meta(
                        "invalidated_digest",
                        serde_json::json!(state.current_digest.as_str()),
                    ),
                clock,
            )?;
            control.commit(
                expected,
                &format!("card: revise {card_id} to r{}", record.revision),
            )?;

            Ok(CommandOutcome::new(
                "card.revise",
                format!(
                    "Revised card {card_id} to revision {}\ndigest: {digest}\nsuperseded revision {} with digest {}\nany handoff, review, or receipt bound to the superseded digest is now stale",
                    record.revision, state.current_revision, state.current_digest
                ),
                serde_json::json!({
                    "card_id": card_id.to_string(),
                    "revision": record.revision,
                    "digest": digest.as_str(),
                    "superseded_revision": state.current_revision,
                    "superseded_digest": state.current_digest.as_str(),
                    "state": CardState::Ready.name(),
                }),
            )
            .with_project(config.project_id.clone()))
        },
    )
}

fn run_status(args: &StatusArgs) -> Result<CommandOutcome, HarnessError> {
    let card_id: CardId = args.card_id.parse()?;
    let control = ControlRepository::open(&args.common.control)?;
    let config = control.project()?;

    let Some(state) = state_of(&control, &card_id)? else {
        // A stored draft with no state is a legitimate pre-activation card.
        let draft = stored_draft(&control, &card_id)?;
        return Ok(CommandOutcome::new(
            "card.status",
            format!(
                "Card {card_id} is a draft in cycle {}\nnot yet activated, so it has no digest",
                draft.cycle_id
            ),
            serde_json::json!({
                "card_id": card_id.to_string(),
                "state": CardState::Draft.name(),
                "cycle_id": draft.cycle_id.to_string(),
                "revision": serde_json::Value::Null,
                "digest": serde_json::Value::Null,
            }),
        )
        .with_project(config.project_id.clone()));
    };

    let record: CardRecord = serde_json::from_str(
        &control.read(&CardRecord::relative_path(&card_id, state.current_revision))?,
    )
    .map_err(|source| HarnessError::Control {
        reason: format!("card {card_id} revision record is malformed: {source}"),
        code: ErrorCode::InternalControlCorrupt,
    })?;

    // Recomputing rather than trusting the stored value: the digest is what
    // every downstream record binds to, so a mismatch means the immutable
    // record was edited and must not pass silently.
    let recomputed = record.digest()?;
    let intact = recomputed == state.current_digest;

    let mut outcome = CommandOutcome::new(
        "card.status",
        format!(
            "Card {card_id}\ntitle: {}\ncycle: {}\nstate: {}\nrevision: {}\ndigest: {recomputed}\nrisk: {}",
            record.title,
            record.cycle_id,
            state.state,
            state.current_revision,
            record.risk.name()
        ),
        serde_json::json!({
            "card_id": card_id.to_string(),
            "cycle_id": record.cycle_id.to_string(),
            "state": state.state.name(),
            "revision": state.current_revision,
            "digest": recomputed.as_str(),
            "recorded_digest": state.current_digest.as_str(),
            "digest_intact": intact,
            "canonical_algorithm": state.canonical_algorithm,
            "risk": record.risk.name(),
            "title": record.title,
        }),
    )
    .with_project(config.project_id.clone());

    if !intact {
        outcome = outcome.with_warning(format!(
            "recomputed digest {recomputed} does not match the recorded {}; the immutable revision was altered outside the harness",
            state.current_digest
        ));
    }
    Ok(outcome)
}
