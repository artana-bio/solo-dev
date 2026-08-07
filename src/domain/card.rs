//! The card: one independently reviewable outcome, immutable once activated.
//!
//! Section 10.3. A card's digest is what every downstream record binds to, so
//! the card must be immutable after activation and its digest must change when
//! and only when something material changes. A card that could be edited in
//! place would silently invalidate every review that cited it.

use std::{collections::BTreeSet, fmt};

use serde::{Deserialize, Serialize};

use crate::{
    domain::artifact::{ArtifactClass, GeneratedArtifact, require_one_class_each},
    domain::{
        clock::Timestamp,
        digest::{CANONICAL_ALGORITHM, Digest},
        ids::{CardId, CycleId},
    },
    error::{ErrorCode, HarnessError},
};

/// Schema identifier for an activated card.
pub const CARD_SCHEMA: &str = "harness.card/v1";

/// Directory holding card records, relative to the control repository.
pub const CARD_DIR: &str = "cards";

/// How much risk a card carries, which sets its minimum review.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// One independent reviewer.
    Low,
    /// One independent reviewer plus the acceptance owner.
    Medium,
    /// Human reviewer plus independent technical review.
    High,
    /// Explicit human acceptance and a second human approval.
    Critical,
}

/// The proof an author declares for one independently testable invariant.
///
/// This is an immutable claim about what a card must demonstrate. It is not a
/// test receipt: later workflow stages record whether this declaration was
/// actually exercised against an exact candidate SHA.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProofMapEntry {
    /// The behavior that must continue to hold.
    pub invariant: String,
    /// The setup required to exercise that behavior.
    pub precondition: String,
    /// The observable assertion that decides the result.
    pub assertion: String,
    /// A discriminating change that must make the assertion fail.
    pub mutation: String,
}

/// A bounded, reviewable declaration of the proof a card needs.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProofMap {
    /// Proof-map schema, kept explicit so later versions do not reinterpret
    /// already activated cards.
    pub schema: String,
    /// The independently testable claims this card makes.
    pub entries: Vec<ProofMapEntry>,
    /// What this proof deliberately does not establish.
    pub claim_boundary: String,
}

/// Schema identifier for the first proof-map representation.
pub const PROOF_MAP_SCHEMA: &str = "harness.proof-map/v1";

impl ProofMap {
    /// Validates a proof declaration before it is frozen into a card record.
    ///
    /// # Errors
    ///
    /// Returns a card-policy error when the schema, claim boundary, entries,
    /// or any required entry field is absent or blank.
    pub fn validate(&self) -> Result<(), HarnessError> {
        let reject = |reason: String| HarnessError::Control {
            reason,
            code: ErrorCode::PolicyInvalidCard,
        };
        if self.schema != PROOF_MAP_SCHEMA {
            return Err(reject(format!(
                "proof_map.schema must be `{PROOF_MAP_SCHEMA}`, found `{}`",
                self.schema
            )));
        }
        if self.entries.is_empty() {
            return Err(reject(
                "a proof map must declare at least one invariant".to_owned(),
            ));
        }
        if self.claim_boundary.trim().is_empty() {
            return Err(reject(
                "a proof map must declare its claim boundary".to_owned(),
            ));
        }
        for (index, entry) in self.entries.iter().enumerate() {
            for (field, value) in [
                ("invariant", &entry.invariant),
                ("precondition", &entry.precondition),
                ("assertion", &entry.assertion),
                ("mutation", &entry.mutation),
            ] {
                if value.trim().is_empty() {
                    return Err(reject(format!(
                        "proof_map.entries[{index}].{field} must not be empty"
                    )));
                }
            }
        }
        Ok(())
    }
}

impl Risk {
    /// Whether Section 15.3 requires a human in the loop.
    #[must_use]
    pub const fn requires_human_review(self) -> bool {
        matches!(self, Self::High | Self::Critical)
    }

    /// Its stable serialized name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }
}

/// Where a card sits in its lifecycle, per Section 11.2.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CardState {
    /// Authored but not yet activated; still mutable.
    Draft,
    /// Activated and immutable; awaiting a lease.
    Ready,
    /// Leased to one actor.
    Leased,
    /// Being implemented.
    Active,
    /// A handoff exists for an exact candidate.
    HandedOff,
    /// A reviewer has been assigned.
    ReviewPending,
    /// Changes were requested and the card returned to work.
    ChangesRequested,
    /// An independent review approved the exact candidate.
    Approved,
    /// Selected into an integration.
    Integrating,
    /// The landing commit was accepted.
    Accepted,
    /// The accepted commit reached the authority branch.
    Landed,
    /// Terminal success.
    Closed,
    /// Halted pending a decision.
    Blocked,
    /// Terminal abandonment.
    Abandoned,
}

impl CardState {
    /// The state an activated card enters.
    pub const INITIAL: Self = Self::Draft;

