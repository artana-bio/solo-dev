//! Integration: the deterministic combination of approved candidates.
//!
//! An integration record is a *plan*, not a result. It names the exact
//! candidates selected, the order they must be merged in, and the authority
//! commit they were selected against. Everything downstream — preflight,
//! landing, promotion — reads this record rather than re-deciding, which is
//! what makes the sequence reproducible and auditable.
//!
//! `SPIKE-001` finding F-3 shapes one deliverable here. The spike produced
//! approved candidates that nothing consumed: approval was recorded, and then
//! the only way to know what was awaiting integration was to remember. The
//! ready-to-integrate view exists so that state is answerable from harness
//! records alone.

use serde::{Deserialize, Serialize};

use crate::{
    domain::{
        clock::Timestamp,
        digest::{CANONICAL_ALGORITHM, Digest},
        ids::{CardId, CycleId, IntegrationId, ReviewId},
    },
    error::{ErrorCode, HarnessError},
};

/// Schema identifier for an integration record.
pub const INTEGRATION_SCHEMA: &str = "harness.integration/v1";

/// Directory holding integration records, relative to the control repository.
pub const INTEGRATION_DIR: &str = "integrations";

/// Where an integration is in its lifecycle.
///
/// Section 11.3.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationStatus {
    /// Selected but not yet prepared.
    Draft,
    /// Members are fixed and the baseline is recorded.
    Prepared,
    /// Combined verification passed.
    Verified,
    /// Integration review recorded.
    Reviewed,
    /// Acceptance recorded; promotion is authorized.
    Accepted,
    /// The protected branch has moved to the landing commit.
    Promoted,
    /// Refs archived and working state cleaned up.
    Archived,
    /// Halted pending a decision outside this integration.
    Blocked,
    /// Abandoned before promotion.
    Abandoned,
}

impl IntegrationStatus {
    /// Its stable serialized name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Prepared => "prepared",
            Self::Verified => "verified",
            Self::Reviewed => "reviewed",
            Self::Accepted => "accepted",
            Self::Promoted => "promoted",
            Self::Archived => "archived",
            Self::Blocked => "blocked",
            Self::Abandoned => "abandoned",
        }
    }

    /// True when this integration still holds its cycle's integration lease.
    ///
    /// An integration that has not reached a terminal state is an outstanding
    /// claim on the cycle. This is what makes the lease "one per cycle"
    /// without a separate lease record: the non-terminal record *is* the
    /// claim, so it cannot drift out of agreement with itself.
    #[must_use]
    pub const fn holds_lease(self) -> bool {
        !matches!(self, Self::Archived | Self::Abandoned)
    }

    /// The states this one may transition to.
    #[must_use]
    pub fn successors(self) -> &'static [Self] {
        match self {
            Self::Draft => &[Self::Prepared, Self::Blocked, Self::Abandoned],
            Self::Prepared => &[Self::Verified, Self::Blocked, Self::Abandoned],
            Self::Verified => &[Self::Reviewed, Self::Blocked, Self::Abandoned],
            Self::Reviewed => &[Self::Accepted, Self::Blocked, Self::Abandoned],
            Self::Accepted => &[Self::Promoted, Self::Abandoned],
            Self::Promoted => &[Self::Archived],
            Self::Blocked => &[Self::Prepared, Self::Abandoned],
            Self::Archived | Self::Abandoned => &[],
        }
    }

    /// Checks a transition against Section 11.3.
    ///
    /// # Errors
    ///
    /// Returns a policy error when the transition is not permitted.
    pub fn check_transition(self, next: Self) -> Result<(), HarnessError> {
        if self.successors().contains(&next) {
            return Ok(());
        }
        Err(HarnessError::Control {
            reason: format!(
                "integration cannot move from {} to {}",
                self.name(),
                next.name()
            ),
            code: ErrorCode::PolicyInvalidTransition,
        })
    }
}

/// Whether an integration lands one card or a set of them.
///
/// `WP-410` requires this to be stated rather than implied. It is recorded on
/// the record instead of inferred from member count, because "a batch that
/// happens to hold one card" and "a single-card integration" carry different
/// expectations downstream, and a reader counting members cannot tell them
/// apart after the fact.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationMode {
    /// Exactly one card lands in this integration.
    Individual,
    /// Several cards land together as one promotion.
    Batch,
}

impl IntegrationMode {
    /// Its stable serialized name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Individual => "individual",
            Self::Batch => "batch",
        }
    }
}

