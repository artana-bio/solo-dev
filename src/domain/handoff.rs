//! Handoffs: binding an exact candidate to the evidence a reviewer receives.
//!
//! Section 10.7. A handoff has two halves that must not be confused. The
//! machine-computed half is derived from Git objects and control state, and is
//! trustworthy. The actor-authored half is a claim, and `SPIKE-001` finding F-5
//! showed exactly why it must be labelled as one: a reviewer caught an
//! implementer's declaration asserting behavior the code did not have.
//!
//! The `delivered_sha` field exists because of `SPIKE-001` finding F-1. Nothing
//! previously bound the commit an actor said they produced to the commit that
//! reached review, so a branch rewritten in that window yielded a handoff that
//! was internally consistent and completely wrong about what was reviewed.

use serde::{Deserialize, Serialize};

use crate::{
    domain::{
        clock::Timestamp,
        digest::{CANONICAL_ALGORITHM, Digest},
        ids::{CardId, CycleId},
    },
    error::{ErrorCode, HarnessError},
    git::diff::ChangedPath,
};

/// Schema identifier for a handoff.
pub const HANDOFF_SCHEMA: &str = "harness.handoff/v1";

/// Directory holding handoffs, relative to the control repository.
pub const HANDOFF_DIR: &str = "handoffs";

/// Whether a handoff still stands.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum HandoffStatus {
    /// The handoff describes the current candidate.
    Active,
    /// The handoff was withdrawn or superseded.
    Revoked,
}

/// What the feature actor claims about their work.
///
/// Every field is required. A handoff whose author skipped the assumptions is
/// not a cheaper handoff; it is one where the reviewer cannot tell whether
/// there were none or whether nobody thought about it.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ActorDeclaration {
    /// The commit the actor says they produced.
    ///
    /// Compared against the branch head when the handoff is created. See F-1.
    pub delivered_sha: String,
    /// What the code now does.
    pub behavior_delivered: String,
    /// Choices the actor made and why.
    pub implementation_decisions: Vec<String>,
    /// Anything inferred rather than specified.
    pub assumptions: Vec<String>,
    /// What was deliberately not done.
    pub known_limitations: Vec<String>,
    /// What could still be wrong.
    pub residual_risks: Vec<String>,
    /// How to undo the change.
    pub rollback_notes: String,
}

impl ActorDeclaration {
    /// Checks the fields a reviewer cannot work without.
    ///
    /// # Errors
    ///
    /// Returns a policy error naming the first missing field.
    pub fn validate(&self) -> Result<(), HarnessError> {
        let reject = |field: &str| HarnessError::Control {
            reason: format!(
                "handoff declaration is missing `{field}`; a reviewer cannot distinguish an empty field from an unconsidered one"
            ),
            code: ErrorCode::PolicyIncompleteHandoff,
        };

        if self.behavior_delivered.trim().is_empty() {
            return Err(reject("behavior_delivered"));
        }
        if self.rollback_notes.trim().is_empty() {
            return Err(reject("rollback_notes"));
        }
        if self.implementation_decisions.is_empty() {
            return Err(reject("implementation_decisions"));
        }
        if self.delivered_sha.len() != 40
            || !self.delivered_sha.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(HarnessError::Control {
                reason: format!(
                    "delivered_sha must be a full 40-character object ID, found `{}`",
                    self.delivered_sha
                ),
                code: ErrorCode::PolicyIncompleteHandoff,
            });
        }
        Ok(())
    }
}

/// A gate receipt as summarized inside a handoff.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EvidenceEntry {
    /// The gate that ran.
    pub gate_id: String,
    /// The exact gate definition that ran.
    pub gate_digest: Digest,
    /// The receipt recording the run.
    pub receipt_id: String,
    /// Whether it passed.
    pub passed: bool,
    /// The commit it evaluated.
    pub evaluated_sha: String,
}

/// One exact candidate, with everything a reviewer needs to judge it.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HandoffRecord {
    /// Always [`HANDOFF_SCHEMA`].
    pub schema: String,
    /// Identifies this handoff.
    pub handoff_id: String,
    /// The card being handed off.
    pub card_id: CardId,
    /// The card revision in force.
    pub card_revision: u32,
    /// The card digest the handoff is bound to.
    pub card_digest: Digest,
    /// The cycle it belongs to.
    pub cycle_id: CycleId,
    /// The frozen cycle baseline.
    pub baseline_sha: String,
    /// The branch the candidate lives on.
    pub branch: String,
    /// The exact candidate commit.
    pub candidate_sha: String,
    /// Commits the candidate introduces, oldest first.
    pub commits: Vec<String>,
    /// Every path the candidate changed, including rename sources.
    pub changed_paths: Vec<ChangedPath>,
    /// Gate evidence in force at handoff time.
    pub receipts: Vec<EvidenceEntry>,
    /// True when the worktree was clean.
    pub worktree_clean: bool,
    /// What the actor claims.
    pub declaration: ActorDeclaration,
    /// Who handed off. Declared, not proven; see D-013.
    pub actor_id: String,
    /// When it was created.
    pub created_at: Timestamp,
    /// Whether it still stands.
    pub status: HandoffStatus,
    /// The canonicalization algorithm its digest was computed under.
    ///
    /// `SPIKE-001` finding F-2: a holder of the record must be able to
    /// recompute the digest without consulting the harness source.
    pub canonical_algorithm: String,
}

