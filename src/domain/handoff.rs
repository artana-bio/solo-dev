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

/// Which commit of a declared dependency a candidate's history contains.
///
/// Section 10.7 requires a handoff to carry dependency SHAs, and invariant
/// 7.3.6 makes a *relevant* dependency SHA change invalidate dependent
/// evidence. This record is what makes "relevant" decidable.
///
/// The binding is the dependency commit the candidate **incorporates**, not the
/// commit the dependency happens to stand approved at. Those are different
/// questions and only the first one is about what the reviewer saw. A card that
/// branched from the cycle baseline and never merged its dependency has
/// `incorporated_sha: None`: the dependency can be re-reviewed all day without
/// changing a line of what was reviewed here, and refusing on that would
/// serialize exactly the parallel work this harness exists to coordinate.
///
/// Section 10.2 says a dependent uses the exact accepted dependency SHA
/// *declared in the card*. Asking Git which dependency commit is actually in
/// the candidate answers the same question without trusting the declaration,
/// and Section 10.7 requires this field to be machine-computed. A card whose
/// `base_sha` names the dependency's accepted commit binds that commit, because
/// it is an ancestor.
///
/// The value is resolved by asking Git whether any commit the dependency has
/// ever handed off is an ancestor of this candidate, newest first. **A
/// dependency commit that was never handed off cannot be bound**, because
/// nothing recorded it: a dependent branched from a mid-branch commit of its
/// dependency binds `None` and this check says nothing about it. Stated here
/// rather than implied.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DependencyBinding {
    /// The declared dependency.
    pub card_id: CardId,
    /// The dependency commit this candidate's history contains, if any.
    pub incorporated_sha: Option<String>,
}

/// Where a dependency stands relative to one record's binding.
///
/// The input side of the dependency check. Resolved live at check time rather
/// than recorded, because the whole question is whether the world moved since
/// the binding was written.
///
/// `approval_contains_binding` is an ancestry fact about two commits, so it
/// cannot be answered here — the caller asks Git. It is carried on this struct
/// rather than left to the comparison below because containment, not equality,
/// is what distinguishes a dependency that moved *forward* from one that was
/// rewritten. See [`dependency_staleness`].
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct DependencyStanding {
    /// The dependency card.
    pub card_id: CardId,
    /// The candidate its standing approval names, if it has one.
    pub approved_candidate_sha: Option<String>,
    /// True when that approved candidate's history contains the bound commit.
    ///
    /// Meaningless, and set true, when either side is absent.
    pub approval_contains_binding: bool,
}

/// Passed where dependency bindings are deliberately not part of the question.
///
/// There is exactly one such site — `review record`'s handoff check — and it is
/// deliberate: a reviewer must always be able to file a verdict about the
/// candidate in front of them. Refusing there would leave a card with no exit,
/// which is defect 20 in the register.
pub const DEPENDENCIES_NOT_CHECKED: &[DependencyStanding] = &[];