    /// Its stable serialized name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Ready => "ready",
            Self::Leased => "leased",
            Self::Active => "active",
            Self::HandedOff => "handed_off",
            Self::ReviewPending => "review_pending",
            Self::ChangesRequested => "changes_requested",
            Self::Approved => "approved",
            Self::Integrating => "integrating",
            Self::Accepted => "accepted",
            Self::Landed => "landed",
            Self::Closed => "closed",
            Self::Blocked => "blocked",
            Self::Abandoned => "abandoned",
        }
    }

    /// True when no further transition is permitted.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Closed | Self::Abandoned)
    }

    /// True when a new revision may supersede the current one.
    ///
    /// Revising returns a card to `ready`, and `card revise` checked only
    /// [`is_terminal`](Self::is_terminal) before doing so. `landed` is not
    /// terminal — it still has to reach `closed` — so a landed card could be
    /// sent back to `ready`, whose only exits are `leased` and `abandoned`. It
    /// could then never be closed, and its integration referenced a card whose
    /// state had gone backwards past the acceptance that authorized it.
    ///
    /// Revising an approved card is deliberately still allowed: Section 15.2
    /// makes a revision invalidate the approval, which is the mechanism working
    /// rather than a hole. The line is drawn where an integration has pinned
    /// the card's digest into a plan.
    #[must_use]
    pub const fn is_revisable(self) -> bool {
        !matches!(
            self,
            Self::Integrating | Self::Accepted | Self::Landed | Self::Closed | Self::Abandoned
        )
    }

    /// True when the card's definition may still be edited.
    ///
    /// Only a draft. Section 7.3.3 makes activation the point of no return; a
    /// change afterwards is a new revision, never an edit.
    #[must_use]
    pub const fn is_mutable(self) -> bool {
        matches!(self, Self::Draft)
    }

    /// Every state this one may transition to, from Section 11.2.
    ///
    /// Arms are kept one-per-state even where two share a successor list. This
    /// is a transcription of a specification table, and collapsing arms would
    /// make a later divergence between two states an edit to shared code rather
    /// than an edit to one line.
    #[must_use]
    #[allow(clippy::match_same_arms)]
    pub fn successors(self) -> &'static [Self] {
        match self {
            Self::Draft => &[Self::Ready, Self::Abandoned],
            Self::Ready => &[Self::Leased, Self::Abandoned],
            Self::Leased => &[Self::Active, Self::Abandoned],
            // `ChangesRequested` and `Blocked` for one reason, stated at
            // `ReviewPending` below and reaching one stage further than it was
            // written for: a reviewer who has begun may still file, and a
            // verdict the schema offers must not be discarded because a
            // transition refused it.
            //
            // A card arrives here from `review_pending` when the delivery side
            // revokes the handoff. Without `ChangesRequested` that revocation
            // decides the verdict: `blocked` filed and `changes_requested` did
            // not, which is an asymmetry nothing justifies, and it handed the
            // implementer a way to discard an adverse review by revoking
            // first. Found by review round 2 of `F-030` (RV-000055), after
            // round 1 caught the same suppression route through the staleness
            // check and the fix closed only half of it.
            //
            // This opens no route around review. `review record` refuses a
            // card with no handoff, so the only way to reach `changes_requested`
            // is still a verdict somebody recorded.
            Self::Active => &[
                Self::HandedOff,
                Self::ChangesRequested,
                Self::Blocked,
                Self::Abandoned,
            ],
            // A handoff can be revoked, returning the card to work.
            Self::HandedOff => &[Self::ReviewPending, Self::Active, Self::Abandoned],
            // `Blocked` because a reviewer may conclude the card needs a
            // decision outside their remit, which is a verdict the schema
            // offers and the table did not admit — so recording it aborted the
            // transaction and filed nothing at all.
            //
            // `Active` because a review can otherwise reach a state with no
            // exit: if the branch moves after the handoff, no verdict is
            // recordable, and without this the only escape was abandoning the
            // card. It is the same revocation `handed_off → active` already
            // allows, one stage later.
            Self::ReviewPending => &[
                Self::Approved,
                Self::ChangesRequested,
                Self::Blocked,
                Self::Active,
                Self::Abandoned,
            ],
            Self::ChangesRequested => &[Self::Active, Self::Abandoned],
            // An approval is invalidated by a candidate change, which returns
            // the card to active work.
            Self::Approved => &[Self::Integrating, Self::Active, Self::Abandoned],
            Self::Integrating => &[Self::Accepted, Self::Blocked, Self::Abandoned],
            Self::Accepted => &[Self::Landed, Self::Abandoned],
            // Section 11.2: landed cannot become abandoned.
            Self::Landed => &[Self::Closed],
            Self::Blocked => &[Self::Active, Self::Integrating, Self::Abandoned],
            Self::Closed | Self::Abandoned => &[],
        }
    }

    /// Checks one transition.
    ///
    /// # Errors
    ///
    /// Returns a policy error when the transition is not in Section 11.2.
    pub fn check_transition(self, next: Self) -> Result<(), HarnessError> {
        if self.successors().contains(&next) {
            return Ok(());
        }
        Err(HarnessError::Control {
            reason: format!(
                "card cannot move from `{}` to `{}`; permitted: {}",
                self.name(),
                next.name(),
                if self.successors().is_empty() {
                    "none, this state is terminal".to_owned()
                } else {
                    self.successors()
                        .iter()
                        .map(|state| state.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ),
            code: ErrorCode::PolicyInvalidTransition,
        })
    }
}

impl fmt::Display for CardState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// Paths a card may and may not write.
#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WriteScope {
    /// Patterns the card owns. Deny-by-default: nothing outside is writable.
    pub include: Vec<String>,
    /// Patterns carved back out of `include`.
    pub exclude: Vec<String>,
}