impl HandoffRecord {
    /// Relative path of a handoff inside the control repository.
    #[must_use]
    pub fn relative_path(handoff_id: &str) -> String {
        format!("{HANDOFF_DIR}/{handoff_id}.json")
    }

    /// The handoff's canonical digest.
    ///
    /// # Errors
    ///
    /// Returns an error when the record cannot be serialized.
    pub fn digest(&self) -> Result<Digest, HarnessError> {
        Digest::of_canonical(self)
    }

    /// The canonicalization algorithm handoffs are digested under.
    #[must_use]
    pub const fn canonical_algorithm() -> &'static str {
        CANONICAL_ALGORITHM
    }

    /// True when this handoff still describes the given candidate.
    #[must_use]
    pub fn is_current_for(&self, candidate_sha: &str, card_digest: &Digest) -> bool {
        self.status == HandoffStatus::Active
            && self.candidate_sha == candidate_sha
            && self.card_digest == *card_digest
    }

    /// Explains why this handoff no longer applies, if it does not.
    #[must_use]
    pub fn staleness(&self, candidate_sha: &str, card_digest: &Digest) -> Option<String> {
        if self.status == HandoffStatus::Revoked {
            return Some("the handoff was revoked".to_owned());
        }
        if self.candidate_sha != candidate_sha {
            return Some(format!(
                "handoff describes candidate {} but the branch is now {candidate_sha}",
                self.candidate_sha
            ));
        }
        if self.card_digest != *card_digest {
            return Some(format!(
                "handoff was bound to card digest {} but the card is now {card_digest}",
                self.card_digest
            ));
        }
        None
    }
}

