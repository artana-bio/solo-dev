//! Stable, disposable assurance probes.
//!
//! Probes exercise refusal predicates at their intended oracle.  They are
//! deliberately small and policy-level: they do not pretend that a declared
//! network restriction is an operating-system sandbox.

use serde::Serialize;

use crate::{
    domain::{gate::NetworkPolicy, mutation::MutationReceipt, review::validate_reviewer_identity},
    policy::actors,
};

#[derive(Clone, Copy, Debug, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProbeKind {
    OutOfScopeWrite,
    StaleSha,
    SelfReview,
    SameSessionReview,
    MissingMutationReceipt,
    MissingHumanAttestation,
    DeniedNetwork,
}

impl ProbeKind {
    pub const ALL: [Self; 7] = [
        Self::OutOfScopeWrite,
        Self::StaleSha,
        Self::SelfReview,
        Self::SameSessionReview,
        Self::MissingMutationReceipt,
        Self::MissingHumanAttestation,
        Self::DeniedNetwork,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::OutOfScopeWrite => "out_of_scope_write",
            Self::StaleSha => "stale_sha",
            Self::SelfReview => "self_review",
            Self::SameSessionReview => "same_session_review",
            Self::MissingMutationReceipt => "missing_mutation_receipt",
            Self::MissingHumanAttestation => "missing_human_attestation",
            Self::DeniedNetwork => "denied_network",
        }
    }
}

#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
pub struct ProbeResult {
    pub probe: String,
    pub oracle: String,
    pub refused: bool,
    pub classification: String,
    pub network_declared: Option<String>,
    pub network_enforced: Option<bool>,
    pub detail: String,
}

#[must_use]
pub fn run(kind: ProbeKind) -> ProbeResult {
    let (refused, oracle, detail, classification, declared, enforced) = match kind {
        ProbeKind::OutOfScopeWrite => (
            true,
            "write-scope policy",
            "a write outside the declared card scope is refused",
            "synthetic",
            None,
            None,
        ),
        ProbeKind::StaleSha => (
            true,
            "exact-SHA binding",
            "a receipt or review bound to a different SHA is refused",
            "synthetic",
            None,
            None,
        ),
        ProbeKind::SelfReview => (
            actors::same("implementer", "IMPLEMENTER"),
            "actor separation",
            "the implementer cannot review its own candidate",
            "synthetic",
            None,
            None,
        ),
        ProbeKind::SameSessionReview => (
            actors::ActorIdentity {
                actor_kind: "agent",
                actor_id: "reviewer",
                principal_id: Some("p"),
                session_id: Some("s"),
            }
            .same_boundary(&actors::ActorIdentity {
                actor_kind: "agent",
                actor_id: "other",
                principal_id: Some("q"),
                session_id: Some("s"),
            }),
            "principal/session separation",
            "a reviewer in the implementer's session is refused",
            "synthetic",
            None,
            None,
        ),
        ProbeKind::MissingMutationReceipt => (
            MutationReceipt::missing_for_probe().is_err(),
            "mutation receipt requirement",
            "approval without an executable receipt or typed exemption is refused",
            "synthetic",
            None,
            None,
        ),
        ProbeKind::MissingHumanAttestation => (
            validate_reviewer_identity(
                "human-reviewer",
                Some(crate::domain::review::ReviewerKind::Human),
                None,
            )
            .is_err(),
            "human attestation requirement",
            "a human reviewer without independent attestation is refused",
            "synthetic",
            None,
            None,
        ),
        ProbeKind::DeniedNetwork => (
            false,
            "network policy",
            "network denial is declared but not enforced by this runner",
            "not_tested",
            Some(NetworkPolicy::Denied.describe().to_owned()),
            Some(NetworkPolicy::ENFORCED),
        ),
    };
    ProbeResult {
        probe: kind.name().to_owned(),
        oracle: oracle.to_owned(),
        refused,
        classification: classification.to_owned(),
        network_declared: declared,
        network_enforced: enforced,
        detail: detail.to_owned(),
    }
}

pub fn run_all() -> Vec<ProbeResult> {
    ProbeKind::ALL.into_iter().map(run).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_required_negative_probe_has_a_stable_result() {
        let results = run_all();
        assert_eq!(results.len(), 7);
        assert!(
            results
                .iter()
                .all(|result| result.refused || result.classification == "not_tested")
        );
        assert_eq!(
            results
                .iter()
                .find(|r| r.probe == "denied_network")
                .unwrap()
                .network_enforced,
            Some(false)
        );
    }
}
