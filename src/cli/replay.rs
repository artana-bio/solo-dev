//! Deriving a floor script from a cycle's journaled events.
//!
//! This is the presentation-planning half of `cycle replay`: a pure function
//! from the authoritative event history to the same [`Script`] vocabulary the
//! demo's canned screenplay uses, so one interpreter renders both and the two
//! can never drift apart visually. Real identifiers, real gate names, and
//! real candidate SHAs ride the beats; nothing here invents data.
//!
//! Honesty rule: every event produces at least one beat — an event type with
//! no set piece becomes a [`Beat::Note`] naming it — and every beat carries
//! its source event's ordinal, so the footer's `event k/n` counter can never
//! present a partial picture as complete.

use std::collections::BTreeMap;

use crate::control::event_store::Event;

use super::floor::{Beat, Script, Station, TimedBeat};

/// How many characters of a commit SHA the floor displays.
///
/// A full 40-character SHA would put 41 frames into every PRESS tumbler and
/// overflow the extra line; seven characters is what Git itself abbreviates
/// to in a repository this size.
const SHORT_SHA: usize = 7;

/// A discrepancy to flash at its moment in history.
///
/// The command layer builds these from the audit cross-check; this module
/// only needs to know which record the discrepancy is about, so it can attach
/// the flash to the event that created that record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceFlash {
    /// The record identifier the discrepancy names, such as a receipt or
    /// review id — or the cycle id for baseline-level findings.
    pub record_id: String,
    /// The full ticker line, already worded.
    pub text: String,
}

/// One event's line in the plain-text timeline.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct TimelineEntry {
    /// When it happened, RFC 3339.
    pub at: String,
    /// The journaled event this line describes.
    pub event_id: String,
    /// The journaled event type, verbatim.
    pub event_type: String,
    /// Who caused it.
    pub actor_id: String,
    /// One human-readable line.
    pub description: String,
}

/// The derived screenplay and its companion timeline.
#[derive(Clone, Debug)]
pub struct Derived {
    /// What the interpreter plays.
    pub script: Script,
    /// One entry per source event, for text mode and the JSON payload.
    pub timeline: Vec<TimelineEntry>,
}

/// Derives the floor screenplay for one cycle's history.
///
/// `baseline` is the cycle's frozen baseline, when it has one. `flashes` are
/// evidence discrepancies to surface; each is attached directly after the
/// event whose metadata names its record, and any that match no event flash
/// before the closing hold rather than being dropped.
#[must_use]
pub fn derive(
    cycle_id: &str,
    baseline: Option<&str>,
    events: &[Event],
    flashes: &[EvidenceFlash],
) -> Derived {
    let mut deriver = Deriver::new(events.len(), flashes);
    for (index, event) in events.iter().enumerate() {
        deriver.on_event(index + 1, event);
    }
    deriver.finish(cycle_id, baseline, events.len())
}

/// Tracks floor positions while translating events into beats.
struct Deriver {
    beats: Vec<TimedBeat>,
    timeline: Vec<TimelineEntry>,
    /// Where each on-floor card currently sits, by identifier.
    positions: BTreeMap<String, Station>,
    /// The member cards of the prepared integration, in plan order.
    members: Vec<String>,
    /// Flashes not yet attached to an event, in report order.
    pending_flashes: Vec<EvidenceFlash>,
    flash_count: usize,
}

impl Deriver {
    fn new(total_events: usize, flashes: &[EvidenceFlash]) -> Self {
        let _ = total_events;
        Self {
            beats: Vec::new(),
            timeline: Vec::new(),
            positions: BTreeMap::new(),
            members: Vec::new(),
            pending_flashes: flashes.to_vec(),
            flash_count: flashes.len(),
        }
    }

    /// Emits one beat. The first beat of each event carries the history
    /// clock and the event ordinal; the rest inherit them.
    fn push(&mut self, first: &mut Option<(String, usize)>, beat: Beat) {
        let (at, progress) = match first.take() {
            Some((timestamp, ordinal)) => (Some(timestamp), Some(ordinal)),
            None => (None, None),
        };
        self.beats.push(TimedBeat { at, progress, beat });
    }

