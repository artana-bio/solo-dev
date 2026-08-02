//! Independent review: a decision bound to exact objects by a different actor.
//!
//! Section 15.1 gives `independent` a precise operational meaning for the
//! one-human, many-agent model: a fresh context, the review packet only, and a
//! different actor identity. D-017 and R-012 are explicit that this is
//! procedural independence, not a cryptographic identity proof, and nothing
//! here claims otherwise.
//!
//! `SPIKE-001` findings F-4 and F-5 shape two details. Findings carry an
//! explicit disposition, because every spike review round needed to say
//! "resolved", "accepted as residual risk", or "unresolvable within this
//! card's write scope", and a binary verdict erases that. And a review records
//! whether the gates could actually observe the acceptance behaviors, because
//! both spike reviewers discovered independently that a green receipt proved
//! nothing about the behavior it appeared to support.

use serde::{Deserialize, Serialize};

use crate::{
    domain::{
        card::Risk,
        clock::Timestamp,
        digest::{CANONICAL_ALGORITHM, Digest},
        handoff::{DependencyBinding, DependencyStanding, dependency_staleness},
        ids::{CardId, CycleId, ReviewId},
    },
    error::{ErrorCode, HarnessError},
    policy::actors,
};

/// Schema identifier for a review.
pub const REVIEW_SCHEMA: &str = "harness.review/v1";

/// Directory holding reviews, relative to the control repository.
pub const REVIEW_DIR: &str = "reviews";

/// What a reviewer concluded.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// The candidate may proceed to integration.
    Approved,
    /// The candidate returns to the feature actor.
    ChangesRequested,
    /// Progress is halted pending a decision outside the reviewer's remit.
    Blocked,
}

impl Decision {
    /// What this decision did to the candidate, as a past-tense verb phrase.
    ///
    /// For messages that say what a review concluded about a candidate it can
    /// no longer speak for. The staleness message used to open "review
    /// approved candidate ..." whatever the decision was, which was harmless
    /// while only approvals survived to be reported — and became the ordinary
    /// output of `review inspect` once `F-028` let a non-approval outlive the
    /// branch it was reached against.
    #[must_use]
    pub const fn reached_on(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::ChangesRequested => "requested changes to",
            Self::Blocked => "blocked",
        }
    }

    /// Its stable serialized name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::ChangesRequested => "changes_requested",
            Self::Blocked => "blocked",
        }
    }
}

/// How serious a finding is.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    /// Must be fixed before the candidate can land.
    Critical,
    /// Should be fixed before landing.
    High,
    /// Worth fixing, but not disqualifying.
    Medium,
    /// Noted for the record.
    Low,
}

impl FindingSeverity {
    /// Its stable serialized name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

/// What happened to a finding.
///
/// `SPIKE-001` finding F-4: a review that can only approve or reject cannot
/// express "this is real, and this card cannot fix it", which is exactly what
/// the spike's reviewer needed to say about untestable acceptance behavior.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    /// Raised in this review and not yet addressed.
    Open,
    /// Addressed by the candidate under review.
    Resolved,
    /// Real, but accepted rather than fixed.
    AcceptedRisk,
    /// Real, and outside what this card is permitted to change.
    OutOfScope,
}

impl Disposition {
    /// True when this finding still demands action before landing.
    #[must_use]
    pub const fn blocks_approval(self) -> bool {
        matches!(self, Self::Open)
    }
}

/// One thing a reviewer found.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    /// How serious it is.
    pub severity: FindingSeverity,
    /// Where it is.
    pub location: String,
    /// What is wrong.
    pub detail: String,
    /// What happened to it.
    pub disposition: Disposition,
}

/// Whether the named gates can observe what the card claims to deliver.
///
/// `SPIKE-001` finding F-5. Both spike reviewers mutation-tested the gates
/// unprompted and proved, three times out of three, that a green receipt was
/// not evidence for the acceptance behavior it appeared to support. Recording
/// this makes the strongest thing reviewers actually did a required output
/// rather than an act of conscience.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GateAdequacy {
    /// True when the reviewer believes the gates cover the acceptance list.
    pub gates_observe_acceptance: bool,
    /// Acceptance behaviors no gate can fail on.
    pub unobserved_behaviors: Vec<String>,
    /// How the reviewer established this.
    pub basis: String,
}