/// One approved candidate selected into an integration.
///
/// Every binding the downstream steps need is pinned here. Re-deriving any of
/// them at merge time would reintroduce exactly the drift `SPIKE-001` finding
/// F-1 closed for handoffs.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct IntegrationMember {
    /// The card being integrated.
    pub card_id: CardId,
    /// The card revision the approval was against.
    pub card_revision: u32,
    /// The card digest the approval was bound to.
    pub card_digest: Digest,
    /// The exact commit that was approved.
    pub candidate_sha: String,
    /// The branch carrying that commit.
    pub branch: String,
    /// The approving review.
    pub review_id: ReviewId,
    /// Digest of that review, so the approval itself is pinned.
    pub review_digest: Digest,
    /// The handoff the approval reviewed.
    pub handoff_id: String,
    /// The atomic group this card belongs to, when it has one.
    pub atomic_group: Option<String>,
}

/// One planned combination of approved candidates.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct IntegrationRecord {
    /// Always [`INTEGRATION_SCHEMA`].
    pub schema: String,
    /// Identifies this integration.
    pub integration_id: IntegrationId,
    /// The cycle it integrates.
    pub cycle_id: CycleId,
    /// Where it is in its lifecycle.
    pub status: IntegrationStatus,
    /// Whether it lands one card or several.
    pub mode: IntegrationMode,
    /// The cycle's frozen baseline.
    pub baseline_sha: String,
    /// The authority protected-branch commit this plan was built against.
    ///
    /// Section 13.6 verifies the protected branch still equals this value
    /// before promoting. Recording it at prepare time is what makes that check
    /// meaningful: it is the commit the merge order was actually computed
    /// against, not whatever the branch happens to be later.
    pub expected_main_sha: String,
    /// The selected candidates, in the order they must be merged.
    pub members: Vec<IntegrationMember>,
    /// Atomic groups fully represented in this integration.
    pub atomic_groups: Vec<String>,
    /// The merged integration commit, once `integration merge` has built it.
    ///
    /// Absent until the candidates have actually been combined. `WP-430` makes
    /// the landing commit from this, and `WP-440` verifies it; both need to
    /// distinguish "not merged yet" from "merged to nothing", which a plain
    /// empty string would not.
    pub integration_head: Option<String>,
    /// The tree that commit carries.
    pub integration_tree: Option<String>,
    /// When the candidates were combined.
    pub merged_at: Option<Timestamp>,
    /// The landing commit promotion will publish, once it has been built.
    ///
    /// Section 13.5 requires it to exist before final verification and to stay
    /// unreachable from the protected branch until accepted, so it is recorded
    /// here rather than inferred from a ref: a ref can be deleted, and the
    /// record is what promotion reloads.
    pub landing_sha: Option<String>,
    /// When the landing commit was built.
    pub landed_at: Option<Timestamp>,
    /// Who prepared it. Declared, not proven; see D-013.
    pub prepared_by: String,
    /// When it was prepared.
    pub prepared_at: Timestamp,
    /// The canonicalization algorithm its digest was computed under.
    pub canonical_algorithm: String,
}

impl IntegrationRecord {
    /// Relative path of an integration inside the control repository.
    #[must_use]
    pub fn relative_path(integration_id: &IntegrationId) -> String {
        format!("{INTEGRATION_DIR}/{integration_id}.json")
    }

    /// The integration's canonical digest.
    ///
    /// # Errors
    ///
    /// Returns an error when the record cannot be serialized.
    pub fn digest(&self) -> Result<Digest, HarnessError> {
        Digest::of_canonical(self)
    }

    /// The canonicalization algorithm integrations are digested under.
    #[must_use]
    pub const fn canonical_algorithm() -> &'static str {
        CANONICAL_ALGORITHM
    }

    /// The cards this integration lands, in merge order.
    pub fn card_ids(&self) -> impl Iterator<Item = &CardId> {
        self.members.iter().map(|member| &member.card_id)
    }

    /// Finds a member by card.
    #[must_use]
    pub fn member(&self, card_id: &CardId) -> Option<&IntegrationMember> {
        self.members
            .iter()
            .find(|member| member.card_id == *card_id)
    }
}

