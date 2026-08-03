//! Pure exact-state receipt compatibility decisions.
//!
//! This module never executes a check, reserves capacity, or grants workflow
//! authority. It only answers whether one prior receipt is exact evidence for
//! one already-planned check, or names the one check that must run again.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    config::ValidationStage,
    domain::{
        digest::Digest,
        ids::{CycleId, IntegrationId},
    },
    policy::progressive_validation::{PlannedCheck, ValidationPlan},
    runner::receipt::{
        ProvenanceDimension, ProvenanceSubject, Receipt, ReceiptLineageKind, ReceiptProvenanceV1,
    },
};

/// Schema identifier for a compatibility decision.
pub const RECEIPT_COMPATIBILITY_SCHEMA: &str = "harness.receipt-compatibility/v1";

/// Stable, ordered reasons evidence cannot be reused.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum RerunReason {
    Missing,
    Foreign,
    Superseded,
    Invalidated,
    Stale,
    Incomplete,
    Candidate,
    Base,
    Card,
    Assignment,
    Integration,
    Gate,
    ProofMap,
    Policy,
    Environment,
    Configuration,
    Toolchain,
    Inputs,
    Fixtures,
    Cache,
    TrustMode,
}

/// The exact consumer state a receipt must match.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct CompatibilityRequest {
    pub plan: ValidationPlan,
    pub stage: ValidationStage,
    pub check: PlannedCheck,
    pub expected: ReceiptProvenanceV1,
}

/// Immutable consumer state for one combined-verification receipt.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct IntegrationCompatibilityContextV1 {
    pub integration_id: IntegrationId,
    pub cycle_id: CycleId,
    pub landing_sha: String,
    pub baseline_sha: String,
    pub integration_digest: Digest,
    pub verification_digest: Digest,
    pub policy_digest: Digest,
}

/// Typed request for final-integration receipt reuse.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct IntegrationCompatibilityRequestV1 {
    pub schema: String,
    pub context: IntegrationCompatibilityContextV1,
    pub stage: ValidationStage,
    pub check: PlannedCheck,
    pub expected: ReceiptProvenanceV1,
}

/// A read-only compatibility decision for an exact integration verification.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct IntegrationCompatibilityDecision {
    pub schema: String,
    pub integration_id: IntegrationId,
    pub integration_digest: Digest,
    pub verification_digest: Digest,
    pub stage: ValidationStage,
    pub gate_id: String,
    pub disposition: CompatibilityDisposition,
}

/// One decision; it does not authorize the next workflow transition.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct CompatibilityDecision {
    pub schema: String,
    pub card_digest: Digest,
    pub stage: ValidationStage,
    pub gate_id: String,
    pub disposition: CompatibilityDisposition,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CompatibilityDisposition {
    CompatibleReuse {
        receipt_id: String,
        receipt_digest: Digest,
        provenance_digest: Digest,
        compatibility_version: String,
        freshness_basis: BTreeMap<String, Digest>,
    },
    RerunRequired {
        reasons: Vec<RerunReason>,
        minimum_rerun_gate: String,
    },
}

