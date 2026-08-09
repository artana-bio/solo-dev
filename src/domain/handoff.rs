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
    policy::convergence::ReasonCategory,
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

/// One gate the actor declares they had to get past before this delivery
/// succeeded.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DeclaredGateFailure {
    /// The gate that failed.
    pub gate_id: String,
    /// Why it failed.
    pub reason_category: ReasonCategory,
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
    /// Gate failures this delivery had to get past, declared by the actor.
    ///
    /// 71-R3 records this at the handoff boundary rather than counting every
    /// red `gate run` along the way. `gate run` is mechanical: nothing about
    /// it declares *why* a run failed, and counting every one would count
    /// ordinary iteration — an implementer runs a gate red repeatedly while
    /// writing the code, and that is not a convergence failure. What is: the
    /// card was declared ready, at `handoff create`, and getting there took
    /// an admitted gate failure. That is why this lives on the declaration a
    /// *successful* handoff makes, not on `gate run`'s own result.
    ///
    /// `#[serde(default, skip_serializing_if = "Vec::is_empty")]` keeps every
    /// declaration written before this field existed readable, and — the
    /// reason it is not merely a preference — keeps a handoff that declares
    /// no gate failures serializing byte-identically to one written before
    /// this field existed, so no already-computed handoff digest moves.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gate_failures: Vec<DeclaredGateFailure>,
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
                "handoff declaration's `{field}` is empty; a reviewer cannot distinguish an empty field from an unconsidered one"
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
/// ever handed off is an ancestor of this candidate, newest first. This is not
/// a binding to the dependency branch: when a candidate incorporates an
/// unhanded commit on top of an earlier handed-off commit, it binds that earlier
/// commit instead. If the dependency later stands approved at a rewrite that
/// still contains the earlier commit, containment passes and records no sign of
/// the unhanded work. `None` occurs only when no handed-off ancestor exists.
/// Closing that gap means binding against the dependency branch, which this
/// record does not do.
///
/// Section 10.2's precondition that a dependent's `base_sha` is the exact
/// accepted dependency SHA is also unenforced here: card validation checks only
/// that `base_sha` is 40 hexadecimal characters. This resolution cannot infer
/// a declaration the card record never proved.
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
///   twice. A rewrite can leave the bound commit outside the approval, but so
///   can evidence that binds a newer dependency commit than the one standing
///   approved. The latter carries no rewrite and does not by itself make a merge
///   carry both versions.
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
    /// Declared implementer principal/session copied from the work lease.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_principal_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_session_id: Option<String>,
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
        self.revocation_staleness()
            .or_else(|| self.candidate_staleness(candidate_sha))
            .or_else(|| self.card_binding_staleness(card_digest))
            .or_else(|| self.dependency_binding_staleness(dependencies))
    }

    /// Whether what this handoff was *bound to* has changed underneath it.
    ///
    /// [`staleness`](Self::staleness) asks four questions. This asks the two
    /// about bindings, and deliberately not the two about standing.
    ///
    /// The candidate question is dropped because a verdict that found problems
    /// is a true statement about the commit the reviewer read, and it stays
    /// true when the branch moves. The revocation question is dropped for a
    /// harder reason: `handoff revoke` belongs to the delivery side, and
    /// `review_pending → active` is a permitted transition, so an
    /// implementer who can make revocation fatal to a verdict can suppress an
    /// adverse review by revoking before the reviewer files. A record naming a
    /// revoked handoff is at least queryable, and
    /// [`is_current_for`](Self::is_current_for) already reports it stale;
    /// findings refused at the door are simply gone.
    ///
    /// What remains is what the reviewer was measuring *against*. A card
    /// revised out from under a handoff, or a dependency that moved, changes
    /// the criteria rather than the delivery, and no verdict outlives that.
    ///
    /// Both entry points are composed from the same components in the same
    /// order, so whichever reason applies is the reason either one reports.
    #[must_use]
    pub fn binding_staleness(
        &self,
        card_digest: &Digest,
        dependencies: &[DependencyStanding],
    ) -> Option<String> {
        self.card_binding_staleness(card_digest)
            .or_else(|| self.dependency_binding_staleness(dependencies))
    }

    /// The handoff was withdrawn by whoever made it.
    fn revocation_staleness(&self) -> Option<String> {
        (self.status == HandoffStatus::Revoked).then(|| "the handoff was revoked".to_owned())
    }

    /// The branch no longer holds the commit that was handed off.
    fn candidate_staleness(&self, candidate_sha: &str) -> Option<String> {
        (self.candidate_sha != candidate_sha).then(|| {
            format!(
                "handoff describes candidate {} but the branch is now {candidate_sha}",
                self.candidate_sha
            )
        })
    }

    /// The card was revised after the handoff bound itself to a revision.
    fn card_binding_staleness(&self, card_digest: &Digest) -> Option<String> {
        (self.card_digest != *card_digest).then(|| {
            format!(
                "handoff was bound to card digest {} but the card is now {card_digest}",
                self.card_digest
            )
        })
    }

    /// A declared dependency moved or lost its approval.
    fn dependency_binding_staleness(&self, dependencies: &[DependencyStanding]) -> Option<String> {
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
            gate_failures: vec![],
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
            actor_principal_id: None,
            actor_session_id: None,
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
        const GUARDED_FIELDS: [&str; 3] = [
            "behavior_delivered",
            "rollback_notes",
            "implementation_decisions",
        ];

        for (field, mutate) in [
            (
                "behavior_delivered",
                (|d: &mut ActorDeclaration| d.behavior_delivered = "  ".to_owned())
                    as fn(&mut ActorDeclaration),
            ),
            (
                "rollback_notes",
                (|d: &mut ActorDeclaration| d.rollback_notes = String::new())
                    as fn(&mut ActorDeclaration),
            ),
            (
                "implementation_decisions",
                (|d: &mut ActorDeclaration| d.implementation_decisions.clear())
                    as fn(&mut ActorDeclaration),
            ),
        ] {
            let mut invalid = declaration();
            mutate(&mut invalid);
            let error = invalid.validate().expect_err("must refuse");
            assert_eq!(error.code(), ErrorCode::PolicyIncompleteHandoff);

            // The `field` interpolation is the entire reason `reject` takes a
            // parameter. A refusal that names the wrong field -- or names
            // every field, technically covering itself while telling the
            // operator nothing -- sends them editing content that was never
            // the problem. Mutation: hardcoding one field name into the
            // closure, or swapping two field arguments at the call sites,
            // must fail this.
            let message = error.to_string();
            let marker = format!("`{field}`");
            assert!(
                message.contains(&marker),
                "the refusal for `{field}` must name itself: {message}"
            );
            for other in GUARDED_FIELDS.iter().filter(|&&other| other != field) {
                let other_marker = format!("`{other}`");
                assert!(
                    !message.contains(&other_marker),
                    "the refusal for `{field}` must not also name `{other}`: {message}"
                );
            }
        }
    }

    #[test]
    fn a_present_but_empty_field_is_not_reported_as_missing() {
        // Contract 117. An operator who wrote `implementation_decisions: []`
        // was told the field was missing. It was not: it was present and
        // empty, and they went looking for a syntax error that did not
        // exist. Mutation: restoring the old wording (`missing
        // `{field}``) must make this fail.
        let mut invalid = declaration();
        invalid.implementation_decisions = vec![];
        let error = invalid
            .validate()
            .expect_err("an empty list must still be refused");
        assert!(
            !error.to_string().contains("missing"),
            "a present-but-empty field must not be described as missing: {error}"
        );
    }

    #[test]
    fn the_rationale_clause_survives() {
        // The clause that teaches *why* an empty field is refused --
        // "a reviewer cannot distinguish an empty field from an
        // unconsidered one" -- must survive independently of whatever the
        // first clause says. Mutation: dropping this clause while keeping a
        // corrected first clause must fail this test without failing
        // `a_present_but_empty_field_is_not_reported_as_missing` above --
        // otherwise the two assertions are just reading the same substring.
        let mut invalid = declaration();
        invalid.implementation_decisions = vec![];
        let error = invalid
            .validate()
            .expect_err("an empty list must still be refused");
        assert!(
            error
                .to_string()
                .contains("a reviewer cannot distinguish an empty field from an unconsidered one"),
            "the rationale clause must survive: {error}"
        );
    }

    #[test]
    fn an_absent_key_fails_deserialization_and_never_reaches_validate() {
        // Contract 117 §6's finding, pinned: `implementation_decisions` (like
        // `behavior_delivered` and `rollback_notes`) carries no
        // `#[serde(default)]`, so `serde` treats the key as required. A
        // document that omits it fails to deserialize into `ActorDeclaration`
        // at all -- it never becomes a value `validate` can inspect, and the
        // error produced is `serde`'s own "missing field", not this type's.
        // Only a document that supplies the key, however emptily, ever
        // produces an `ActorDeclaration` for `validate` to reject. That is
        // why the fixed message says "empty" outright rather than hedging
        // with "missing or empty": at the point this message is produced,
        // "missing" cannot be what happened. This mirrors the real pipeline
        // in `commands::handoff::read_declaration`, which parses with
        // `serde_yaml_ng` before `validate` ever runs.
        let omits_the_key = format!(
            "delivered_sha: {}\nbehavior_delivered: converts temperatures\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert the commit\n",
            "a".repeat(40)
        );
        let deserialize_error = serde_yaml_ng::from_str::<ActorDeclaration>(&omits_the_key)
            .expect_err("omitting the key must fail to deserialize rather than arrive at validate() as empty");
        assert!(
            deserialize_error.to_string().contains("missing field"),
            "the absent-key error is serde's own, not validate()'s: {deserialize_error}"
        );

        let states_it_empty = format!(
            "delivered_sha: {}\nbehavior_delivered: converts temperatures\nimplementation_decisions: []\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert the commit\n",
            "a".repeat(40)
        );
        let parsed: ActorDeclaration = serde_yaml_ng::from_str(&states_it_empty)
            .expect("an explicit empty list is a well-formed document");
        assert!(parsed.implementation_decisions.is_empty());
        assert_eq!(
            parsed.validate().expect_err("must refuse").code(),
            ErrorCode::PolicyIncompleteHandoff
        );
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
    fn binding_staleness_asks_about_the_bindings_and_nothing_else() {
        let card = Digest::of_bytes(b"card");
        let revised = Digest::of_bytes(b"revised");

        assert!(
            handoff()
                .binding_staleness(&card, DEPENDENCIES_NOT_CHECKED)
                .is_none()
        );

        // Neither of the two questions about standing. Both are dropped on
        // purpose and the argument is on the method; the point here is that
        // dropping them is a property of the code, not an accident of a
        // fixture that happens never to be revoked.
        let revoked = HandoffRecord {
            status: HandoffStatus::Revoked,
            ..handoff()
        };
        assert!(
            revoked
                .binding_staleness(&card, DEPENDENCIES_NOT_CHECKED)
                .is_none(),
            "revocation is a delivery-side control and must not silence a reviewer"
        );

        // Both of the questions about what the reviewer measured against.
        let rebound = handoff()
            .binding_staleness(&revised, DEPENDENCIES_NOT_CHECKED)
            .expect("a card revised underneath the handoff is still staleness");
        assert!(rebound.contains("card is now"), "{rebound}");

        let dependency = handoff()
            .binding_staleness(&card, &standing(Some(&"e".repeat(40)), true))
            .expect("the dependency question is still asked when it is asked at all");
        assert!(dependency.contains("dependency"), "{dependency}");
    }

    #[test]
    fn both_entry_points_report_the_same_reason_in_the_same_order() {
        // The two are composed from one set of components precisely so that
        // the narrower one cannot silently reorder what it keeps. With a moved
        // branch *and* a revised card, the whole check reports the candidate
        // first, and the narrower one reports the card rather than falling
        // silent.
        let record = handoff();
        let revised = Digest::of_bytes(b"revised");

        let whole = record
            .staleness(&"c".repeat(40), &revised, DEPENDENCIES_NOT_CHECKED)
            .unwrap();
        assert!(whole.contains("branch is now"), "{whole}");

        let narrower = record
            .binding_staleness(&revised, DEPENDENCIES_NOT_CHECKED)
            .unwrap();
        assert!(narrower.contains("card is now"), "{narrower}");

        // Revocation still precedes everything in the whole check, and is
        // still absent from the narrower one, with both bindings also stale.
        let revoked = HandoffRecord {
            status: HandoffStatus::Revoked,
            ..handoff()
        };
        assert_eq!(
            revoked
                .staleness(&"c".repeat(40), &revised, DEPENDENCIES_NOT_CHECKED)
                .as_deref(),
            Some("the handoff was revoked")
        );
        let narrower = revoked
            .binding_staleness(&revised, DEPENDENCIES_NOT_CHECKED)
            .expect("the card binding still answers");
        assert!(narrower.contains("card is now"), "{narrower}");
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
    fn a_handoff_with_no_declared_gate_failures_serializes_byte_identically_to_before_71r3() {
        // 71-R3 added `ActorDeclaration::gate_failures`. This exact digest was
        // captured from this exact fixture on `1f1aaf2`, the commit
        // immediately before the field existed at all, by printing
        // `handoff().digest()` and reading it back. Pinning that value here,
        // rather than trusting that `skip_serializing_if` alone must be
        // enough, is what makes this a verification instead of an assumption.
        assert_eq!(
            handoff().digest().unwrap().as_str(),
            "sha256:bf6e81fd6bdde83878d58466b05fbb2fc65db38bad330c097a3f6c6b38255628",
            "a handoff declaring no gate failures must not move any already-computed handoff digest"
        );
        assert!(
            !serde_json::to_string(&handoff())
                .unwrap()
                .contains("gate_failures"),
            "an empty declared list must be omitted entirely, not written out as `[]`"
        );
    }

    #[test]
    fn a_declared_gate_failure_moves_the_handoff_digest() {
        // The other half of the claim above: the field is not merely absent,
        // it is live. A reviewer's digest has to cover a declared gate
        // failure, or binding evidence to it below would mean nothing.
        let base = handoff().digest().unwrap();
        let mut declared = handoff();
        declared
            .declaration
            .gate_failures
            .push(DeclaredGateFailure {
                gate_id: "gate.unit".to_owned(),
                reason_category: ReasonCategory::Regression,
            });
        assert_ne!(base, declared.digest().unwrap());
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