    /// Moves a card to `target`, arriving it first if it is not on the floor.
    ///
    /// Replaying a partial history can name a card whose arrival predates the
    /// window, so an unknown card enters at INTAKE rather than being dropped.
    fn ensure(&mut self, first: &mut Option<(String, usize)>, card: &str, target: Station) {
        if !self.positions.contains_key(card) {
            self.push(
                first,
                Beat::Arrive {
                    card: card.to_owned(),
                },
            );
            self.positions.insert(card.to_owned(), Station::Intake);
        }
        if self.positions.get(card) != Some(&target) {
            self.push(
                first,
                Beat::Advance {
                    card: card.to_owned(),
                    to: target,
                },
            );
            self.positions.insert(card.to_owned(), target);
        }
    }

    #[allow(clippy::too_many_lines)]
    fn on_event(&mut self, ordinal: usize, event: &Event) {
        let timestamp = event.occurred_at.to_rfc3339();
        let mut first = Some((timestamp.clone(), ordinal));
        let card = event.card_id.as_ref().map(ToString::to_string);
        let head_short = event.head_sha.as_deref().map(short);

        let description = match event.event_type.as_str() {
            "cycle.created" => {
                let text = "cycle created in draft".to_owned();
                self.push(&mut first, Beat::Note { text: text.clone() });
                text
            }
            "cycle.activated" => {
                let text = format!(
                    "cycle activated · baseline {}",
                    head_short.as_deref().unwrap_or("unknown")
                );
                self.push(&mut first, Beat::Note { text: text.clone() });
                text
            }
            "cycle.group-declared" => {
                let text = format!(
                    "atomic group `{}` declared",
                    meta_str(event, "name").unwrap_or("unnamed")
                );
                self.push(&mut first, Beat::Note { text: text.clone() });
                text
            }
            "cycle.abandoned" => {
                let text = "cycle abandoned".to_owned();
                self.push(&mut first, Beat::Note { text: text.clone() });
                text
            }
            "card.created" => {
                // `card.created` journals no card identifier — the draft is
                // not yet an activated card — so the narration must not
                // pretend to know one.
                let text = card.as_ref().map_or_else(
                    || "a card was authored (draft)".to_owned(),
                    |card| format!("card {card} authored (draft)"),
                );
                self.push(&mut first, Beat::Note { text: text.clone() });
                text
            }
            "card.activated" => {
                let card = card.unwrap_or_default();
                self.push(&mut first, Beat::Arrive { card: card.clone() });
                self.positions.insert(card.clone(), Station::Intake);
                format!("{card} activated and entered the floor")
            }
            "card.revised" => {
                let card = card.as_deref().unwrap_or("a card");
                let revision = event.card_revision.unwrap_or(0);
                let text = format!("card {card} revised to r{revision}");
                self.push(&mut first, Beat::Note { text: text.clone() });
                text
            }
            "card.abandoned" => {
                let card = card.unwrap_or_default();
                self.push(
                    &mut first,
                    Beat::Depart {
                        card: card.clone(),
                        reason: "card abandoned".to_owned(),
                    },
                );
                self.positions.remove(&card);
                format!("{card} abandoned")
            }
            "work.started" => {
                let card = card.unwrap_or_default();
                self.ensure(&mut first, &card, Station::Intake);
                let branch = meta_str(event, "branch").unwrap_or("its branch");
                let text = format!("work started on {card} ({branch})");
                self.push(&mut first, Beat::Note { text: text.clone() });
                text
            }
            "work.checkpoint" => {
                let card = card.as_deref().unwrap_or("a card");
                let note = meta_str(event, "note").unwrap_or("no note");
                let text = format!("checkpoint on {card}: {note}");
                self.push(&mut first, Beat::Note { text: text.clone() });
                text
            }
            "work.resumed" => {
                let card = card.as_deref().unwrap_or("a card");
                let text = format!("work resumed on {card}");
                self.push(&mut first, Beat::Note { text: text.clone() });
                text
            }
            "work.blocked" => {
                let card = card.as_deref().unwrap_or("a card");
                let text = format!("{card} blocked pending a decision");
                self.push(&mut first, Beat::Note { text: text.clone() });
                text
            }
            "work.reclaimed" => {
                let card = card.as_deref().unwrap_or("a card");
                let text = format!("stale lease on {card} reclaimed");
                self.push(&mut first, Beat::Note { text: text.clone() });
                text
            }
            "gate.ran" => {
                let card = card.unwrap_or_default();
                let gate = meta_str(event, "gate_id").unwrap_or("a gate").to_owned();
                let attempt =
                    u32::try_from(meta_u64(event, "attempt").unwrap_or(1)).unwrap_or(u32::MAX);
                self.ensure(&mut first, &card, Station::Bench);
                if meta_bool(event, "passed").unwrap_or(false) {
                    self.push(
                        &mut first,
                        Beat::GatePass {
                            card: card.clone(),
                            gate: gate.clone(),
                            attempt,
                        },
                    );
                    format!("gate `{gate}` passed for {card} (attempt {attempt})")
                } else {
                    self.push(
                        &mut first,
                        Beat::GateFailEject {
                            card: card.clone(),
                            gate: gate.clone(),
                        },
                    );
                    // The interpreter re-enters the card at INTAKE after the
                    // rework crawl; the tracker must agree or the next ensure
                    // would skip the advance back to BENCH.
                    self.positions.insert(card.clone(), Station::Intake);
                    format!("gate `{gate}` failed for {card} (attempt {attempt}) — reworked")
                }
            }
            "handoff.created" => {
                let card = card.unwrap_or_default();
                let sha = head_short.clone().unwrap_or_default();
                self.ensure(&mut first, &card, Station::Press);
                self.push(
                    &mut first,
                    Beat::Stamp {
                        card: card.clone(),
                        sha: sha.clone(),
                    },
                );
                format!("handoff sealed {card} at candidate {sha}")
            }
            "handoff.revoked" => {
                let card = card.unwrap_or_default();
                let text = format!("handoff on {card} revoked — back to work");
                self.push(&mut first, Beat::Note { text: text.clone() });
                self.ensure(&mut first, &card, Station::Intake);
                text
            }
            "review.begun" => {
                // The actor is not repeated here: every timeline line already
                // carries `· by <actor>`, and the ticker stays short.
                let card = card.unwrap_or_default();
                self.ensure(&mut first, &card, Station::Scan);
                let text = format!("review begun for {card}");
                self.push(&mut first, Beat::Note { text: text.clone() });
                text
            }
            "review.recorded" => {
                let card = card.unwrap_or_default();
                let findings =
                    usize::try_from(meta_u64(event, "findings").unwrap_or(0)).unwrap_or(usize::MAX);
                self.ensure(&mut first, &card, Station::Scan);
                match meta_str(event, "decision") {
                    Some("approved") => {
                        self.push(&mut first, Beat::ReviewApprove { card: card.clone() });
                        format!("review approved {card}")
                    }
                    Some("changes_requested") => {
                        self.push(
                            &mut first,
                            Beat::ReviewReject {
                                card: card.clone(),
                                findings,
                            },
                        );
                        self.positions.insert(card.clone(), Station::Intake);
                        format!("review requested changes on {card} ({findings} finding(s))")
                    }
                    decision => {
                        let text = format!(
                            "review recorded `{}` on {card}",
                            decision.unwrap_or("an unknown decision")
                        );
                        self.push(&mut first, Beat::Note { text: text.clone() });
                        text
                    }
                }
            }
            "integration.prepared" => {
                self.members = meta_str_list(event, "cards");
                let members = self.members.clone();
                for member in &members {
                    self.ensure(&mut first, member, Station::Weld);
                }
                let text = format!(
                    "integration `{}` prepared: {}",
                    meta_str(event, "integration_id").unwrap_or("unknown"),
                    if members.is_empty() {
                        "no members".to_owned()
                    } else {
                        members.join(", ")
                    }
                );
                self.push(&mut first, Beat::Note { text: text.clone() });
                text
            }
            "integration.merged" => {
                let text = format!(
                    "members merged ▸ {}",
                    head_short.as_deref().unwrap_or("unknown")
                );
                self.push(&mut first, Beat::Note { text: text.clone() });
                text
            }
            "integration.landing-built" => {
                let text = format!(
                    "landing commit built ▸ {}",
                    head_short.as_deref().unwrap_or("unknown")
                );
                self.push(&mut first, Beat::Note { text: text.clone() });
                text
            }
            "integration.verified" => {
                let sha = head_short.clone().unwrap_or_default();
                if meta_bool(event, "passed").unwrap_or(false) {
                    self.push(
                        &mut first,
                        Beat::Weld {
                            members: self.weld_members(),
                            merged_sha: sha.clone(),
                        },
                    );
                    format!("combined verification passed ▸ {sha}")
                } else {
                    let text = format!("✗ combined verification failed ▸ {sha}");
                    self.push(&mut first, Beat::Note { text: text.clone() });
                    text
                }
            }
            "integration.reviewed" => {
                let text = "integration review recorded".to_owned();
                self.push(&mut first, Beat::Note { text: text.clone() });
                text
            }
            "acceptance.recorded" => {
                let text = "acceptance recorded — promotion authorized".to_owned();
                self.push(&mut first, Beat::Note { text: text.clone() });
                text
            }
            "integration.promoted" => {
                let from = meta_str(event, "previous_main_sha")
                    .map_or_else(|| "unknown".to_owned(), short);
                let to = meta_str(event, "landing_sha")
                    .map(short)
                    .or(head_short.clone())
                    .unwrap_or_default();
                let members = self.weld_members();
                for member in &members {
                    self.ensure(&mut first, member, Station::Ship);
                }
                self.push(
                    &mut first,
                    Beat::Ship {
                        members,
                        from: from.clone(),
                        to: to.clone(),
                    },
                );
                format!("promoted — authority advanced {from} → {to}")
            }
            "integration.abandoned" => {
                let text = "integration abandoned before promotion".to_owned();
                self.push(&mut first, Beat::Note { text: text.clone() });
                text
            }
            "archive.created" => {
                let text = "archive created — refs preserved".to_owned();
                self.push(&mut first, Beat::Note { text: text.clone() });
                text
            }
            "archive.closed" => {
                let text = "archive closed — cleanup complete".to_owned();
                self.push(&mut first, Beat::Note { text: text.clone() });
                text
            }
            other => {
                // No set piece, but never silently dropped: the event's own
                // name narrates it and the ordinal still advances.
                let text = other.to_owned();
                self.push(&mut first, Beat::Note { text: text.clone() });
                text
            }
        };

        self.attach_flashes(&mut first, event);

        self.timeline.push(TimelineEntry {
            at: timestamp,
            event_id: event.event_id.to_string(),
            event_type: event.event_type.clone(),
            actor_id: event.actor_id.clone(),
            description,
        });
    }

