//! Governed lessons: durable, scoped guidance learned from prior work.
//!
//! A lesson is intentionally smaller and stricter than free-form agent memory.
//! It is an immutable, versioned control record with explicit applicability
//! selectors and an explicit enforcement level.  Agents may propose one, but
//! only an operator can activate it; lifecycle commands consume the resulting
//! manifest rather than silently consulting prose from an earlier run.

use serde::{Deserialize, Serialize};

use crate::{
    domain::{
        card::Risk,
        clock::Timestamp,
        digest::{CANONICAL_ALGORITHM, Digest},
        ids::{CardId, LessonId},
    },
    error::{ErrorCode, HarnessError},
};

/// Schema identifier for lesson records.
pub const LESSON_SCHEMA: &str = "harness.lesson/v1";
/// Directory containing lesson revisions in the control repository.
pub const LESSON_DIR: &str = "lessons";
/// Schema identifier for a card's computed lesson manifest.
pub const LESSON_MANIFEST_SCHEMA: &str = "harness.lesson-manifest/v1";
/// Schema identifier for the authoritative activation-time manifest binding.
pub const CARD_LESSON_BINDING_SCHEMA: &str = "harness.card-lesson-binding/v1";

/// Lifecycle state of one lesson identity.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LessonStatus {
    /// Proposed but not used by workflow policy.
    Proposed,
    /// Authorized for matching and enforcement.
    Active,
    /// No longer applied to new work; retained for history.
    Retired,
}

/// How strongly a matching lesson constrains a lifecycle transition.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq, Ord, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum LessonEnforcement {
    /// Missing the lesson's evidence refuses the transition.
    Required,
    /// The packet and result warn when the lesson was not addressed.
    Preferred,
    /// The lesson is context only and never blocks.
    Informational,
}

impl LessonEnforcement {
    /// Stable wire name used in human output and receipts.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::Preferred => "preferred",
            Self::Informational => "informational",
        }
    }
}

/// Explicit facts that decide whether a lesson applies to a card.
#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LessonSelectors {
    /// Path patterns that intersect the card's declared write scope.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Contract domains read or changed by the card.
    #[serde(default)]
    pub contracts: Vec<String>,
    /// Exact change-kind names, such as `feature` or `migration`.
    #[serde(default)]
    pub change_kinds: Vec<String>,
    /// Apply at or above this risk level.
    #[serde(default)]
    pub minimum_risk: Option<Risk>,
}

/// Machine-checkable obligations attached to a lesson.
#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LessonObligations {
    /// Named feature gates that must have passing evidence at handoff.
    #[serde(default)]
    pub feature_gates: Vec<String>,
    /// Named integration gates that must be present in integration evidence.
    #[serde(default)]
    pub integration_gates: Vec<String>,
    /// Stable review questions that must be explicitly dispositioned.
    #[serde(default)]
    pub review_checks: Vec<String>,
}

impl LessonObligations {
    /// Whether the lesson carries at least one enforceable obligation.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.feature_gates.is_empty()
            && self.integration_gates.is_empty()
            && self.review_checks.is_empty()
    }

    /// All gate identifiers, in declaration order with duplicates removed.
    #[must_use]
    pub fn gate_ids(&self) -> Vec<String> {
        let mut ids = self.feature_gates.clone();
        ids.extend(self.integration_gates.iter().cloned());
        ids.sort();
        ids.dedup();
        ids
    }
}

/// Provenance for a lesson.  Free text is allowed, but it is never optional:
/// a lesson without a source cannot be independently reviewed or retired.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LessonProvenance {
    /// Kind of source, for example `review`, `incident`, or `operator`.
    pub source_kind: String,
    /// Stable identifier of the source when one exists.
    pub source_id: String,
    /// What the source established.
    pub evidence: String,
}

/// A mutable authoring document used by `lesson propose`.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LessonDraft {
    pub title: String,
    pub rule: String,
    pub rationale: String,
    pub selectors: LessonSelectors,
    pub enforcement: LessonEnforcement,
    pub obligations: LessonObligations,
    pub provenance: LessonProvenance,
}

