//! First-class evidence for an executable mutation probe.

use serde::{Deserialize, Serialize};

use crate::{
    domain::{clock::Timestamp, digest::Digest},
    error::{ErrorCode, HarnessError},
};

pub const MUTATION_RECEIPT_SCHEMA: &str = "harness.mutation-receipt/v1";
pub const MUTATION_RECEIPT_DIR: &str = "mutation-receipts";

/// A policy-valid reason why a material mutation could not be executed.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MutationExemption {
    pub code: String,
    pub reason: String,
    pub approved_by: String,
}

/// Immutable project-policy facts resolved when an exemption-backed approval
/// is recorded.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MutationExemptionBinding {
    pub policy_digest: Digest,
    pub code: String,
    pub approved_by: String,
    pub approver_principal_id: String,
    pub approver_session_id: String,
}

impl MutationExemption {
    /// # Errors
    ///
    /// Returns a policy error when the exemption is incomplete or self-approved.
    pub fn validate(&self, reviewer_actor_id: &str) -> Result<(), HarnessError> {
        if self.code.trim().is_empty()
            || self.reason.trim().is_empty()
            || self.approved_by.trim().is_empty()
        {
            return Err(invalid(
                "exemption",
                "typed mutation exemptions require code, reason, and approver",
            ));
        }
        if crate::policy::actors::same(&self.approved_by, reviewer_actor_id) {
            return Err(invalid(
                "exemption",
                "the reviewer cannot approve their own mutation exemption",
            ));
        }
        Ok(())
    }
}

/// Immutable evidence that a declared mutation was actually executed.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MutationReceipt {
    pub schema: String,
    pub receipt_id: String,
    pub card_revision: String,
    pub candidate_sha: String,
    pub reviewer_actor_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer_principal_id: Option<String>,
    pub reviewer_session_id: Option<String>,
    pub mutation_digest: Digest,
    pub patch_digest: Digest,
    pub command: Vec<String>,
    pub gate_oracle: String,
    pub expected_failure: String,
    pub observed_result: String,
    pub failed_at_oracle: bool,
    pub restoration_proof: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restoration_sha: Option<String>,
    pub created_at: Timestamp,
    pub exemption: Option<MutationExemption>,
}

/// Immutable receipt facts pinned into a review approval.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MutationReceiptBinding {
    pub receipt_id: String,
    pub receipt_digest: Digest,
    pub card_revision: String,
    pub candidate_sha: String,
    pub reviewer_actor_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer_principal_id: Option<String>,
    pub reviewer_session_id: Option<String>,
    pub gate_oracle: String,
}

impl MutationReceiptBinding {
    /// # Errors
    ///
    /// Returns an error if the receipt cannot be canonically digested.
    pub fn from_receipt(receipt: &MutationReceipt) -> Result<Self, HarnessError> {
        Ok(Self {
            receipt_id: receipt.receipt_id.clone(),
            receipt_digest: Digest::of_canonical(receipt)?,
            card_revision: receipt.card_revision.clone(),
            candidate_sha: receipt.candidate_sha.clone(),
            reviewer_actor_id: receipt.reviewer_actor_id.clone(),
            reviewer_principal_id: receipt.reviewer_principal_id.clone(),
            reviewer_session_id: receipt.reviewer_session_id.clone(),
            gate_oracle: receipt.gate_oracle.clone(),
        })
    }
}