/// Orders cards so every dependency precedes its dependents.
///
/// Kahn's algorithm, with ties broken by card identifier. The tie-break is not
/// cosmetic: without it the order would depend on map iteration or input
/// order, and `WP-410` requires the same selection to always produce the same
/// merge sequence. Only dependencies *within the selection* constrain the
/// order; a dependency on a card that already landed is checked elsewhere.
///
/// # Errors
///
/// Returns a conflict error naming the cards involved in a dependency cycle.
pub fn topological_order(selected: &[(CardId, Vec<CardId>)]) -> Result<Vec<CardId>, HarnessError> {
    let members: Vec<&CardId> = selected.iter().map(|(card, _)| card).collect();
    let mut remaining: Vec<(CardId, Vec<CardId>)> = selected
        .iter()
        .map(|(card, dependencies)| {
            let inside = dependencies
                .iter()
                .filter(|dependency| members.contains(dependency))
                .cloned()
                .collect();
            (card.clone(), inside)
        })
        .collect();
    remaining.sort_by(|left, right| left.0.as_str().cmp(right.0.as_str()));

    let mut ordered: Vec<CardId> = Vec::with_capacity(remaining.len());
    while !remaining.is_empty() {
        // The first ready card in identifier order, so the result is a
        // function of the selection alone.
        let next = remaining
            .iter()
            .position(|(_, dependencies)| {
                dependencies
                    .iter()
                    .all(|dependency| ordered.contains(dependency))
            })
            .ok_or_else(|| {
                let involved: Vec<&str> = remaining.iter().map(|(card, _)| card.as_str()).collect();
                HarnessError::Control {
                    reason: format!(
                        "selected cards form a dependency cycle: {}",
                        involved.join(", ")
                    ),
                    code: ErrorCode::PolicyDependencyCycle,
                }
            })?;
        ordered.push(remaining.remove(next).0);
    }
    Ok(ordered)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str) -> CardId {
        id.parse().expect("a card id")
    }

    fn ids(cards: &[CardId]) -> Vec<&str> {
        cards.iter().map(CardId::as_str).collect()
    }

    #[test]
    fn independent_cards_order_by_identifier() {
        let order = topological_order(&[
            (card("F-003"), vec![]),
            (card("F-001"), vec![]),
            (card("F-002"), vec![]),
        ])
        .expect("an order");
        assert_eq!(ids(&order), ["F-001", "F-002", "F-003"]);
    }

    #[test]
    fn dependencies_precede_their_dependents() {
        let order = topological_order(&[
            (card("F-001"), vec![card("F-003")]),
            (card("F-002"), vec![card("F-001")]),
            (card("F-003"), vec![]),
        ])
        .expect("an order");
        assert_eq!(ids(&order), ["F-003", "F-001", "F-002"]);
    }

    #[test]
    fn ordering_is_deterministic_regardless_of_input_order() {
        let forward = topological_order(&[
            (card("F-001"), vec![]),
            (card("F-002"), vec![card("F-001")]),
            (card("F-003"), vec![card("F-001")]),
        ])
        .expect("an order");
        let reversed = topological_order(&[
            (card("F-003"), vec![card("F-001")]),
            (card("F-002"), vec![card("F-001")]),
            (card("F-001"), vec![]),
        ])
        .expect("an order");
        assert_eq!(ids(&forward), ids(&reversed));
    }

    #[test]
    fn dependencies_outside_the_selection_do_not_constrain_the_order() {
        // F-009 is not selected — it landed in an earlier integration. Its
        // absence must not make F-001 permanently unready.
        let order = topological_order(&[(card("F-001"), vec![card("F-009")])]).expect("an order");
        assert_eq!(ids(&order), ["F-001"]);
    }

    #[test]
    fn a_dependency_cycle_is_a_conflict_naming_its_members() {
        let error = topological_order(&[
            (card("F-001"), vec![card("F-002")]),
            (card("F-002"), vec![card("F-001")]),
        ])
        .expect_err("a cycle should be refused");
        let rendered = error.to_string();
        assert!(rendered.contains("F-001"), "unexpected: {rendered}");
        assert!(rendered.contains("F-002"), "unexpected: {rendered}");
    }

    #[test]
    fn every_status_transition_matches_section_11_3() {
        use IntegrationStatus::{
            Abandoned, Accepted, Archived, Blocked, Draft, Prepared, Promoted, Reviewed, Verified,
        };
        assert!(Draft.check_transition(Prepared).is_ok());
        assert!(Prepared.check_transition(Verified).is_ok());
        assert!(Verified.check_transition(Reviewed).is_ok());
        assert!(Reviewed.check_transition(Accepted).is_ok());
        assert!(Accepted.check_transition(Promoted).is_ok());
        assert!(Promoted.check_transition(Archived).is_ok());
        assert!(Blocked.check_transition(Prepared).is_ok());

        // Skipping verification would let an unverified tree reach promotion.
        assert!(Prepared.check_transition(Accepted).is_err());
        // Promotion cannot be undone by abandoning it.
        assert!(Promoted.check_transition(Abandoned).is_err());
        assert!(Archived.check_transition(Prepared).is_err());
    }

    #[test]
    fn only_terminal_states_release_the_cycle_lease() {
        assert!(IntegrationStatus::Prepared.holds_lease());
        assert!(IntegrationStatus::Blocked.holds_lease());
        assert!(IntegrationStatus::Promoted.holds_lease());
        assert!(!IntegrationStatus::Archived.holds_lease());
        assert!(!IntegrationStatus::Abandoned.holds_lease());
    }
}