/// Named gates a card must pass at each stage.
#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NamedGates {
    /// Gates the feature actor runs before handoff.
    pub feature: Vec<String>,
    /// Gates the reviewer requires.
    pub review: Vec<String>,
    /// Gates the integrator reruns from a clean worktree.
    pub integration: Vec<String>,
}

/// What the card must deliver.
#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Acceptance {
    /// Behaviors the delivered code must exhibit.
    pub behaviors: Vec<String>,
    /// Existing behaviors that must not change.
    pub regressions: Vec<String>,
}

/// The fields an author supplies.
///
/// A draft is mutable and may be authored in YAML. Only the canonical activated
/// JSON and its digest are authoritative, per D-010 and Section 10.1.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CardDraft {
    /// Identifies the card.
    pub card_id: CardId,
    /// The cycle it belongs to.
    pub cycle_id: CycleId,
    /// One-line summary.
    pub title: String,
    /// What the card is for.
    pub goal: String,
    /// What it deliberately does not do.
    #[serde(default)]
    pub non_goals: Vec<String>,
    /// How much risk it carries.
    pub risk: Risk,
    /// What kind of change it is.
    pub change_kind: String,
    /// The exact commit work starts from.
    pub base_sha: String,
    /// Paths it may write.
    pub write_scope: WriteScope,
    /// Contract domains it reads.
    #[serde(default)]
    pub contract_reads: Vec<String>,
    /// Contract domains it changes.
    #[serde(default)]
    pub contract_changes: Vec<String>,
    /// Cards that must land first.
    #[serde(default)]
    pub depends_on: Vec<CardId>,
    /// Resources it needs exclusively.
    #[serde(default)]
    pub exclusive_resources: Vec<String>,
    /// Gates it must pass.
    pub named_gates: NamedGates,
    /// What it must deliver.
    pub acceptance: Acceptance,
    /// Generated paths it produces.
    #[serde(default)]
    pub generated_artifacts: Vec<GeneratedArtifact>,
    /// How review is conducted.
    pub review_policy: String,
    /// How to undo it.
    pub rollback_strategy: String,
    /// The declared proof for risks whose project policy requires it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof_map: Option<ProofMap>,
}

/// True when every path described by a generated-artifact source belongs to
/// the card's write scope.
///
/// Concrete sources use ordinary scope membership. Glob sources need a
/// containment proof instead: asking whether the include matches the glob text
/// treats metacharacters as filename characters and can admit a source broader
/// than the scope. This deliberately proves only the safe common cases: an
/// identical pattern, `**`, or a source nested below a literal `/**` prefix.
/// More complex relationships fail closed rather than turning intersection
/// into containment. Any intersection with an exclude also fails closed.
fn generated_source_is_owned(
    source: &str,
    scope: &crate::policy::paths::Scope<'_>,
    include: &[String],
    exclude: &[String],
    case: crate::policy::paths::CaseSensitivity,
) -> bool {
    use crate::policy::paths::{patterns_intersect, provably_contains_pattern};

    if !source.contains('*') {
        return scope.allows(source);
    }
    if exclude
        .iter()
        .any(|pattern| patterns_intersect(pattern, source, case))
    {
        return false;
    }
    include
        .iter()
        .any(|pattern| provably_contains_pattern(pattern, source, case))
}

/// #142: no `card` subcommand emits a draft example — `SKILL.md`'s "Write a
/// bounded card" section carries a hand-authored template instead of a
/// generated one, and #142 §11 asks for a *command* reference, so this does
/// not point at a document section instead (see
/// `HANDOFF_DECLARATION_PARSE_RECOVERY` in `src/commands/handoff.rs` for the
/// same reasoning and the regression it avoids repeating). `serde_yaml_ng`
/// also offers no syntax/schema split — see `GATE_DEFINITION_PARSE_RECOVERY`
/// in `src/commands/gate.rs` — so this is one honest-for-both message.
///
/// Reached from two contexts: `commands::card::read_draft`'s external,
/// operator-supplied file (already given its own read-failure recovery one
/// call site earlier — B1's second reproduction, #142 §1) and
/// `stored_draft`'s re-read of this project's own already-activated,
/// control-managed draft, where a failure here means external corruption
/// rather than an operator's typo. The text below commits to neither story —
/// "re-check the position or field" reads correctly either way — rather
/// than picking the likelier case and being wrong for the other, per #142
/// §10.
const DRAFT_PARSE_RECOVERY: &str = "This card draft could not be parsed as YAML, or it parsed but does not match the schema; the message above names the position or the field. There is no generated example for a card draft document; re-check the position or field the message names.";

