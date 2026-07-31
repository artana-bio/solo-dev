//! Append-only authoritative events.
//!
//! Section 10.4 makes the control repository's Git history the integrity chain.
//! An event is never rewritten: a correction is a new event, so the record of
//! what was believed at each point survives.
//!
//! D-011 is why there is no `previous_event_digest` field. Git already chains
//! commits, and a second hash chain would be redundant until events travel over
//! a non-Git transport.

use std::{collections::BTreeMap, fs};

use serde::{Deserialize, Serialize};

use crate::{
    domain::{
        clock::{Clock, Timestamp},
        digest::Digest,
        ids::{CardId, CycleId, EventId, ProjectId},
    },
    error::{ErrorCode, HarnessError},
};

use super::repository::ControlRepository;

/// Directory holding events, relative to the control repository.
pub const EVENT_DIR: &str = "events";

/// Schema identifier for an event.
pub const EVENT_SCHEMA: &str = "harness.event/v1";

/// One authoritative state transition.
///
/// Card fields are optional because a cycle-level transition names no card.
/// They are still present as explicit nulls so a reader never has to branch on
/// key existence, matching the envelope convention.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Event {
    /// Always [`EVENT_SCHEMA`].
    pub schema: String,
    /// Identifies this event.
    pub event_id: EventId,
    /// The project it belongs to.
    pub project_id: ProjectId,
    /// The cycle it belongs to, when there is one.
    pub cycle_id: Option<CycleId>,
    /// The card it concerns, when there is one.
    pub card_id: Option<CardId>,
    /// The card revision in force, when a card is named.
    pub card_revision: Option<u32>,
    /// The card digest in force, when a card is named.
    pub card_digest: Option<Digest>,
    /// What happened, such as `cycle.activated`.
    pub event_type: String,
    /// Who caused it. Declared, not proven; see D-013 and R-012.
    pub actor_id: String,
    /// When it happened.
    pub occurred_at: Timestamp,
    /// The state before the transition, when the subject had one.
    pub previous_state: Option<String>,
    /// The state after the transition.
    pub next_state: Option<String>,
    /// The Git commit the transition refers to, when it names one.
    pub head_sha: Option<String>,
    /// Additional structured context.
    pub metadata: BTreeMap<String, serde_json::Value>,
}

impl Event {
    /// Relative path of this event inside the control repository.
    #[must_use]
    pub fn relative_path(event_id: &EventId) -> String {
        format!("{EVENT_DIR}/{event_id}.json")
    }
}

/// Builds an event without requiring every optional field at the call site.
#[derive(Debug)]
pub struct EventDraft {
    event_type: String,
    actor_id: String,
    cycle_id: Option<CycleId>,
    card_id: Option<CardId>,
    card_revision: Option<u32>,
    card_digest: Option<Digest>,
    previous_state: Option<String>,
    next_state: Option<String>,
    head_sha: Option<String>,
    metadata: BTreeMap<String, serde_json::Value>,
}

impl EventDraft {
    /// Starts a draft for one event type and actor.
    #[must_use]
    pub fn new(event_type: impl Into<String>, actor_id: impl Into<String>) -> Self {
        Self {
            event_type: event_type.into(),
            actor_id: actor_id.into(),
            cycle_id: None,
            card_id: None,
            card_revision: None,
            card_digest: None,
            previous_state: None,
            next_state: None,
            head_sha: None,
            metadata: BTreeMap::new(),
        }
    }

    /// Names the cycle this event concerns.
    #[must_use]
    pub fn cycle(mut self, cycle_id: CycleId) -> Self {
        self.cycle_id = Some(cycle_id);
        self
    }

    /// Names the card this event concerns, with its revision and digest.
    #[must_use]
    pub fn card(mut self, card_id: CardId, revision: u32, digest: Digest) -> Self {
        self.card_id = Some(card_id);
        self.card_revision = Some(revision);
        self.card_digest = Some(digest);
        self
    }

    /// Records the transition this event represents.
    #[must_use]
    pub fn transition(
        mut self,
        previous: Option<impl Into<String>>,
        next: impl Into<String>,
    ) -> Self {
        self.previous_state = previous.map(Into::into);
        self.next_state = Some(next.into());
        self
    }

    /// Records the Git commit this event refers to.
    #[must_use]
    pub fn head(mut self, sha: impl Into<String>) -> Self {
        self.head_sha = Some(sha.into());
        self
    }

    /// Attaches one structured metadata field.
    #[must_use]
    pub fn meta(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }
}

/// Appends and reads authoritative events.
#[derive(Debug)]
pub struct EventStore<'a> {
    control: &'a ControlRepository,
}

impl<'a> EventStore<'a> {
    /// Binds a store to a control repository.
    #[must_use]
    pub const fn new(control: &'a ControlRepository) -> Self {
        Self { control }
    }

