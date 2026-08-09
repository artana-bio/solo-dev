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
//!
//! #28 closes a gap in that last sentence: the record could say a green
//! receipt was inadequate, but never who had produced the mutation behind
//! that claim, and nothing distinguished a card whose review ran in a fresh
//! process from one where it did not — 31 records claim `reviewer_actor_id:
//! codex`, one is known false, and nothing in any of them tells the other
//! thirty apart from it. [`ReviewConduct`] and [`MutationAuthorship`] are the
//! two declared facts that close it. D-013 and R-012 still hold: both are
//! declared, not proven, in exactly the register [`check_independence`]
//! already uses.

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

/// The declared kind of actor that authored a review.  This is deliberately
/// a declaration, not an identity proof; the harness can require the shape
/// without pretending a local process has proven who is behind it.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewerKind {
    Agent,
    Human,
}

/// Optional provenance for a reviewer declaration.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReviewerProvenance {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub session_id: Option<String>,
    pub principal_id: Option<String>,
}

/// Independently created evidence for a human-required review.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HumanAttestation {
    pub evidence_id: String,
    pub attestor_actor_id: String,
    pub attestor_principal_id: Option<String>,
    pub attestor_session_id: Option<String>,
    pub statement: String,
    pub independently_created: bool,
}

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

/// Where a review was actually carried out.
///
/// #28's decision: an `independent`-policy card needs a genuinely separate
/// process, not merely a distinct declared identity — the control repository
/// holds 31 review records claiming `reviewer_actor_id: codex`, one of them
/// known false, and nothing in any of the 31 distinguishes it from the other
/// thirty. This is the field that lets a review say which it was.
///
/// Lives on the review itself ([`ReviewRecord::review_conduct`] /
/// [`crate::commands::review::Verdict::review_conduct`]), not inside
/// [`GateAdequacy`] or [`MutationEvidence`]. See [`check_review_conduct`]'s
/// doc for the argument that conduct and mutation authorship are different
/// claims and belong on different objects.
///
/// Declared, not proven — D-013 and R-012 hold here exactly as they do for
/// [`check_independence`]: nothing in this tool can observe which process a
/// reviewer actually worked in. What changes is that the record stops being
/// silent about it.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewConduct {
    /// The reviewer worked in a process genuinely separate from the one that
    /// produced the candidate: a fresh context, holding only the review
    /// packet.
    SeparateProcess,
    /// The review was carried out in the same context as the work under
    /// review.
    SameContext,
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
///
/// #95 gap 1 finishes that thought. `basis` records *that* a reviewer reached
/// a conclusion and *why*, in prose nothing can check or count — a real
/// recorded one reads "Mutation-tested both behaviors. Removing `Ready` from
/// `resumes_to_active` fails `a_card_revised_...`; skipping the locator check
/// ... fails `resuming_a_revised_card_...`. Neither passes without its
/// mechanism." That is already mutation-shaped reasoning, trapped where
/// nothing can verify it happened.
/// [`mutation_evidence`](Self::mutation_evidence) is that same act, given a
/// shape a later card can cross-check mechanically instead of parsing prose
/// for it.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GateAdequacy {
    /// True when the reviewer believes the gates cover the acceptance list.
    pub gates_observe_acceptance: bool,
    /// Acceptance behaviors no gate can fail on.
    pub unobserved_behaviors: Vec<String>,
    /// How the reviewer established this.
    pub basis: String,
    /// Structured evidence that this claim was earned by mutation, or a
    /// declared reason none applies.
    ///
    /// `Option`, not because an absent claim is acceptable — see
    /// [`ReviewRecord::validate`], which refuses `None` exactly as it
    /// refuses an empty `basis` — but because 72 review facts already
    /// recorded in the control repository, every one carrying `gate_adequacy`
    /// with exactly the three fields above, predate this field and both this
    /// struct and [`ReviewRecord`] derive `deny_unknown_fields`. A bare
    /// required field would fail to deserialize every one of them; `#[serde(default)]`
    /// is what lets a record written before this field existed still
    /// project. `skip_serializing_if` is the other half of that: without it,
    /// reading one of those 72 records back and re-serializing it — which
    /// [`ReviewRecord::digest`] does, and which `integration::member_implementers`
    /// relies on producing the same bytes it always has, to catch a review
    /// altered after the fact — would silently move its digest by writing out
    /// a key the stored record never had. `ActorDeclaration::gate_failures`
    /// (`src/domain/handoff.rs`) established exactly this pattern, for
    /// exactly this reason, first.
    ///
    /// So this field being `Option` at the schema boundary is a compatibility
    /// shim for facts recorded before it existed, not a statement that the
    /// question is optional. [`ReviewRecord::validate`] is where the question
    /// is actually asked, and it runs only on a review newly being recorded
    /// — never on one of the 72 being read back — so the two constraints
    /// (old records must still deserialize; new records must not skip this)
    /// do not conflict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutation_evidence: Option<MutationEvidence>,
}

