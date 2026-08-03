//! Deterministic, read-only projection of a card's validation ladder.
//!
//! This module deliberately does not execute gates or decide whether an
//! existing receipt is reusable. It states the frozen order and exact
//! definitions later workflow boundaries must honor.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::{
    config::{ValidationPolicy, ValidationStage},
    domain::{card::CardRecord, digest::Digest, gate::GateDefinition},
    error::{ErrorCode, HarnessError},
    runner::receipt::RECEIPT_SCHEMA,
};

/// Schema identifier for a derived progressive-validation plan.
pub const VALIDATION_PLAN_SCHEMA: &str = "harness.validation-plan/v1";

/// One registered check required by a planned stage.
#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
pub struct PlannedCheck {
    /// Registered gate identifier.
    pub gate_id: String,
    /// Exact gate definition the next workflow stage must use.
    pub gate_digest: Digest,
    /// Receipt representation required to demonstrate this check later ran.
    pub receipt_schema: String,
}

/// One ordered validation stage.
#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
pub struct PlannedStage {
    /// Stable stage identifier.
    pub stage: ValidationStage,
    /// Checks in the frozen order declared by the card.
    pub checks: Vec<PlannedCheck>,
}

/// The immutable inputs and ordered checks a later execution boundary uses.
#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
pub struct ValidationPlan {
    /// Always [`VALIDATION_PLAN_SCHEMA`].
    pub schema: String,
    /// Activated card revision the plan describes.
    pub card_revision: u32,
    /// Exact immutable card definition.
    pub card_digest: Digest,
    /// Frozen cycle base the card declares.
    pub base_sha: String,
    /// Declared risk, serialized as its stable name.
    pub risk: String,
    /// Exact policy that selected proof requirements.
    pub policy_digest: Digest,
    /// Exact immutable proof declaration.
    pub proof_map_digest: Option<Digest>,
    /// The only permitted stage order.
    pub stages: Vec<PlannedStage>,
    /// The first stage with required checks. Execution is deliberately owned by
    /// the next child; this plan never claims any check has run.
    pub next_permitted_stage: Option<ValidationStage>,
}

/// Projects the validation ladder from frozen record definitions.
///
/// # Errors
///
/// Returns a policy refusal when the proof requirement is absent or malformed,
/// or when a named gate is absent, duplicated across stages, or supplied with
/// a digest that does not match its registered definition.
pub fn plan(
    card: &CardRecord,
    card_digest: Digest,
    policy: &ValidationPolicy,
    policy_digest: Digest,
    registered_gates: &[GateDefinition],
) -> Result<ValidationPlan, HarnessError> {
    let proof_map = card.proof_map.as_ref();
    if policy.requires_proof_map(card.risk) && proof_map.is_none() {
        return Err(HarnessError::Control {
            reason: format!(
                "card {} has `{}` risk, and validation policy {} requires a complete proof_map",
                card.card_id,
                card.risk.name(),
                policy.version
            ),
            code: ErrorCode::PolicyInvalidCard,
        });
    }
    let proof_map_digest = proof_map
        .map(|map| {
            map.validate()?;
            Digest::of_canonical(map)
        })
        .transpose()?;
    // Validate all declarations before selecting the subset the risk profile
    // requires. Otherwise a low-risk card could hide a conflicting duplicate
    // in its non-required handoff list and leave later policy versions with an
    // ambiguous gate position.
    let mut seen = BTreeSet::new();
    for gate_id in card
        .named_gates
        .feature
        .iter()
        .chain(&card.named_gates.review)
        .chain(&card.named_gates.integration)
    {
        if !seen.insert(gate_id.as_str()) {
            return Err(HarnessError::Control {
                reason: format!(
                    "gate `{gate_id}` is declared in more than one validation stage; a check must have one deterministic position"
                ),
                code: ErrorCode::PolicyInvalidCard,
            });
        }
        if !registered_gates.iter().any(|gate| gate.gate_id == *gate_id) {
            return Err(HarnessError::Control {
                reason: format!("gate `{gate_id}` is not registered"),
                code: ErrorCode::ConfigUnknownGate,
            });
        }
    }
    let mut planned = Vec::with_capacity(policy.required_stages(card.risk).len());
    for stage in policy.required_stages(card.risk) {
        let gate_ids = match stage {
            ValidationStage::Narrow => &card.named_gates.feature,
            ValidationStage::Handoff => &card.named_gates.review,
            ValidationStage::FinalIntegration => &card.named_gates.integration,
        };
        if gate_ids.is_empty() {
            return Err(HarnessError::Control {
                reason: format!(
                    "card {} is `{}` risk and validation policy {} requires a `{}` stage with at least one registered gate",
                    card.card_id,
                    card.risk.name(),
                    policy.version,
                    stage.name()
                ),
                code: ErrorCode::PolicyInvalidCard,
            });
        }
        let mut checks = Vec::with_capacity(gate_ids.len());
        for gate_id in gate_ids {
            let gate = registered_gates
                .iter()
                .find(|gate| gate.gate_id == *gate_id)
                .ok_or_else(|| HarnessError::Control {
                    reason: format!("gate `{gate_id}` is not registered"),
                    code: ErrorCode::ConfigUnknownGate,
                })?;
            checks.push(PlannedCheck {
                gate_id: gate.gate_id.clone(),
                gate_digest: gate.digest()?,
                receipt_schema: RECEIPT_SCHEMA.to_owned(),
            });
        }
        planned.push(PlannedStage {
            stage: *stage,
            checks,
        });
    }
    let next_permitted_stage = planned
        .iter()
        .find(|stage| !stage.checks.is_empty())
        .map(|stage| stage.stage);
    Ok(ValidationPlan {
        schema: VALIDATION_PLAN_SCHEMA.to_owned(),
        card_revision: card.revision,
        card_digest,
        base_sha: card.base_sha.clone(),
        risk: card.risk.name().to_owned(),
        policy_digest,
        proof_map_digest,
        stages: planned,
        next_permitted_stage,
    })
}
