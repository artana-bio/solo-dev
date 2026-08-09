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
        lesson::LessonManifest,
    },
    error::{ErrorCode, HarnessError},
};

/// Schema identifier for an integration record.
pub const INTEGRATION_SCHEMA: &str = "harness.integration/v1";

/// Directory holding integration records, relative to the control repository.
pub const INTEGRATION_DIR: &str = "integrations";

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
}

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
        Err(HarnessError::ControlWithRecovery {
            reason: format!(
                "integration cannot move from {} to {}",
                self.name(),
                next.name()
            ),
            code: ErrorCode::PolicyInvalidTransition,
            recovery: self.transition_recovery(),
        })
    }

    /// Recovery guidance for a refused transition: the states this one may
    /// still reach, not a command.
    ///
    /// #112: this guards *every* integration transition (Section 11.3), not
    /// one command's precondition, so it cannot hardcode any one command's
    /// name — `integration verify` would be wrong advice attached to, say, a
    /// refused `accepted -> archived` move, which has nothing to do with
    /// verification. `domain/integration.rs` also has no CLI vocabulary to
    /// begin with: this module describes states, and the commands that
    /// produce them live in `src/commands/integration.rs` and
    /// `src/commands/acceptance.rs`. Naming the permitted successor
    /// *states* is the answer this function can compute correctly for
    /// every caller, and it keeps that boundary intact rather than
    /// smuggling a command name into a domain type through the one field
    /// that happens to accept a string. Sites that already know their own
    /// single correct command — `acceptance::require_reviewed` and
    /// `check_promotion` in `commands/integration.rs` — say so directly
    /// instead; they are commands already, so naming one is not a layering
    /// violation for them the way it would be here.
    ///
    /// Returns `&'static str`, not a formatted `String`, because
    /// [`HarnessError::ControlWithRecovery`]'s `recovery` field requires
    /// it. There are only nine states, so one literal per state (matching
    /// [`Self::successors`] one arm at a time) costs nothing that function
    /// does not already pay, and `transition_recovery_names_every_successor_state`
    /// below fails if the two drift apart.
    const fn transition_recovery(self) -> &'static str {
        match self {
            Self::Draft => {
                "From `draft`, the permitted next states are `prepared`, `blocked`, or `abandoned`."
            }
            Self::Prepared => {
                "From `prepared`, the permitted next states are `verified`, `blocked`, or `abandoned`."
            }
            Self::Verified => {
                "From `verified`, the permitted next states are `reviewed`, `blocked`, or `abandoned`."
            }
            Self::Reviewed => {
                "From `reviewed`, the permitted next states are `accepted`, `blocked`, or `abandoned`."
            }
            Self::Accepted => {
                "From `accepted`, the permitted next states are `promoted` or `abandoned`."
            }
            Self::Promoted => "From `promoted`, the only permitted next state is `archived`.",
            Self::Blocked => {
                "From `blocked`, the permitted next states are `prepared` or `abandoned`."
            }
            Self::Archived => "`archived` is a terminal state; no further transition is possible.",
            Self::Abandoned => {
                "`abandoned` is a terminal state; no further transition is possible."
            }
        }
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
    /// Whether this is the one complete integration for a sealed cycle.
    #[serde(default, skip_serializing_if = "is_false")]
    pub final_for_cycle: bool,
    /// Digest of the exact sealed cycle record this final plan accounted for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed_cycle_digest: Option<Digest>,
    /// Sealed members deliberately abandoned instead of selected.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub abandoned_card_ids: Vec<CardId>,
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
    /// Digest of the ordered lesson manifests whose obligations were used for
    /// combined verification. Absent only on integrations verified before
    /// governed lessons were introduced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lesson_manifest_digest: Option<Digest>,
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

    /// The digest of everything acceptance binds, ignoring lifecycle position.
    ///
    /// Acceptance authorizes one exact set of members landing as one exact
    /// commit. It does not authorize the record's `status`, which necessarily
    /// moves `reviewed → accepted → promoted → archived` afterwards. Recording
    /// [`digest`](Self::digest) at acceptance and re-deriving it at promotion
    /// therefore compares a value against one that changed for a reason
    /// acceptance never objected to, and would refuse every promotion.
    ///
    /// That is why the recorded digest was never checked: the check could not
    /// have passed. Excluding `status` is what makes checking it possible, and
    /// a check that runs is worth more than a field that binds everything and
    /// is consulted by nothing.
    ///
    /// # Errors
    ///
    /// Returns an error when the record cannot be serialized.
    pub fn substantive_digest(&self) -> Result<Digest, HarnessError> {
        let mut value = serde_json::to_value(self)?;
        if let Some(object) = value.as_object_mut() {
            object.remove("status");
        }
        Digest::of_canonical(&value)
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
        // A draft integration must be prepared, verified, reviewed, and
        // accepted before promotion; command-level checks independently guard
        // promotion today, but this protects the transition table itself.
        assert!(Draft.check_transition(Promoted).is_err());
        // Promotion cannot be undone by abandoning it.
        assert!(Promoted.check_transition(Abandoned).is_err());
        assert!(Archived.check_transition(Prepared).is_err());
    }

    /// #112, §8: `transition_recovery` is hand-written per state rather than
    /// formatted from `successors()` (it has to be — `recovery` is
    /// `&'static str`), so nothing forces the two to agree after an edit to
    /// either. This is the test that would fail if they drifted: a
    /// `successors()` edit that adds or removes a reachable state without a
    /// matching edit to `transition_recovery` fails here on the state whose
    /// text fell out of sync, rather than shipping a recovery message that
    /// silently stops matching what the transition table actually allows.
    #[test]
    fn transition_recovery_names_every_successor_state() {
        use IntegrationStatus::{
            Abandoned, Accepted, Archived, Blocked, Draft, Prepared, Promoted, Reviewed, Verified,
        };
        for status in [
            Draft, Prepared, Verified, Reviewed, Accepted, Promoted, Blocked, Archived, Abandoned,
        ] {
            let recovery = status.transition_recovery();
            for successor in status.successors() {
                assert!(
                    recovery.contains(successor.name()),
                    "{status:?}'s transition_recovery ({recovery:?}) does not name successor `{}`",
                    successor.name()
                );
            }
        }
    }

    /// The reverse of the test above: every *other* state named in a
    /// `transition_recovery` string is a real successor of `status`.
    /// `transition_recovery_names_every_successor_state` only checks that
    /// nothing is missing; a table that also names states that are not
    /// reachable is just as misleading — an operator refused `prepared ->
    /// reviewed` who reads "the permitted next states are `verified`,
    /// `reviewed`, ..." tries `reviewed` again and gets the identical
    /// refusal a second time. Two prior cards in this wave (#107's `Close`,
    /// #121's `Landed`) each shipped recovery text naming an action the
    /// operator could not actually take; this is the same class of mistake
    /// on this card's own new surface, closed at the same time as the
    /// table itself rather than after the fact.
    ///
    /// Matching rule: plain substring containment,
    /// `recovery.contains(other.name())` — the same rule the forward test
    /// above already uses, kept identical rather than inventing a second
    /// one for the other direction. Two things make it defensible here
    /// rather than merely convenient:
    ///
    /// - **Self-reference is excluded on purpose.** Every `transition_recovery`
    ///   string opens by naming its own state ("From `draft`, ..." /
    ///   "`archived` is a terminal state...") to say what the sentence is
    ///   about, not to claim the state can reach itself — `successors()`
    ///   never contains `self`, so counting that mention as a claim would
    ///   make every non-terminal state fail against itself, and would
    ///   false-positive the two terminal states (`archived`, `abandoned`),
    ///   whose text *only* contains their own name and no successor at
    ///   all. Skipping `other == status` in the loop below is what keeps
    ///   "names nothing else" from reading as a violation.
    /// - **What this rule would miss:** a state name occurring as a
    ///   substring of a longer, unrelated word — e.g. prose using
    ///   "unprepared" would trip a naive check for `prepared`, and
    ///   `abandoned` itself contains `abandon` as a substring, so a rule
    ///   that matched stems rather than full names (the way this card's
    ///   own CLI test deliberately does, elsewhere, for `verif`) would
    ///   read `abandon` out of `abandoned` and misfire. None of the nine
    ///   state names are substrings of one another, and today's nine
    ///   `transition_recovery` strings are short, controlled prose with no
    ///   such accidental larger word, so this gap is real but not
    ///   currently live. Recorded here rather than silently relied on.
    #[test]
    fn transition_recovery_names_no_state_that_is_not_a_successor() {
        use IntegrationStatus::{
            Abandoned, Accepted, Archived, Blocked, Draft, Prepared, Promoted, Reviewed, Verified,
        };
        let all = [
            Draft, Prepared, Verified, Reviewed, Accepted, Promoted, Blocked, Archived, Abandoned,
        ];
        for status in all {
            let recovery = status.transition_recovery();
            let successors = status.successors();
            for other in all {
                if other == status {
                    continue; // naming itself is not a reachability claim
                }
                if recovery.contains(other.name()) {
                    assert!(
                        successors.contains(&other),
                        "{status:?}'s transition_recovery ({recovery:?}) names `{}`, which is not a real successor of {status:?} (successors: {successors:?})",
                        other.name()
                    );
                }
            }
        }
    }

    /// #112, test 2: the no-false-positive check for site 1. `check_transition`
    /// guards every integration transition, so a refusal that has nothing to
    /// do with verification — here, an abandoned (terminal) integration
    /// refusing to move anywhere at all — must not claim `verified` is
    /// reachable or relevant. Without this, a site 1 fix that unconditionally
    /// named `verified` (or `integration verify`) for every invalid
    /// transition would pass a test that only checked the `prepared ->
    /// reviewed` case.
    #[test]
    fn a_generic_invalid_transition_does_not_claim_verification_is_needed() {
        let error = IntegrationStatus::Abandoned
            .check_transition(IntegrationStatus::Reviewed)
            .expect_err("abandoned is terminal; no transition is valid");
        let recovery = error.recovery();
        assert!(
            !recovery.to_lowercase().contains("verif"),
            "an abandoned integration's refusal must not mention verification: {recovery:?}"
        );
        assert!(
            recovery.contains("terminal"),
            "expected terminal-state guidance, got: {recovery:?}"
        );
    }

    #[test]
    fn interactions_are_found_in_both_directions_separately() {
        let found = interactions(&[
            (
                card("F-001"),
                vec!["api.orders".to_owned()],
                vec!["api.users".to_owned()],
            ),
            (
                card("F-002"),
                vec!["api.users".to_owned()],
                vec!["api.orders".to_owned()],
            ),
        ]);
        assert_eq!(found.len(), 2, "each direction is its own thing to check");
        assert_eq!(found[0].changes.as_str(), "F-001");
        assert_eq!(found[0].reads.as_str(), "F-002");
        assert_eq!(found[0].shared, ["api.orders"]);
        assert_eq!(found[1].changes.as_str(), "F-002");
        assert_eq!(found[1].reads.as_str(), "F-001");
    }

    #[test]
    fn cards_with_disjoint_contracts_do_not_interact() {
        let found = interactions(&[
            (card("F-001"), vec!["api.orders".to_owned()], vec![]),
            (card("F-002"), vec!["api.users".to_owned()], vec![]),
        ]);
        assert!(found.is_empty(), "unexpected: {found:?}");
    }

    #[test]
    fn a_card_does_not_interact_with_itself() {
        let found = interactions(&[(
            card("F-001"),
            vec!["api.orders".to_owned()],
            vec!["api.orders".to_owned()],
        )]);
        assert!(
            found.is_empty(),
            "a card reading what it changes is ordinary, not an interaction"
        );
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

/// Schema identifier for a combined-verification record.
pub const VERIFICATION_SCHEMA: &str = "harness.integration-verification/v1";
/// Verification schema that binds the exact governed-lesson manifests rerun
/// evidence was selected from.
pub const VERIFICATION_V2_SCHEMA: &str = "harness.integration-verification/v2";

/// Directory holding verification records, relative to the control repository.
pub const VERIFICATION_DIR: &str = "verifications";

/// One pair of member cards whose declared contracts touch.
///
/// The combined interaction checklist. Two cards that each passed their own
/// gates can still be wrong together, and the pairs worth looking at are not a
/// matter of taste: they are the ones where what one card changes is what the
/// other reads. Deriving the list means a reviewer is handed the interactions
/// rather than asked to imagine them.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Interaction {
    /// The card that changes a contract.
    pub changes: CardId,
    /// The card that reads it.
    pub reads: CardId,
    /// The contract domains they share.
    pub shared: Vec<String>,
}