    /// The cards the WELD and SHIP set pieces act on: the prepared
    /// integration's members, or whoever is at WELD when a partial history
    /// never recorded the preparation.
    fn weld_members(&self) -> Vec<String> {
        if self.members.is_empty() {
            self.positions
                .iter()
                .filter(|(_, station)| **station == Station::Weld)
                .map(|(card, _)| card.clone())
                .collect()
        } else {
            self.members.clone()
        }
    }

    /// Flashes every pending discrepancy whose record this event created.
    fn attach_flashes(&mut self, first: &mut Option<(String, usize)>, event: &Event) {
        let mut index = 0;
        while index < self.pending_flashes.len() {
            if event_names_record(event, &self.pending_flashes[index].record_id) {
                let flash = self.pending_flashes.remove(index);
                self.push(first, Beat::Flash { text: flash.text });
            } else {
                index += 1;
            }
        }
    }

    fn finish(mut self, cycle_id: &str, baseline: Option<&str>, total_events: usize) -> Derived {
        // A discrepancy whose record matched no event still surfaces; being
        // unplaceable in time must not mean being invisible.
        let unplaced: Vec<EvidenceFlash> = std::mem::take(&mut self.pending_flashes);
        for flash in unplaced {
            self.beats
                .push(TimedBeat::untimed(Beat::Flash { text: flash.text }));
        }

        let close = if self.flash_count == 0 {
            format!("replay complete — {total_events} event(s), evidence holds")
        } else {
            format!(
                "replay complete — {total_events} event(s), {} discrepancy(ies)",
                self.flash_count
            )
        };
        self.beats
            .push(TimedBeat::untimed(Beat::Close { text: close }));

        let baseline_short = baseline.map_or_else(|| "not frozen".to_owned(), short);
        Derived {
            script: Script {
                header_base: format!("cycle {cycle_id} · baseline {baseline_short}"),
                header_compact: format!("{cycle_id} · {baseline_short}"),
                progress_total: Some(total_events),
                beats: self.beats,
            },
            timeline: self.timeline,
        }
    }
}