/// Explains which dependency binding broke, if one did.
///
/// `dependencies` must be the card's *declared* dependencies resolved against
/// this record's own bindings; an empty slice asks no dependency question at
/// all. A declared dependency with no recorded binding is treated as stale,
/// because a record written before this field existed cannot be distinguished
/// from one whose candidate genuinely incorporates nothing — and the remedy,
/// re-creating the handoff, adds a record rather than rewriting one.
///
/// Two things are deliberately **not** stale, and both are the difference
/// between a check and a blockade:
///
/// - A dependency approved at a commit that *contains* the bound one. The
///   dependency gained review-requested fixes on top; what this candidate holds
///   is a prefix of what will land, so nothing is superseded and nothing lands
///   twice. Only a rewrite — a rebase, an amend, a different line of history —
///   leaves the bound commit outside the approval, and that is exactly the case
///   where the merge would carry both versions.
/// - A dependency standing approved at nothing, because its own review is
///   pending or asked for changes. The dependent may still be reviewed on its
///   merits; it cannot be *integrated*, because `check_dependencies` already
///   refuses a selection whose dependency is neither included nor landed, and a
///   dependency with no standing approval can be neither. Refusing here as well
///   would void a dependent's evidence for the whole time its dependency is
///   under review, which is the parallelism this harness exists to coordinate.
pub(crate) fn dependency_staleness(
    subject: &str,
    bindings: &[DependencyBinding],
    dependencies: &[DependencyStanding],
) -> Option<String> {
    for dependency in dependencies {
        let Some(binding) = bindings
            .iter()
            .find(|binding| binding.card_id == dependency.card_id)
        else {
            return Some(format!(
                "this {subject} records no dependency binding for {}, which the card declares; re-create the handoff so the dependency SHA is bound",
                dependency.card_id
            ));
        };
        let Some(incorporated) = &binding.incorporated_sha else {
            continue;
        };
        let Some(approved) = &dependency.approved_candidate_sha else {
            continue;
        };
        if !dependency.approval_contains_binding {
            // Deliberately does not say "rewritten". Containment fails for a
            // rewrite, and also when this evidence binds a *newer* dependency
            // commit than the one standing approved — no rewrite anywhere. A
            // reviewer reproduced the second case, and a diagnostic that names
            // the wrong cause sends an operator looking in the wrong place.
            return Some(format!(
                "this {subject} incorporates {} at {incorporated}, but {} now stands approved at {approved}, which does not contain it",
                dependency.card_id, dependency.card_id
            ));
        }
    }
    None
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
    /// Which commit of each declared dependency this candidate incorporates.
    ///
    /// `#[serde(default)]` follows the `human_reviewer` precedent: a record
    /// written before this field existed still deserializes, so no evidence on
    /// disk has to be rewritten. It is not a silent pass — `staleness` refuses a
    /// record that declares a dependency and binds nothing.
    #[serde(default)]
    pub dependency_bindings: Vec<DependencyBinding>,
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
    pub fn is_current_for(
        &self,
        candidate_sha: &str,
        card_digest: &Digest,
        dependencies: &[DependencyStanding],
    ) -> bool {
        self.staleness(candidate_sha, card_digest, dependencies)
            .is_none()
    }

    /// Explains why this handoff no longer applies, if it does not.
    ///
    /// Section 15.2 lists seven invalidation triggers. Three are checked here:
    /// the candidate SHA, the card digest, and the required dependency SHA. The
    /// remaining four — cycle invariant, gate definition, reviewer-required
    /// receipt, declared contract change — are still not checked anywhere, and
    /// saying so here is cheaper than the docstring that used to narrow the
    /// specification to what happened to be implemented.
    ///
    /// `dependencies` is the card's declared dependencies with their current
    /// approvals. [`DEPENDENCIES_NOT_CHECKED`] asks no dependency question.
    #[must_use]
    pub fn staleness(
        &self,
        candidate_sha: &str,
        card_digest: &Digest,
        dependencies: &[DependencyStanding],
    ) -> Option<String> {
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
        dependency_staleness("handoff", &self.dependency_bindings, dependencies)
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
            dependency_bindings: vec![],
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
        assert!(record.is_current_for(&"a".repeat(40), &card, DEPENDENCIES_NOT_CHECKED));
        assert!(!record.is_current_for(&"b".repeat(40), &card, DEPENDENCIES_NOT_CHECKED));
        assert!(!record.is_current_for(
            &"a".repeat(40),
            &Digest::of_bytes(b"revised card"),
            DEPENDENCIES_NOT_CHECKED
        ));
    }

    #[test]
    fn staleness_explains_which_binding_broke() {
        let record = handoff();
        let card = Digest::of_bytes(b"card");
        assert!(
            record
                .staleness(&"a".repeat(40), &card, DEPENDENCIES_NOT_CHECKED)
                .is_none()
        );

        let moved = record
            .staleness(&"c".repeat(40), &card, DEPENDENCIES_NOT_CHECKED)
            .unwrap();
        assert!(moved.contains("branch is now"), "{moved}");

        let revised = record
            .staleness(
                &"a".repeat(40),
                &Digest::of_bytes(b"revised"),
                DEPENDENCIES_NOT_CHECKED,
            )
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
        assert!(!revoked.is_current_for(&"a".repeat(40), &card, DEPENDENCIES_NOT_CHECKED));
        assert_eq!(
            revoked
                .staleness(&"a".repeat(40), &card, DEPENDENCIES_NOT_CHECKED)
                .as_deref(),
            Some("the handoff was revoked")
        );
    }

    fn binding(incorporated: Option<&str>) -> Vec<DependencyBinding> {
        vec![DependencyBinding {
            card_id: "F-000".parse().unwrap(),
            incorporated_sha: incorporated.map(ToOwned::to_owned),
        }]
    }

    fn standing(approved: Option<&str>, contains: bool) -> Vec<DependencyStanding> {
        vec![DependencyStanding {
            card_id: "F-000".parse().unwrap(),
            approved_candidate_sha: approved.map(ToOwned::to_owned),
            approval_contains_binding: contains,
        }]
    }

    #[test]
    fn the_handoff_record_reports_a_stale_dependency_through_staleness() {
        // Finding 3 from the F-016 review. `dependency_staleness` was tested
        // directly, but `HandoffRecord::staleness`'s call to it was not: a
        // reviewer replaced that whole arm with `None` and the entire suite —
        // 840 tests across 35 binaries, including their own nine probes —
        // stayed green. A guarded helper reached through an unguarded call site
        // is the same vacuity as an untested mechanism, one level up.
        let record = HandoffRecord {
            dependency_bindings: binding(Some(&"d".repeat(40))),
            ..handoff()
        };
        let reason = record
            .staleness(
                &record.candidate_sha.clone(),
                &record.card_digest.clone(),
                &standing(Some(&"e".repeat(40)), false),
            )
            .expect("the record must surface what dependency_staleness found");
        assert!(reason.contains("F-000"), "{reason}");

        // And the same record with a containing approval is current, so the arm
        // is not simply returning Some unconditionally.
        assert!(
            record
                .staleness(
                    &record.candidate_sha.clone(),
                    &record.card_digest.clone(),
                    &standing(Some(&"e".repeat(40)), true),
                )
                .is_none()
        );
    }

    #[test]
    fn a_dependency_whose_commit_git_cannot_resolve_does_not_invalidate() {
        // Finding 6. Neither direction of the unanswerable case was reached by
        // any test: flipping the caller's default failed exactly one test in
        // the suite, and it was the reviewer's own. The conservative direction
        // is deliberate — an ancestry question Git cannot answer must not
        // invalidate standing evidence — so it needs a test saying so.
        assert!(
            dependency_staleness(
                "handoff",
                &binding(Some(&"d".repeat(40))),
                &standing(Some(&"e".repeat(40)), true),
            )
            .is_none(),
            "an unanswerable ancestry resolves to `contains`, which does not invalidate"
        );
    }

    #[test]
    fn a_dependency_rewritten_past_the_bound_commit_is_stale() {
        let reason = dependency_staleness(
            "handoff",
            &binding(Some(&"d".repeat(40))),
            &standing(Some(&"e".repeat(40)), false),
        )
        .expect("a rewritten dependency invalidates what was built on it");
        assert!(reason.contains("F-000"), "{reason}");
        assert!(reason.contains(&"d".repeat(40)), "{reason}");
        assert!(reason.contains(&"e".repeat(40)), "{reason}");
    }

    #[test]
    fn a_dependency_that_still_contains_the_bound_commit_is_current() {
        // The overcorrection guard, at the unit where the comparison lives: a
        // dependency that gained commits on top has moved, and must not
        // invalidate anything.
        assert!(
            dependency_staleness(
                "handoff",
                &binding(Some(&"d".repeat(40))),
                &standing(Some(&"e".repeat(40)), true),
            )
            .is_none()
        );
    }

    #[test]
    fn incorporating_nothing_asks_nothing_of_the_dependency() {
        for approved in [None, Some(&"e".repeat(40) as &str)] {
            assert!(
                dependency_staleness("handoff", &binding(None), &standing(approved, false))
                    .is_none(),
                "a candidate that holds no commit of its dependency cannot be superseded by one"
            );
        }
    }

    #[test]
    fn a_dependency_with_no_standing_approval_does_not_invalidate() {
        // Deliberate, and the trade is named at `dependency_staleness`:
        // integration refuses such a selection anyway, and refusing here would
        // void a dependent's evidence for as long as its dependency is under
        // review.
        assert!(
            dependency_staleness(
                "handoff",
                &binding(Some(&"d".repeat(40))),
                &standing(None, false),
            )
            .is_none()
        );
    }

    #[test]
    fn a_declared_dependency_with_no_binding_is_stale() {
        // A record written before this field existed is indistinguishable from
        // one whose candidate genuinely incorporates nothing, so it is refused
        // rather than read as a pass.
        let reason = dependency_staleness("review", &[], &standing(Some(&"e".repeat(40)), true))
            .expect("an unbound declared dependency cannot be judged");
        assert!(reason.contains("F-000"), "{reason}");
        assert!(reason.contains("re-create the handoff"), "{reason}");
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