impl MutationReceipt {
    /// # Errors
    ///
    /// Always returns the stable missing-receipt policy refusal.
    pub fn missing_for_probe() -> Result<(), HarnessError> {
        Err(HarnessError::Control {
            reason: "mutation receipt missing".to_owned(),
            code: ErrorCode::PolicyIncompleteReview,
        })
    }
    #[must_use]
    pub fn relative_path(receipt_id: &str) -> String {
        format!("{MUTATION_RECEIPT_DIR}/{receipt_id}.json")
    }
    /// Refuses prose-only or contradictory mutation evidence.
    ///
    /// # Errors
    ///
    /// Returns a policy error when executable bindings, the failing oracle, or
    /// restoration proof is missing.
    pub fn validate(&self) -> Result<(), HarnessError> {
        let required = [
            ("schema", self.schema.as_str()),
            ("receipt_id", self.receipt_id.as_str()),
            ("card_revision", self.card_revision.as_str()),
            ("candidate_sha", self.candidate_sha.as_str()),
            ("reviewer_actor_id", self.reviewer_actor_id.as_str()),
            ("gate_oracle", self.gate_oracle.as_str()),
            ("expected_failure", self.expected_failure.as_str()),
            ("observed_result", self.observed_result.as_str()),
            ("restoration_proof", self.restoration_proof.as_str()),
        ];
        if self.schema != MUTATION_RECEIPT_SCHEMA {
            return Err(invalid("schema", "unsupported mutation receipt schema"));
        }
        if required.iter().any(|(_, value)| value.trim().is_empty()) || self.command.is_empty() {
            return Err(invalid(
                "fields",
                "mutation receipts require executable bindings, not prose alone",
            ));
        }
        if !self.failed_at_oracle {
            return Err(invalid(
                "failed_at_oracle",
                "the mutation did not reach a failing oracle",
            ));
        }
        if self
            .reviewer_principal_id
            .as_deref()
            .is_some_and(|id| id.trim().is_empty())
            || self
                .reviewer_session_id
                .as_deref()
                .is_none_or(|id| id.trim().is_empty())
        {
            return Err(invalid(
                "reviewer_identity",
                "new mutation receipts require a nonblank reviewer session; principal bindings must not be blank",
            ));
        }
        if self
            .restoration_sha
            .as_deref()
            .is_some_and(|sha| sha.trim().is_empty())
        {
            return Err(invalid(
                "restoration_sha",
                "restoration SHA must not be blank",
            ));
        }
        if self.exemption.is_some() {
            return Err(invalid(
                "exemption",
                "an executed mutation receipt cannot also be an exemption",
            ));
        }
        if !self
            .restoration_proof
            .to_ascii_lowercase()
            .contains("restor")
        {
            return Err(invalid(
                "restoration_proof",
                "restoration proof must identify how the mutation was restored",
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.validate().is_ok()
    }
}

fn invalid(field: &str, reason: &str) -> HarnessError {
    HarnessError::Control {
        reason: format!("mutation receipt {field}: {reason}"),
        code: ErrorCode::PolicyIncompleteReview,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::clock::{Clock, FixedClock};

    fn receipt() -> MutationReceipt {
        MutationReceipt {
            schema: MUTATION_RECEIPT_SCHEMA.to_owned(),
            receipt_id: "MR-001".to_owned(),
            card_revision: "F-001-r1".to_owned(),
            candidate_sha: "a".repeat(40),
            reviewer_actor_id: "reviewer".to_owned(),
            reviewer_principal_id: Some("principal".to_owned()),
            reviewer_session_id: Some("session".to_owned()),
            mutation_digest: Digest::of_bytes(b"mutation"),
            patch_digest: Digest::of_bytes(b"patch"),
            command: vec!["cargo".to_owned(), "test".to_owned()],
            gate_oracle: "gate.unit".to_owned(),
            expected_failure: "assertion fails".to_owned(),
            observed_result: "exit 101".to_owned(),
            failed_at_oracle: true,
            restoration_proof: "clean restore".to_owned(),
            restoration_sha: Some("a".repeat(40)),
            created_at: FixedClock::at_unix_seconds(1_785_196_800).unwrap().now(),
            exemption: None,
        }
    }

    #[test]
    fn executable_receipt_requires_the_failing_oracle() {
        assert!(receipt().validate().is_ok());
        let mut invalid = receipt();
        invalid.failed_at_oracle = false;
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn executable_receipt_requires_a_reviewer_session() {
        let mut invalid = receipt();
        invalid.reviewer_session_id = None;
        assert!(invalid.validate().is_err());
    }
}