/// Decides one check from immutable inputs, independently of receipt order.
#[must_use]
pub fn evaluate(request: &CompatibilityRequest, receipts: &[Receipt]) -> CompatibilityDecision {
    if request.plan.schema != crate::policy::progressive_validation::VALIDATION_PLAN_SCHEMA
        || !request_matches_plan(request)
    {
        return rerun(request, vec![RerunReason::Stale]);
    }
    if !request
        .plan
        .stages
        .iter()
        .find(|stage| stage.stage == request.stage)
        .is_some_and(|stage| stage.checks.iter().any(|check| check == &request.check))
    {
        return rerun(request, vec![RerunReason::Stale]);
    }
    let mut ordered: Vec<&Receipt> = receipts.iter().collect();
    ordered.sort_by_key(|receipt| receipt.receipt_id.as_str());
    let mut reasons = Vec::new();
    for receipt in ordered {
        if receipt.gate_id != request.check.gate_id {
            continue;
        }
        match compatible(request, receipt, receipts) {
            Ok((receipt_digest, provenance_digest)) => {
                return CompatibilityDecision {
                    schema: RECEIPT_COMPATIBILITY_SCHEMA.to_owned(),
                    card_digest: request.plan.card_digest.clone(),
                    stage: request.stage,
                    gate_id: request.check.gate_id.clone(),
                    disposition: CompatibilityDisposition::CompatibleReuse {
                        receipt_id: receipt.receipt_id.to_string(),
                        receipt_digest,
                        provenance_digest,
                        compatibility_version: RECEIPT_COMPATIBILITY_SCHEMA.to_owned(),
                        freshness_basis: request.expected.freshness_dependencies.clone(),
                    },
                };
            }
            Err(reason) => reasons.push(reason),
        }
    }
    if reasons.is_empty() {
        reasons.push(RerunReason::Missing);
    }
    reasons.sort();
    reasons.dedup();
    rerun(request, reasons)
}

/// Decides compatibility for one exact final-integration check.
#[must_use]
pub fn evaluate_integration(
    request: &IntegrationCompatibilityRequestV1,
    receipts: &[Receipt],
) -> IntegrationCompatibilityDecision {
    let rerun = |mut reasons: Vec<RerunReason>| {
        reasons.sort();
        reasons.dedup();
        IntegrationCompatibilityDecision {
            schema: RECEIPT_COMPATIBILITY_SCHEMA.to_owned(),
            integration_id: request.context.integration_id.clone(),
            integration_digest: request.context.integration_digest.clone(),
            verification_digest: request.context.verification_digest.clone(),
            stage: request.stage,
            gate_id: request.check.gate_id.clone(),
            disposition: CompatibilityDisposition::RerunRequired {
                reasons,
                minimum_rerun_gate: request.check.gate_id.clone(),
            },
        }
    };
    if request.schema != RECEIPT_COMPATIBILITY_SCHEMA
        || request.stage != ValidationStage::FinalIntegration
        || !integration_request_matches_context(request)
    {
        return rerun(vec![RerunReason::Stale]);
    }
    let mut ordered: Vec<&Receipt> = receipts.iter().collect();
    ordered.sort_by_key(|receipt| receipt.receipt_id.as_str());
    let mut reasons = Vec::new();
    for receipt in ordered {
        if receipt.gate_id != request.check.gate_id {
            continue;
        }
        match compatible_integration(request, receipt, receipts) {
            Ok((receipt_digest, provenance_digest)) => {
                return IntegrationCompatibilityDecision {
                    schema: RECEIPT_COMPATIBILITY_SCHEMA.to_owned(),
                    integration_id: request.context.integration_id.clone(),
                    integration_digest: request.context.integration_digest.clone(),
                    verification_digest: request.context.verification_digest.clone(),
                    stage: request.stage,
                    gate_id: request.check.gate_id.clone(),
                    disposition: CompatibilityDisposition::CompatibleReuse {
                        receipt_id: receipt.receipt_id.to_string(),
                        receipt_digest,
                        provenance_digest,
                        compatibility_version: RECEIPT_COMPATIBILITY_SCHEMA.to_owned(),
                        freshness_basis: request.expected.freshness_dependencies.clone(),
                    },
                };
            }
            Err(reason) => reasons.push(reason),
        }
    }
    if reasons.is_empty() {
        reasons.push(RerunReason::Missing);
    }
    rerun(reasons)
}

fn integration_request_matches_context(request: &IntegrationCompatibilityRequestV1) -> bool {
    let ProvenanceSubject::Integration {
        landing_sha,
        base_sha,
        cycle_id,
        integration_id,
        integration_digest,
    } = &request.expected.subject
    else {
        return false;
    };
    landing_sha == &request.context.landing_sha
        && base_sha == &request.context.baseline_sha
        && cycle_id == &request.context.cycle_id
        && integration_id == &request.context.integration_id
        && integration_digest == &request.context.integration_digest
        && request.expected.policy_digest == request.context.policy_digest
        && request.expected.proof_map == crate::runner::receipt::ProofMapBinding::NotApplicable
        && request.expected.validate().is_ok()
        && request.expected.has_all_reuse_dimensions()
}

