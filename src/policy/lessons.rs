//! Deterministic lesson applicability and manifest construction.

use crate::{
    domain::{
        card::CardRecord,
        lesson::{
            ApplicableLesson, LESSON_MANIFEST_SCHEMA, LessonManifest, LessonRecord, LessonStatus,
        },
    },
    error::{ErrorCode, HarnessError},
    policy::paths::Scope,
};

/// Selects active lessons that apply to one exact card revision.
///
/// Every non-empty selector dimension is conjunctive. This is deliberately
/// conservative: a lesson with a path and a risk selector applies only when
/// both facts match. Within one dimension, alternatives are disjunctive.
///
/// # Errors
///
/// Returns a lesson-policy or encoding error when an active lesson is malformed
/// or the manifest cannot be digested.
pub fn build_manifest(
    card: &CardRecord,
    lessons: &[LessonRecord],
) -> Result<LessonManifest, HarnessError> {
    let mut selected = Vec::new();
    for lesson in lessons {
        lesson.validate()?;
        if lesson.status != LessonStatus::Active || !matches_card(card, lesson) {
            continue;
        }
        selected.push(ApplicableLesson {
            lesson_id: lesson.lesson_id.clone(),
            revision: lesson.revision,
            lesson_digest: lesson.digest()?,
            enforcement: lesson.enforcement,
            title: lesson.title.clone(),
            rule: lesson.rule.clone(),
            obligations: lesson.obligations.clone(),
        });
    }
    selected.sort_by(|left, right| {
        left.lesson_id
            .cmp(&right.lesson_id)
            .then(left.revision.cmp(&right.revision))
    });
    Ok(LessonManifest {
        schema: LESSON_MANIFEST_SCHEMA.to_owned(),
        card_id: card.card_id.clone(),
        card_revision: card.revision,
        card_digest: card.digest()?,
        lessons: selected,
    })
}

/// Checks that the manifest belongs to the card that is about to use it.
///
/// # Errors
///
/// Returns a stale-manifest error when any card binding differs.
pub fn validate_manifest_for_card(
    manifest: &LessonManifest,
    card: &CardRecord,
) -> Result<(), HarnessError> {
    if manifest.schema != LESSON_MANIFEST_SCHEMA
        || manifest.card_id != card.card_id
        || manifest.card_revision != card.revision
        || manifest.card_digest != card.digest()?
    {
        return Err(HarnessError::Control {
            reason: format!(
                "lesson manifest is not bound to card {} revision {}",
                card.card_id, card.revision
            ),
            code: ErrorCode::PolicyLessonManifestStale,
        });
    }
    Ok(())
}

/// True when every selector dimension declared by a lesson matches the card.
#[must_use]
pub fn matches_card(card: &CardRecord, lesson: &LessonRecord) -> bool {
    let selectors = &lesson.selectors;
    let card_scope = Scope::new(&card.write_scope.include, &card.write_scope.exclude);
    let lesson_scope = Scope::new(&selectors.paths, &[]);
    let paths_match = selectors.paths.is_empty() || card_scope.overlaps(&lesson_scope).is_some();
    let contracts_match = selectors.contracts.is_empty()
        || selectors.contracts.iter().any(|contract| {
            card.contract_reads.iter().any(|value| value == contract)
                || card.contract_changes.iter().any(|value| value == contract)
        });
    let change_kind_match = selectors.change_kinds.is_empty()
        || selectors
            .change_kinds
            .iter()
            .any(|kind| kind == &card.change_kind);
    let risk_match = selectors
        .minimum_risk
        .is_none_or(|minimum| risk_rank(card.risk) >= risk_rank(minimum));
    paths_match && contracts_match && change_kind_match && risk_match
}