    /// Allocates the next event identifier.
    ///
    /// # Errors
    ///
    /// Returns an error when the event directory cannot be read.
    pub fn next_id(&self) -> Result<EventId, HarnessError> {
        let directory = self.control.path(EVENT_DIR);
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
                        .and_then(|stem| stem.strip_prefix("E-"))
                        .and_then(|digits| digits.parse::<u64>().ok())
                })
                .max()
                .unwrap_or(0)
        } else {
            0
        };
        format!("E-{:06}", highest + 1).parse()
    }

    /// Appends one event, writing it atomically.
    ///
    /// The caller commits control state afterwards, so several events belonging
    /// to one transition land in one commit rather than several.
    ///
    /// # Errors
    ///
    /// Returns an error when the event cannot be written.
    pub fn append(
        &self,
        project_id: &ProjectId,
        draft: EventDraft,
        clock: &dyn Clock,
    ) -> Result<Event, HarnessError> {
        let event = Event {
            schema: EVENT_SCHEMA.to_owned(),
            event_id: self.next_id()?,
            project_id: project_id.clone(),
            cycle_id: draft.cycle_id,
            card_id: draft.card_id,
            card_revision: draft.card_revision,
            card_digest: draft.card_digest,
            event_type: draft.event_type,
            actor_id: draft.actor_id,
            occurred_at: clock.now(),
            previous_state: draft.previous_state,
            next_state: draft.next_state,
            head_sha: draft.head_sha,
            metadata: draft.metadata,
        };
        self.control.write_atomic(
            &Event::relative_path(&event.event_id),
            &format!("{}\n", serde_json::to_string_pretty(&event)?),
        )?;
        Ok(event)
    }

    /// Reads one event.
    ///
    /// # Errors
    ///
    /// Returns an error when the event is missing or malformed.
    pub fn read(&self, event_id: &EventId) -> Result<Event, HarnessError> {
        let raw = self.control.read(&Event::relative_path(event_id))?;
        serde_json::from_str(&raw).map_err(|source| HarnessError::Control {
            reason: format!("event {event_id} is malformed: {source}"),
            code: ErrorCode::InternalControlCorrupt,
        })
    }

    /// Every event, oldest first.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory cannot be read or an event is
    /// malformed.
    pub fn all(&self) -> Result<Vec<Event>, HarnessError> {
        let directory = self.control.path(EVENT_DIR);
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

        names.iter().map(|name| self.read(&name.parse()?)).collect()
    }

    /// Every event concerning one cycle, oldest first.
    ///
    /// # Errors
    ///
    /// Returns an error when the store cannot be read.
    pub fn for_cycle(&self, cycle_id: &CycleId) -> Result<Vec<Event>, HarnessError> {
        Ok(self
            .all()?
            .into_iter()
            .filter(|event| event.cycle_id.as_ref() == Some(cycle_id))
            .collect())
    }

    /// Every event concerning one card, oldest first.
    ///
    /// # Errors
    ///
    /// Returns an error when the store cannot be read.
    pub fn for_card(&self, card_id: &CardId) -> Result<Vec<Event>, HarnessError> {
        Ok(self
            .all()?
            .into_iter()
            .filter(|event| event.card_id.as_ref() == Some(card_id))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::clock::FixedClock;

    fn clock() -> FixedClock {
        FixedClock::at_unix_seconds(1_785_196_800).unwrap()
    }

    fn control() -> (tempfile::TempDir, ControlRepository) {
        let temp = tempfile::tempdir().unwrap();
        let control = ControlRepository::at(temp.path());
        control.initialize_git().unwrap();
        (temp, control)
    }

    fn project() -> ProjectId {
        "example".parse().unwrap()
    }

    #[test]
    fn identifiers_are_dense_and_monotonic() {
        let (_temp, control) = control();
        let store = EventStore::new(&control);
        assert_eq!(store.next_id().unwrap().as_str(), "E-000001");
        let first = store
            .append(
                &project(),
                EventDraft::new("cycle.created", "alvaro"),
                &clock(),
            )
            .unwrap();
        assert_eq!(store.next_id().unwrap().as_str(), "E-000002");

        let second = store
            .append(
                &project(),
                EventDraft::new("cycle.activated", "alvaro"),
                &clock(),
            )
            .unwrap();
        let third = store
            .append(
                &project(),
                EventDraft::new("cycle.abandoned", "alvaro"),
                &clock(),
            )
            .unwrap();
        assert_eq!(
            [
                first.event_id.as_str(),
                second.event_id.as_str(),
                third.event_id.as_str(),
            ],
            ["E-000001", "E-000002", "E-000003"],
            "ordinary allocation must be dense"
        );

        let second_path = control.path(&Event::relative_path(&second.event_id));
        let third_path = Event::relative_path(&third.event_id);
        let third_before = control.read(&third_path).unwrap();
        std::fs::remove_file(&second_path).unwrap();
        assert!(
            !second_path.exists() && control.path(&third_path).exists(),
            "the fixture must contain a real gap below the highest event identifier"
        );

        assert_eq!(
            store.next_id().unwrap().as_str(),
            "E-000004",
            "the next identifier must advance beyond every existing event"
        );
        let fourth = store
            .append(
                &project(),
                EventDraft::new("cycle.closed", "alvaro"),
                &clock(),
            )
            .unwrap();
        assert_eq!(fourth.event_id.as_str(), "E-000004");
        assert_eq!(
            control.read(&third_path).unwrap(),
            third_before,
            "allocating after a gap must not overwrite the highest existing event"
        );
    }

    #[test]
    fn an_appended_event_round_trips() {
        let (_temp, control) = control();
        let store = EventStore::new(&control);
        let event = store
            .append(
                &project(),
                EventDraft::new("cycle.activated", "alvaro")
                    .cycle("C-001".parse().unwrap())
                    .transition(Some("draft"), "active")
                    .head("abc123")
                    .meta("baseline_sha", serde_json::json!("abc123")),
                &clock(),
            )
            .unwrap();

        let read_back = store.read(&event.event_id).unwrap();
        assert_eq!(read_back, event);
        assert_eq!(read_back.event_type, "cycle.activated");
        assert_eq!(read_back.previous_state.as_deref(), Some("draft"));
        assert_eq!(read_back.next_state.as_deref(), Some("active"));
        assert_eq!(read_back.metadata["baseline_sha"], "abc123");
    }

    #[test]
    fn a_cycle_level_event_carries_explicit_nulls_for_card_fields() {
        let (_temp, control) = control();
        let store = EventStore::new(&control);
        let event = store
            .append(
                &project(),
                EventDraft::new("cycle.created", "alvaro"),
                &clock(),
            )
            .unwrap();

        let raw = control
            .read(&Event::relative_path(&event.event_id))
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        for key in ["card_id", "card_revision", "card_digest", "cycle_id"] {
            assert!(value.get(key).is_some(), "`{key}` must be present as null");
            assert!(value[key].is_null());
        }
    }

    #[test]
    fn events_are_returned_oldest_first() {
        let (_temp, control) = control();
        let store = EventStore::new(&control);
        for kind in ["cycle.created", "cycle.activated", "cycle.abandoned"] {
            store
                .append(&project(), EventDraft::new(kind, "alvaro"), &clock())
                .unwrap();
        }
        let kinds: Vec<String> = store
            .all()
            .unwrap()
            .into_iter()
            .map(|event| event.event_type)
            .collect();
        assert_eq!(
            kinds,
            vec!["cycle.created", "cycle.activated", "cycle.abandoned"]
        );
    }

    #[test]
    fn events_filter_by_cycle_and_card() {
        let (_temp, control) = control();
        let store = EventStore::new(&control);
        let cycle: CycleId = "C-001".parse().unwrap();
        let card: CardId = "F-001".parse().unwrap();
        let digest = Digest::of_bytes(b"card");

        store
            .append(
                &project(),
                EventDraft::new("cycle.created", "a").cycle(cycle.clone()),
                &clock(),
            )
            .unwrap();
        store
            .append(
                &project(),
                EventDraft::new("card.activated", "a")
                    .cycle(cycle.clone())
                    .card(card.clone(), 1, digest),
                &clock(),
            )
            .unwrap();
        store
            .append(
                &project(),
                EventDraft::new("cycle.created", "a").cycle("C-002".parse().unwrap()),
                &clock(),
            )
            .unwrap();

        assert_eq!(store.for_cycle(&cycle).unwrap().len(), 2);
        assert_eq!(store.for_card(&card).unwrap().len(), 1);
    }

    #[test]
    fn an_empty_store_reads_as_empty() {
        let (_temp, control) = control();
        assert!(EventStore::new(&control).all().unwrap().is_empty());
    }

    #[test]
    fn a_malformed_event_is_reported_as_corruption() {
        let (_temp, control) = control();
        control
            .write_atomic(&format!("{EVENT_DIR}/E-000001.json"), "not json\n")
            .unwrap();
        let error = EventStore::new(&control)
            .all()
            .expect_err("must fail loudly");
        assert_eq!(error.code(), ErrorCode::InternalControlCorrupt);
    }

    #[test]
    fn events_enter_control_history_unlike_the_journal() {
        let (_temp, control) = control();
        let store = EventStore::new(&control);
        store
            .append(&project(), EventDraft::new("cycle.created", "a"), &clock())
            .unwrap();
        control.commit(None, "record event").unwrap();

        let tracked = crate::git::command::run_ok(&control.scope(), ["ls-files"]).unwrap();
        assert!(
            tracked.trimmed_stdout().contains("events/E-000001.json"),
            "events are authoritative and must be versioned: {}",
            tracked.trimmed_stdout()
        );
    }
}