impl CardDraft {
    /// Checks each generated declaration, and what its class implies about
    /// this card's write scope.
    ///
    /// The scope rules are the point of classifying at all. A per-card artifact
    /// generated from sources the card does not own would go stale the moment
    /// somebody else changed them, with nothing to notice. A shared artifact
    /// inside the card's scope is two owners for one path, which is the exact
    /// collision the classes exist to prevent.
    /// Checks each declared artifact against what the card actually owns.
    ///
    /// Both arms used to compare write-scope *patterns* to artifact *paths*
    /// with `==`, which is only ever right by accident. Section 2716's rule is
    /// stated in ownership terms — "sources the card does not own", "inside a
    /// card's write scope" — and ownership is a matching question.
    ///
    /// The two arms deliberately use different tests, and the asymmetry is the
    /// point rather than an oversight:
    ///
    /// - A per-card **source** must be *contained* in the scope. Concrete paths
    ///   use `Scope::allows`; glob sources require a conservative containment
    ///   proof and must not intersect an exclude. Intersection alone would be
    ///   wrong here: a card owning `src/a/**` could then name `src/**` as a
    ///   source, which is exactly the staleness the rule exists to prevent.
    /// - A shared **path** is refused on *intersection*, because
    ///   `artifact.path` may itself be a glob and any overlap means two owners.
    ///   Over-approximating toward refusal is `patterns_intersect`'s documented
    ///   policy.
    ///
    /// A shared glob path is considered carved out only when one exclude is
    /// proven to contain the entire artifact pattern. Treating that pattern as
    /// one concrete path would let `src/*` appear to carve out `src/**`, even
    /// though deeper shared files would still have two owners.
    ///
    /// Case sensitivity follows the host (`CaseSensitivity::host()`), as scope
    /// matching does everywhere. This is the first place it governs *draft
    /// validation*, which mints an immutable record, so the same draft can be
    /// admissible on Linux and refused on macOS.
    ///
    /// Three adjacent holes this does **not** close, recorded so the next
    /// reader does not assume otherwise:
    /// - Nothing compares one card's declared shared artifact against another
    ///   *active* card's write scope. `allocation` compares scope to scope only.
    /// - The artifact's own `path` is never checked against the scope in the
    ///   per-card arm, only its `sources`.
    /// - `Transient` and `Serialized` are not checked at all.
    fn validate_generated_artifacts(&self) -> Result<(), HarnessError> {
        use crate::policy::paths::{
            CaseSensitivity, Scope, patterns_intersect, provably_contains_pattern,
        };

        require_one_class_each(&self.generated_artifacts)?;

        let case = CaseSensitivity::host();
        let scope = Scope::new(&self.write_scope.include, &self.write_scope.exclude);

        for artifact in &self.generated_artifacts {
            let invalid = |reason: String| HarnessError::Control {
                reason,
                code: ErrorCode::PolicyInvalidArtifact,
            };
            match artifact.class {
                ArtifactClass::PerCard => {
                    for source in &artifact.sources {
                        if !generated_source_is_owned(
                            source,
                            &scope,
                            &self.write_scope.include,
                            &self.write_scope.exclude,
                            case,
                        ) {
                            return Err(invalid(format!(
                                "per-card artifact `{}` is generated from `{source}`, which this card does not own; it would go stale the moment somebody else changed it",
                                artifact.path
                            )));
                        }
                    }
                }
                ArtifactClass::Shared => {
                    let carved_out =
                        self.write_scope.exclude.iter().any(|pattern| {
                            provably_contains_pattern(pattern, &artifact.path, case)
                        });
                    let claimed = !carved_out
                        && self
                            .write_scope
                            .include
                            .iter()
                            .any(|pattern| patterns_intersect(pattern, &artifact.path, case));
                    if claimed {
                        return Err(invalid(format!(
                            "shared artifact `{}` is also in this card's write scope; integration generates it, so claiming it here gives one path two owners",
                            artifact.path
                        )));
                    }
                }
                ArtifactClass::Transient | ArtifactClass::Serialized => {}
            }
        }
        Ok(())
    }

    /// Parses a draft from YAML or JSON.
    ///
    /// YAML is accepted because drafts are human-authored; JSON is accepted
    /// because it is a subset of YAML and machine authors emit it. Either way
    /// the activated record is JSON.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when the document is malformed or carries
    /// an undefined field.
    pub fn parse(raw: &str) -> Result<Self, HarnessError> {
        serde_yaml_ng::from_str(raw).map_err(|source| HarnessError::ControlWithRecovery {
            reason: format!("card draft is malformed: {source}"),
            code: ErrorCode::ConfigMalformed,
            recovery: DRAFT_PARSE_RECOVERY,
        })
    }