/// One recorded review of one exact candidate.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReviewRecord {
    /// Always [`REVIEW_SCHEMA`].
    pub schema: String,
    /// Identifies this review.
    pub review_id: ReviewId,
    /// The card reviewed.
    pub card_id: CardId,
    /// The card revision in force.
    pub card_revision: u32,
    /// The card digest the review is bound to.
    pub card_digest: Digest,
    /// The cycle it belongs to.
    pub cycle_id: CycleId,
    /// The frozen baseline.
    pub baseline_sha: String,
    /// The exact candidate reviewed.
    pub candidate_sha: String,
    /// Which commit of each declared dependency the reviewed candidate holds.
    ///
    /// Copied from the handoff, exactly as `baseline_sha` and `candidate_sha`
    /// are, so a review can be judged without re-reading its handoff. Defaulted
    /// for the same reason the handoff's copy is; see that field.
    #[serde(default)]
    pub dependency_bindings: Vec<DependencyBinding>,
    /// The handoff the reviewer received.
    pub handoff_id: String,
    /// Digest of that handoff, so the packet is pinned too.
    pub handoff_digest: Digest,
    /// Who reviewed. Declared, not proven; see D-013 and R-012.
    pub reviewer_actor_id: String,
    /// Who produced the candidate.
    pub feature_actor_id: String,
    /// The conclusion.
    pub decision: Decision,
    /// What the reviewer found.
    pub findings: Vec<Finding>,
    /// Whether the gates can observe the acceptance list.
    pub gate_adequacy: GateAdequacy,
    /// Risks accepted if this is an approval.
    pub residual_risks: Vec<String>,
    /// Whether a human performed this review. Declared, not proven; see D-013.
    #[serde(default)]
    pub human_reviewer: bool,
    /// The review this one supersedes, when it is a re-review.
    pub supersedes: Option<ReviewId>,
    /// When it was recorded.
    pub reviewed_at: Timestamp,
    /// The canonicalization algorithm its digest was computed under.
    pub canonical_algorithm: String,
}

impl ReviewRecord {
    /// Relative path of a review inside the control repository.
    #[must_use]
    pub fn relative_path(review_id: &ReviewId) -> String {
        format!("{REVIEW_DIR}/{review_id}.json")
    }

    /// The review's canonical digest.
    ///
    /// # Errors
    ///
    /// Returns an error when the record cannot be serialized.
    pub fn digest(&self) -> Result<Digest, HarnessError> {
        Digest::of_canonical(self)
    }