/// Evidence that a [`GateAdequacy::gates_observe_acceptance`] claim was
/// earned by mutating the candidate and watching a gate catch it — or a
/// declared reason no mutation applies to this review.
///
/// #95 names the shape directly: what was changed, which test failed, and at
/// which oracle — enough that a later card could cross-check it mechanically,
/// which free-text `basis` cannot be. Each field of [`Self::Demonstrated`] is
/// one of those three:
///
/// - `mutation`: what was changed. Not a diff — a reviewer's description of
///   the change, the same register `basis` already writes in.
/// - `failing_test`: which test failed against it. A name, so a later card
///   could look it up.
/// - `oracle`: at which gate or command the failure was observed. Distinct
///   from `failing_test` because the two answer different questions a
///   cross-check would need separately — which assertion caught it, and which
///   run surfaced that assertion.
///
/// [`Self::Exempt`] exists because #95 names the trap on the other side too:
/// "a required field that cannot honestly be filled invites a fabricated
/// one." A documentation-only card has no code to mutate, and demanding
/// `Demonstrated` from it would manufacture exactly that fabrication. Unlike
/// [`GateAdequacy::mutation_evidence`] being absent, though, an exemption is
/// itself a declared, reviewed claim — see [`ReviewRecord::validate`], which
/// refuses either variant filled with nothing but its own required shape and
/// no content, the same way `ActorDeclaration::validate` refuses a
/// present-but-empty `implementation_decisions` (#117): "a reviewer cannot
/// distinguish an empty field from an unconsidered one."
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "snake_case", tag = "status")]
pub enum MutationEvidence {
    /// The reviewer mutated the candidate and a gate caught it.
    Demonstrated {
        /// What was changed to test whether a gate would catch it.
        mutation: String,
        /// The test that failed against the mutated code.
        failing_test: String,
        /// The gate or command whose run reported the failure.
        oracle: String,
        /// Whose mutation this is. See [`MutationAuthorship`].
        authorship: MutationAuthorship,
    },
    /// No mutation applies to this review, and why.
    Exempt {
        /// Why this review has no mutation to demonstrate.
        reason: String,
    },
}

/// Whose mutation a [`MutationEvidence::Demonstrated`] claim rests on.
///
/// #28's gap in #95 gap 1's field: `mutation_evidence` recorded *a* mutation
/// from the day it landed, never *whose*. #28 §2 is explicit about why that
/// matters: what made separate review actually find defects "was not the
/// identity string. It was that the reviewer chose a different thing to
/// mutate." A reviewer who restates the implementer's own mutation as their
/// "evidence" has not done that, even when every other fact on the review —
/// including a genuinely separate [`ReviewConduct::SeparateProcess`] — is
/// true. The wave this card ships in produced four reviews in exactly that
/// state, which is why this is a fact recorded separately from conduct
/// rather than folded into it; see [`check_review_conduct`]'s doc.
///
/// Required, with no `#[serde(default)]`: unlike [`GateAdequacy::mutation_evidence`]
/// itself, this field has no population of pre-existing records to stay
/// compatible with. Verified, not assumed — no review record in the control
/// repository carries `mutation_evidence` at all yet (`git --git-dir=$HOME/Documents/Code/change-harness-control/.git grep -l mutation_evidence HEAD -- reviews/`
/// returns nothing), so there is no stored `Demonstrated` value anywhere for
/// a bare required field to orphan. That makes this exactly as new as
/// [`MutationEvidence::Demonstrated`]'s other three fields, which carry no
/// compatibility shim of their own either.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MutationAuthorship {
    /// The reviewer devised this mutation independently.
    ReviewerDevised,
    /// The reviewer is restating a mutation the implementer already ran or
    /// described, not one they produced themselves.
    ImplementerRestated,
}

impl GateAdequacy {
    /// Checks that `mutation_evidence` is filled, honestly, one way or the
    /// other.
    ///
    /// #95 gap 1, §8.3, extracted so `ReviewRecord::validate` and
    /// `review record --dry-run`'s preview share exactly one implementation
    /// of this rule rather than two that can drift. Before this extraction,
    /// `validate` had the only call site — inside `run_record`'s real
    /// transaction, on a fully-built `ReviewRecord` — and the preview path
    /// never constructs one, so it never asked this question: a verdict
    /// with no mutation evidence at all was accepted by `--dry-run` and
    /// refused by the real command, exactly the failure
    /// `tests/dry_run_parity.rs`'s module doc names. This method takes only
    /// `&self`, so either caller can run it against what it already has —
    /// `self.gate_adequacy` on a constructed record, or `verdict.gate_adequacy`
    /// on the reviewer's submitted document, before a `ReviewRecord` exists at
    /// all.
    ///
    /// # Errors
    ///
    /// Returns a policy error when the field is absent, or present but empty.
    pub fn validate_mutation_evidence(&self) -> Result<(), HarnessError> {
        match &self.mutation_evidence {
            None => Err(HarnessError::Control {
                reason: "a review must record the mutation its gate-adequacy claim was earned by — what was changed, which test failed, and at which oracle — or declare, on the record, why none applies".to_owned(),
                code: ErrorCode::PolicyIncompleteReview,
            }),
            Some(MutationEvidence::Demonstrated {
                mutation,
                failing_test,
                oracle,
                // `authorship` is checked by nothing here: it is an enum, not
                // a `String`, so there is no empty state for it to be caught
                // in silently the way #117's shape catches blank text — the
                // deserializer already refuses any value that is not one of
                // `MutationAuthorship`'s two variants.
                authorship: _,
            }) => {
                if mutation.trim().is_empty()
                    || failing_test.trim().is_empty()
                    || oracle.trim().is_empty()
                {
                    // #117's shape, one field short of that card rather than
                    // this one: a value is present but empty, which is not
                    // the same fact as absent and must not be read as it.
                    Err(HarnessError::Control {
                        reason: "mutation evidence is present but empty; a reviewer cannot distinguish an empty field from an unconsidered one".to_owned(),
                        code: ErrorCode::PolicyIncompleteReview,
                    })
                } else {
                    Ok(())
                }
            }
            Some(MutationEvidence::Exempt { reason }) => {
                if reason.trim().is_empty() {
                    Err(HarnessError::Control {
                        reason: "mutation evidence declares an exemption but gives no reason; a reviewer cannot distinguish an empty field from an unconsidered one".to_owned(),
                        code: ErrorCode::PolicyIncompleteReview,
                    })
                } else {
                    Ok(())
                }
            }
        }
    }
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
    /// Typed actor kind. `None` is the compatibility representation for
    /// records written before the typed contract shipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer_kind: Option<ReviewerKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer_provenance: Option<ReviewerProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_attestation: Option<HumanAttestation>,
    /// First-class executable mutation receipts supporting this review.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mutation_receipt_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mutation_receipt_bindings: Vec<crate::domain::mutation::MutationReceiptBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutation_exemption: Option<crate::domain::mutation::MutationExemption>,
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
    /// Where this review was actually carried out. Declared, not proven; see
    /// [`ReviewConduct`] and D-013.
    ///
    /// `Option`, for the same reason [`GateAdequacy::mutation_evidence`] is:
    /// the roughly 72 reviews already recorded in the control repository
    /// predate this field entirely, and [`ReviewRecord`] derives
    /// `deny_unknown_fields`, so a bare required field would orphan every one
    /// of them on read. `#[serde(default)]` is what lets an old record still
    /// deserialize; `skip_serializing_if` is what keeps re-serializing one
    /// from writing out a key it never had, which would move
    /// [`ReviewRecord::digest`] for a record nothing about actually changed —
    /// see `mutation_evidence`'s doc for the full argument, identical here.
    ///
    /// Not read by [`ReviewRecord::validate`] itself — only
    /// [`check_review_conduct`] reads it, called beside `validate` rather
    /// than from inside it — and, exactly as [`GateAdequacy::mutation_evidence`]
    /// is, refused when absent: on a card whose `review_policy` is exactly
    /// `"independent"`, `None` is refused precisely as
    /// [`ReviewConduct::SameContext`] is. Only a card whose `review_policy`
    /// is not exactly `"independent"` accepts an absent declaration; see
    /// `check_review_conduct`'s doc for the full argument, including why
    /// requiring the declaration here does not orphan the roughly 72 reviews
    /// already on disk — the same reason `mutation_evidence` itself does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_conduct: Option<ReviewConduct>,
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