/// One cycle release invariant, carried into verification unanswered.
///
/// Section 10.2 lets a cycle declare invariants in free text, which no gate can
/// evaluate. They are surfaced here rather than dropped: an invariant nobody is
/// shown is an invariant nobody checks.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InvariantCheck {
    /// The invariant as the cycle stated it.
    pub invariant: String,
    /// Whether any gate could observe it. Always false for now; free-text
    /// invariants are a reviewer's judgment, not a machine's.
    pub machine_checked: bool,
}

/// The frozen lesson packet for one integration member.
///
/// `manifest: None` is an explicit compatibility marker for a handoff written
/// before governed lessons existed. New handoffs always carry a manifest,
/// including an empty one, so absence is never inferred for new evidence.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LessonManifestBinding {
    /// The member this binding belongs to.
    pub card_id: CardId,
    /// The exact frozen manifest, or the explicit legacy marker.
    pub manifest: Option<LessonManifest>,
}

/// The result of running every required gate against the landing commit.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VerificationRecord {
    /// Always [`VERIFICATION_SCHEMA`].
    pub schema: String,
    /// The integration verified.
    pub integration_id: IntegrationId,
    /// The cycle it belongs to.
    pub cycle_id: CycleId,
    /// The exact commit every gate ran against.
    pub landing_sha: String,
    /// The tree that commit carries.
    pub landing_tree: String,
    /// The receipts produced, one per gate.
    pub receipt_ids: Vec<String>,
    /// Gates that did not pass.
    pub failed_gates: Vec<String>,
    /// The cycle invariants a reviewer must still judge.
    pub invariants: Vec<InvariantCheck>,
    /// Member pairs whose contracts interact.
    pub interactions: Vec<Interaction>,
    /// Exact per-card lesson manifests whose required integration gates were
    /// selected. Empty only for compatible v1 verification records.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lesson_manifests: Vec<LessonManifestBinding>,
    /// True when the worktree was clean after every gate ran.
    pub worktree_clean_after: bool,
    /// Who ran it. Declared, not proven; see D-013.
    pub verified_by: String,
    /// When it completed.
    pub verified_at: Timestamp,
    /// The canonicalization algorithm its digest was computed under.
    pub canonical_algorithm: String,
}