impl LessonDraft {
    /// Validates a draft before allocating an immutable identity.
    ///
    /// # Errors
    ///
    /// Returns a lesson-policy error when required fields or selectors are absent.
    pub fn validate(&self) -> Result<(), HarnessError> {
        validate_text("title", &self.title)?;
        validate_text("rule", &self.rule)?;
        validate_text("rationale", &self.rationale)?;
        validate_selectors(&self.selectors)?;
        validate_obligations(&self.obligations)?;
        if self.enforcement == LessonEnforcement::Required && self.obligations.is_empty() {
            return Err(HarnessError::Control {
                reason: "a required lesson must declare a gate or review-check obligation"
                    .to_owned(),
                code: ErrorCode::PolicyLessonInvalid,
            });
        }
        validate_provenance(&self.provenance)
    }
}

/// One immutable revision of a lesson.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LessonRecord {
    /// Always [`LESSON_SCHEMA`].
    pub schema: String,
    /// Stable lesson identity.
    pub lesson_id: LessonId,
    /// Monotonic revision, starting at one.
    pub revision: u32,
    /// Whether this revision participates in matching.
    pub status: LessonStatus,
    pub title: String,
    pub rule: String,
    pub rationale: String,
    pub selectors: LessonSelectors,
    pub enforcement: LessonEnforcement,
    pub obligations: LessonObligations,
    pub provenance: LessonProvenance,
    pub created_by: String,
    pub created_at: Timestamp,
    /// Previous revision when this record changes the same lesson identity.
    #[serde(default)]
    pub supersedes: Option<u32>,
    /// Digest algorithm used by [`Self::digest`].
    pub canonical_algorithm: String,
}

impl LessonRecord {
    /// Relative path of one immutable lesson revision.
    #[must_use]
    pub fn relative_path(lesson_id: &LessonId, revision: u32) -> String {
        format!("{LESSON_DIR}/{lesson_id}/r{revision}.json")
    }

    /// Validates the record at a control boundary.
    ///
    /// # Errors
    ///
    /// Returns a lesson-policy error when the schema, provenance, or obligations are invalid.
    pub fn validate(&self) -> Result<(), HarnessError> {
        let reject = |reason: String| HarnessError::Control {
            reason,
            code: ErrorCode::PolicyLessonInvalid,
        };
        if self.schema != LESSON_SCHEMA {
            return Err(reject(format!(
                "lesson.schema must be `{LESSON_SCHEMA}`, found `{}`",
                self.schema
            )));
        }
        if self.revision == 0 {
            return Err(reject("lesson revisions begin at 1".to_owned()));
        }
        validate_text("title", &self.title)?;
        validate_text("rule", &self.rule)?;
        validate_text("rationale", &self.rationale)?;
        validate_text("created_by", &self.created_by)?;
        validate_selectors(&self.selectors)?;
        validate_obligations(&self.obligations)?;
        validate_provenance(&self.provenance)?;
        if self.canonical_algorithm != CANONICAL_ALGORITHM {
            return Err(reject(format!(
                "lesson.canonical_algorithm must be `{CANONICAL_ALGORITHM}`"
            )));
        }
        if self.enforcement == LessonEnforcement::Required && self.obligations.is_empty() {
            return Err(reject(
                "a required lesson must declare a gate or review-check obligation".to_owned(),
            ));
        }
        Ok(())
    }

    /// Digest of this exact immutable revision.
    ///
    /// # Errors
    ///
    /// Returns an encoding error when the record cannot be canonicalized.
    pub fn digest(&self) -> Result<Digest, HarnessError> {
        Digest::of_canonical(self)
    }
}

/// One lesson selected into a deterministic card manifest.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ApplicableLesson {
    pub lesson_id: LessonId,
    pub revision: u32,
    pub lesson_digest: Digest,
    pub enforcement: LessonEnforcement,
    pub title: String,
    pub rule: String,
    pub obligations: LessonObligations,
}

/// Frozen, exact set of lessons applicable to one card revision.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LessonManifest {
    pub schema: String,
    pub card_id: CardId,
    pub card_revision: u32,
    pub card_digest: Digest,
    pub lessons: Vec<ApplicableLesson>,
}

/// Immutable activation-time binding between one card revision and its
/// applicable lesson manifest.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CardLessonBindingRecord {
    pub schema: String,
    pub card_id: CardId,
    pub card_revision: u32,
    pub card_digest: Digest,
    pub manifest: LessonManifest,
    pub manifest_digest: Digest,
    pub canonical_algorithm: String,
}