fn compatible_integration(
    request: &IntegrationCompatibilityRequestV1,
    receipt: &Receipt,
    all_receipts: &[Receipt],
) -> Result<(Digest, Digest), RerunReason> {
    if receipt.integration_id.as_ref() != Some(&request.context.integration_id) {
        return Err(RerunReason::Foreign);
    }
    if receipt.schema != request.check.receipt_schema
        || receipt.gate_digest != request.check.gate_digest
        || receipt.attempt == 0
        || receipt.attempt > request.check.max_attempts
    {
        return Err(RerunReason::Gate);
    }
    if !receipt.passed || receipt.worktree_clean != Some(true) {
        return Err(RerunReason::Stale);
    }
    if has_lineage_relation(all_receipts, receipt, ReceiptLineageKind::Invalidates)? {
        return Err(RerunReason::Invalidated);
    }
    if has_lineage_relation(all_receipts, receipt, ReceiptLineageKind::Supersedes)? {
        return Err(RerunReason::Superseded);
    }
    let provenance = receipt
        .reuse_material()
        .map_err(|_| RerunReason::Incomplete)?;
    compare_provenance(&request.expected, provenance)?;
    Ok((
        receipt.digest().map_err(|_| RerunReason::Stale)?,
        provenance.digest().map_err(|_| RerunReason::Stale)?,
    ))
}

/// Refuse a caller-composed request whose expected provenance is not the
/// immutable card state the plan actually describes.  Without this check a
/// caller could reuse a receipt for a different base, revision, policy, or
/// proof declaration merely by supplying an otherwise valid planned check.
fn request_matches_plan(request: &CompatibilityRequest) -> bool {
    let ProvenanceSubject::Card {
        base_sha,
        card_revision,
        card_digest,
        ..
    } = &request.expected.subject
    else {
        return false;
    };
    base_sha == &request.plan.base_sha
        && *card_revision == request.plan.card_revision
        && card_digest == &request.plan.card_digest
        && request.expected.policy_digest == request.plan.policy_digest
        && match (&request.plan.proof_map_digest, &request.expected.proof_map) {
            (None, crate::runner::receipt::ProofMapBinding::NotApplicable) => true,
            (Some(expected), crate::runner::receipt::ProofMapBinding::Bound(actual)) => {
                expected == actual
            }
            _ => false,
        }
}

fn rerun(request: &CompatibilityRequest, mut reasons: Vec<RerunReason>) -> CompatibilityDecision {
    reasons.sort();
    reasons.dedup();
    CompatibilityDecision {
        schema: RECEIPT_COMPATIBILITY_SCHEMA.to_owned(),
        card_digest: request.plan.card_digest.clone(),
        stage: request.stage,
        gate_id: request.check.gate_id.clone(),
        disposition: CompatibilityDisposition::RerunRequired {
            reasons,
            minimum_rerun_gate: request.check.gate_id.clone(),
        },
    }
}

fn compatible(
    request: &CompatibilityRequest,
    receipt: &Receipt,
    all_receipts: &[Receipt],
) -> Result<(Digest, Digest), RerunReason> {
    if receipt.schema != request.check.receipt_schema {
        return Err(RerunReason::Stale);
    }
    if receipt.attempt == 0 || receipt.attempt > request.check.max_attempts {
        return Err(RerunReason::Stale);
    }
    if receipt.card_digest.as_ref() != Some(&request.plan.card_digest) {
        return Err(RerunReason::Foreign);
    }
    if receipt.evaluated_sha != subject_sha(&request.expected) {
        return Err(RerunReason::Candidate);
    }
    if receipt.gate_digest != request.check.gate_digest {
        return Err(RerunReason::Gate);
    }
    if receipt.worktree_clean != Some(true) || !receipt.passed {
        return Err(RerunReason::Stale);
    }
    if has_lineage_relation(all_receipts, receipt, ReceiptLineageKind::Invalidates)? {
        return Err(RerunReason::Invalidated);
    }
    if has_lineage_relation(all_receipts, receipt, ReceiptLineageKind::Supersedes)? {
        return Err(RerunReason::Superseded);
    }
    let provenance = receipt
        .reuse_material()
        .map_err(|_| RerunReason::Incomplete)?;
    compare_provenance(&request.expected, provenance)?;
    Ok((
        receipt.digest().map_err(|_| RerunReason::Stale)?,
        provenance.digest().map_err(|_| RerunReason::Stale)?,
    ))
}