    /// Checks everything activation requires.
    ///
    /// # Errors
    ///
    /// Returns a policy error naming the first violated rule.
    pub fn validate(&self) -> Result<(), HarnessError> {
        let reject = |reason: String| -> HarnessError {
            HarnessError::Control {
                reason,
                code: ErrorCode::PolicyInvalidCard,
            }
        };

        if self.title.trim().is_empty() {
            return Err(reject("a card must have a title".to_owned()));
        }
        if self.goal.trim().is_empty() {
            return Err(reject("a card must state a goal".to_owned()));
        }
        if self.review_policy.trim().is_empty() {
            return Err(reject("a card must declare a review policy".to_owned()));
        }
        if self.rollback_strategy.trim().is_empty() {
            return Err(reject("a card must declare a rollback strategy".to_owned()));
        }
        if let Some(proof_map) = &self.proof_map {
            proof_map.validate()?;
        }
        if self.acceptance.behaviors.is_empty() {
            return Err(reject(
                "a card must declare at least one acceptance behavior; otherwise nothing can be reviewed against it".to_owned(),
            ));
        }
        if self.named_gates.feature.is_empty() {
            return Err(reject(
                "a card must name at least one feature gate".to_owned(),
            ));
        }
        if self.write_scope.include.is_empty() {
            return Err(reject(
                "a card must declare a write scope; scope is deny-by-default and an empty include owns nothing".to_owned(),
            ));
        }
        if self.base_sha.len() != 40 || !self.base_sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(reject(format!(
                "base_sha must be a full 40-character object ID, found `{}`",
                self.base_sha
            )));
        }
        if self.depends_on.contains(&self.card_id) {
            return Err(reject(format!(
                "card {} cannot depend on itself",
                self.card_id
            )));
        }
        for pattern in self
            .write_scope
            .include
            .iter()
            .chain(&self.write_scope.exclude)
        {
            validate_pattern("write-scope", pattern).map_err(reject)?;
        }
        for artifact in &self.generated_artifacts {
            validate_pattern("generated artifact path", &artifact.path).map_err(reject)?;
            for source in &artifact.sources {
                validate_pattern("generated artifact source", source).map_err(reject)?;
            }
        }
        self.validate_generated_artifacts()?;
        Ok(())
    }
}

/// Rejects patterns that could escape the repository or touch Git internals.
fn validate_pattern(kind: &str, pattern: &str) -> Result<(), String> {
    if pattern.trim().is_empty() {
        return Err(format!("{kind} patterns must not be empty"));
    }
    if pattern.starts_with('/') {
        return Err(format!(
            "{kind} pattern `{pattern}` must be repository-relative, not absolute"
        ));
    }
    if pattern.split('/').any(|segment| segment == "..") {
        return Err(format!(
            "{kind} pattern `{pattern}` must not traverse upward"
        ));
    }
    let first_semantic_segment = pattern
        .split('/')
        .find(|segment| !segment.is_empty() && *segment != ".");
    if first_semantic_segment.is_some_and(|segment| segment.eq_ignore_ascii_case(".git")) {
        return Err(format!(
            "{kind} pattern `{pattern}` must not name Git internals"
        ));
    }
    if pattern.contains('\\') {
        return Err(format!(
            "{kind} pattern `{pattern}` must use `/` separators"
        ));
    }
    Ok(())
}

/// An activated, immutable card.
///
/// Field order in this struct is irrelevant to the digest: canonical JSON sorts
/// keys, so two drafts that differ only in authoring order digest identically.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CardRecord {
    /// Always [`CARD_SCHEMA`].
    pub schema: String,
    /// Identifies the card.
    pub card_id: CardId,
    /// Starts at 1 and increases by exactly one.
    pub revision: u32,
    /// The cycle it belongs to.
    pub cycle_id: CycleId,
    /// One-line summary.
    pub title: String,
    /// What the card is for.
    pub goal: String,
    /// What it deliberately does not do.
    pub non_goals: Vec<String>,
    /// How much risk it carries.
    pub risk: Risk,
    /// What kind of change it is.
    pub change_kind: String,
    /// The exact commit work starts from.
    pub base_sha: String,
    /// Paths it may write.
    pub write_scope: WriteScope,
    /// Contract domains it reads.
    pub contract_reads: Vec<String>,
    /// Contract domains it changes.
    pub contract_changes: Vec<String>,
    /// Cards that must land first.
    pub depends_on: Vec<CardId>,
    /// Resources it needs exclusively.
    pub exclusive_resources: Vec<String>,
    /// Gates it must pass.
    pub named_gates: NamedGates,
    /// What it must deliver.
    pub acceptance: Acceptance,
    /// Generated paths it produces.
    pub generated_artifacts: Vec<GeneratedArtifact>,
    /// How review is conducted.
    pub review_policy: String,
    /// How to undo it.
    pub rollback_strategy: String,
    /// The immutable proof declaration supplied by the activated draft.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof_map: Option<ProofMap>,
    /// Who authored it. Declared, not proven.
    pub created_by: String,
    /// When it was authored.
    pub created_at: Timestamp,
}

impl CardRecord {
    /// Relative path of an activated card inside the control repository.
    #[must_use]
    pub fn relative_path(card_id: &CardId, revision: u32) -> String {
        format!("{CARD_DIR}/{card_id}/r{revision}.json")
    }

    /// Builds an activated record from a validated draft.
    ///
    /// # Errors
    ///
    /// Returns a policy error when the draft is invalid.
    pub fn activate(
        draft: &CardDraft,
        revision: u32,
        created_by: &str,
        created_at: Timestamp,
    ) -> Result<Self, HarnessError> {
        draft.validate()?;
        if revision == 0 {
            return Err(HarnessError::Control {
                reason: "card revisions begin at 1".to_owned(),
                code: ErrorCode::PolicyInvalidCard,
            });
        }
        Ok(Self {
            schema: CARD_SCHEMA.to_owned(),
            card_id: draft.card_id.clone(),
            revision,
            cycle_id: draft.cycle_id.clone(),
            title: draft.title.clone(),
            goal: draft.goal.clone(),
            non_goals: draft.non_goals.clone(),
            risk: draft.risk,
            change_kind: draft.change_kind.clone(),
            base_sha: draft.base_sha.clone(),
            write_scope: draft.write_scope.clone(),
            contract_reads: draft.contract_reads.clone(),
            contract_changes: draft.contract_changes.clone(),
            depends_on: draft.depends_on.clone(),
            exclusive_resources: draft.exclusive_resources.clone(),
            named_gates: draft.named_gates.clone(),
            acceptance: draft.acceptance.clone(),
            generated_artifacts: draft.generated_artifacts.clone(),
            review_policy: draft.review_policy.clone(),
            rollback_strategy: draft.rollback_strategy.clone(),
            proof_map: draft.proof_map.clone(),
            created_by: created_by.to_owned(),
            created_at,
        })
    }

