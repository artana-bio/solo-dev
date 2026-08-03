//! Durable, exact-key reservations for expensive validation.

use serde::{Deserialize, Serialize};

use crate::{
    config::ValidationStage,
    domain::{
        clock::Timestamp,
        digest::Digest,
        ids::{CardId, CycleId, LeaseId, ValidationReservationId},
    },
    policy::progressive_validation::PlannedCheck,
};

/// Directory holding immutable validation reservations.
pub const VALIDATION_RESERVATION_DIR: &str = "validation-reservations";
/// Schema for a reservation record.
pub const VALIDATION_RESERVATION_SCHEMA: &str = "harness.validation-reservation/v1";
/// Schema for an exact reservation key.
pub const VALIDATION_RESERVATION_KEY_SCHEMA: &str = "harness.validation-reservation-key/v1";
/// Directory holding immutable terminal outcomes for reservations.
pub const VALIDATION_RESERVATION_SETTLEMENT_DIR: &str = "validation-reservation-settlements";
/// Schema for a terminal reservation settlement.
pub const VALIDATION_RESERVATION_SETTLEMENT_SCHEMA: &str =
    "harness.validation-reservation-settlement/v1";

/// The only execution mode this first reservation slice permits.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ValidationExecutionMode {
    /// A registered named gate; execution itself remains a later slice.
    NamedGate,
    /// A fixed, reviewed campaign of reversible source mutations.
    DeclaredMutations,
}

impl std::str::FromStr for ValidationExecutionMode {
    type Err = crate::error::HarnessError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "named-gate" => Ok(Self::NamedGate),
            "declared-mutations" => Ok(Self::DeclaredMutations),
            _ => Err(crate::error::HarnessError::Control {
                reason: format!("unsupported validation execution mode `{value}`"),
                code: crate::error::ErrorCode::UsageInvalidArguments,
            }),
        }
    }
}

/// Every authoritative input that determines whether two requests may share a run.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ValidationReservationKeyV1 {
    pub schema: String,
    pub card_id: CardId,
    pub cycle_id: CycleId,
    pub card_revision: u32,
    pub card_digest: Digest,
    pub lease_id: LeaseId,
    pub candidate_sha: String,
    pub base_sha: String,
    pub stage: ValidationStage,
    pub check: PlannedCheck,
    pub policy_digest: Digest,
    pub proof_map_digest: Option<Digest>,
    pub execution_mode: ValidationExecutionMode,
    /// Canonical digest of the declared mutation campaign, when this is a
    /// mutation reservation. It is part of the sharing identity.
    pub campaign_digest: Option<Digest>,
}

impl ValidationReservationKeyV1 {
    /// Canonical key digest used for equality and audit output.
    ///
    /// # Errors
    ///
    /// Returns an error when canonical serialization fails.
    pub fn digest(&self) -> Result<Digest, crate::error::HarnessError> {
        Digest::of_canonical(self)
    }
}

/// One immutable winner decision. It is not a receipt or execution permit.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ValidationReservationRecord {
    pub schema: String,
    pub reservation_id: ValidationReservationId,
    pub key: ValidationReservationKeyV1,
    pub key_digest: Digest,
    pub holder_actor_id: String,
    pub reserved_at: Timestamp,
    pub expires_at: Timestamp,
    pub recovery_policy: String,
}

impl ValidationReservationRecord {
    #[must_use]
    pub fn relative_path(reservation_id: &ValidationReservationId) -> String {
        format!("{VALIDATION_RESERVATION_DIR}/{reservation_id}.json")
    }
}

/// The terminal outcome recorded for one reservation.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ValidationReservationOutcome {
    /// One exact receipt was recorded; whether it is reusable remains #31's decision.
    ReceiptRecorded {
        receipt_id: String,
        receipt_digest: Digest,
    },
    /// The reserved execution completed unsuccessfully.
    Failed,
    /// An operator deliberately stopped the reserved execution.
    Abandoned,
}

/// The only terminal fact for one immutable reservation.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ValidationReservationSettlementRecord {
    pub schema: String,
    pub reservation_id: ValidationReservationId,
    pub reservation_key_digest: Digest,
    pub holder_actor_id: String,
    pub settled_by_actor_id: String,
    pub settled_at: Timestamp,
    pub outcome: ValidationReservationOutcome,
}

impl ValidationReservationSettlementRecord {
    #[must_use]
    pub fn relative_path(reservation_id: &ValidationReservationId) -> String {
        format!("{VALIDATION_RESERVATION_SETTLEMENT_DIR}/{reservation_id}.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> ValidationReservationKeyV1 {
        ValidationReservationKeyV1 {
            schema: VALIDATION_RESERVATION_KEY_SCHEMA.to_owned(),
            card_id: "F-001".parse().unwrap(),
            cycle_id: "C-001".parse().unwrap(),
            card_revision: 1,
            card_digest: Digest::of_bytes(b"card"),
            lease_id: "L-000001".parse().unwrap(),
            candidate_sha: "a".repeat(40),
            base_sha: "b".repeat(40),
            stage: ValidationStage::Narrow,
            check: PlannedCheck {
                gate_id: "gate.unit".to_owned(),
                gate_digest: Digest::of_bytes(b"gate"),
                receipt_schema: "harness.gate-receipt/v1".to_owned(),
                max_attempts: 1,
            },
            policy_digest: Digest::of_bytes(b"policy-a"),
            proof_map_digest: None,
            execution_mode: ValidationExecutionMode::NamedGate,
            campaign_digest: None,
        }
    }

    #[test]
    fn policy_digest_is_part_of_the_exact_reservation_key() {
        let first = key();
        let mut changed = first.clone();
        changed.policy_digest = Digest::of_bytes(b"policy-b");
        assert_ne!(first.digest().unwrap(), changed.digest().unwrap());
    }

    #[test]
    fn absent_and_bound_proof_maps_have_distinct_keys() {
        let first = key();
        let mut changed = first.clone();
        changed.proof_map_digest = Some(Digest::of_bytes(b"proof-map"));
        assert_ne!(first.digest().unwrap(), changed.digest().unwrap());
    }
}