impl CardLessonBindingRecord {
    #[must_use]
    pub fn relative_path(card_id: &CardId, revision: u32) -> String {
        // Card revision records use an `/r*.json` leaf. Keep this binding in
        // its own namespace without that prefix so snapshot readers cannot
        // misclassify it as a `CardRecord`.
        format!("cards/{card_id}/lesson-manifests/{revision}.json")
    }

    /// Validates the immutable binding and its canonical manifest digest.
    ///
    /// # Errors
    ///
    /// Returns a stale-manifest policy error when any subject, schema, or
    /// digest field disagrees with the embedded activation manifest.
    pub fn validate(&self) -> Result<(), HarnessError> {
        if self.schema != CARD_LESSON_BINDING_SCHEMA
            || self.canonical_algorithm != CANONICAL_ALGORITHM
            || self.card_id != self.manifest.card_id
            || self.card_revision != self.manifest.card_revision
            || self.card_digest != self.manifest.card_digest
            || self.manifest.schema != LESSON_MANIFEST_SCHEMA
            || self.manifest_digest != self.manifest.digest()?
        {
            return Err(HarnessError::Control {
                reason: format!(
                    "card {} revision {} has an invalid frozen lesson binding",
                    self.card_id, self.card_revision
                ),
                code: ErrorCode::PolicyLessonManifestStale,
            });
        }
        Ok(())
    }
}

/// How a reviewer dispositioned one named lesson question.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LessonCheckStatus {
    /// The reviewer found the lesson satisfied by the candidate.
    Satisfied,
    /// The lesson does not apply to this exact review despite packet inclusion.
    NotApplicable,
    /// The reviewer found the lesson unmet; this blocks approval.
    NotSatisfied,
}

/// A structured, auditable disposition for one lesson review check.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LessonCheck {
    pub lesson_id: LessonId,
    pub check_id: String,
    pub status: LessonCheckStatus,
    pub evidence: String,
}

impl LessonCheck {
    /// Validates that a disposition is not an empty checkbox.
    ///
    /// # Errors
    ///
    /// Returns a lesson-evidence error when the check or evidence is blank.
    pub fn validate(&self) -> Result<(), HarnessError> {
        if self.check_id.trim().is_empty() || self.evidence.trim().is_empty() {
            return Err(HarnessError::Control {
                reason: "lesson check must name a check and record evidence".to_owned(),
                code: ErrorCode::PolicyLessonEvidenceMissing,
            });
        }
        Ok(())
    }
}

impl LessonManifest {
    /// Digest used to bind packets and lifecycle records to this exact set.
    ///
    /// # Errors
    ///
    /// Returns an encoding error when the manifest cannot be canonicalized.
    pub fn digest(&self) -> Result<Digest, HarnessError> {
        Digest::of_canonical(self)
    }

    /// Matching lessons whose omission must refuse a transition.
    pub fn required(&self) -> impl Iterator<Item = &ApplicableLesson> {
        self.lessons
            .iter()
            .filter(|lesson| lesson.enforcement == LessonEnforcement::Required)
    }

    /// Matching lessons that are advisory but should remain visible.
    pub fn advisory(&self) -> impl Iterator<Item = &ApplicableLesson> {
        self.lessons
            .iter()
            .filter(|lesson| lesson.enforcement != LessonEnforcement::Required)
    }
}

fn validate_text(field: &str, value: &str) -> Result<(), HarnessError> {
    if value.trim().is_empty() {
        return Err(HarnessError::Control {
            reason: format!("lesson `{field}` must not be empty"),
            code: ErrorCode::PolicyLessonInvalid,
        });
    }
    Ok(())
}

fn validate_selectors(selectors: &LessonSelectors) -> Result<(), HarnessError> {
    if selectors.paths.is_empty()
        && selectors.contracts.is_empty()
        && selectors.change_kinds.is_empty()
        && selectors.minimum_risk.is_none()
    {
        return Err(HarnessError::Control {
            reason: "a lesson must declare at least one applicability selector".to_owned(),
            code: ErrorCode::PolicyLessonInvalid,
        });
    }
    for (kind, values) in [
        ("path", &selectors.paths),
        ("contract", &selectors.contracts),
        ("change_kind", &selectors.change_kinds),
    ] {
        if values.iter().any(|value| value.trim().is_empty()) {
            return Err(HarnessError::Control {
                reason: format!("lesson selector `{kind}` contains an empty value"),
                code: ErrorCode::PolicyLessonInvalid,
            });
        }
    }
    Ok(())
}

