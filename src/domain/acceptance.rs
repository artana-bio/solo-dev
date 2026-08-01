//! Acceptance: the human decision that authorizes promotion.
//!
//! Section 10.9 makes this the only thing that can authorize moving the
//! protected branch. Everything before it — gates, reviews, verification —
//! establishes facts; acceptance is where someone takes responsibility for what
//! those facts add up to. The record names that someone, and D-013 is explicit
//! that the identity is declared rather than proven.

use serde::{Deserialize, Serialize};

use crate::{
    domain::{
        clock::Timestamp,
        digest::{CANONICAL_ALGORITHM, Digest},
        ids::{AcceptanceId, IntegrationId},
    },
    error::HarnessError,
    policy::hygiene,
};

/// Schema identifier for an acceptance record.
pub const ACCEPTANCE_SCHEMA: &str = "harness.acceptance/v1";

/// Directory holding acceptance records, relative to the control repository.
pub const ACCEPTANCE_DIR: &str = "acceptances";

/// What the acceptance owner concluded.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceDecision {
    /// Promotion is authorized.
    Accepted,
    /// Promotion is refused; the integration does not land as it stands.
    Rejected,
}

impl AcceptanceDecision {
    /// Its stable serialized name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
        }
    }

    /// True when this decision authorizes promotion.
    ///
    /// Section 10.9: only `accepted` does. Written as its own method so no
    /// caller has to remember that a rejection is not merely "not yet".
    #[must_use]
    pub const fn authorizes_promotion(self) -> bool {
        matches!(self, Self::Accepted)
    }
}

/// One recorded acceptance decision over one landing commit.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceRecord {
    /// Always [`ACCEPTANCE_SCHEMA`].
    pub schema: String,
    /// Identifies this acceptance.
    pub acceptance_id: AcceptanceId,
    /// The integration accepted.
    pub integration_id: IntegrationId,
    /// The exact commit accepted.
    pub landing_sha: String,
    /// Digest of the integration record as it stood when accepted.
    pub integration_record_digest: Digest,
    /// The receipts the decision rests on.
    pub receipt_ids: Vec<String>,
    /// Who accepted it. Declared, not proven; see D-013.
    pub acceptance_owner: String,
    /// The conclusion.
    pub decision: AcceptanceDecision,
    /// Risks accepted along with the change.
    pub residual_risks: Vec<String>,
    /// How to undo this if it turns out badly.
    pub rollback_reference: String,
    /// When it was recorded.
    pub accepted_at: Timestamp,
    /// The canonicalization algorithm its digest was computed under.
    pub canonical_algorithm: String,
}

impl AcceptanceRecord {
    /// Relative path of an acceptance inside the control repository.
    #[must_use]
    pub fn relative_path(acceptance_id: &AcceptanceId) -> String {
        format!("{ACCEPTANCE_DIR}/{acceptance_id}.json")
    }

    /// Refuses a record carrying a recognized credential.
    ///
    /// The acceptance was the one durable record with no hygiene check at all,
    /// which is a poor place for the omission: it is the record that
    /// authorizes publishing a commit, so it is the one an auditor is most
    /// likely to read years later.
    ///
    /// # Errors
    ///
    /// Returns a policy error naming the offending field and not its value.
    pub fn validate(&self) -> Result<(), HarnessError> {
        hygiene::refuse_secrets([
            (
                "acceptance.acceptance_owner",
                self.acceptance_owner.as_str(),
            ),
            (
                "acceptance.rollback_reference",
                self.rollback_reference.as_str(),
            ),
        ])?;
        for (index, risk) in self.residual_risks.iter().enumerate() {
            hygiene::refuse_secret(&format!("acceptance.residual_risks[{index}]"), risk)?;
        }
        Ok(())
    }

    /// The acceptance's canonical digest.
    ///
    /// # Errors
    ///
    /// Returns an error when the record cannot be serialized.
    pub fn digest(&self) -> Result<Digest, HarnessError> {
        Digest::of_canonical(self)
    }

    /// The canonicalization algorithm acceptances are digested under.
    #[must_use]
    pub const fn canonical_algorithm() -> &'static str {
        CANONICAL_ALGORITHM
    }

    /// Explains why this acceptance does not authorize promoting the given
    /// landing commit, if it does not.
    ///
    /// Both the decision and the exact SHA are checked. Either alone would let
    /// something through: a rejection for the right commit, or an approval for
    /// a commit that has since been rebuilt.
    #[must_use]
    pub fn blocks_promotion_of(&self, landing_sha: &str) -> Option<String> {
        if !self.decision.authorizes_promotion() {
            return Some(format!(
                "acceptance {} recorded `{}`",
                self.acceptance_id,
                self.decision.name()
            ));
        }
        if self.landing_sha != landing_sha {
            return Some(format!(
                "acceptance {} accepted landing commit {} but the integration now holds {landing_sha}",
                self.acceptance_id, self.landing_sha
            ));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::clock::{Clock, FixedClock};

    fn record(decision: AcceptanceDecision, landing_sha: &str) -> AcceptanceRecord {
        AcceptanceRecord {
            schema: ACCEPTANCE_SCHEMA.to_owned(),
            acceptance_id: "ACC-000001".parse().unwrap(),
            integration_id: "INT-001".parse().unwrap(),
            landing_sha: landing_sha.to_owned(),
            integration_record_digest: Digest::of_bytes(b"integration"),
            receipt_ids: vec!["R-000001".to_owned()],
            acceptance_owner: "owner".to_owned(),
            decision,
            residual_risks: Vec::new(),
            rollback_reference: "revert the landing commit".to_owned(),
            accepted_at: FixedClock::at_unix_seconds(1_785_196_800).unwrap().now(),
            canonical_algorithm: CANONICAL_ALGORITHM.to_owned(),
        }
    }

    #[test]
    fn an_acceptance_authorizes_only_the_commit_it_named() {
        let accepted = record(AcceptanceDecision::Accepted, &"a".repeat(40));
        assert!(accepted.blocks_promotion_of(&"a".repeat(40)).is_none());

        let reason = accepted
            .blocks_promotion_of(&"b".repeat(40))
            .expect("a different commit is not authorized");
        assert!(reason.contains("but the integration now holds"), "{reason}");
    }

    #[test]
    fn a_rejection_never_authorizes_promotion() {
        let rejected = record(AcceptanceDecision::Rejected, &"a".repeat(40));
        let reason = rejected
            .blocks_promotion_of(&"a".repeat(40))
            .expect("a rejection blocks its own commit");
        assert!(reason.contains("rejected"), "{reason}");
        assert!(!AcceptanceDecision::Rejected.authorizes_promotion());
    }

    #[test]
    fn the_digest_covers_the_decision() {
        // Two records identical but for the verdict must not share a digest,
        // or an approval could be swapped for a rejection undetected.
        let accepted = record(AcceptanceDecision::Accepted, &"a".repeat(40));
        let rejected = record(AcceptanceDecision::Rejected, &"a".repeat(40));
        assert_ne!(
            accepted.digest().unwrap(),
            rejected.digest().unwrap(),
            "the digest must bind the decision itself"
        );
    }
}