    /// The canonicalization algorithm reviews are digested under.
    #[must_use]
    pub const fn canonical_algorithm() -> &'static str {
        CANONICAL_ALGORITHM
    }

    /// True when this review still describes the given candidate and card.
    ///
    /// Section 15.2: approval becomes invalid when the candidate SHA, the card
    /// digest, or a required dependency SHA changes. All three are checked,
    /// because any one alone would let a stale approval through.
    #[must_use]
    pub fn is_current_for(
        &self,
        candidate_sha: &str,
        card_digest: &Digest,
        dependencies: &[DependencyStanding],
    ) -> bool {
        self.staleness(candidate_sha, card_digest, dependencies)
            .is_none()
    }

    /// Explains why this review no longer applies, if it does not.
    ///
    /// Three of Section 15.2's seven triggers are checked. The four that are
    /// not — cycle invariant, required gate definition, reviewer-required
    /// receipt, declared contract change — are named here rather than left to
    /// be inferred from the code, because the docstring that used to sit here
    /// described the specification as being the subset that was implemented.
    #[must_use]
    pub fn staleness(
        &self,
        candidate_sha: &str,
        card_digest: &Digest,
        dependencies: &[DependencyStanding],
    ) -> Option<String> {
        if self.candidate_sha != candidate_sha {
            return Some(format!(
                "review {} candidate {} but the branch is now {candidate_sha}",
                self.decision.reached_on(),
                self.candidate_sha
            ));
        }
        if self.card_digest != *card_digest {
            return Some(format!(
                "review was bound to card digest {} but the card is now {card_digest}",
                self.card_digest
            ));
        }
        dependency_staleness("review", &self.dependency_bindings, dependencies)
    }

    /// The card's standing verdict, when the reviewers left it approved.
    ///
    /// Not "the latest review that approved". Reviews on a card form a chain,
    /// each superseding the one before it, so an approval followed by a
    /// `changes_requested` no longer stands, and a search that skipped
    /// backwards past the later verdict would report a decision the reviewer
    /// has since withdrawn.
    ///
    /// `reviews` must be oldest first, as [`crate::commands::review::reviews_for`]
    /// returns them.
    #[must_use]
    pub fn standing_approval(reviews: &[Self]) -> Option<&Self> {
        reviews
            .last()
            .filter(|review| review.decision == Decision::Approved)
    }

    /// Findings that still demand action.
    #[must_use]
    pub fn open_findings(&self) -> Vec<&Finding> {
        self.findings
            .iter()
            .filter(|finding| finding.disposition.blocks_approval())
            .collect()
    }

    /// Checks the record's internal consistency.
    ///
    /// # Errors
    ///
    /// Returns a policy error when the decision contradicts the findings or the
    /// reviewer is the feature actor.
    pub fn validate(&self) -> Result<(), HarnessError> {
        check_independence(&self.reviewer_actor_id, &self.feature_actor_id)?;

        if self.decision == Decision::Approved && !self.open_findings().is_empty() {
            let open: Vec<&str> = self
                .open_findings()
                .iter()
                .map(|finding| finding.location.as_str())
                .collect();
            return Err(HarnessError::Control {
                reason: format!(
                    "cannot approve with open findings at {}; disposition each as resolved, accepted risk, or out of scope",
                    open.join(", ")
                ),
                code: ErrorCode::PolicyOpenFindings,
            });
        }

        if self.decision == Decision::ChangesRequested && self.findings.is_empty() {
            return Err(HarnessError::Control {
                reason: "changes were requested but no finding says what to change".to_owned(),
                code: ErrorCode::PolicyIncompleteReview,
            });
        }

        if self.gate_adequacy.basis.trim().is_empty() {
            return Err(HarnessError::Control {
                reason: "a review must state how gate adequacy was established".to_owned(),
                code: ErrorCode::PolicyIncompleteReview,
            });
        }

        Ok(())
    }

    /// Refuses an approval that Section 15.3's risk policy does not permit.
    ///
    /// `Risk::requires_human_review` existed, was unit-tested, and was called
    /// from nowhere, so a `critical`-risk card received exactly the treatment a
    /// `low`-risk one did — the policy was documented, modelled, and never
    /// enforced.
    ///
    /// What is enforced is the declaration, not the fact. D-013 makes every
    /// identity in this harness a claim rather than a proof, and nothing here
    /// can tell a human from an agent. Requiring the claim turns an unstated
    /// rule into a recorded, refusable step and puts the assertion on the
    /// review where an auditor can see who made it.
    ///
    /// Section 15.3's further requirements for `critical` — a rollback exercise
    /// and a second human approval — are **not** enforced here. Recorded as
    /// unenforced rather than implied.
    ///
    /// # Errors
    ///
    /// Returns a policy error when an approval lacks a required declaration.
    pub fn check_risk_policy(&self, risk: Risk) -> Result<(), HarnessError> {
        if self.decision != Decision::Approved
            || !risk.requires_human_review()
            || self.human_reviewer
        {
            return Ok(());
        }
        Err(HarnessError::Control {
            reason: format!(
                "card risk `{}` requires a human reviewer under Section 15.3 and this review declared none; set `human_reviewer: true` on the verdict. Declared, not proven: see D-013",
                risk.name()
            ),
            code: ErrorCode::PolicyRiskReview,
        })
    }

    /// Refuses a re-review that drops a prior open finding instead of
    /// dispositioning it.
    ///
    /// `WP-320` required this and it was never implemented. Without it a
    /// re-review carrying `findings: []` approves away a prior critical finding
    /// silently: the earlier record stays on disk and stays open, and nothing
    /// consults it again. Supersession is what makes a finding survive a
    /// re-review, and supersession without this check is only filing.
    ///
    /// A finding is accounted for when the new review names the same
    /// `location` with a disposition that no longer blocks. Silence is not
    /// resolution. Two distinct findings sharing one location collapse into a
    /// single obligation, which is a real weakness of using `location` as the
    /// identity — but the reviewer must still have looked at that location and
    /// asserted something about it, and it is the eliminating of silence that
    /// this check is for.
    ///
    /// This applies to every re-review, not only approvals. A
    /// `changes_requested` that drops a prior finding would leave the next
    /// reviewer looking at a clean predecessor, which is the same defect one
    /// step removed.
    ///
    /// # Errors
    ///
    /// Returns a policy error naming each unaccounted-for finding.
    pub fn check_supersedes(&self, superseded: &Self) -> Result<(), HarnessError> {
        let dropped: Vec<String> = superseded
            .open_findings()
            .iter()
            .filter(|prior| {
                !self.findings.iter().any(|current| {
                    current.location == prior.location && !current.disposition.blocks_approval()
                })
            })
            .map(|finding| format!("{} ({})", finding.location, finding.severity.name()))
            .collect();

        if dropped.is_empty() {
            return Ok(());
        }

        Err(HarnessError::Control {
            reason: format!(
                "review {} left findings open at {}; this review must disposition each as resolved, accepted risk, or out of scope rather than omit them",
                superseded.review_id,
                dropped.join(", ")
            ),
            code: ErrorCode::PolicyOpenFindings,
        })
    }
}