/// True when the event's metadata names `record_id`, or when the finding is
/// about the cycle itself and this is the event that froze its baseline.
fn event_names_record(event: &Event, record_id: &str) -> bool {
    if event
        .metadata
        .values()
        .any(|value| value.as_str() == Some(record_id))
    {
        return true;
    }
    event.event_type == "cycle.activated"
        && event.cycle_id.as_ref().map(ToString::to_string).as_deref() == Some(record_id)
}

fn short(sha: &str) -> String {
    sha.chars().take(SHORT_SHA).collect()
}

fn meta_str<'a>(event: &'a Event, key: &str) -> Option<&'a str> {
    event.metadata.get(key).and_then(serde_json::Value::as_str)
}

fn meta_u64(event: &Event, key: &str) -> Option<u64> {
    event.metadata.get(key).and_then(serde_json::Value::as_u64)
}

fn meta_bool(event: &Event, key: &str) -> Option<bool> {
    event.metadata.get(key).and_then(serde_json::Value::as_bool)
}

fn meta_str_list(event: &Event, key: &str) -> Vec<String> {
    event
        .metadata
        .get(key)
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{control::event_store::EVENT_SCHEMA, domain::clock::Timestamp};

    struct EventBuilder {
        event: Event,
    }

    impl EventBuilder {
        fn new(number: u64, event_type: &str) -> Self {
            Self {
                event: Event {
                    schema: EVENT_SCHEMA.to_owned(),
                    event_id: format!("E-{number:06}").parse().unwrap(),
                    project_id: "example".parse().unwrap(),
                    cycle_id: Some("C-001".parse().unwrap()),
                    card_id: None,
                    card_revision: None,
                    card_digest: None,
                    event_type: event_type.to_owned(),
                    actor_id: "actor-a".to_owned(),
                    occurred_at: Timestamp::from_unix_seconds(
                        1_785_196_800 + i64::try_from(number).unwrap() * 3600,
                    )
                    .unwrap(),
                    previous_state: None,
                    next_state: None,
                    head_sha: None,
                    metadata: std::collections::BTreeMap::new(),
                },
            }
        }

        fn card(mut self, card: &str) -> Self {
            self.event.card_id = Some(card.parse().unwrap());
            self
        }

        fn head(mut self, sha: &str) -> Self {
            self.event.head_sha = Some(sha.to_owned());
            self
        }

        fn meta(mut self, key: &str, value: serde_json::Value) -> Self {
            self.event.metadata.insert(key.to_owned(), value);
            self
        }

        fn build(self) -> Event {
            self.event
        }
    }

    fn full_sha(prefix: char) -> String {
        std::iter::repeat_n(prefix, 40).collect()
    }

    /// A complete happy-path history: one card through the whole lifecycle.
    fn happy_history() -> Vec<Event> {
        vec![
            EventBuilder::new(1, "cycle.created").build(),
            EventBuilder::new(2, "cycle.activated")
                .head(&full_sha('a'))
                .build(),
            EventBuilder::new(3, "card.created").card("F-100").build(),
            EventBuilder::new(4, "card.activated").card("F-100").build(),
            EventBuilder::new(5, "work.started")
                .card("F-100")
                .meta("branch", serde_json::json!("card/F-100"))
                .build(),
            EventBuilder::new(6, "gate.ran")
                .card("F-100")
                .head(&full_sha('b'))
                .meta("gate_id", serde_json::json!("gate.unit"))
                .meta("attempt", serde_json::json!(1))
                .meta("passed", serde_json::json!(true))
                .meta("receipt_id", serde_json::json!("R-000001"))
                .build(),
            EventBuilder::new(7, "handoff.created")
                .card("F-100")
                .head(&full_sha('c'))
                .meta("handoff_id", serde_json::json!("H-000001"))
                .build(),
            EventBuilder::new(8, "review.begun").card("F-100").build(),
            EventBuilder::new(9, "review.recorded")
                .card("F-100")
                .meta("decision", serde_json::json!("approved"))
                .meta("findings", serde_json::json!(0))
                .meta("review_id", serde_json::json!("REV-000001"))
                .build(),
            EventBuilder::new(10, "integration.prepared")
                .meta("integration_id", serde_json::json!("INT-001"))
                .meta("cards", serde_json::json!(["F-100"]))
                .build(),
            EventBuilder::new(11, "integration.verified")
                .head(&full_sha('d'))
                .meta("passed", serde_json::json!(true))
                .build(),
            EventBuilder::new(12, "acceptance.recorded").build(),
            EventBuilder::new(13, "integration.promoted")
                .head(&full_sha('d'))
                .meta("previous_main_sha", serde_json::json!(full_sha('a')))
                .meta("landing_sha", serde_json::json!(full_sha('d')))
                .build(),
        ]
    }

    fn beats_of(derived: &Derived) -> Vec<&Beat> {
        derived
            .script
            .beats
            .iter()
            .map(|timed| &timed.beat)
            .collect()
    }

    #[test]
    fn the_happy_path_produces_the_full_set_piece_sequence() {
        let derived = derive("C-001", Some(&full_sha('a')), &happy_history(), &[]);
        let beats = beats_of(&derived);

        assert!(matches!(
            beats
                .iter()
                .find(|beat| matches!(beat, Beat::Arrive { .. })),
            Some(Beat::Arrive { card }) if card == "F-100"
        ));
        assert!(beats.iter().any(
            |beat| matches!(beat, Beat::GatePass { card, gate, attempt: 1 } if card == "F-100" && gate == "gate.unit")
        ));
        assert!(beats.iter().any(
            |beat| matches!(beat, Beat::Stamp { card, sha } if card == "F-100" && sha == "ccccccc")
        ));
        assert!(
            beats
                .iter()
                .any(|beat| matches!(beat, Beat::ReviewApprove { card } if card == "F-100"))
        );
        assert!(beats.iter().any(
            |beat| matches!(beat, Beat::Weld { members, merged_sha } if members == &["F-100"] && merged_sha == "ddddddd")
        ));
        assert!(beats.iter().any(
            |beat| matches!(beat, Beat::Ship { members, from, to } if members == &["F-100"] && from == "aaaaaaa" && to == "ddddddd")
        ));
        assert!(matches!(
            beats.last(),
            Some(Beat::Close { text }) if text.contains("13 event(s)") && text.contains("evidence holds")
        ));
    }

    #[test]
    fn every_event_appears_in_the_timeline_in_order() {
        let history = happy_history();
        let derived = derive("C-001", None, &history, &[]);
        assert_eq!(derived.timeline.len(), history.len());
        for (entry, event) in derived.timeline.iter().zip(&history) {
            assert_eq!(entry.event_id, event.event_id.to_string());
            assert_eq!(entry.event_type, event.event_type);
            assert!(!entry.description.is_empty());
        }
    }

    #[test]
    fn the_first_beat_of_each_event_is_timed_and_the_rest_inherit() {
        let derived = derive("C-001", None, &happy_history(), &[]);
        let mut seen_ordinals = Vec::new();
        for timed in &derived.script.beats {
            match (&timed.at, timed.progress) {
                (Some(_), Some(ordinal)) => seen_ordinals.push(ordinal),
                (None, None) => {}
                other => panic!("a beat must be fully timed or fully untimed, got {other:?}"),
            }
        }
        assert_eq!(
            seen_ordinals,
            (1..=13).collect::<Vec<_>>(),
            "each event contributes exactly one timed beat, in order"
        );
        assert_eq!(derived.script.progress_total, Some(13));
    }

    #[test]
    fn a_failed_gate_ejects_and_the_card_advances_back_for_the_retry() {
        let history = vec![
            EventBuilder::new(1, "card.activated").card("F-100").build(),
            EventBuilder::new(2, "gate.ran")
                .card("F-100")
                .meta("gate_id", serde_json::json!("gate.unit"))
                .meta("attempt", serde_json::json!(1))
                .meta("passed", serde_json::json!(false))
                .build(),
            EventBuilder::new(3, "gate.ran")
                .card("F-100")
                .meta("gate_id", serde_json::json!("gate.unit"))
                .meta("attempt", serde_json::json!(2))
                .meta("passed", serde_json::json!(true))
                .build(),
        ];
        let derived = derive("C-001", None, &history, &[]);
        let beats = beats_of(&derived);

        let fail = beats
            .iter()
            .position(|beat| matches!(beat, Beat::GateFailEject { .. }))
            .expect("the failure must play");
        let advance_back = beats
            .iter()
            .skip(fail)
            .position(|beat| {
                matches!(beat, Beat::Advance { card, to: Station::Bench } if card == "F-100")
            })
            .expect("the card must walk back to BENCH for the retry");
        assert!(
            beats
                .iter()
                .skip(fail + advance_back)
                .any(|beat| matches!(beat, Beat::GatePass { attempt: 2, .. })),
            "the retry passes"
        );
    }

    #[test]
    fn a_changes_requested_review_ejects_the_card() {
        let history = vec![
            EventBuilder::new(1, "card.activated").card("F-100").build(),
            EventBuilder::new(2, "review.recorded")
                .card("F-100")
                .meta("decision", serde_json::json!("changes_requested"))
                .meta("findings", serde_json::json!(2))
                .build(),
        ];
        let derived = derive("C-001", None, &history, &[]);
        assert!(beats_of(&derived).iter().any(
            |beat| matches!(beat, Beat::ReviewReject { card, findings: 2 } if card == "F-100")
        ));
    }

    #[test]
    fn an_unknown_event_type_still_narrates_and_counts() {
        let history = vec![
            EventBuilder::new(1, "cycle.created").build(),
            EventBuilder::new(2, "some.future-event").build(),
        ];
        let derived = derive("C-001", None, &history, &[]);
        assert!(
            beats_of(&derived)
                .iter()
                .any(|beat| matches!(beat, Beat::Note { text } if text == "some.future-event")),
            "an unmapped event narrates itself rather than vanishing"
        );
        assert_eq!(derived.timeline.len(), 2);
        assert_eq!(derived.script.progress_total, Some(2));
    }

    #[test]
    fn a_card_first_seen_mid_history_arrives_before_it_moves() {
        // A partial replay window can open after the card's activation.
        let history = vec![
            EventBuilder::new(1, "gate.ran")
                .card("F-100")
                .meta("gate_id", serde_json::json!("gate.unit"))
                .meta("passed", serde_json::json!(true))
                .build(),
        ];
        let derived = derive("C-001", None, &history, &[]);
        let beats = beats_of(&derived);
        assert!(matches!(beats[0], Beat::Arrive { card } if card == "F-100"));
        assert!(
            matches!(
                &beats[1],
                Beat::Advance {
                    to: Station::Bench,
                    ..
                }
            ),
            "and walks to BENCH before the gate plays"
        );
    }

    #[test]
    fn a_flash_attaches_directly_after_the_event_naming_its_record() {
        let history = happy_history();
        let flashes = vec![EvidenceFlash {
            record_id: "R-000001".to_owned(),
            text: "✗ evidence: receipt R-000001 claims a commit that is gone".to_owned(),
        }];
        let derived = derive("C-001", None, &history, &flashes);
        let beats = beats_of(&derived);

        let gate = beats
            .iter()
            .position(|beat| matches!(beat, Beat::GatePass { .. }))
            .expect("the gate plays");
        assert!(
            matches!(beats[gate + 1], Beat::Flash { text } if text.contains("R-000001")),
            "the flash plays immediately after the gate whose receipt it names"
        );
        assert!(matches!(
            beats.last(),
            Some(Beat::Close { text }) if text.contains("1 discrepancy(ies)")
        ));
    }

    #[test]
    fn a_baseline_flash_attaches_to_the_activation_event() {
        let history = happy_history();
        let flashes = vec![EvidenceFlash {
            record_id: "C-001".to_owned(),
            text: "✗ evidence: cycle C-001 claims a baseline that is gone".to_owned(),
        }];
        let derived = derive("C-001", None, &history, &flashes);
        let beats = beats_of(&derived);
        let activation = beats
            .iter()
            .position(
                |beat| matches!(beat, Beat::Note { text } if text.starts_with("cycle activated")),
            )
            .expect("activation narrates");
        assert!(matches!(
            beats[activation + 1],
            Beat::Flash { text } if text.contains("baseline")
        ));
    }

    #[test]
    fn an_unmatched_flash_still_plays_before_the_close() {
        let history = vec![EventBuilder::new(1, "cycle.created").build()];
        let flashes = vec![EvidenceFlash {
            record_id: "R-999999".to_owned(),
            text: "✗ evidence: receipt R-999999 names nothing in this history".to_owned(),
        }];
        let derived = derive("C-001", None, &history, &flashes);
        let beats = beats_of(&derived);
        let len = beats.len();
        assert!(
            matches!(beats[len - 2], Beat::Flash { text } if text.contains("R-999999")),
            "unplaceable in time must not mean invisible"
        );
        assert!(matches!(beats[len - 1], Beat::Close { .. }));
    }

    #[test]
    fn the_header_names_the_cycle_and_shortens_the_baseline() {
        let derived = derive("C-001", Some(&full_sha('a')), &happy_history(), &[]);
        assert_eq!(derived.script.header_base, "cycle C-001 · baseline aaaaaaa");
        assert_eq!(derived.script.header_compact, "C-001 · aaaaaaa");

        let unfrozen = derive("C-001", None, &[], &[]);
        assert_eq!(
            unfrozen.script.header_base,
            "cycle C-001 · baseline not frozen"
        );
    }

    #[test]
    fn an_empty_history_still_closes_cleanly() {
        let derived = derive("C-001", None, &[], &[]);
        assert_eq!(derived.timeline.len(), 0);
        let beats = beats_of(&derived);
        assert_eq!(beats.len(), 1);
        assert!(matches!(beats[0], Beat::Close { text } if text.contains("0 event(s)")));
    }

    #[test]
    fn derivation_is_deterministic() {
        let history = happy_history();
        let first = derive("C-001", None, &history, &[]);
        let second = derive("C-001", None, &history, &[]);
        assert_eq!(first.script, second.script);
        assert_eq!(first.timeline, second.timeline);
    }
}