    /// The card's canonical digest.
    ///
    /// # Errors
    ///
    /// Returns an error when the record cannot be serialized.
    pub fn digest(&self) -> Result<Digest, HarnessError> {
        Digest::of_canonical(self)
    }

    /// The canonicalization algorithm this record's digest was computed under.
    ///
    /// `SPIKE-001` finding F-2: a holder of the record must be able to
    /// recompute its digest without consulting the harness source.
    #[must_use]
    pub const fn canonical_algorithm() -> &'static str {
        CANONICAL_ALGORITHM
    }

    /// Builds the next revision of this card from a new draft.
    ///
    /// # Errors
    ///
    /// Returns a policy error when the draft changes the card's identity or the
    /// draft is otherwise invalid.
    pub fn revise(
        &self,
        draft: &CardDraft,
        created_by: &str,
        created_at: Timestamp,
    ) -> Result<Self, HarnessError> {
        if draft.card_id != self.card_id {
            return Err(HarnessError::Control {
                reason: format!(
                    "a revision must keep the card identifier; expected {} but the draft names {}",
                    self.card_id, draft.card_id
                ),
                code: ErrorCode::PolicyInvalidCard,
            });
        }
        Self::activate(draft, self.revision + 1, created_by, created_at)
    }

    /// Every path pattern this card claims, for overlap checks.
    #[must_use]
    pub fn claimed_patterns(&self) -> BTreeSet<&str> {
        self.write_scope
            .include
            .iter()
            .map(String::as_str)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::clock::{Clock as _, FixedClock};

    fn stamp() -> Timestamp {
        FixedClock::at_unix_seconds(1_785_196_800).unwrap().now()
    }

    fn draft() -> CardDraft {
        CardDraft {
            card_id: "F-001".parse().unwrap(),
            cycle_id: "C-001".parse().unwrap(),
            title: "Implement temperature conversion".to_owned(),
            goal: "Convert between Celsius and Fahrenheit".to_owned(),
            non_goals: vec!["No Kelvin".to_owned()],
            risk: Risk::Low,
            change_kind: "feature".to_owned(),
            base_sha: "a".repeat(40),
            write_scope: WriteScope {
                include: vec!["src/temperature.rs".to_owned()],
                exclude: vec!["tests/**".to_owned()],
            },
            contract_reads: vec![],
            contract_changes: vec![],
            depends_on: vec![],
            exclusive_resources: vec![],
            named_gates: NamedGates {
                feature: vec!["gate.unit".to_owned()],
                review: vec![],
                integration: vec!["gate.all".to_owned()],
            },
            acceptance: Acceptance {
                behaviors: vec!["converts correctly".to_owned()],
                regressions: vec![],
            },
            generated_artifacts: vec![],
            review_policy: "independent".to_owned(),
            rollback_strategy: "revert the commit".to_owned(),
            proof_map: None,
        }
    }

    fn activated() -> CardRecord {
        CardRecord::activate(&draft(), 1, "alvaro", stamp()).unwrap()
    }

    #[test]
    fn equivalent_drafts_produce_the_same_digest() {
        // Same content authored with fields in a different order.
        let yaml_one = "\
card_id: F-001
cycle_id: C-001
title: T
goal: G
risk: low
change_kind: feature
base_sha: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
write_scope:
  include: [src/a.rs]
  exclude: []
named_gates:
  feature: [g]
  review: []
  integration: []
acceptance:
  behaviors: [b]
  regressions: []
review_policy: independent
rollback_strategy: revert
";
        let yaml_two = "\
goal: G
title: T
cycle_id: C-001
card_id: F-001
rollback_strategy: revert
review_policy: independent
acceptance:
  regressions: []
  behaviors: [b]
named_gates:
  integration: []
  review: []
  feature: [g]
write_scope:
  exclude: []
  include: [src/a.rs]
base_sha: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
change_kind: feature
risk: low
";
        let one =
            CardRecord::activate(&CardDraft::parse(yaml_one).unwrap(), 1, "a", stamp()).unwrap();
        let two =
            CardRecord::activate(&CardDraft::parse(yaml_two).unwrap(), 1, "a", stamp()).unwrap();
        assert_eq!(one.digest().unwrap(), two.digest().unwrap());
    }

    #[test]
    fn json_and_yaml_drafts_agree() {
        let json = serde_json::to_string(&draft()).unwrap();
        assert_eq!(CardDraft::parse(&json).unwrap(), draft());
    }

    #[test]
    fn a_material_change_moves_the_digest() {
        let base = activated().digest().unwrap();
        for mutate in [
            (|d: &mut CardDraft| d.goal = "different".to_owned()) as fn(&mut CardDraft),
            |d: &mut CardDraft| d.base_sha = "b".repeat(40),
            |d: &mut CardDraft| d.risk = Risk::High,
            |d: &mut CardDraft| d.write_scope.include = vec!["src/other.rs".to_owned()],
            |d: &mut CardDraft| d.acceptance.behaviors.push("more".to_owned()),
            |d: &mut CardDraft| d.named_gates.feature.push("gate.extra".to_owned()),
        ] {
            let mut changed = draft();
            mutate(&mut changed);
            let digest = CardRecord::activate(&changed, 1, "alvaro", stamp())
                .unwrap()
                .digest()
                .unwrap();
            assert_ne!(digest, base, "a material change must move the digest");
        }
    }

    #[test]
    fn the_revision_number_participates_in_the_digest() {
        let first = activated().digest().unwrap();
        let second = CardRecord::activate(&draft(), 2, "alvaro", stamp())
            .unwrap()
            .digest()
            .unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn a_record_names_its_canonicalization_algorithm() {
        assert_eq!(CardRecord::canonical_algorithm(), CANONICAL_ALGORITHM);
    }

    #[test]
    fn activation_rejects_a_missing_goal_acceptance_or_gate() {
        for (mutate, expected) in [
            (
                (|d: &mut CardDraft| d.goal = "  ".to_owned()) as fn(&mut CardDraft),
                "goal",
            ),
            (
                |d: &mut CardDraft| d.acceptance.behaviors.clear(),
                "acceptance behavior",
            ),
            (
                |d: &mut CardDraft| d.named_gates.feature.clear(),
                "feature gate",
            ),
            (
                |d: &mut CardDraft| d.write_scope.include.clear(),
                "write scope",
            ),
            (
                |d: &mut CardDraft| d.review_policy = String::new(),
                "review policy",
            ),
            (
                |d: &mut CardDraft| d.rollback_strategy = String::new(),
                "rollback strategy",
            ),
        ] {
            let mut invalid = draft();
            mutate(&mut invalid);
            let error =
                CardRecord::activate(&invalid, 1, "alvaro", stamp()).expect_err("must reject");
            assert_eq!(error.code(), ErrorCode::PolicyInvalidCard);
            assert!(
                error.to_string().contains(expected),
                "expected `{expected}` in: {error}"
            );
        }
    }

    #[test]
    fn an_incomplete_optional_proof_map_is_refused_before_activation() {
        let complete = ProofMap {
            schema: PROOF_MAP_SCHEMA.to_owned(),
            entries: vec![ProofMapEntry {
                invariant: "the intended behavior remains true".to_owned(),
                precondition: "a focused fixture is installed".to_owned(),
                assertion: "the check sees the behavior".to_owned(),
                mutation: "bypass the check".to_owned(),
            }],
            claim_boundary: "only the focused behavior".to_owned(),
        };
        for (field, mutate) in [
            (
                "invariant",
                (|map: &mut ProofMap| map.entries[0].invariant = String::new())
                    as fn(&mut ProofMap),
            ),
            ("precondition", |map: &mut ProofMap| {
                map.entries[0].precondition = String::new();
            }),
            ("assertion", |map: &mut ProofMap| {
                map.entries[0].assertion = String::new();
            }),
            ("mutation", |map: &mut ProofMap| {
                map.entries[0].mutation = String::new();
            }),
            ("claim boundary", |map: &mut ProofMap| {
                map.claim_boundary = String::new();
            }),
        ] {
            let mut draft = draft();
            let mut incomplete = complete.clone();
            mutate(&mut incomplete);
            draft.proof_map = Some(incomplete);
            let error =
                CardRecord::activate(&draft, 1, "alvaro", stamp()).expect_err("must refuse");
            assert_eq!(error.code(), ErrorCode::PolicyInvalidCard);
            assert!(
                error.to_string().contains(field),
                "expected `{field}` in: {error}"
            );
        }
    }

    #[test]
    fn activation_requires_a_full_object_id_baseline() {
        for bad in ["abc", "", &"g".repeat(40), &"a".repeat(39)] {
            let mut invalid = draft();
            invalid.base_sha = bad.to_owned();
            assert!(
                CardRecord::activate(&invalid, 1, "alvaro", stamp()).is_err(),
                "`{bad}` must be rejected"
            );
        }
    }

    #[test]
    fn a_card_cannot_depend_on_itself() {
        let mut invalid = draft();
        invalid.depends_on = vec!["F-001".parse().unwrap()];
        assert!(CardRecord::activate(&invalid, 1, "alvaro", stamp()).is_err());
    }

    #[test]
    fn unsafe_write_scope_patterns_are_rejected() {
        for bad in [
            "/absolute/path",
            "../escape",
            "src/../../escape",
            ".git",
            ".git/config",
            "./.git/**",
            ".GIT/**",
            "src\\windows",
            "   ",
        ] {
            let mut invalid = draft();
            invalid.write_scope.include = vec![bad.to_owned()];
            assert!(
                CardRecord::activate(&invalid, 1, "alvaro", stamp()).is_err(),
                "`{bad}` must be rejected"
            );
        }
    }

    #[test]
    fn an_unknown_draft_field_fails() {
        let yaml = "\
card_id: F-001
cycle_id: C-001
title: T
goal: G
risk: low
change_kind: feature
base_sha: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
surprise: 1
write_scope: {include: [a], exclude: []}
named_gates: {feature: [g], review: [], integration: []}
acceptance: {behaviors: [b], regressions: []}
review_policy: independent
rollback_strategy: revert
";
        assert!(CardDraft::parse(yaml).is_err());
    }

    #[test]
    fn revisions_increase_by_exactly_one_and_keep_identity() {
        let first = activated();
        let second = first.revise(&draft(), "alvaro", stamp()).unwrap();
        assert_eq!(second.revision, 2);
        assert_eq!(second.card_id, first.card_id);

        let third = second.revise(&draft(), "alvaro", stamp()).unwrap();
        assert_eq!(third.revision, 3);
    }

    #[test]
    fn a_revision_cannot_change_the_card_identifier() {
        let first = activated();
        let mut renamed = draft();
        renamed.card_id = "F-002".parse().unwrap();
        let error = first
            .revise(&renamed, "alvaro", stamp())
            .expect_err("must reject");
        assert!(error.to_string().contains("keep the card identifier"));
    }

    #[test]
    fn revision_zero_is_rejected() {
        // #119: every production call site passes a literal `1` or
        // `self.revision + 1` where `self.revision >= 1` (enforced by this
        // same guard on every prior activation), so this guard is not
        // reachable through `card activate` or `card revise`. It is kept
        // because it is the sole enforcement of "revisions begin at 1" for
        // `CardRecord::activate`, a `pub` constructor callable by any future
        // caller within the crate; removing it would leave the invariant
        // unchecked everywhere rather than covered by a surviving refusal
        // elsewhere, unlike `gate.rs`'s `live_reservation_for_run` guard.
        // Asserting the exact code and message (not just `.is_err()`) is
        // what makes this test fail if either is later broken.
        let error = CardRecord::activate(&draft(), 0, "alvaro", stamp()).expect_err("must reject");
        assert_eq!(error.code(), ErrorCode::PolicyInvalidCard);
        assert_eq!(
            error.to_string(),
            "control state: card revisions begin at 1"
        );
    }

    #[test]
    fn only_a_draft_is_mutable() {
        assert!(CardState::Draft.is_mutable());
        for state in [
            CardState::Ready,
            CardState::Active,
            CardState::HandedOff,
            CardState::Approved,
            CardState::Landed,
        ] {
            assert!(!state.is_mutable(), "{state} must be immutable");
        }
    }

    #[test]
    fn the_documented_card_transitions_are_permitted() {
        for (from, to) in [
            (CardState::Draft, CardState::Ready),
            (CardState::Ready, CardState::Leased),
            (CardState::Leased, CardState::Active),
            (CardState::Active, CardState::HandedOff),
            (CardState::HandedOff, CardState::ReviewPending),
            (CardState::ReviewPending, CardState::Approved),
            (CardState::ReviewPending, CardState::ChangesRequested),
            (CardState::ChangesRequested, CardState::Active),
            (CardState::HandedOff, CardState::Active),
            // Reachable when the handoff is revoked while a review is open;
            // the reviewer may still file what they found.
            (CardState::Active, CardState::ChangesRequested),
            (CardState::Active, CardState::Blocked),
            (CardState::Approved, CardState::Active),
            (CardState::Approved, CardState::Integrating),
            (CardState::Integrating, CardState::Accepted),
            (CardState::Accepted, CardState::Landed),
            (CardState::Landed, CardState::Closed),
        ] {
            from.check_transition(to)
                .unwrap_or_else(|error| panic!("{from} -> {to}: {error}"));
        }
    }

    #[test]
    fn a_landed_card_cannot_be_abandoned() {
        assert!(
            CardState::Landed
                .check_transition(CardState::Abandoned)
                .is_err(),
            "Section 11.2: landed cannot transition to abandoned"
        );
        assert!(CardState::Closed.is_terminal());
    }

    #[test]
    fn high_and_critical_risk_require_a_human() {
        assert!(!Risk::Low.requires_human_review());
        assert!(!Risk::Medium.requires_human_review());
        assert!(Risk::High.requires_human_review());
        assert!(Risk::Critical.requires_human_review());
    }

    #[test]
    fn a_legacy_record_without_a_proof_map_round_trips_and_keeps_its_digest() {
        let record = activated();
        let digest = record.digest().unwrap();
        let encoded = serde_json::to_string_pretty(&record).unwrap();
        assert!(
            !encoded.contains("proof_map"),
            "the additive field must not rewrite legacy canonical bytes"
        );
        let decoded: CardRecord = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, record);
        assert_eq!(decoded.digest().unwrap(), digest);
    }

    #[test]
    fn committed_digest_vector_pins_the_canonical_form() {
        // Section 10.1 requires digest vectors to be committed before
        // activation is implemented. This pins the exact bytes a known card
        // digests to, so an accidental change to canonicalization fails here
        // rather than silently invalidating every stored review.
        let record = activated();
        assert_eq!(
            record.digest().unwrap().as_str(),
            "sha256:b1f04af3e07cb36e7b45351a93f975f48e58cb7e7af684a544d9e4cf0f124774"
        );
    }
}