fn risk_rank(risk: crate::domain::card::Risk) -> u8 {
    match risk {
        crate::domain::card::Risk::Low => 0,
        crate::domain::card::Risk::Medium => 1,
        crate::domain::card::Risk::High => 2,
        crate::domain::card::Risk::Critical => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        card::{Acceptance, NamedGates, Risk, WriteScope},
        clock::Timestamp,
        digest::CANONICAL_ALGORITHM,
        lesson::{
            LessonEnforcement, LessonObligations, LessonProvenance, LessonSelectors, LessonStatus,
        },
    };

    fn card() -> CardRecord {
        CardRecord {
            schema: crate::domain::card::CARD_SCHEMA.to_owned(),
            card_id: "F-001".parse().unwrap(),
            revision: 1,
            cycle_id: "C-001".parse().unwrap(),
            title: "feature".to_owned(),
            goal: "goal".to_owned(),
            non_goals: vec![],
            risk: Risk::Medium,
            change_kind: "feature".to_owned(),
            base_sha: "a".repeat(40),
            write_scope: WriteScope {
                include: vec!["src/**".to_owned()],
                exclude: vec![],
            },
            contract_reads: vec!["api.v1".to_owned()],
            contract_changes: vec![],
            depends_on: vec![],
            exclusive_resources: vec![],
            named_gates: NamedGates {
                feature: vec!["gate.unit".to_owned()],
                review: vec![],
                integration: vec![],
            },
            acceptance: Acceptance {
                behaviors: vec!["works".to_owned()],
                regressions: vec![],
            },
            generated_artifacts: vec![],
            review_policy: "independent".to_owned(),
            rollback_strategy: "revert".to_owned(),
            proof_map: None,
            created_by: "operator".to_owned(),
            created_at: Timestamp::from_unix_seconds(0).unwrap(),
        }
    }

    fn lesson(path: &str) -> LessonRecord {
        LessonRecord {
            schema: crate::domain::lesson::LESSON_SCHEMA.to_owned(),
            lesson_id: "LS-000001".parse().unwrap(),
            revision: 1,
            status: LessonStatus::Active,
            title: "lesson".to_owned(),
            rule: "rule".to_owned(),
            rationale: "rationale".to_owned(),
            selectors: LessonSelectors {
                paths: vec![path.to_owned()],
                ..LessonSelectors::default()
            },
            enforcement: LessonEnforcement::Required,
            obligations: LessonObligations {
                review_checks: vec!["check".to_owned()],
                ..LessonObligations::default()
            },
            provenance: LessonProvenance {
                source_kind: "review".to_owned(),
                source_id: "RV-000001".to_owned(),
                evidence: "evidence".to_owned(),
            },
            created_by: "operator".to_owned(),
            created_at: Timestamp::from_unix_seconds(0).unwrap(),
            supersedes: None,
            canonical_algorithm: CANONICAL_ALGORITHM.to_owned(),
        }
    }

    #[test]
    fn unrelated_paths_are_excluded() {
        let manifest = build_manifest(&card(), &[lesson("docs/**")]).unwrap();
        assert!(manifest.lessons.is_empty());
    }

    #[test]
    fn matching_paths_are_selected_deterministically() {
        let manifest = build_manifest(&card(), &[lesson("src/**")]).unwrap();
        assert_eq!(manifest.lessons.len(), 1);
        assert_eq!(manifest.lessons[0].lesson_id.as_str(), "LS-000001");
    }

    #[test]
    fn lesson_targeting_only_an_excluded_subtree_is_not_selected() {
        let mut scoped_card = card();
        scoped_card.write_scope.exclude = vec!["src/generated/**".to_owned()];
        let excluded_lesson = lesson("src/generated/**");

        assert!(!matches_card(&scoped_card, &excluded_lesson));
        let manifest = build_manifest(&scoped_card, &[excluded_lesson]).unwrap();
        assert!(manifest.lessons.is_empty());
    }

    #[test]
    fn mixed_lesson_paths_match_through_unexcluded_scope_regardless_of_order() {
        let mut scoped_card = card();
        scoped_card.write_scope.exclude = vec!["src/generated/**".to_owned()];

        let mut excluded_first = lesson("src/generated/**");
        excluded_first.selectors.paths.push("src/api/**".to_owned());
        let mut included_first = lesson("src/api/**");
        included_first
            .selectors
            .paths
            .push("src/generated/**".to_owned());

        assert!(matches_card(&scoped_card, &excluded_first));
        assert!(matches_card(&scoped_card, &included_first));
    }
}
