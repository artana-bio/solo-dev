//! Leases: the exclusive assignment of one card to one actor and worktree.
//!
//! Invariant 7.3.1 allows a card at most one active lease. The lease is what
//! makes "who is working on this, and where" answerable from control state
//! rather than from whoever happens to remember.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::domain::{
    clock::Timestamp,
    ids::{CardId, LeaseId, ProjectId},
};

/// Schema identifier for a lease record.
pub const LEASE_SCHEMA: &str = "harness.lease/v1";

/// Directory holding lease records, relative to the control repository.
pub const LEASE_DIR: &str = "leases";

/// Schema identifier for the worktree locator file.
pub const WORKTREE_LINK_SCHEMA: &str = "harness.worktree-link/v1";

/// How long a held lease may show no recorded sign of life before `project
/// status` and `project recover` (`src/commands/project.rs`) report it as
/// silent. See #80.
///
/// Fixed, not policy-configurable. Every knob this crate lets an operator
/// tune lives in `ProjectConfig` and its policy types under `src/config/`,
/// which #80's frozen file scope does not include, so there is nowhere to
/// hang a per-project override without widening that scope — and a fixed
/// constant beside the struct it governs is the simpler choice on its own
/// merits, needing no plumbing through `project init`/`set-*-policy` for a
/// first cut at a problem nothing currently detects at all.
///
/// Thirty minutes is argued from the incident that motivated this card
/// (#80 §1): an actor that hung for 35 minutes went unnoticed for the rest
/// of a session. Set below that so the same failure would already have
/// been visible, while staying well above the gap between an ordinary
/// checkpoint and the next — this repository's own `cargo test`/`cargo
/// clippy` runs complete in low single-digit minutes.
///
/// What this misses: an actor who checkpoints every 29 minutes while doing
/// nothing in between is indistinguishable from one working normally. This
/// measures *recorded* activity, not truthful activity — see D-013 on why
/// a declared actor identity is never proven, which is the same limit
/// applied to declared progress. It also never flags a lease under thirty
/// minutes old that has not yet checkpointed: `granted_at` itself counts
/// as the first sign of life, so a card is only ever reported after a full
/// threshold of silence, never merely for being new.
pub const SILENT_LEASE_THRESHOLD_SECONDS: i64 = 30 * 60;

/// Whether a lease is still held.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LeaseStatus {
    /// The actor holds the card and its worktree.
    Held,
    /// The lease was released and the allocation is free.
    Released,
}

/// One exclusive card assignment.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LeaseRecord {
    /// Always [`LEASE_SCHEMA`].
    pub schema: String,
    /// Identifies this lease.
    pub lease_id: LeaseId,
    /// The card it assigns.
    pub card_id: CardId,
    /// The card revision in force when it was granted.
    pub card_revision: u32,
    /// Who holds it. Declared, not proven; see D-013.
    pub actor_id: String,
    /// The branch created for the work.
    pub branch: String,
    /// The worktree allocated for the work.
    pub worktree_path: PathBuf,
    /// The exact commit the branch started from.
    pub base_sha: String,
    /// Whether it is still held.
    pub status: LeaseStatus,
    /// When it was granted.
    pub granted_at: Timestamp,
    /// When it was released.
    pub released_at: Option<Timestamp>,
    /// Free-text progress notes, appended by `work checkpoint`.
    pub progress: Vec<ProgressNote>,
}

/// One recorded progress note.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProgressNote {
    /// When it was recorded.
    pub recorded_at: Timestamp,
    /// What the actor reported.
    pub note: String,
    /// The candidate head at the time, when the worktree had commits.
    pub head_sha: Option<String>,
}

impl LeaseRecord {
    /// Relative path of a lease inside the control repository.
    #[must_use]
    pub fn relative_path(lease_id: &LeaseId) -> String {
        format!("{LEASE_DIR}/{lease_id}.json")
    }

    /// True when this lease still holds its allocation.
    #[must_use]
    pub const fn is_held(&self) -> bool {
        matches!(self.status, LeaseStatus::Held)
    }

    /// The most recent instant this lease showed a recorded sign of life:
    /// the newest progress note, or `granted_at` when none has been
    /// recorded yet.
    ///
    /// Folds across every timestamp on the record rather than trusting
    /// `progress`'s last element to already be the newest, so a note
    /// appended out of order can never make this go backwards.
    #[must_use]
    pub fn last_activity_at(&self) -> Timestamp {
        self.progress
            .iter()
            .map(|note| note.recorded_at)
            .fold(self.granted_at, Timestamp::max)
    }

