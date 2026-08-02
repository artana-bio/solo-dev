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

use std::collections::BTreeMap;

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
    /// Raised by an earlier review, worked on, and still there.
    ///
    /// The vocabulary above assumes every carried finding has been settled,
    /// and the common shape of a long-running card is one that has not been.
    /// Recording `RV-000039` on `F-027` hit it: the prior critical finding was
    /// that the actor comparison lost to a case variant, the fix closed the
    /// character that had been demonstrated, and the class survived at a
    /// different character. Neither resolved, nor accepted, nor out of scope —
    /// still open, at the same location, for the same reason.
    ///
    /// The workaround was to file the demonstrated instance `Resolved` and
    /// raise the survivor as a new finding. That happened to be honest there,
    /// because the two characters really were distinct defects, and it would
    /// not be in general: it makes a fourth-round defect read as freshly
    /// raised, and anyone counting findings per round sees churn where there
    /// was persistence.
    StillOpen,
    /// Addressed by the candidate under review.
    Resolved,
    /// Real, but accepted rather than fixed.
    AcceptedRisk,
    /// Real, and outside what this card is permitted to change.
    OutOfScope,
}

impl Disposition {
    /// True when this finding still demands action before landing.
    ///
    /// `StillOpen` blocks exactly as `Open` does. That is what lets every
    /// existing caller stay as it is: a carried-open finding refuses an
    /// approval, counts toward the open total, and reaches convergence
    /// detection without any of them learning the new variant.
    #[must_use]
    pub const fn blocks_approval(self) -> bool {
        matches!(self, Self::Open | Self::StillOpen)
    }