/// Refuses a review whose reviewer is the feature actor.
///
/// Invariant 7.3.7. This compares declared identities and proves nothing about
/// who actually ran the command; D-017 and R-012 say so plainly. It catches the
/// mistake, not the adversary.
///
/// # Errors
///
/// Returns a policy error when the two identities match.
pub fn check_independence(reviewer: &str, feature_actor: &str) -> Result<(), HarnessError> {
    actors::refuse_unusable("reviewer", reviewer)?;
    actors::refuse_unusable("feature actor", feature_actor)?;
    // Normalized, because `reviewer-b ` and `Reviewer-B` were the same person
    // every time they differed from `reviewer-b`, and a separation check a
    // trailing space defeats is not a check.
    if actors::same(reviewer, feature_actor) {
        return Err(HarnessError::Control {
            reason: format!(
                "`{reviewer}` produced this candidate and cannot review it; independence is procedural, so the harness can only refuse the obvious case"
            ),
            code: ErrorCode::PolicySelfReview,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        clock::{Clock as _, FixedClock},
        handoff::DEPENDENCIES_NOT_CHECKED,
    };

    fn adequacy() -> GateAdequacy {
        GateAdequacy {
            gates_observe_acceptance: true,
            unobserved_behaviors: vec![],
            basis: "ran the suite and probed each acceptance behavior directly".to_owned(),
        }
    }

    fn review(decision: Decision, findings: Vec<Finding>) -> ReviewRecord {
        ReviewRecord {
            schema: REVIEW_SCHEMA.to_owned(),
            review_id: "RV-000001".parse().unwrap(),
            card_id: "F-001".parse().unwrap(),
            card_revision: 1,
            card_digest: Digest::of_bytes(b"card"),
            cycle_id: "C-001".parse().unwrap(),
            baseline_sha: "b".repeat(40),
            candidate_sha: "a".repeat(40),
            dependency_bindings: vec![],
            handoff_id: "F-001-r1-aaaaaaaaaaaa".to_owned(),
            handoff_digest: Digest::of_bytes(b"handoff"),
            reviewer_actor_id: "reviewer-session-a".to_owned(),
            feature_actor_id: "implementer-session-1".to_owned(),
            decision,
            findings,
            gate_adequacy: adequacy(),
            residual_risks: vec![],
            human_reviewer: false,
            supersedes: None,
            reviewed_at: FixedClock::at_unix_seconds(1_785_196_800).unwrap().now(),
            canonical_algorithm: CANONICAL_ALGORITHM.to_owned(),
        }
    }

    fn finding(disposition: Disposition) -> Finding {
        Finding {
            severity: FindingSeverity::Critical,
            location: "src/temperature.rs:fahrenheit_to_celsius".to_owned(),
            detail: "no absolute-zero guard".to_owned(),
            disposition,
        }
    }

    #[test]
    fn a_clean_approval_validates() {
        review(Decision::Approved, vec![])
            .validate()
            .expect("nothing to object to");
    }

    #[test]
    fn self_review_is_refused() {
        let mut invalid = review(Decision::Approved, vec![]);
        invalid.reviewer_actor_id = invalid.feature_actor_id.clone();
        let error = invalid.validate().expect_err("must refuse");
        assert_eq!(error.code(), ErrorCode::PolicySelfReview);
        assert!(
            error.to_string().contains("procedural"),
            "the message must not overclaim what this check proves"
        );
    }

    #[test]
    fn an_anonymous_review_is_refused() {
        let mut invalid = review(Decision::Approved, vec![]);
        invalid.reviewer_actor_id = "  ".to_owned();
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn approving_over_an_open_finding_is_refused() {
        let error = review(Decision::Approved, vec![finding(Disposition::Open)])
            .validate()
            .expect_err("must refuse");
        assert_eq!(error.code(), ErrorCode::PolicyOpenFindings);
        assert!(error.to_string().contains("src/temperature.rs"));
    }

    #[test]
    fn approving_over_a_dispositioned_finding_is_permitted() {
        // SPIKE-001 F-4: a reviewer must be able to approve while recording
        // that a real problem is accepted or unfixable within this card.
        for disposition in [
            Disposition::Resolved,
            Disposition::AcceptedRisk,
            Disposition::OutOfScope,
        ] {
            review(Decision::Approved, vec![finding(disposition)])
                .validate()
                .unwrap_or_else(|error| panic!("{disposition:?} should permit approval: {error}"));
        }
    }

    #[test]
    fn requesting_changes_without_a_finding_is_refused() {
        let error = review(Decision::ChangesRequested, vec![])
            .validate()
            .expect_err("must refuse");
        assert_eq!(error.code(), ErrorCode::PolicyIncompleteReview);
    }

    #[test]
    fn a_low_risk_card_may_be_approved_without_a_human() {
        // The check must key on the card's risk. A blanket requirement would
        // make every agent review impossible, which is the model this harness
        // exists to serve.
        let approved = review(Decision::Approved, vec![]);
        assert!(approved.check_risk_policy(Risk::Low).is_ok());
        assert!(approved.check_risk_policy(Risk::Medium).is_ok());
    }

    #[test]
    fn a_high_or_critical_card_needs_the_declaration() {
        // Tier 3, defect 22. `requires_human_review` was modelled, unit-tested,
        // and called from nowhere, so a critical-risk card received exactly the
        // treatment a low-risk one did.
        let approved = review(Decision::Approved, vec![]);
        assert!(approved.check_risk_policy(Risk::High).is_err());
        assert!(approved.check_risk_policy(Risk::Critical).is_err());

        let declared = ReviewRecord {
            human_reviewer: true,
            ..review(Decision::Approved, vec![])
        };
        assert!(declared.check_risk_policy(Risk::High).is_ok());
        assert!(declared.check_risk_policy(Risk::Critical).is_ok());
    }

    #[test]
    fn the_risk_policy_gates_approval_and_nothing_else() {
        // A reviewer must still be able to request changes on, or block, a
        // high-risk card without claiming to be human. Those verdicts land
        // nothing, and refusing them would leave the card with no exit — the
        // defect two entries above this one.
        for decision in [Decision::ChangesRequested, Decision::Blocked] {
            let verdict = review(decision, vec![finding(Disposition::Open)]);
            assert!(
                verdict.check_risk_policy(Risk::Critical).is_ok(),
                "{decision:?} lands nothing and must not need the declaration"
            );
        }
    }

    #[test]
    fn a_review_must_say_how_gate_adequacy_was_established() {
        let mut invalid = review(Decision::Approved, vec![]);
        invalid.gate_adequacy.basis = String::new();
        let error = invalid.validate().expect_err("must refuse");
        assert_eq!(error.code(), ErrorCode::PolicyIncompleteReview);
    }

    #[test]
    fn a_review_may_approve_while_reporting_inadequate_gates() {
        // The spike's reviewers approved corrected candidates while recording
        // that the gates could not observe one acceptance behavior. That is a
        // legitimate and valuable outcome, not a contradiction.
        let mut honest = review(Decision::Approved, vec![finding(Disposition::OutOfScope)]);
        honest.gate_adequacy = GateAdequacy {
            gates_observe_acceptance: false,
            unobserved_behaviors: vec!["raises ValueError below absolute zero".to_owned()],
            basis: "mutation-tested the suite; it passes with the guard removed".to_owned(),
        };
        honest
            .validate()
            .expect("an honest approval is still an approval");
        assert!(!honest.gate_adequacy.gates_observe_acceptance);
    }

    #[test]
    fn a_review_applies_only_to_its_exact_candidate_and_card() {
        let record = review(Decision::Approved, vec![]);
        let card = Digest::of_bytes(b"card");
        assert!(record.is_current_for(&"a".repeat(40), &card, DEPENDENCIES_NOT_CHECKED));
        assert!(!record.is_current_for(&"c".repeat(40), &card, DEPENDENCIES_NOT_CHECKED));
        assert!(!record.is_current_for(
            &"a".repeat(40),
            &Digest::of_bytes(b"revised"),
            DEPENDENCIES_NOT_CHECKED
        ));
    }

    #[test]
    fn staleness_says_what_the_review_actually_decided() {
        // The message opened "review approved candidate ..." for every
        // decision. Harmless while only approvals outlived their candidate —
        // and `F-028` made a superseded `changes_requested` the ordinary
        // output of `review inspect`, so the common case was a record stating
        // the opposite of the verdict beside it.
        let card = Digest::of_bytes(b"card");
        let moved = "c".repeat(40);

        for (decision, expected, forbidden) in [
            (Decision::Approved, "approved candidate", "requested"),
            (
                Decision::ChangesRequested,
                "requested changes to candidate",
                "approved",
            ),
            (Decision::Blocked, "blocked candidate", "approved"),
        ] {
            let message = review(decision, vec![])
                .staleness(&moved, &card, DEPENDENCIES_NOT_CHECKED)
                .expect("a moved branch is stale");
            assert!(
                message.contains(expected),
                "{decision:?} should say `{expected}`: {message}"
            );
            assert!(
                !message.contains(forbidden),
                "{decision:?} must not claim `{forbidden}`: {message}"
            );
        }
    }

    #[test]
    fn staleness_explains_which_binding_broke() {
        let record = review(Decision::Approved, vec![]);
        let card = Digest::of_bytes(b"card");
        assert!(
            record
                .staleness(&"a".repeat(40), &card, DEPENDENCIES_NOT_CHECKED)
                .is_none()
        );
        assert!(
            record
                .staleness(&"c".repeat(40), &card, DEPENDENCIES_NOT_CHECKED)
                .unwrap()
                .contains("branch is now")
        );
        assert!(
            record
                .staleness(
                    &"a".repeat(40),
                    &Digest::of_bytes(b"revised"),
                    DEPENDENCIES_NOT_CHECKED
                )
                .unwrap()
                .contains("card is now")
        );
    }

    #[test]
    fn a_later_verdict_supersedes_an_earlier_approval() {
        // The hole a `rfind(approved)` search leaves: it walks backwards past
        // the standing verdict and reports a decision the reviewer withdrew.
        // Everything that asks "which commit of this dependency was blessed"
        // reads this.
        let approved = review(Decision::Approved, vec![]);
        let rejected = review(Decision::ChangesRequested, vec![finding(Disposition::Open)]);

        assert_eq!(
            ReviewRecord::standing_approval(std::slice::from_ref(&approved)),
            Some(&approved)
        );
        assert_eq!(
            ReviewRecord::standing_approval(&[approved.clone(), rejected.clone()]),
            None,
            "an approval followed by a request for changes no longer stands"
        );
        assert_eq!(
            ReviewRecord::standing_approval(&[rejected, approved.clone()]),
            Some(&approved),
            "and the approval that came after it does"
        );
        assert_eq!(ReviewRecord::standing_approval(&[]), None);
    }

    #[test]
    fn findings_remain_visible_after_a_later_approval() {
        // Invariant: a re-review supersedes rather than erases, so the earlier
        // finding survives in its own record.
        let first = review(Decision::ChangesRequested, vec![finding(Disposition::Open)]);
        let mut second = review(Decision::Approved, vec![finding(Disposition::Resolved)]);
        second.review_id = "RV-000002".parse().unwrap();
        second.supersedes = Some(first.review_id.clone());

        assert_eq!(second.supersedes.as_ref(), Some(&first.review_id));
        assert_eq!(first.findings.len(), 1);
        assert_eq!(first.findings[0].disposition, Disposition::Open);
    }

    #[test]
    fn a_record_round_trips_and_rejects_unknown_fields() {
        let record = review(Decision::Approved, vec![finding(Disposition::Resolved)]);
        let encoded = serde_json::to_string_pretty(&record).unwrap();
        let decoded: ReviewRecord = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, record);
        assert_eq!(decoded.digest().unwrap(), record.digest().unwrap());

        let mut value = serde_json::to_value(&record).unwrap();
        value["surprise"] = serde_json::json!(1);
        assert!(serde_json::from_value::<ReviewRecord>(value).is_err());
    }

    #[test]
    fn a_review_names_its_canonicalization_algorithm() {
        assert_eq!(ReviewRecord::canonical_algorithm(), CANONICAL_ALGORITHM);
    }
}