fn has_lineage_relation(
    receipts: &[Receipt],
    prior: &Receipt,
    kind: ReceiptLineageKind,
) -> Result<bool, RerunReason> {
    let prior_digest = prior.digest().map_err(|_| RerunReason::Stale)?;
    for other in receipts {
        let Some(provenance) = other.provenance.as_ref() else {
            continue;
        };
        for fact in &provenance.lineage {
            if fact.kind == kind && fact.prior_receipt_id == prior.receipt_id {
                if provenance.validate().is_err() || !provenance.has_all_reuse_dimensions() {
                    return Err(RerunReason::Stale);
                }
                if fact.prior_receipt_digest != prior_digest {
                    return Err(RerunReason::Stale);
                }
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn subject_sha(provenance: &ReceiptProvenanceV1) -> String {
    match &provenance.subject {
        crate::runner::receipt::ProvenanceSubject::Card { candidate_sha, .. } => {
            candidate_sha.clone()
        }
        crate::runner::receipt::ProvenanceSubject::Integration { landing_sha, .. } => {
            landing_sha.clone()
        }
    }
}

fn compare_provenance(
    expected: &ReceiptProvenanceV1,
    actual: &ReceiptProvenanceV1,
) -> Result<(), RerunReason> {
    if expected.subject != actual.subject {
        return Err(subject_reason(&expected.subject, &actual.subject));
    }
    if expected.gate_definition_digest != actual.gate_definition_digest
        || expected.argv_digest != actual.argv_digest
    {
        return Err(RerunReason::Gate);
    }
    if expected.policy_digest != actual.policy_digest {
        return Err(RerunReason::Policy);
    }
    if expected.proof_map != actual.proof_map {
        return Err(RerunReason::ProofMap);
    }
    for dimension in [
        ProvenanceDimension::Environment,
        ProvenanceDimension::Configuration,
        ProvenanceDimension::Toolchain,
        ProvenanceDimension::Inputs,
        ProvenanceDimension::Fixtures,
        ProvenanceDimension::Cache,
        ProvenanceDimension::TrustMode,
    ] {
        if expected.dimensions.get(&dimension) != actual.dimensions.get(&dimension) {
            return Err(reason_for(dimension));
        }
    }
    if expected.freshness_dependencies != actual.freshness_dependencies {
        return Err(RerunReason::Stale);
    }
    Ok(())
}

fn subject_reason(expected: &ProvenanceSubject, actual: &ProvenanceSubject) -> RerunReason {
    match (expected, actual) {
        (
            ProvenanceSubject::Card {
                candidate_sha: expected_candidate,
                base_sha: expected_base,
                cycle_id: expected_cycle,
                card_id: expected_card,
                card_revision: expected_revision,
                card_digest: expected_digest,
                lease_id: expected_lease,
            },
            ProvenanceSubject::Card {
                candidate_sha: actual_candidate,
                base_sha: actual_base,
                cycle_id: actual_cycle,
                card_id: actual_card,
                card_revision: actual_revision,
                card_digest: actual_digest,
                lease_id: actual_lease,
            },
        ) => {
            if expected_candidate != actual_candidate {
                RerunReason::Candidate
            } else if expected_base != actual_base {
                RerunReason::Base
            } else if expected_lease != actual_lease {
                RerunReason::Assignment
            } else if expected_cycle != actual_cycle
                || expected_card != actual_card
                || expected_revision != actual_revision
                || expected_digest != actual_digest
            {
                RerunReason::Card
            } else {
                RerunReason::Stale
            }
        }
        (
            ProvenanceSubject::Integration {
                landing_sha: expected_landing,
                base_sha: expected_base,
                cycle_id: expected_cycle,
                integration_id: expected_id,
                integration_digest: expected_digest,
            },
            ProvenanceSubject::Integration {
                landing_sha: actual_landing,
                base_sha: actual_base,
                cycle_id: actual_cycle,
                integration_id: actual_id,
                integration_digest: actual_digest,
            },
        ) => {
            if expected_landing != actual_landing {
                RerunReason::Candidate
            } else if expected_base != actual_base {
                RerunReason::Base
            } else if expected_cycle != actual_cycle
                || expected_id != actual_id
                || expected_digest != actual_digest
            {
                RerunReason::Integration
            } else {
                RerunReason::Stale
            }
        }
        _ => RerunReason::Foreign,
    }
}

fn reason_for(dimension: ProvenanceDimension) -> RerunReason {
    match dimension {
        ProvenanceDimension::Environment => RerunReason::Environment,
        ProvenanceDimension::Configuration => RerunReason::Configuration,
        ProvenanceDimension::Toolchain => RerunReason::Toolchain,
        ProvenanceDimension::Inputs => RerunReason::Inputs,
        ProvenanceDimension::Fixtures => RerunReason::Fixtures,
        ProvenanceDimension::Cache => RerunReason::Cache,
        ProvenanceDimension::TrustMode => RerunReason::TrustMode,
        _ => RerunReason::Stale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::{
            clock::{Clock as _, FixedClock},
            ids::{CardId, CycleId, IntegrationId, ProjectId, ReceiptId},
        },
        runner::receipt::{
            ProofMapBinding, ProvenanceSubject, RECEIPT_PROVENANCE_SCHEMA, RECEIPT_SCHEMA,
            Termination,
        },
    };
    use std::path::PathBuf;

    fn digest(text: &str) -> Digest {
        Digest::of_bytes(text.as_bytes())
    }
    fn stamp() -> crate::domain::clock::Timestamp {
        FixedClock::at_unix_seconds(1_785_196_800).unwrap().now()
    }
    fn provenance() -> ReceiptProvenanceV1 {
        ReceiptProvenanceV1 {
            schema: RECEIPT_PROVENANCE_SCHEMA.to_owned(),
            subject: ProvenanceSubject::Card {
                candidate_sha: "a".repeat(40),
                base_sha: "b".repeat(40),
                cycle_id: "C-001".parse().unwrap(),
                card_id: "F-001".parse().unwrap(),
                card_revision: 1,
                card_digest: digest("card"),
                lease_id: "L-000001".parse().unwrap(),
            },
            gate_definition_digest: digest("gate"),
            argv_digest: digest("argv"),
            policy_digest: digest("policy"),
            proof_map: ProofMapBinding::NotApplicable,
            dimensions: BTreeMap::from([
                (ProvenanceDimension::Environment, digest("environment")),
                (ProvenanceDimension::Configuration, digest("configuration")),
                (ProvenanceDimension::Toolchain, digest("toolchain")),
                (ProvenanceDimension::Inputs, digest("inputs")),
                (ProvenanceDimension::Fixtures, digest("fixtures")),
                (ProvenanceDimension::Cache, digest("cache")),
                (ProvenanceDimension::TrustMode, digest("trust")),
            ]),
            freshness_dependencies: BTreeMap::from([("policy".to_owned(), digest("policy"))]),
            lineage: vec![],
            validation_reservation: None,
        }
    }
    fn request() -> CompatibilityRequest {
        let expected = provenance();
        let check = PlannedCheck {
            gate_id: "gate.unit".to_owned(),
            gate_digest: digest("gate"),
            receipt_schema: RECEIPT_SCHEMA.to_owned(),
            max_attempts: 1,
        };
        CompatibilityRequest {
            plan: ValidationPlan {
                schema: "harness.validation-plan/v1".to_owned(),
                card_revision: 1,
                card_digest: digest("card"),
                base_sha: "b".repeat(40),
                risk: "medium".to_owned(),
                policy_digest: digest("policy"),
                proof_map_digest: None,
                stages: vec![crate::policy::progressive_validation::PlannedStage {
                    stage: ValidationStage::Narrow,
                    checks: vec![check.clone()],
                }],
                next_permitted_stage: Some(ValidationStage::Narrow),
            },
            stage: ValidationStage::Narrow,
            check,
            expected,
        }
    }
    fn receipt(id: &str) -> Receipt {
        Receipt {
            schema: RECEIPT_SCHEMA.to_owned(),
            receipt_id: id.parse::<ReceiptId>().unwrap(),
            project_id: "example".parse::<ProjectId>().unwrap(),
            cycle_id: "C-001".parse::<CycleId>().unwrap(),
            card_id: Some("F-001".parse::<CardId>().unwrap()),
            card_digest: Some(digest("card")),
            integration_id: None,
            evaluated_sha: "a".repeat(40),
            gate_id: "gate.unit".to_owned(),
            gate_digest: digest("gate"),
            harness_version: "test".to_owned(),
            environment_fingerprint: "test".to_owned(),
            started_at: stamp(),
            finished_at: stamp(),
            duration_ms: 1,
            exit_code: Some(0),
            termination: Termination::Completed,
            stdout_digest: digest("out"),
            stderr_digest: digest("err"),
            artifact_digests: BTreeMap::new(),
            log_location: PathBuf::from("/tmp/log"),
            attempt: 1,
            passed: true,
            worktree_clean: Some(true),
            provenance: Some(provenance()),
        }
    }

    #[test]
    fn exact_complete_receipt_reuses_and_is_order_independent() {
        let request = request();
        let one = receipt("R-000001");
        let two = receipt("R-000002");
        let first = evaluate(&request, &[two.clone(), one.clone()]);
        let second = evaluate(&request, &[one, two]);
        assert_eq!(
            serde_json::to_string(&first).unwrap(),
            serde_json::to_string(&second).unwrap()
        );
        assert!(matches!(
            first.disposition,
            CompatibilityDisposition::CompatibleReuse { .. }
        ));
    }

    #[test]
    fn one_changed_dimension_requires_only_its_planned_check() {
        let request = request();
        let mut receipt = receipt("R-000001");
        receipt
            .provenance
            .as_mut()
            .unwrap()
            .dimensions
            .insert(ProvenanceDimension::Fixtures, digest("other"));
        let decision = evaluate(&request, &[receipt]);
        assert_eq!(
            decision.disposition,
            CompatibilityDisposition::RerunRequired {
                reasons: vec![RerunReason::Fixtures],
                minimum_rerun_gate: "gate.unit".to_owned()
            }
        );
    }

    #[test]
    fn successor_invalidation_refuses_an_otherwise_exact_receipt() {
        let request = request();
        let old = receipt("R-000001");
        let mut successor = receipt("R-000002");
        successor.gate_id = "gate.other".to_owned();
        successor.provenance.as_mut().unwrap().lineage.push(
            crate::runner::receipt::ReceiptLineageFact {
                kind: ReceiptLineageKind::Invalidates,
                prior_receipt_id: old.receipt_id.clone(),
                prior_receipt_digest: old.digest().unwrap(),
                dimension: ProvenanceDimension::Policy,
                prior_digest: digest("policy"),
                current_digest: digest("new-policy"),
                actor_digest: digest("actor"),
                recorded_at: stamp(),
            },
        );
        let decision = evaluate(&request, &[old, successor]);
        assert_eq!(
            decision.disposition,
            CompatibilityDisposition::RerunRequired {
                reasons: vec![RerunReason::Invalidated],
                minimum_rerun_gate: "gate.unit".to_owned()
            }
        );
    }

    #[test]
    fn successor_supersession_selects_the_newest_exact_receipt() {
        let request = request();
        let old = receipt("R-000001");
        let mut successor = receipt("R-000002");
        successor.provenance.as_mut().unwrap().lineage.push(
            crate::runner::receipt::ReceiptLineageFact {
                kind: ReceiptLineageKind::Supersedes,
                prior_receipt_id: old.receipt_id.clone(),
                prior_receipt_digest: old.digest().unwrap(),
                dimension: ProvenanceDimension::Candidate,
                prior_digest: digest("subject"),
                current_digest: digest("subject"),
                actor_digest: digest("actor"),
                recorded_at: stamp(),
            },
        );
        let decision = evaluate(&request, &[old, successor]);
        assert!(matches!(
            decision.disposition,
            CompatibilityDisposition::CompatibleReuse { ref receipt_id, .. } if receipt_id == "R-000002"
        ));
    }

    #[test]
    fn missing_and_foreign_receipts_have_distinct_refusal_reasons() {
        let request = request();
        let missing = evaluate(&request, &[]);
        assert_eq!(
            missing.disposition,
            CompatibilityDisposition::RerunRequired {
                reasons: vec![RerunReason::Missing],
                minimum_rerun_gate: "gate.unit".to_owned()
            }
        );
        let mut foreign = receipt("R-000001");
        foreign.card_digest = Some(digest("other-card"));
        let decision = evaluate(&request, &[foreign]);
        assert_eq!(
            decision.disposition,
            CompatibilityDisposition::RerunRequired {
                reasons: vec![RerunReason::Foreign],
                minimum_rerun_gate: "gate.unit".to_owned()
            }
        );
    }

    #[test]
    fn legacy_or_incomplete_provenance_cannot_be_silently_reused() {
        let request = request();
        let mut legacy = receipt("R-000001");
        legacy.provenance = None;
        let decision = evaluate(&request, &[legacy]);
        assert_eq!(
            decision.disposition,
            CompatibilityDisposition::RerunRequired {
                reasons: vec![RerunReason::Incomplete],
                minimum_rerun_gate: "gate.unit".to_owned()
            }
        );
    }

    #[test]
    fn request_must_bind_expected_identity_policy_and_proof_to_frozen_plan() {
        let mut request = request();
        if let ProvenanceSubject::Card { base_sha, .. } = &mut request.expected.subject {
            *base_sha = "c".repeat(40);
        }
        let decision = evaluate(&request, &[receipt("R-000001")]);
        assert_eq!(
            decision.disposition,
            CompatibilityDisposition::RerunRequired {
                reasons: vec![RerunReason::Stale],
                minimum_rerun_gate: "gate.unit".to_owned()
            }
        );
    }

    #[test]
    fn subject_mismatches_are_reported_by_their_exact_dimension() {
        let request = request();
        for (change, reason) in [
            ("base", RerunReason::Base),
            ("lease", RerunReason::Assignment),
            ("revision", RerunReason::Card),
        ] {
            let mut candidate = receipt("R-000001");
            let ProvenanceSubject::Card {
                base_sha,
                card_revision,
                lease_id,
                ..
            } = &mut candidate.provenance.as_mut().unwrap().subject
            else {
                unreachable!();
            };
            match change {
                "base" => *base_sha = "c".repeat(40),
                "lease" => *lease_id = "L-000002".parse().unwrap(),
                "revision" => *card_revision = 2,
                _ => unreachable!(),
            }
            let decision = evaluate(&request, &[candidate]);
            assert_eq!(
                decision.disposition,
                CompatibilityDisposition::RerunRequired {
                    reasons: vec![reason],
                    minimum_rerun_gate: "gate.unit".to_owned(),
                }
            );
        }
    }

    #[test]
    fn integration_subject_mismatch_is_not_collapsed_into_card() {
        let expected = ProvenanceSubject::Integration {
            landing_sha: "a".repeat(40),
            base_sha: "b".repeat(40),
            cycle_id: "C-001".parse().unwrap(),
            integration_id: "INT-000001".parse().unwrap(),
            integration_digest: digest("integration"),
        };
        let actual = ProvenanceSubject::Integration {
            landing_sha: "a".repeat(40),
            base_sha: "b".repeat(40),
            cycle_id: "C-001".parse().unwrap(),
            integration_id: "INT-000002".parse().unwrap(),
            integration_digest: digest("integration"),
        };
        assert_eq!(subject_reason(&expected, &actual), RerunReason::Integration);
    }

    #[test]
    fn malformed_successor_cannot_invalidate_an_exact_receipt() {
        let request = request();
        let old = receipt("R-000001");
        let mut successor = receipt("R-000002");
        successor.gate_id = "gate.other".to_owned();
        successor.provenance.as_mut().unwrap().lineage.push(
            crate::runner::receipt::ReceiptLineageFact {
                kind: ReceiptLineageKind::Invalidates,
                prior_receipt_id: old.receipt_id.clone(),
                prior_receipt_digest: old.digest().unwrap(),
                dimension: ProvenanceDimension::Policy,
                prior_digest: digest("policy"),
                current_digest: digest("new-policy"),
                actor_digest: digest("actor"),
                recorded_at: stamp(),
            },
        );
        successor.provenance.as_mut().unwrap().schema = "unsupported".to_owned();
        let decision = evaluate(&request, &[old, successor]);
        assert_eq!(
            decision.disposition,
            CompatibilityDisposition::RerunRequired {
                reasons: vec![RerunReason::Stale],
                minimum_rerun_gate: "gate.unit".to_owned(),
            }
        );
    }

    #[test]
    fn contradictory_receipt_identity_or_plan_schema_never_reuses() {
        let request = request();
        let mut contradictory = receipt("R-000001");
        contradictory.card_id = Some("F-999".parse().unwrap());
        let decision = evaluate(&request, &[contradictory]);
        assert!(matches!(
            decision.disposition,
            CompatibilityDisposition::RerunRequired { ref reasons, .. }
                if reasons == &vec![RerunReason::Incomplete]
        ));

        let mut wrong_schema = receipt("R-000002");
        wrong_schema.schema = "harness.receipt/other".to_owned();
        let decision = evaluate(&request, &[wrong_schema]);
        assert!(matches!(
            decision.disposition,
            CompatibilityDisposition::RerunRequired { ref reasons, .. }
                if reasons == &vec![RerunReason::Stale]
        ));
    }

    #[test]
    fn exact_complete_integration_receipt_reuses() {
        let mut integration_receipt = receipt("R-000001");
        integration_receipt.card_id = None;
        integration_receipt.card_digest = None;
        integration_receipt.integration_id = Some("INT-000001".parse().unwrap());
        integration_receipt.provenance.as_mut().unwrap().subject = ProvenanceSubject::Integration {
            landing_sha: "a".repeat(40),
            base_sha: "b".repeat(40),
            cycle_id: "C-001".parse::<CycleId>().unwrap(),
            integration_id: "INT-000001".parse::<IntegrationId>().unwrap(),
            integration_digest: digest("integration"),
        };
        let expected = integration_receipt.provenance.clone().unwrap();
        let request = IntegrationCompatibilityRequestV1 {
            schema: RECEIPT_COMPATIBILITY_SCHEMA.to_owned(),
            context: IntegrationCompatibilityContextV1 {
                integration_id: "INT-000001".parse().unwrap(),
                cycle_id: "C-001".parse().unwrap(),
                landing_sha: "a".repeat(40),
                baseline_sha: "b".repeat(40),
                integration_digest: digest("integration"),
                verification_digest: digest("verification"),
                policy_digest: digest("policy"),
            },
            stage: ValidationStage::FinalIntegration,
            check: PlannedCheck {
                gate_id: "gate.unit".to_owned(),
                gate_digest: digest("gate"),
                receipt_schema: RECEIPT_SCHEMA.to_owned(),
                max_attempts: 1,
            },
            expected,
        };
        assert!(matches!(
            evaluate_integration(&request, &[integration_receipt]).disposition,
            CompatibilityDisposition::CompatibleReuse { .. }
        ));
    }
}