    /// True when this is a held lease with no recorded sign of life for at
    /// least [`SILENT_LEASE_THRESHOLD_SECONDS`], as of `now`.
    ///
    /// A released lease is never silent, regardless of age: nobody is
    /// waiting on a released allocation, so there is nothing here for an
    /// operator to check on. This says nothing about *why* a held one is
    /// quiet — a slow, legitimate task and an actor who is not coming back
    /// look identical from here, on purpose. See `work reclaim`
    /// (`src/commands/work.rs`) for the operator judgment this
    /// deliberately does not make.
    #[must_use]
    pub fn is_silent(&self, now: Timestamp) -> bool {
        self.is_held()
            && now.unix_seconds() - self.last_activity_at().unix_seconds()
                >= SILENT_LEASE_THRESHOLD_SECONDS
    }
}

/// The ignored locator written into an allocated worktree.
///
/// Section 9.3 is explicit that this is a locator only. It exists so an actor
/// dropped into a directory can find the control repository; it is never
/// trusted as a source of truth, because it sits inside a tree the actor can
/// edit. Every command compares it against control state before acting.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorktreeLink {
    /// Always [`WORKTREE_LINK_SCHEMA`].
    pub schema: String,
    /// The project this worktree belongs to.
    pub project_id: ProjectId,
    /// The card being worked.
    pub card_id: CardId,
    /// The card revision in force at allocation.
    pub card_revision: u32,
    /// Where the authoritative control repository lives.
    pub control_repository: PathBuf,
    /// The lease that granted this worktree.
    pub lease_id: LeaseId,
}

impl WorktreeLink {
    /// Path of the locator inside a worktree.
    #[must_use]
    pub fn path_in(worktree: &std::path::Path) -> PathBuf {
        worktree
            .join(crate::git::worktree::AGENT_DIR)
            .join("project.json")
    }