impl VerificationRecord {
    /// Relative path of a verification inside the control repository.
    #[must_use]
    pub fn relative_path(integration_id: &IntegrationId) -> String {
        format!("{VERIFICATION_DIR}/{integration_id}.json")
    }

    /// The verification's canonical digest.
    ///
    /// # Errors
    ///
    /// Returns an error when the record cannot be serialized.
    pub fn digest(&self) -> Result<Digest, HarnessError> {
        Digest::of_canonical(self)
    }

    /// True when every gate passed and the worktree stayed clean.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.failed_gates.is_empty() && self.worktree_clean_after
    }
}

/// Derives the interaction checklist from the members' declared contracts.
///
/// Ordered pairs: "A changes what B reads" is a different thing to look at than
/// the reverse, and collapsing them would hide one direction.
#[must_use]
pub fn interactions(members: &[(CardId, Vec<String>, Vec<String>)]) -> Vec<Interaction> {
    let mut found = Vec::new();
    for (changer, changes, _) in members {
        for (reader, _, reads) in members {
            if changer == reader {
                continue;
            }
            let shared: Vec<String> = changes
                .iter()
                .filter(|domain| reads.contains(domain))
                .cloned()
                .collect();
            if !shared.is_empty() {
                found.push(Interaction {
                    changes: changer.clone(),
                    reads: reader.clone(),
                    shared,
                });
            }
        }
    }
    found
}