        validate_reviewer_identity(
            &self.reviewer_actor_id,
            self.reviewer_kind,
            self.human_attestation.as_ref(),
        )?;
        if self.reviewer_kind == Some(ReviewerKind::Human) {
            validate_human_attestation_boundary(
                &self.reviewer_actor_id,
                self.reviewer_provenance.as_ref(),
                self.human_attestation.as_ref(),
                &self.feature_actor_id,
                None,
                None,
            )?;
        }

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

        // #95 gap 1, §8.3. Required, with a declared exemption, not optional
        // and silent — see `GateAdequacy::validate_mutation_evidence` for the
        // rule itself and why it is shared with `review record --dry-run`'s
        // preview rather than living only here.
        self.gate_adequacy.validate_mutation_evidence()?;

        if self.decision == Decision::Approved
            && self.reviewer_kind.is_some()
            && self.mutation_receipt_ids.is_empty()
            && self.mutation_exemption.is_none()
        {
            return Err(HarnessError::Control {
                reason: "an approval using the typed reviewer contract must reference an executable mutation receipt or a policy-valid typed exemption".to_owned(),
                code: ErrorCode::PolicyIncompleteReview,
            });
        }

        if let Some(exemption) = &self.mutation_exemption
            && (exemption.code.trim().is_empty()
                || exemption.reason.trim().is_empty()
                || exemption.approved_by.trim().is_empty())
        {
            return Err(HarnessError::Control {
                reason: "mutation exemption must name a code, reason, and approver".to_owned(),
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
            || self.reviewer_kind == Some(ReviewerKind::Human)
        {
            return Ok(());
        }
        Err(HarnessError::Control {
            reason: format!(
                "card risk `{}` requires a declared typed human reviewer with independent attestation; legacy human_reviewer is migration input only",
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

/// Validates the new typed reviewer contract while preserving old records.
/// New `human` declarations require an independently created attestation;
/// the legacy boolean remains readable only as a migration bridge.
///
/// # Errors
///
/// Returns a policy error when the typed declaration contradicts attestation
/// evidence or the reviewer self-creates that evidence.
pub fn validate_reviewer_identity(
    reviewer_actor_id: &str,
    kind: Option<ReviewerKind>,
    attestation: Option<&HumanAttestation>,
) -> Result<(), HarnessError> {
    match kind {
        Some(ReviewerKind::Agent) => {
            if attestation.is_some() {
                Err(HarnessError::Control {
                    reason: "an agent-authored verdict cannot carry human-attestation evidence"
                        .to_owned(),
                    code: ErrorCode::PolicyRiskReview,
                })
            } else {
                Ok(())
            }
        }
        Some(ReviewerKind::Human) => {
            let evidence = attestation.ok_or_else(|| HarnessError::Control {
                reason:
                    "a human reviewer requires independently created human-attestation evidence"
                        .to_owned(),
                code: ErrorCode::PolicyRiskReview,
            })?;
            if evidence.evidence_id.trim().is_empty()
                || evidence.attestor_actor_id.trim().is_empty()
                || evidence
                    .attestor_principal_id
                    .as_deref()
                    .is_some_and(|id| id.trim().is_empty())
                || evidence
                    .attestor_session_id
                    .as_deref()
                    .is_some_and(|id| id.trim().is_empty())
                || evidence.statement.trim().is_empty()
                || !evidence.independently_created
            {
                return Err(HarnessError::Control {
                    reason:
                        "human-attestation evidence must be independently created and non-empty"
                            .to_owned(),
                    code: ErrorCode::PolicyRiskReview,
                });
            }
            if actors::same(reviewer_actor_id, &evidence.attestor_actor_id) {
                return Err(HarnessError::Control {
                    reason: "the reviewer cannot self-create the human-attestation evidence"
                        .to_owned(),
                    code: ErrorCode::PolicyRiskReview,
                });
            }
            Ok(())
        }
        None => Ok(()),
    }
}

/// Validates the declared provenance boundary for independently-created
/// human attestation evidence. These identifiers are caller-declared, not
/// host-attested, but the protocol still refuses reused principal or session
/// boundaries hidden behind different actor labels.
///
/// # Errors
///
/// Returns a policy error when attestor provenance is missing, blank, or
/// shared with the reviewer or persisted implementer boundary.
pub fn validate_human_attestation_boundary(
    reviewer_actor_id: &str,
    reviewer_provenance: Option<&ReviewerProvenance>,
    attestation: Option<&HumanAttestation>,
    implementer_actor_id: &str,
    implementer_principal_id: Option<&str>,
    implementer_session_id: Option<&str>,
) -> Result<(), HarnessError> {
    let evidence = attestation.ok_or_else(|| HarnessError::Control {
        reason: "a human reviewer requires independently created human-attestation evidence"
            .to_owned(),
        code: ErrorCode::PolicyRiskReview,
    })?;
    let principal = evidence
        .attestor_principal_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| HarnessError::Control {
            reason: "human attestation requires a nonblank attestor principal_id".to_owned(),
            code: ErrorCode::PolicyRiskReview,
        })?;
    let session = evidence
        .attestor_session_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| HarnessError::Control {
            reason: "human attestation requires a nonblank attestor session_id".to_owned(),
            code: ErrorCode::PolicyRiskReview,
        })?;
    if actors::same(reviewer_actor_id, &evidence.attestor_actor_id)
        || reviewer_provenance
            .and_then(|provenance| provenance.principal_id.as_deref())
            .is_some_and(|id| actors::same(id, principal))
        || reviewer_provenance
            .and_then(|provenance| provenance.session_id.as_deref())
            .is_some_and(|id| actors::same(id, session))
        || actors::same(implementer_actor_id, &evidence.attestor_actor_id)
        || implementer_principal_id.is_some_and(|id| actors::same(id, principal))
        || implementer_session_id.is_some_and(|id| actors::same(id, session))
    {
        return Err(HarnessError::Control {
            reason: "human attestation must come from a distinct declared actor, principal, and session boundary"
                .to_owned(),
            code: ErrorCode::PolicyRiskReview,
        });
    }
    Ok(())
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

/// Refuses a review of an `independent`-policy card that does not declare a
/// genuinely separate conduct — either because it says nothing, or because it
/// says `same_context`.
///
/// #28 §1: "a genuinely separate process is required for an
/// `independent`-policy card." §4 of the same card established that
/// `CardDraft::review_policy` (`src/domain/card.rs`) had never been read for
/// its value anywhere in this codebase — validated only for non-emptiness —
/// so this is the first place `review_policy` becomes load-bearing.
///
/// A free function, not a method on [`ReviewRecord`] or folded into
/// [`ReviewRecord::validate`], for a reason [`check_independence`] does not
/// have to answer: `review_policy` lives on the card, not on the review, and
/// `validate` takes only `&self`. Changing its signature to thread
/// `review_policy` through would touch every one of its call sites in this
/// module's own tests for a question `validate` otherwise has no stake in.
/// The shape this mirrors instead is [`ReviewRecord::check_risk_policy`]:
/// both are a declared fact on the review, checked against an external fact
/// the card carries, called next to `validate` rather than inside it. Unlike
/// `check_risk_policy` — which `commands::review::preview_record`'s own doc
/// names as a check `--dry-run` deliberately does not mirror — this one
/// must run on both paths: #189 already lists seven checks that skip
/// `review record --dry-run`, and #120 closed one by running it ahead of the
/// dry-run branch rather than only inside the real transaction. This card
/// follows that solution, not the count: `commands::review::preview_record`
/// calls this directly, in the same relative position `check_independence`
/// holds there, so a preview cannot report success for a verdict the real
/// command would refuse.
///
/// # `None` is refused here, and it is not `mutation_evidence`'s gap repeated
///
/// The first version of this function read `conduct != Some(SameContext)` as
/// the whole rule and left `None` accepted on every `review_policy`,
/// reasoning that refusing it would orphan the roughly 72 reviews already on
/// disk. That reasoning does not survive contact with
/// [`GateAdequacy::validate_mutation_evidence`], which sits a few hundred
/// lines above this function and already answers the identical question for
/// an identical shape: those 72 records carry no `mutation_evidence` at all,
/// `validate_mutation_evidence` refuses `None` unconditionally, and nothing
/// broke, because **stored records are read, never re-validated** —
/// `ReviewRecord::validate` (and this function, called beside it) runs
/// exactly once, at the moment a new review is recorded, never again when an
/// old one is read back by `review inspect` or `integration::member_implementers`.
/// A required field orphans old records only if something asks them to
/// satisfy it after the fact; nothing here does.
///
/// The narrower reading was this function's own error, not a misreading of
/// #28: §5 says a separate process "is required" for an `independent` card,
/// and omission does not satisfy a requirement — it is silence about one.
/// Leaving `None` unrefused made the check's only casualty the reviewer
/// honest enough to write `same_context`; one who wrote nothing passed
/// unremarked, which is the shape of a rule that punishes candor.
///
/// So, following [`GateAdequacy::validate_mutation_evidence`]'s established
/// shape and register — required, with the refusal naming exactly what the
/// reviewer must supply — `None` is now refused on an `independent` card
/// exactly as [`ReviewConduct::SameContext`] is, under
/// [`ErrorCode::PolicyIncompleteReview`] rather than
/// [`ErrorCode::PolicySelfReview`]: a missing declaration and a false one are
/// different failures, and this codebase already keeps them under different
/// codes everywhere else `ReviewRecord::validate` checks both in the same
/// pass (an empty `basis` versus a self-review, for instance). D-013 still
/// holds without qualification: requiring the declaration is not proving the
/// process, and the message below says so.
///
/// # What this still refuses only the obvious form of
///
/// A `review_policy` of `"Independent"`, `"INDEPENDENT"`, `"independent-ish"`,
/// or any other string that is not the exact literal `"independent"` is not
/// refused, whatever conduct is declared or omitted. §8 of #28 forbids
/// widening `CardDraft.review_policy`'s type or validation — it stays a
/// free-form `String` — so there is no enum this function could match
/// against instead, and no normalization this function invents is authorized
/// by the card that owns the field. A card author who misspells the policy
/// they meant gets silence, not a refusal, from this check. Named here
/// rather than closed silently, for the same reason the `None` gap used to
/// be: D-013 already commits this tool to declared-not-proven facts, and a
/// check that only catches the exact spelling of whoever filled in the
/// policy field is consistent with that, not a departure from it.
///
/// # Errors
///
/// On a card whose `review_policy` is exactly `"independent"`:
///
/// - Returns [`ErrorCode::PolicyIncompleteReview`] when `conduct` is `None` —
///   the same code, and the same "required, with a declared reason or value,
///   not optional and silent" register, [`GateAdequacy::validate_mutation_evidence`]
///   already uses for an absent `mutation_evidence`.
/// - Returns [`ErrorCode::PolicySelfReview`] when `conduct` is declared
///   [`ReviewConduct::SameContext`] — the closest existing code, reused
///   rather than added per #28 §8, for the same underlying concern
///   `check_independence` already reports under it: a review that admits it
///   was not independent of the work it reviews.
///
/// Any other `review_policy` accepts every `conduct`, declared or not — see
/// the false-positive direction this must not create, tested directly.
pub fn check_review_conduct(
    review_policy: &str,
    conduct: Option<ReviewConduct>,
) -> Result<(), HarnessError> {
    if review_policy != "independent" {
        return Ok(());
    }
    match conduct {
        None => Err(HarnessError::Control {
            reason: "a review of an `independent`-policy card must declare whether it was conducted in a separate process — set `review_conduct: separate_process` or `review_conduct: same_context` on the record; independence is procedural, so the harness can require the declaration but cannot confirm it".to_owned(),
            code: ErrorCode::PolicyIncompleteReview,
        }),
        Some(ReviewConduct::SameContext) => Err(HarnessError::Control {
            reason: "review_policy `independent` requires a genuinely separate review process, and this review declares `same_context` conduct; independence is procedural, so the harness can only refuse the declared case".to_owned(),
            code: ErrorCode::PolicySelfReview,
        }),
        Some(ReviewConduct::SeparateProcess) => Ok(()),
    }
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
            mutation_evidence: Some(MutationEvidence::Demonstrated {
                mutation: "removed the absolute-zero guard in fahrenheit_to_celsius".to_owned(),
                failing_test: "rejects_below_absolute_zero".to_owned(),
                oracle: "gate.unit".to_owned(),
                authorship: MutationAuthorship::ReviewerDevised,
            }),
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
            reviewer_kind: None,
            reviewer_provenance: None,
            human_attestation: None,
            mutation_receipt_ids: vec![],
            mutation_receipt_bindings: vec![],
            mutation_exemption: None,
            feature_actor_id: "implementer-session-1".to_owned(),
            decision,
            findings,
            gate_adequacy: adequacy(),
            residual_risks: vec![],
            human_reviewer: false,
            review_conduct: None,
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

    // #95 gap 1, §8.3 and §10 test 3. `None` is exactly what a review
    // deserializes to when the field is entirely absent — see
    // `GateAdequacy::mutation_evidence` — so this also stands in for "an
    // operator who wrote a verdict with no `mutation_evidence` key at all".
    //
    // Mutation (§11.2): delete the `None => { ... }` arm (or the whole
    // `match` this belongs to) from `ReviewRecord::validate`. This test must
    // fail — `validate()` would then return `Ok(())` for a review with no
    // mutation evidence at all.
    #[test]
    fn a_review_with_no_mutation_evidence_is_refused() {
        let mut invalid = review(Decision::Approved, vec![]);
        invalid.gate_adequacy.mutation_evidence = None;
        let error = invalid.validate().expect_err("must refuse");
        assert_eq!(error.code(), ErrorCode::PolicyIncompleteReview);
        assert!(
            error.to_string().contains("mutation"),
            "the refusal must name what is missing: {error}"
        );
    }

    // §10 test 3, the #117 shape named in §11.4. A `Demonstrated` value is
    // present — the key exists, `serde` accepted the document — but its
    // fields are empty, which is a different fact from absent and must not
    // be silently treated as filled in.
    //
    // Mutation (§11.4): delete the `mutation.trim().is_empty() || ...` guard
    // inside the `Some(MutationEvidence::Demonstrated { .. })` arm. This test
    // must fail — an empty-but-present `Demonstrated` would then validate.
    #[test]
    fn a_present_but_empty_demonstrated_mutation_is_refused() {
        let mut invalid = review(Decision::Approved, vec![]);
        invalid.gate_adequacy.mutation_evidence = Some(MutationEvidence::Demonstrated {
            mutation: String::new(),
            failing_test: String::new(),
            oracle: String::new(),
            authorship: MutationAuthorship::ReviewerDevised,
        });
        let error = invalid.validate().expect_err("must refuse");
        assert_eq!(error.code(), ErrorCode::PolicyIncompleteReview);
        assert!(
            error
                .to_string()
                .contains("a reviewer cannot distinguish an empty field from an unconsidered one"),
            "the #117 rationale must survive for this field too: {error}"
        );
    }

    /// Each field of `Demonstrated` is checked independently: a mutation that
    /// only guarded the first field would let the other two through empty.
    #[test]
    fn each_field_of_a_demonstrated_mutation_is_checked() {
        for (mutation, failing_test, oracle) in [
            ("", "rejects_below_absolute_zero", "gate.unit"),
            ("removed the guard", "", "gate.unit"),
            ("removed the guard", "rejects_below_absolute_zero", ""),
        ] {
            let mut invalid = review(Decision::Approved, vec![]);
            invalid.gate_adequacy.mutation_evidence = Some(MutationEvidence::Demonstrated {
                mutation: mutation.to_owned(),
                failing_test: failing_test.to_owned(),
                oracle: oracle.to_owned(),
                authorship: MutationAuthorship::ReviewerDevised,
            });
            let error = invalid.validate().expect_err("must refuse");
            assert_eq!(error.code(), ErrorCode::PolicyIncompleteReview);
        }
    }

    // §10 test 3, the #117 shape, on the other variant: an exemption whose
    // `reason` is blank is present-but-empty in exactly the same way an
    // unfilled `Demonstrated` is, and must be refused for the same reason.
    //
    // Mutation (§11.4): delete the `reason.trim().is_empty()` guard inside
    // the `Some(MutationEvidence::Exempt { .. })` arm. This test must fail.
    #[test]
    fn an_exemption_with_no_reason_is_refused() {
        let mut invalid = review(Decision::Approved, vec![]);
        invalid.gate_adequacy.mutation_evidence = Some(MutationEvidence::Exempt {
            reason: "   ".to_owned(),
        });
        let error = invalid.validate().expect_err("must refuse");
        assert_eq!(error.code(), ErrorCode::PolicyIncompleteReview);
        assert!(
            error
                .to_string()
                .contains("a reviewer cannot distinguish an empty field from an unconsidered one"),
            "{error}"
        );
    }

    // The positive half of §8.3: an exemption is a real way through, not a
    // refusal wearing a different shape. #95's trap runs both directions —
    // this is the "a required field that cannot honestly be filled invites a
    // fabricated one" half; the two tests above are the "an `Option` that is
    // always `None` teaches nothing" half.
    #[test]
    fn a_review_may_declare_an_exemption_from_mutation_evidence() {
        let mut declared = review(Decision::Approved, vec![]);
        declared.gate_adequacy.mutation_evidence = Some(MutationEvidence::Exempt {
            reason: "documentation-only card; no behavior to mutate".to_owned(),
        });
        declared
            .validate()
            .expect("a declared, non-empty exemption is honest and must be accepted");
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
            mutation_evidence: Some(MutationEvidence::Demonstrated {
                mutation: "removed the absolute-zero guard".to_owned(),
                failing_test: "none — this is the unobserved behavior".to_owned(),
                oracle: "gate.unit".to_owned(),
                authorship: MutationAuthorship::ReviewerDevised,
            }),
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

    // #95 gap 1, §4 and §10 test 1. Verified §4 counts (2026-08-07): the
    // control repository at `~/Documents/Code/change-harness-control` holds
    // exactly 72 files under `reviews/`, and every one of their
    // `gate_adequacy` objects carries exactly the three keys
    // `gates_observe_acceptance`, `unobserved_behaviors`, and `basis` --
    // verified with a script reading all 72, not merely one. This is one of
    // those 72, `reviews/RV-000005.json`, embedded byte-for-byte as read from
    // disk -- not a synthetic fixture built to look like one. It predates
    // `mutation_evidence` entirely: no such key appears anywhere below.
    //
    // Mutation (§11.3): edit the constant below to add
    // `deny_unknown_fields`-breaking content (an extra key inside
    // `gate_adequacy`, for instance), or otherwise break backward
    // compatibility -- for instance, by giving `mutation_evidence` a
    // `#[serde(default)]` that does not actually satisfy `deny_unknown_fields`
    // on a document lacking the key, or by removing `#[serde(default)]`
    // outright. This test must fail either way: it is reading a real record,
    // not a convenient one, so a mutation to the fixture below is exactly as
    // load-bearing as a mutation to the production deserializer.
    //
    // #28 §9: re-verified the same count today (2026-08-07) and re-checked
    // whether any of the 72 already carry `mutation_evidence` -- none do
    // (`git --git-dir=$HOME/Documents/Code/change-harness-control/.git grep
    // -l mutation_evidence HEAD -- reviews/` returns nothing), so this same
    // fixture is also the only real record available to prove compatibility
    // for `review_conduct`, the field this card adds: it predates that field
    // exactly as it predates `mutation_evidence`. No second fixture is
    // fabricated for it below -- see the added assertions in the test itself.
    const REAL_PRE_EXISTING_REVIEW_RV_000005: &str = r#"{
  "schema": "harness.review/v1",
  "review_id": "RV-000005",
  "card_id": "F-005",
  "card_revision": 1,
  "card_digest": "sha256:57679db3d3c1fb461530536ff9f262608c7e6ce3fd8dd932eb052fcb6870fb05",
  "cycle_id": "C-004",
  "baseline_sha": "8dfe3b9fa8752d5205708898172b5143af9d1a02",
  "candidate_sha": "794f046f8fb2cb1718043f531e6e7193c2f902ab",
  "handoff_id": "H-000006",
  "handoff_digest": "sha256:31ec24776f7f94f0d28f3bf93b51b225262cfd0a8051b571cb68eaaa06ddb511",
  "reviewer_actor_id": "f005-reviewer",
  "feature_actor_id": "operator",
  "decision": "approved",
  "findings": [
    {
      "severity": "medium",
      "location": "src/commands/work.rs",
      "detail": "Recorded again for this card: the review was made by a distinct declared actor but not from a genuinely fresh context.",
      "disposition": "accepted_risk"
    }
  ],
  "gate_adequacy": {
    "gates_observe_acceptance": true,
    "unobserved_behaviors": [],
    "basis": "Mutation-tested both behaviors. Removing Ready from resumes_to_active fails a_card_revised_while_allocated_can_be_resumed; skipping the locator check for the ready case fails resuming_a_revised_card_still_checks_the_locator. Neither passes without its mechanism."
  },
  "residual_risks": [],
  "supersedes": null,
  "reviewed_at": "2026-07-29T05:07:28Z",
  "canonical_algorithm": "harness.canonical-json/v1"
}
"#;

    #[test]
    fn a_real_pre_existing_review_record_still_projects() {
        let record: ReviewRecord = serde_json::from_str(REAL_PRE_EXISTING_REVIEW_RV_000005)
            .expect("a review recorded before mutation_evidence existed must still deserialize");

        // It really does predate the field: reading it back must not have
        // manufactured content the file never had.
        assert_eq!(record.review_id.to_string(), "RV-000005");
        assert!(record.gate_adequacy.mutation_evidence.is_none());
        // #28: it predates `review_conduct` exactly as it predates
        // `mutation_evidence`, and for the same reason must read back `None`
        // rather than manufacture a declaration this record never made.
        assert!(record.review_conduct.is_none());

        // Half of §4's claim is deserialization; the other half is that a
        // record already digested under the old schema must keep digesting
        // to the same value now that a new field exists on the type, because
        // `integration::member_implementers` recomputes a review's digest
        // from a fresh read and refuses when it no longer matches what an
        // integration pinned -- the exact meaning of "orphaned" this harness
        // already guards elsewhere. `skip_serializing_if` on
        // `mutation_evidence` is what keeps that recomputation from moving:
        // this value was captured with `ReviewRecord::digest()` on the
        // unmodified pre-#95 code, from this exact fixture's content, before
        // `mutation_evidence` was added at all. The same digest, unchanged
        // again here, is therefore also proof that `review_conduct` --
        // added after that capture, under the identical
        // `#[serde(default, skip_serializing_if = "Option::is_none")]`
        // shape -- does not move it either.
        assert_eq!(
            record.digest().unwrap().as_str(),
            "sha256:b826c2bab33d692053c1608a7d872f545071585259898bd139e97325b8d6d14a",
            "adding mutation_evidence and review_conduct must not move the digest of a record \
             that predates both"
        );

        // And the mechanism, not just its result: re-serializing this record
        // must not write out a key the stored file never had.
        let reencoded = serde_json::to_string(&record).unwrap();
        assert!(
            !reencoded.contains("mutation_evidence"),
            "a record with no mutation evidence must omit the key entirely, not write it out \
             as null or as an empty value: {reencoded}"
        );
        assert!(
            !reencoded.contains("review_conduct"),
            "a record with no declared conduct must omit the key entirely too: {reencoded}"
        );
    }

    /// §10 test 4, for the type this card adds specifically: both
    /// `MutationEvidence` variants, written and read back.
    #[test]
    fn mutation_evidence_round_trips_both_variants() {
        let demonstrated = review(Decision::Approved, vec![]);
        let encoded = serde_json::to_string_pretty(&demonstrated).unwrap();
        let decoded: ReviewRecord = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, demonstrated);
        assert_eq!(
            decoded.gate_adequacy.mutation_evidence,
            demonstrated.gate_adequacy.mutation_evidence
        );

        let mut exempted = review(Decision::Approved, vec![]);
        exempted.gate_adequacy.mutation_evidence = Some(MutationEvidence::Exempt {
            reason: "documentation-only card; no behavior to mutate".to_owned(),
        });
        let encoded = serde_json::to_string_pretty(&exempted).unwrap();
        let decoded: ReviewRecord = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, exempted);
        assert_eq!(decoded.digest().unwrap(), exempted.digest().unwrap());
    }

    /// #28 §10 test 4 (`ReviewConduct`'s half): both variants, written and
    /// read back, plus the undeclared case old records and old verdicts both
    /// deserialize to.
    #[test]
    fn review_conduct_round_trips_both_variants() {
        for conduct in [ReviewConduct::SeparateProcess, ReviewConduct::SameContext] {
            let mut declared = review(Decision::Approved, vec![]);
            declared.review_conduct = Some(conduct);
            let encoded = serde_json::to_string_pretty(&declared).unwrap();
            let decoded: ReviewRecord = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, declared);
            assert_eq!(decoded.review_conduct, Some(conduct));
        }

        let undeclared = review(Decision::Approved, vec![]);
        assert_eq!(undeclared.review_conduct, None);
        let encoded = serde_json::to_string_pretty(&undeclared).unwrap();
        assert!(
            !encoded.contains("review_conduct"),
            "an undeclared conduct must not be written out as a null or empty value: {encoded}"
        );
        let decoded: ReviewRecord = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, undeclared);
    }

    /// #28 §10 test 4 (`MutationAuthorship`'s half). Distinct from
    /// `mutation_evidence_round_trips_both_variants` above, which only
    /// exercises `ReviewerDevised` incidentally through the shared `review()`
    /// fixture; this pins `ImplementerRestated` too.
    #[test]
    fn mutation_authorship_round_trips_both_variants() {
        for authorship in [
            MutationAuthorship::ReviewerDevised,
            MutationAuthorship::ImplementerRestated,
        ] {
            let mut declared = review(Decision::Approved, vec![]);
            declared.gate_adequacy.mutation_evidence = Some(MutationEvidence::Demonstrated {
                mutation: "removed the absolute-zero guard".to_owned(),
                failing_test: "rejects_below_absolute_zero".to_owned(),
                oracle: "gate.unit".to_owned(),
                authorship,
            });
            let encoded = serde_json::to_string_pretty(&declared).unwrap();
            let decoded: ReviewRecord = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, declared);
            assert_eq!(
                decoded.gate_adequacy.mutation_evidence,
                declared.gate_adequacy.mutation_evidence
            );
        }
    }

    // #28 §12 mutation 1. Mutation: delete the `Some(ReviewConduct::SameContext)`
    // arm (or the whole early-return `if`) from `check_review_conduct`. This
    // test must fail -- the function would then return `Ok(())` for the
    // declared-same-context, independent-policy case it exists to refuse.
    #[test]
    fn an_independent_policy_card_refuses_declared_same_context_conduct() {
        let error = check_review_conduct("independent", Some(ReviewConduct::SameContext))
            .expect_err("must refuse");
        assert_eq!(error.code(), ErrorCode::PolicySelfReview);
        assert!(
            error.to_string().contains("procedural"),
            "the message must not overclaim what this check proves, in the register \
             `check_independence` already uses: {error}"
        );
    }

    #[test]
    fn an_independent_policy_card_accepts_declared_separate_process_conduct() {
        check_review_conduct("independent", Some(ReviewConduct::SeparateProcess))
            .expect("the honest declaration this check exists to require must be accepted");
    }

    // Repair, post-review. The first version of this function accepted a
    // review that said nothing about its conduct on an `independent` card --
    // reasoning that refusing `None` would orphan the roughly 72 reviews
    // already on disk, which does not hold: `GateAdequacy::validate_mutation_evidence`
    // already refuses an absent `mutation_evidence` unconditionally, on an
    // identical `Option` shape, because stored records are read, never
    // re-validated. The practical effect of the old rule was that the only
    // reviewer it ever refused was one honest enough to write
    // `same_context`; one who wrote nothing passed unremarked. §28 §1 says a
    // separate process "is required", and omission does not satisfy a
    // requirement.
    //
    // Mutation: restore `conduct != Some(ReviewConduct::SameContext)` as the
    // whole rule (or otherwise make the `None` arm return `Ok(())`). This
    // test must fail -- an undeclared conduct would then be accepted again.
    #[test]
    fn an_independent_policy_card_refuses_undeclared_conduct() {
        let error = check_review_conduct("independent", None).expect_err("must refuse");
        assert_eq!(error.code(), ErrorCode::PolicyIncompleteReview);
        assert!(
            error.to_string().contains("separate process")
                && error.to_string().contains("separate_process"),
            "the refusal must name what the reviewer must declare, in `mutation_evidence`'s \
             established register: {error}"
        );
        assert!(
            error.to_string().contains("cannot confirm"),
            "the message must not overclaim what requiring the declaration proves: {error}"
        );
    }

    // #28 §12 mutation 2, the false-positive direction. Mutation: make the
    // refusal fire regardless of `review_policy` (drop the `review_policy !=
    // "independent"` half of the guard, or the parameter entirely). This test
    // must fail -- a card whose policy is not `independent` would then have
    // its same-context conduct refused too, which #28 never asks for: the
    // decision is scoped to `independent`-policy cards specifically.
    #[test]
    fn a_non_independent_policy_card_accepts_declared_same_context_conduct() {
        for policy in ["solo", "pair", ""] {
            check_review_conduct(policy, Some(ReviewConduct::SameContext)).unwrap_or_else(
                |error| {
                    panic!(
                        "policy `{policy}` must not trigger the independent-only refusal: {error}"
                    )
                },
            );
        }
    }

    /// The repair above is scoped to `independent`-policy cards exactly as
    /// the same-context refusal always was: a non-`independent` card accepts
    /// an undeclared conduct too, not only a declared `same_context` one.
    /// Both halves of the false-positive direction, pinned separately so a
    /// mutation widening either one independently is caught.
    #[test]
    fn a_non_independent_policy_card_accepts_undeclared_conduct() {
        for policy in ["solo", "pair", ""] {
            check_review_conduct(policy, None).unwrap_or_else(|error| {
                panic!("policy `{policy}` must not require the declaration at all: {error}")
            });
        }
    }

    /// §10.3: the obvious rule is an exact match against the literal
    /// `"independent"`. A spelling or casing variant is not read as meaning
    /// the same thing, because #28 §8 forbids this card from turning
    /// `CardDraft.review_policy` into anything other than the free-form
    /// `String` it already is, and no normalization here is authorized by
    /// the card that owns that field. Documented as a real gap, not fixed:
    /// a card author who misspells `independent` gets silence, not a
    /// refusal.
    #[test]
    fn a_misspelled_or_miscased_independent_policy_is_not_refused() {
        for policy in [
            "Independent",
            "INDEPENDENT",
            "independent-ish",
            " independent",
        ] {
            check_review_conduct(policy, Some(ReviewConduct::SameContext)).unwrap_or_else(
                |error| {
                    panic!(
                        "policy `{policy}` is not the exact literal `independent`, so this check \
                     must not refuse it: {error}"
                    )
                },
            );
        }
    }

    /// #28 §10.1 and §10.2, made concrete. §10.2's evidence for keeping
    /// conduct and mutation authorship as two separate fields rather than
    /// collapsing them into one: "a review that is separate but whose
    /// mutation is the implementer's restated is a real state and the wave
    /// produced four of them." That state is `SeparateProcess` conduct
    /// paired with `ImplementerRestated` authorship, and it must validate —
    /// neither field's rule reads the other, so the combination is neither
    /// refused by `check_review_conduct` (conduct is honestly
    /// `SeparateProcess`) nor by `GateAdequacy::validate_mutation_evidence`
    /// (authorship is filled, not empty). Collapsing the two into one field
    /// would need a third value purely to name this combination; keeping
    /// them apart names it for free as the cross product of two fields that
    /// already have two values each.
    #[test]
    fn a_separate_review_may_honestly_restate_the_implementer_s_mutation() {
        let mut record = review(Decision::Approved, vec![]);
        record.review_conduct = Some(ReviewConduct::SeparateProcess);
        record.gate_adequacy.mutation_evidence = Some(MutationEvidence::Demonstrated {
            mutation:
                "removed the absolute-zero guard, as the implementer's handoff already described"
                    .to_owned(),
            failing_test: "rejects_below_absolute_zero".to_owned(),
            oracle: "gate.unit".to_owned(),
            authorship: MutationAuthorship::ImplementerRestated,
        });

        record.validate().expect(
            "a separate review restating the implementer's own mutation is honest and real",
        );
        check_review_conduct("independent", record.review_conduct)
            .expect("declared separate-process conduct is never refused, whatever the mutation's authorship");
    }
}