    /// Compares this locator with the authoritative lease.
    ///
    /// Returns the first field that disagrees, or `None` when they match.
    #[must_use]
    pub fn disagreement(&self, lease: &LeaseRecord, project_id: &ProjectId) -> Option<String> {
        if self.project_id != *project_id {
            return Some(format!(
                "project_id: locator says {}, control says {project_id}",
                self.project_id
            ));
        }
        if self.card_id != lease.card_id {
            return Some(format!(
                "card_id: locator says {}, control says {}",
                self.card_id, lease.card_id
            ));
        }
        if self.lease_id != lease.lease_id {
            return Some(format!(
                "lease_id: locator says {}, control says {}",
                self.lease_id, lease.lease_id
            ));
        }
        if self.card_revision != lease.card_revision {
            return Some(format!(
                "card_revision: locator says {}, control says {}",
                self.card_revision, lease.card_revision
            ));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::clock::{Clock as _, FixedClock};

    fn stamp() -> Timestamp {
        FixedClock::at_unix_seconds(1_785_196_800).unwrap().now()
    }

    fn lease() -> LeaseRecord {
        LeaseRecord {
            schema: LEASE_SCHEMA.to_owned(),
            lease_id: "L-000001".parse().unwrap(),
            card_id: "F-001".parse().unwrap(),
            card_revision: 1,
            actor_id: "alvaro".to_owned(),
            branch: "card/F-001".to_owned(),
            worktree_path: PathBuf::from("/work/F-001"),
            base_sha: "a".repeat(40),
            status: LeaseStatus::Held,
            granted_at: stamp(),
            released_at: None,
            progress: vec![],
        }
    }

    fn link() -> WorktreeLink {
        WorktreeLink {
            schema: WORKTREE_LINK_SCHEMA.to_owned(),
            project_id: "example".parse().unwrap(),
            card_id: "F-001".parse().unwrap(),
            card_revision: 1,
            control_repository: PathBuf::from("/control"),
            lease_id: "L-000001".parse().unwrap(),
        }
    }

    #[test]
    fn a_matching_locator_reports_no_disagreement() {
        assert!(
            link()
                .disagreement(&lease(), &"example".parse().unwrap())
                .is_none()
        );
    }

    #[test]
    fn every_bound_field_is_compared() {
        let project: ProjectId = "example".parse().unwrap();

        let mut wrong_project = link();
        wrong_project.project_id = "other".parse().unwrap();
        assert!(
            wrong_project
                .disagreement(&lease(), &project)
                .unwrap()
                .contains("project_id")
        );

        let mut wrong_card = link();
        wrong_card.card_id = "F-999".parse().unwrap();
        assert!(
            wrong_card
                .disagreement(&lease(), &project)
                .unwrap()
                .contains("card_id")
        );

        let mut wrong_lease = link();
        wrong_lease.lease_id = "L-000999".parse().unwrap();
        assert!(
            wrong_lease
                .disagreement(&lease(), &project)
                .unwrap()
                .contains("lease_id")
        );

        let mut wrong_revision = link();
        wrong_revision.card_revision = 2;
        assert!(
            wrong_revision
                .disagreement(&lease(), &project)
                .unwrap()
                .contains("card_revision")
        );
    }

    #[test]
    fn a_released_lease_no_longer_holds() {
        let mut released = lease();
        assert!(released.is_held());
        released.status = LeaseStatus::Released;
        released.released_at = Some(stamp());
        assert!(!released.is_held());
    }

    #[test]
    fn records_round_trip_through_json() {
        let mut record = lease();
        record.progress.push(ProgressNote {
            recorded_at: stamp(),
            note: "started".to_owned(),
            head_sha: Some("b".repeat(40)),
        });
        let encoded = serde_json::to_string_pretty(&record).unwrap();
        let decoded: LeaseRecord = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, record);

        let encoded_link = serde_json::to_string_pretty(&link()).unwrap();
        let decoded_link: WorktreeLink = serde_json::from_str(&encoded_link).unwrap();
        assert_eq!(decoded_link, link());
    }

    #[test]
    fn unknown_fields_are_rejected_on_load() {
        let mut value = serde_json::to_value(lease()).unwrap();
        value["surprise"] = serde_json::json!(1);
        assert!(serde_json::from_value::<LeaseRecord>(value).is_err());
    }

    /// #80: a lease with no checkpoints is not silent until a full
    /// threshold has elapsed since `granted_at` itself, and is silent from
    /// the instant it has.
    #[test]
    fn a_lease_with_no_checkpoints_turns_silent_exactly_at_the_threshold() {
        let record = lease(); // granted_at = stamp(), progress = []
        let base = stamp().unix_seconds();

        let one_second_short =
            FixedClock::at_unix_seconds(base + SILENT_LEASE_THRESHOLD_SECONDS - 1)
                .unwrap()
                .now();
        assert!(
            !record.is_silent(one_second_short),
            "must not be silent one second before the threshold"
        );

        let exactly_at_threshold =
            FixedClock::at_unix_seconds(base + SILENT_LEASE_THRESHOLD_SECONDS)
                .unwrap()
                .now();
        assert!(
            record.is_silent(exactly_at_threshold),
            "must be silent the instant the threshold is reached"
        );
    }

    /// #80 §10 mutation 3's proof: a checkpoint after `granted_at` is what
    /// `last_activity_at` must track, not `granted_at` alone. A lease
    /// granted long enough ago to be silent on `granted_at` alone must read
    /// as active again once a recent checkpoint lands.
    #[test]
    fn a_recent_progress_note_counts_as_a_sign_of_life() {
        let mut record = lease();
        let base = stamp().unix_seconds();
        let checkpoint_at = FixedClock::at_unix_seconds(base + SILENT_LEASE_THRESHOLD_SECONDS - 5)
            .unwrap()
            .now();
        record.progress.push(ProgressNote {
            recorded_at: checkpoint_at,
            note: "still going".to_owned(),
            head_sha: None,
        });

        // Elapsed since `granted_at` alone already exceeds the threshold;
        // elapsed since the checkpoint does not.
        let query_at = FixedClock::at_unix_seconds(base + SILENT_LEASE_THRESHOLD_SECONDS + 5)
            .unwrap()
            .now();
        assert_eq!(record.last_activity_at(), checkpoint_at);
        assert!(
            !record.is_silent(query_at),
            "a checkpoint 10s ago must count as a sign of life even though granted_at is old"
        );
    }

    #[test]
    fn last_activity_at_is_the_maximum_recorded_timestamp_even_written_out_of_order() {
        let mut record = lease();
        let base = stamp().unix_seconds();
        let earlier = FixedClock::at_unix_seconds(base + 100).unwrap().now();
        let later = FixedClock::at_unix_seconds(base + 500).unwrap().now();
        // Pushed out of chronological order: the newer note first.
        record.progress.push(ProgressNote {
            recorded_at: later,
            note: "second".to_owned(),
            head_sha: None,
        });
        record.progress.push(ProgressNote {
            recorded_at: earlier,
            note: "first, appended after, timestamped before".to_owned(),
            head_sha: None,
        });
        assert_eq!(record.last_activity_at(), later);
    }

    /// A released lease is never silent, no matter how old: nobody is
    /// waiting on a released allocation.
    ///
    /// The offset is a fixed 100 years, deliberately not derived from
    /// `SILENT_LEASE_THRESHOLD_SECONDS`: this test's claim ("released is
    /// never silent") does not depend on that constant's value at all, and
    /// deriving from it made this test's own fixture construction overflow
    /// under §10 mutation 1 (an enormous mutated threshold), which would
    /// have obscured that mutation's real effect behind an unrelated
    /// `time`-crate range panic instead of a clean assertion result.
    #[test]
    fn a_released_lease_is_never_silent_regardless_of_age() {
        let mut record = lease();
        record.status = LeaseStatus::Released;
        record.released_at = Some(stamp());
        let base = stamp().unix_seconds();
        let hundred_years_seconds = 100 * 365 * 24 * 3600;
        let far_future = FixedClock::at_unix_seconds(base + hundred_years_seconds)
            .unwrap()
            .now();
        assert!(!record.is_silent(far_future));
    }
}
