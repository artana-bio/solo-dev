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
}