    /// True when naming a prior finding's location with this disposition is
    /// an accounting for it rather than a fresh observation.
    ///
    /// The distinction [`Self::blocks_approval`] cannot make. Silently
    /// dropping a prior finding is how a real defect disappears between
    /// rounds, so a re-review has to say something about each one — but
    /// "something" and "settled" are different requirements, and conflating
    /// them is what left a persisting finding unrecordable.
    ///
    /// `Open` does not account for a prior finding. A reviewer who raises a
    /// new problem at a location an earlier round already flagged has not
    /// spoken to the earlier one, and reading it as though they had is the
    /// silence this rule exists to eliminate.
    #[must_use]
    pub const fn accounts_for_prior(self) -> bool {
        !matches!(self, Self::Open)
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
                    "cannot approve with open findings at {}; disposition each as resolved, accepted risk, or out of scope — carrying one forward as still open does not settle it",
                    open.join(", ")
                ),
                code: ErrorCode::PolicyOpenFindings,
            });
        }

        // A first review has nothing to carry anything forward from.
        // `check_supersedes` catches a fabricated carry-forward, but only runs
        // when a predecessor exists, so without this the very first review on
        // a card could claim a finding had persisted from nowhere. `supersedes`
        // is `Some` exactly when there is a predecessor, which makes this
        // answerable here rather than at the call site.
        if self.supersedes.is_none() {
            let carried: Vec<&str> = self
                .findings
                .iter()
                .filter(|finding| finding.disposition == Disposition::StillOpen)
                .map(|finding| finding.location.as_str())
                .collect();
            if !carried.is_empty() {
                return Err(HarnessError::Control {
                    reason: format!(
                        "this is the first review of the card and it carries findings forward as still open at {}; there is no earlier review to carry them from, and a finding raised here is `open`",
                        carried.join(", ")
                    ),
                    code: ErrorCode::PolicyOpenFindings,
                });
            }
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
    /// `location` with a disposition that accounts for a prior one — settled,
    /// or explicitly carried forward as [`Disposition::StillOpen`]. Silence is
    /// not resolution.
    ///
    /// Counted per location, not merely present. This docstring used to say
    /// that two findings sharing one location collapse into a single
    /// obligation, and defend it: the reviewer must at least have looked at
    /// that location, and eliminating silence was what the check was for. A
    /// reviewer refuted it by demonstration — two critical findings at
    /// `src/a.rs:10`, one `resolved` naming that location, and the approval
    /// was recorded with the second finding gone. Looking at a location is not
    /// answering for everything found there, and the rule exists precisely so
    /// that a real defect cannot disappear between rounds.
    ///
    /// So each location needs as many accounting entries as it had open
    /// findings. Two findings resolved together need two entries saying so,
    /// which is the record stating what happened to each rather than to the
    /// file. `location` is still the identity, with all the imprecision that
    /// carries; what is gone is the arithmetic that let one answer stand in
    /// for many.
    ///
    /// The test used to be "no longer blocks", which made settling the only
    /// way to account for a finding and left a persisting one unrecordable.
    /// Accounting and settling are separate questions, and `StillOpen`
    /// answers the first while failing the second on purpose.
    ///
    /// The converse is refused too: `StillOpen` at a location the superseded
    /// review left nothing open at is not carrying anything forward, and
    /// permitting it would let a review manufacture a history of persistence
    /// that never happened.
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
        // Kept as findings rather than a count so the refusal can still name
        // the severity of what it is protecting, which is the first thing a
        // reader wants and the reason the old message carried it.
        let mut prior_open: BTreeMap<&str, Vec<&Finding>> = BTreeMap::new();
        for finding in superseded.open_findings() {
            prior_open
                .entry(finding.location.as_str())
                .or_default()
                .push(finding);
        }

        let mut accounting: BTreeMap<&str, usize> = BTreeMap::new();
        let mut carried: BTreeMap<&str, usize> = BTreeMap::new();
        for finding in &self.findings {
            if finding.disposition.accounts_for_prior() {
                *accounting.entry(finding.location.as_str()).or_default() += 1;
            }
            if finding.disposition == Disposition::StillOpen {
                *carried.entry(finding.location.as_str()).or_default() += 1;
            }
        }

        let dropped: Vec<String> = prior_open
            .iter()
            .filter_map(|(location, open)| {
                let named = accounting.get(location).copied().unwrap_or_default();
                if named >= open.len() {
                    return None;
                }
                let severities: Vec<&str> =
                    open.iter().map(|finding| finding.severity.name()).collect();
                Some(format!(
                    "{location} ({named} of {} accounted for; {})",
                    open.len(),
                    severities.join(", ")
                ))
            })
            .collect();

        if !dropped.is_empty() {
            return Err(HarnessError::Control {
                reason: format!(
                    "review {} left findings open at {}; this review must disposition each one as resolved, accepted risk, out of scope, or still open — one entry does not answer for several findings at the same location",
                    superseded.review_id,
                    dropped.join(", ")
                ),
                code: ErrorCode::PolicyOpenFindings,
            });
        }

        let invented: Vec<String> = carried
            .iter()
            .filter_map(|(location, forward)| {
                let open = prior_open.get(location).map_or(0, Vec::len);
                (*forward > open).then(|| format!("{location} ({forward} carried, {open} open)"))
            })
            .collect();

        if !invented.is_empty() {
            return Err(HarnessError::Control {
                reason: format!(
                    "review {} did not leave that many findings open at {}; a finding can only be carried forward as still open from a review that raised it, and one raised here is `open`",
                    superseded.review_id,
                    invented.join(", ")
                ),
                code: ErrorCode::PolicyOpenFindings,
            });
        }

        Ok(())
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

    /// A finding at `location` with `disposition`.
    fn finding_at(location: &str, disposition: Disposition) -> Finding {
        Finding {
            severity: FindingSeverity::Critical,
            location: location.to_owned(),
            detail: "the comparison loses to a case variant".to_owned(),
            disposition,
        }
    }

    /// A re-review, so `supersedes` is populated as the caller populates it.
    fn re_review(decision: Decision, findings: Vec<Finding>) -> ReviewRecord {
        let mut record = review(decision, findings);
        record.supersedes = Some("RV-000001".parse().unwrap());
        record
    }

    #[test]
    fn a_finding_can_be_carried_forward_as_still_open() {
        // The case that could not be recorded: worked on, partly closed, still
        // there. Filing it `resolved` and raising a new finding was the only
        // way to satisfy the carry rule, which makes a fourth-round defect
        // read as freshly raised.
        let first = review(
            Decision::ChangesRequested,
            vec![finding_at("src/policy/actors.rs", Disposition::Open)],
        );
        let again = re_review(
            Decision::ChangesRequested,
            vec![finding_at("src/policy/actors.rs", Disposition::StillOpen)],
        );

        again
            .validate()
            .expect("a persisting finding is recordable");
        again
            .check_supersedes(&first)
            .expect("naming it accounts for the prior finding");
    }

    #[test]
    fn carrying_a_finding_forward_does_not_settle_it() {
        // `StillOpen` satisfies the carry rule and nothing else. An approval
        // over one must refuse exactly as it does over a fresh `open`.
        let carried = re_review(
            Decision::Approved,
            vec![finding_at("src/policy/actors.rs", Disposition::StillOpen)],
        );
        let message = carried.validate().unwrap_err().to_string();
        assert!(
            message.contains("cannot approve with open findings"),
            "{message}"
        );

        assert!(Disposition::StillOpen.blocks_approval());
        assert_eq!(
            carried.open_findings().len(),
            1,
            "and it counts toward the open total, which is what reaches \
             convergence detection unchanged"
        );
    }

    #[test]
    fn a_carried_finding_is_distinguishable_from_a_fresh_one() {
        // "This is round five of the same defect" has to be legible in the
        // record, which means the two must not serialize alike.
        let carried = serde_json::to_string(&Disposition::StillOpen).unwrap();
        let fresh = serde_json::to_string(&Disposition::Open).unwrap();
        assert_eq!(carried, "\"still_open\"");
        assert_eq!(fresh, "\"open\"");
        assert_ne!(carried, fresh);
    }

    #[test]
    fn dropping_a_prior_finding_is_still_refused() {
        // The rule this change had to leave standing. Widening what accounts
        // for a finding must not widen it to silence.
        let first = review(
            Decision::ChangesRequested,
            vec![finding_at("src/policy/actors.rs", Disposition::Open)],
        );
        let silent = re_review(
            Decision::ChangesRequested,
            vec![finding_at("src/other.rs", Disposition::Open)],
        );

        let message = silent.check_supersedes(&first).unwrap_err().to_string();
        assert!(message.contains("src/policy/actors.rs"), "{message}");
        assert!(
            message.contains("still open"),
            "the refusal names the new option: {message}"
        );
    }

    #[test]
    fn a_fresh_open_finding_does_not_account_for_a_prior_one() {
        // Raising a new problem where an earlier round found one is not
        // speaking to the earlier one. Reading it as though it were is the
        // silence the carry rule exists to eliminate.
        let first = review(
            Decision::ChangesRequested,
            vec![finding_at("src/policy/actors.rs", Disposition::Open)],
        );
        let fresh = re_review(
            Decision::ChangesRequested,
            vec![finding_at("src/policy/actors.rs", Disposition::Open)],
        );
        assert!(
            fresh.check_supersedes(&first).is_err(),
            "`open` at the same location is a new observation, not an accounting"
        );
    }

    #[test]
    fn a_carry_forward_from_nothing_is_refused() {
        // Otherwise the disposition can manufacture a history of persistence.
        let first = review(
            Decision::ChangesRequested,
            vec![finding_at("src/a.rs", Disposition::Open)],
        );
        let invented = re_review(
            Decision::ChangesRequested,
            vec![
                finding_at("src/a.rs", Disposition::Resolved),
                finding_at("src/never-seen.rs", Disposition::StillOpen),
            ],
        );

        let message = invented.check_supersedes(&first).unwrap_err().to_string();
        assert!(message.contains("src/never-seen.rs"), "{message}");
        assert!(message.contains("1 carried, 0 open"), "{message}");
    }

    #[test]
    fn the_first_review_of_a_card_cannot_carry_anything_forward() {
        // `check_supersedes` only runs when a predecessor exists, so without
        // this the very first review could claim a finding had persisted from
        // nowhere. `supersedes` is `None` exactly then.
        let first = review(
            Decision::ChangesRequested,
            vec![finding_at("src/a.rs", Disposition::StillOpen)],
        );
        assert!(first.supersedes.is_none(), "the fixture is a first review");

        let message = first.validate().unwrap_err().to_string();
        assert!(message.contains("first review"), "{message}");
        assert!(message.contains("src/a.rs"), "{message}");
    }

    #[test]
    fn one_resolution_cannot_answer_for_two_findings_at_one_location() {
        // Round 1 of this card's own review, and the worst defect it found.
        // Accounting was existential, so one `resolved` naming a location
        // discharged every prior finding there. The reviewer gave a superseded
        // review two criticals at `src/a.rs:10`, resolved one, and the
        // approval was recorded with the second gone — the disappearance this
        // whole rule exists to prevent, reaching an approval.
        let first = review(
            Decision::ChangesRequested,
            vec![
                finding_at("src/a.rs:10", Disposition::Open),
                finding_at("src/a.rs:10", Disposition::Open),
            ],
        );
        let half = re_review(
            Decision::Approved,
            vec![finding_at("src/a.rs:10", Disposition::Resolved)],
        );

        let message = half.check_supersedes(&first).unwrap_err().to_string();
        assert!(message.contains("src/a.rs:10"), "{message}");
        assert!(
            message.contains("1 of 2 accounted for"),
            "the refusal has to say how far short it fell: {message}"
        );

        // Two entries for two findings is the way through, and it must work.
        let both = re_review(
            Decision::Approved,
            vec![
                finding_at("src/a.rs:10", Disposition::Resolved),
                finding_at("src/a.rs:10", Disposition::Resolved),
            ],
        );
        both.check_supersedes(&first)
            .expect("one entry per finding accounts for both");
    }

    #[test]
    fn more_findings_cannot_be_carried_forward_than_were_left_open() {
        // The inverse of the same arithmetic: one prior finding permitted any
        // number of carried-forward findings at that location, fabricating a
        // history of persistence for observations nobody had made before.
        let first = review(
            Decision::ChangesRequested,
            vec![finding_at("src/a.rs:10", Disposition::Open)],
        );
        let inflated = re_review(
            Decision::ChangesRequested,
            vec![
                finding_at("src/a.rs:10", Disposition::StillOpen),
                finding_at("src/a.rs:10", Disposition::StillOpen),
            ],
        );

        let message = inflated.check_supersedes(&first).unwrap_err().to_string();
        assert!(message.contains("2 carried, 1 open"), "{message}");

        // Carrying exactly what was left open is fine.
        let honest = re_review(
            Decision::ChangesRequested,
            vec![finding_at("src/a.rs:10", Disposition::StillOpen)],
        );
        honest
            .check_supersedes(&first)
            .expect("one carried from one open");
    }

    #[test]
    fn a_split_verdict_at_one_location_is_counted_as_two() {
        // The mixed case: two findings at a location, one genuinely fixed and
        // one still there. Both entries count toward the accounting, and the
        // carried one still blocks an approval.
        let first = review(
            Decision::ChangesRequested,
            vec![
                finding_at("src/a.rs:10", Disposition::Open),
                finding_at("src/a.rs:10", Disposition::Open),
            ],
        );
        let split = re_review(
            Decision::ChangesRequested,
            vec![
                finding_at("src/a.rs:10", Disposition::Resolved),
                finding_at("src/a.rs:10", Disposition::StillOpen),
            ],
        );

        split
            .check_supersedes(&first)
            .expect("two entries answer for two findings");
        assert_eq!(split.open_findings().len(), 1, "and one is still open");

        // Round 2 of this card's own review: supplying two entries and
        // asserting they are accepted does not show the second was needed.
        // Restoring existential accounting left this green until the negative
        // half was added.
        for lone in [Disposition::Resolved, Disposition::StillOpen] {
            let single = re_review(
                Decision::ChangesRequested,
                vec![finding_at("src/a.rs:10", lone)],
            );
            let message = single.check_supersedes(&first).unwrap_err().to_string();
            assert!(
                message.contains("1 of 2 accounted for"),
                "one {lone:?} entry cannot answer for two findings: {message}"
            );
        }
    }

    #[test]
    fn the_settled_dispositions_are_unchanged() {
        // The regression half: widening `accounts_for_prior` must not have
        // changed what settles a finding or what blocks an approval.
        for settled in [
            Disposition::Resolved,
            Disposition::AcceptedRisk,
            Disposition::OutOfScope,
        ] {
            assert!(!settled.blocks_approval(), "{settled:?} must not block");
            assert!(settled.accounts_for_prior(), "{settled:?} must account");

            let first = review(
                Decision::ChangesRequested,
                vec![finding_at("src/a.rs", Disposition::Open)],
            );
            let after = re_review(Decision::Approved, vec![finding_at("src/a.rs", settled)]);
            after
                .validate()
                .expect("a settled finding permits approval");
            after
                .check_supersedes(&first)
                .expect("and accounts for the prior one");
        }

        assert!(Disposition::Open.blocks_approval());
        assert!(!Disposition::Open.accounts_for_prior());
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