fn validate_obligations(obligations: &LessonObligations) -> Result<(), HarnessError> {
    for (kind, values) in [
        ("feature_gate", &obligations.feature_gates),
        ("integration_gate", &obligations.integration_gates),
        ("review_check", &obligations.review_checks),
    ] {
        if values.iter().any(|value| value.trim().is_empty()) {
            return Err(HarnessError::Control {
                reason: format!("lesson obligation `{kind}` contains an empty value"),
                code: ErrorCode::PolicyLessonInvalid,
            });
        }
    }
    Ok(())
}

fn validate_provenance(provenance: &LessonProvenance) -> Result<(), HarnessError> {
    validate_text("provenance.source_kind", &provenance.source_kind)?;
    validate_text("provenance.source_id", &provenance.source_id)?;
    validate_text("provenance.evidence", &provenance.evidence)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft() -> LessonDraft {
        LessonDraft {
            title: "Keep packets explicit".to_owned(),
            rule: "Include the generated packet in every handoff".to_owned(),
            rationale: "Fresh agents otherwise miss prior findings".to_owned(),
            selectors: LessonSelectors {
                paths: vec!["src/**".to_owned()],
                ..LessonSelectors::default()
            },
            enforcement: LessonEnforcement::Required,
            obligations: LessonObligations {
                review_checks: vec!["packet-reviewed".to_owned()],
                ..LessonObligations::default()
            },
            provenance: LessonProvenance {
                source_kind: "review".to_owned(),
                source_id: "RV-000001".to_owned(),
                evidence: "A prior review found packet omission".to_owned(),
            },
        }
    }

    #[test]
    fn required_lessons_need_an_obligation() {
        let mut value = draft();
        value.obligations = LessonObligations::default();
        assert_eq!(
            value.validate().unwrap_err().code(),
            ErrorCode::PolicyLessonInvalid
        );
        let record = LessonRecord {
            schema: LESSON_SCHEMA.to_owned(),
            lesson_id: "LS-000001".parse().unwrap(),
            revision: 1,
            status: LessonStatus::Active,
            title: value.title,
            rule: value.rule,
            rationale: value.rationale,
            selectors: value.selectors,
            enforcement: value.enforcement,
            obligations: value.obligations,
            provenance: value.provenance,
            created_by: "operator".to_owned(),
            created_at: Timestamp::from_unix_seconds(0).unwrap(),
            supersedes: None,
            canonical_algorithm: CANONICAL_ALGORITHM.to_owned(),
        };
        assert_eq!(
            record.validate().unwrap_err().code(),
            ErrorCode::PolicyLessonInvalid
        );
    }

    #[test]
    fn manifest_digest_changes_when_a_lesson_changes() {
        let value = draft();
        let lesson = LessonRecord {
            schema: LESSON_SCHEMA.to_owned(),
            lesson_id: "LS-000001".parse().unwrap(),
            revision: 1,
            status: LessonStatus::Active,
            title: value.title,
            rule: value.rule,
            rationale: value.rationale,
            selectors: value.selectors,
            enforcement: value.enforcement,
            obligations: value.obligations,
            provenance: value.provenance,
            created_by: "operator".to_owned(),
            created_at: Timestamp::from_unix_seconds(0).unwrap(),
            supersedes: None,
            canonical_algorithm: CANONICAL_ALGORITHM.to_owned(),
        };
        let mut manifest = LessonManifest {
            schema: LESSON_MANIFEST_SCHEMA.to_owned(),
            card_id: "F-001".parse().unwrap(),
            card_revision: 1,
            card_digest: Digest::of_bytes(b"card"),
            lessons: vec![ApplicableLesson {
                lesson_id: lesson.lesson_id.clone(),
                revision: lesson.revision,
                lesson_digest: lesson.digest().unwrap(),
                enforcement: lesson.enforcement,
                title: lesson.title.clone(),
                rule: lesson.rule.clone(),
                obligations: lesson.obligations.clone(),
            }],
        };
        let first = manifest.digest().unwrap();
        manifest.lessons[0].rule.push_str(" with receipts");
        assert_ne!(first, manifest.digest().unwrap());
    }
}
