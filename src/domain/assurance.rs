//! Stable, disposable assurance probes.
//!
//! Probes exercise refusal predicates at their intended oracle.  They are
//! deliberately small and policy-level: they do not pretend that a declared
//! network restriction is an operating-system sandbox.

use serde::Serialize;

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
    pub run_id: String,
    pub probe_id: String,
    pub probe: String,
    pub oracle: String,
    pub expected_error_code: Option<String>,
    pub observed_error_code: Option<String>,
    pub command_path: String,
    pub refused: bool,
    pub classification: String,
    pub network_declared: Option<String>,
    pub network_enforced: Option<bool>,
    pub state_change_evidence: String,
    pub cleanup_completed: bool,
    pub detail: String,
}