/// Compares what the actor says they delivered with what the branch holds.
///
/// `SPIKE-001` finding F-1. In the spike, a branch was rewritten between the
/// implementer finishing and the reviewer looking, and nothing noticed. The
/// implementer caught it by reading the reflog, which is not a control.
///
/// # Errors
///
/// Returns a policy error naming both SHAs when they disagree.
pub fn check_delivered_sha(declared: &str, actual_head: &str) -> Result<(), HarnessError> {
    if declared == actual_head {
        return Ok(());
    }
    Err(HarnessError::Control {
        reason: format!(
            "the branch was rewritten after delivery: the actor delivered {declared} but the branch now holds {actual_head}; review would have examined code the actor did not produce"
        ),
        code: ErrorCode::PolicyDeliveredShaMismatch,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::clock::{Clock as _, FixedClock};

    fn declaration() -> ActorDeclaration {
        ActorDeclaration {
            delivered_sha: "a".repeat(40),
            behavior_delivered: "converts temperatures".to_owned(),
            implementation_decisions: vec!["type check before range check".to_owned()],
            assumptions: vec![],
            known_limitations: vec![],
            residual_risks: vec![],
            rollback_notes: "revert the commit".to_owned(),
        }
    }

    fn handoff() -> HandoffRecord {
        HandoffRecord {
            schema: HANDOFF_SCHEMA.to_owned(),
            handoff_id: "F-001-r1-aaaaaaa".to_owned(),
            card_id: "F-001".parse().unwrap(),
            card_revision: 1,
            card_digest: Digest::of_bytes(b"card"),
            cycle_id: "C-001".parse().unwrap(),
            baseline_sha: "b".repeat(40),
            branch: "card/F-001".to_owned(),
            candidate_sha: "a".repeat(40),
            commits: vec!["a".repeat(40)],
            changed_paths: vec![],
            receipts: vec![],
            worktree_clean: true,
            declaration: declaration(),
            actor_id: "alvaro".to_owned(),
            created_at: FixedClock::at_unix_seconds(1_785_196_800).unwrap().now(),
            status: HandoffStatus::Active,
            canonical_algorithm: CANONICAL_ALGORITHM.to_owned(),
        }
    }

    #[test]
    fn a_matching_delivered_sha_is_accepted() {
        check_delivered_sha(&"a".repeat(40), &"a".repeat(40)).expect("these agree");
    }

    #[test]
    fn a_rewritten_branch_is_refused_and_names_both_shas() {
        // SPIKE-001 F-1: the exact situation the spike surfaced.
        let delivered = "a".repeat(40);
        let rewritten = "b".repeat(40);
        let error = check_delivered_sha(&delivered, &rewritten).expect_err("must refuse");

        assert_eq!(error.code(), ErrorCode::PolicyDeliveredShaMismatch);
        assert!(error.to_string().contains(&delivered));
        assert!(error.to_string().contains(&rewritten));
        assert!(
            error.to_string().contains("did not produce"),
            "the message must say what actually goes wrong"
        );
    }

    #[test]
    fn a_declaration_missing_required_narrative_is_refused() {
        for mutate in [
            (|d: &mut ActorDeclaration| d.behavior_delivered = "  ".to_owned())
                as fn(&mut ActorDeclaration),
            |d: &mut ActorDeclaration| d.rollback_notes = String::new(),
            |d: &mut ActorDeclaration| d.implementation_decisions.clear(),
        ] {
            let mut invalid = declaration();
            mutate(&mut invalid);
            let error = invalid.validate().expect_err("must refuse");
            assert_eq!(error.code(), ErrorCode::PolicyIncompleteHandoff);
        }
    }

    #[test]
    fn empty_assumptions_and_risks_are_permitted() {
        // These may legitimately be empty; the point is the author had to say
        // so rather than omit the field.
        declaration()
            .validate()
            .expect("empty lists are a valid claim");
    }

    #[test]
    fn a_delivered_sha_must_be_a_full_object_id() {
        for bad in ["abc", "", &"g".repeat(40), &"a".repeat(39)] {
            let mut invalid = declaration();
            invalid.delivered_sha = bad.to_owned();
            assert!(invalid.validate().is_err(), "`{bad}` must be refused");
        }
    }

    #[test]
    fn a_handoff_applies_only_to_its_exact_candidate_and_card() {
        let record = handoff();
        let card = Digest::of_bytes(b"card");
        assert!(record.is_current_for(&"a".repeat(40), &card));
        assert!(!record.is_current_for(&"b".repeat(40), &card));
        assert!(!record.is_current_for(&"a".repeat(40), &Digest::of_bytes(b"revised card")));
    }

    #[test]
    fn staleness_explains_which_binding_broke() {
        let record = handoff();
        let card = Digest::of_bytes(b"card");
        assert!(record.staleness(&"a".repeat(40), &card).is_none());

        let moved = record.staleness(&"c".repeat(40), &card).unwrap();
        assert!(moved.contains("branch is now"), "{moved}");

        let revised = record
            .staleness(&"a".repeat(40), &Digest::of_bytes(b"revised"))
            .unwrap();
        assert!(revised.contains("card is now"), "{revised}");
    }

    #[test]
    fn a_revoked_handoff_never_applies() {
        let revoked = HandoffRecord {
            status: HandoffStatus::Revoked,
            ..handoff()
        };
        let card = Digest::of_bytes(b"card");
        assert!(!revoked.is_current_for(&"a".repeat(40), &card));
        assert_eq!(
            revoked.staleness(&"a".repeat(40), &card).as_deref(),
            Some("the handoff was revoked")
        );
    }

    #[test]
    fn a_handoff_names_its_canonicalization_algorithm() {
        assert_eq!(handoff().canonical_algorithm, CANONICAL_ALGORITHM);
        assert_eq!(HandoffRecord::canonical_algorithm(), CANONICAL_ALGORITHM);
    }

    #[test]
    fn a_material_change_moves_the_handoff_digest() {
        let base = handoff().digest().unwrap();
        let mut changed = handoff();
        changed.candidate_sha = "c".repeat(40);
        assert_ne!(base, changed.digest().unwrap());

        let mut narrative = handoff();
        narrative.declaration.behavior_delivered = "something else".to_owned();
        assert_ne!(
            base,
            narrative.digest().unwrap(),
            "the actor's claim is part of what a review is bound to"
        );
    }

    #[test]
    fn a_record_round_trips_and_rejects_unknown_fields() {
        let record = handoff();
        let encoded = serde_json::to_string_pretty(&record).unwrap();
        let decoded: HandoffRecord = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, record);
        assert_eq!(decoded.digest().unwrap(), record.digest().unwrap());

        let mut value = serde_json::to_value(&record).unwrap();
        value["surprise"] = serde_json::json!(1);
        assert!(serde_json::from_value::<HandoffRecord>(value).is_err());
    }
}
