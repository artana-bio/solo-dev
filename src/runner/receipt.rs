//! Gate receipts: the structured result of one gate run against one commit.
//!
//! Section 10.6 and invariant 7.4. A receipt names the exact gate definition
//! and the exact evaluated commit, so reusing it after either changes is
//! detectable rather than convenient. Passing and failing attempts are both
//! recorded, because a gate that only passed on its third try is not the same
//! evidence as one that passed on its first.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::domain::{
    clock::Timestamp,
    digest::Digest,
    ids::{CardId, CycleId, IntegrationId, LeaseId, ProjectId, ReceiptId, ValidationReservationId},
};
use crate::error::{ErrorCode, HarnessError};

/// Schema identifier for a receipt.
pub const RECEIPT_SCHEMA: &str = "harness.receipt/v1";

/// Schema identifier for the optional, privacy-safe provenance extension.
///
/// Receipts predate this extension, so the extension intentionally has its
/// own schema instead of changing [`RECEIPT_SCHEMA`].  Old receipts remain
/// readable but cannot become reusable evidence merely because they parse.
pub const RECEIPT_PROVENANCE_SCHEMA: &str = "harness.receipt-provenance/v1";

const REUSE_DIMENSIONS: [ProvenanceDimension; 7] = [
    ProvenanceDimension::Environment,
    ProvenanceDimension::Configuration,
    ProvenanceDimension::Toolchain,
    ProvenanceDimension::Inputs,
    ProvenanceDimension::Fixtures,
    ProvenanceDimension::Cache,
    ProvenanceDimension::TrustMode,
];

/// Directory holding receipts, relative to the control repository.
pub const RECEIPT_DIR: &str = "receipts";

/// Directory holding gate logs, relative to the control repository.
///
/// Logs live outside Git history: they are large, uninteresting when passing,
/// and Section 14.3 gives them retention windows rather than permanence. The
/// receipt records their location and digest, which is what invariant 7.4.2
/// requires.
pub const LOG_DIR: &str = "logs";

/// How a gate process ended.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Termination {
    /// The process exited on its own before the timeout.
    Completed,
    /// The process was still running at its deadline and was terminated.
    Timeout,
    /// The process was killed by a signal.
    Signal,
    /// The runner could not execute or supervise the process.
    RunnerError,
}

/// A bounded, digest-only dimension that can make validation evidence stale.
///
/// The names are deliberately an enum rather than caller-provided strings:
/// accepting arbitrary names would let a producer make a receipt appear
/// complete while quietly omitting a dimension a consumer needs.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceDimension {
    /// Exact candidate or landing state.
    Candidate,
    /// Frozen base state.
    Base,
    /// Activated card definition.
    Card,
    /// Registered gate definition.
    Gate,
    /// Exclusive work assignment.
    Assignment,
    /// Validation policy or proof policy.
    Policy,
    /// Integration definition.
    Integration,
    /// The gate execution environment.
    Environment,
    /// Project and gate configuration.
    Configuration,
    /// Compiler, interpreter, and other toolchain identity.
    Toolchain,
    /// Declared execution inputs.
    Inputs,
    /// Declared fixtures and mocks.
    Fixtures,
    /// Cache policy and compatible cache identity.
    Cache,
    /// Declared trust boundary or execution mode.
    TrustMode,
}

impl ProvenanceDimension {
    /// Stable machine-readable name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Base => "base",
            Self::Card => "card",
            Self::Gate => "gate",
            Self::Assignment => "assignment",
            Self::Policy => "policy",
            Self::Integration => "integration",
            Self::Environment => "environment",
            Self::Configuration => "configuration",
            Self::Toolchain => "toolchain",
            Self::Inputs => "inputs",
            Self::Fixtures => "fixtures",
            Self::Cache => "cache",
            Self::TrustMode => "trust_mode",
        }
    }
}

/// Whether a proof map is bound to the receipt's exact validation policy.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case", tag = "kind", content = "digest")]
pub enum ProofMapBinding {
    /// A card's declared proof map was bound to this run.
    Bound(Digest),
    /// No proof map applies to this receipt's subject.
    NotApplicable,
}

/// The receipt subject repeated in the provenance record.
///
/// This repetition is intentional.  The receipt's top-level fields make old
/// records readable; the typed subject makes a later reuse decision prove all
/// identity bindings in one versioned object.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ProvenanceSubject {
    /// A gate run for one exact activated card and assignment.
    Card {
        /// Candidate commit the gate evaluated.
        candidate_sha: String,
        /// Frozen base for that card.
        base_sha: String,
        /// Card's cycle.
        cycle_id: CycleId,
        /// Card identity.
        card_id: CardId,
        /// Activated card revision.
        card_revision: u32,
        /// Exact activated card definition.
        card_digest: Digest,
        /// Exact lease/assignment identity.
        lease_id: LeaseId,
    },
    /// A combined gate run for one exact integration landing.
    Integration {
        /// Landing commit the gate evaluated.
        landing_sha: String,
        /// Frozen integration baseline.
        base_sha: String,
        /// Integration's cycle.
        cycle_id: CycleId,
        /// Integration identity.
        integration_id: IntegrationId,
        /// Exact substantive integration state.
        integration_digest: Digest,
    },
}

/// Why one new receipt records a relationship to a prior receipt.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptLineageKind {
    /// Same identity, later execution attempt.
    Supersedes,
    /// A declared freshness dimension changed.
    Invalidates,
}

/// One immutable, privacy-safe lineage fact carried by a successor receipt.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReceiptLineageFact {
    /// Whether this is a retry or an invalidation.
    pub kind: ReceiptLineageKind,
    /// Receipt whose evidence is no longer the newest applicable fact.
    pub prior_receipt_id: ReceiptId,
    /// Canonical digest of the prior receipt bytes.
    pub prior_receipt_digest: Digest,
    /// The exact state dimension that is unchanged or changed.
    pub dimension: ProvenanceDimension,
    /// Prior value for an invalidation; equal to `current_digest` for a retry.
    pub prior_digest: Digest,
    /// Current successor value for the named dimension.
    pub current_digest: Digest,
    /// Digest of the declared actor; never the actor's raw name.
    pub actor_digest: Digest,
    /// When this successor recorded the relationship.
    pub recorded_at: Timestamp,
}

/// Versioned, privacy-safe facts needed before a receipt can be reused.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReceiptProvenanceV1 {
    /// Always [`RECEIPT_PROVENANCE_SCHEMA`].
    pub schema: String,
    /// Exact card or integration state evaluated by the receipt.
    pub subject: ProvenanceSubject,
    /// Registered gate definition digest.
    pub gate_definition_digest: Digest,
    /// Canonical digest of the gate argv only; argv itself is never retained.
    pub argv_digest: Digest,
    /// Exact validation policy that selected this check.
    pub policy_digest: Digest,
    /// Proof map binding for this validation subject.
    pub proof_map: ProofMapBinding,
    /// Required privacy-safe context values.  A missing dimension makes the
    /// receipt structurally insufficient for reuse rather than guessed safe.
    pub dimensions: BTreeMap<ProvenanceDimension, Digest>,
    /// Additional ordered dependencies outside the fixed dimensions.
    pub freshness_dependencies: BTreeMap<String, Digest>,
    /// Immutable predecessor relationships observed by this new receipt.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lineage: Vec<ReceiptLineageFact>,
    /// The exact durable reservation that authorized this expensive execution.
    /// It attributes execution only; receipt-compatibility decisions ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation_reservation: Option<ValidationReservationBinding>,
}

/// Immutable execution authorization recorded with one card gate receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationReservationBinding {
    /// Durable reservation identifier.
    pub reservation_id: ValidationReservationId,
    /// Digest of the frozen reservation key the run executed.
    pub key_digest: Digest,
}

impl ReceiptProvenanceV1 {
    /// Validates the schema and all structural boundaries without deciding
    /// whether two separate receipts are compatible.
    ///
    /// # Errors
    ///
    /// Returns `CH-GATE-EVIDENCE-STALE` when the record is malformed or
    /// contradicts its own lineage contract.
    pub fn validate(&self) -> Result<(), HarnessError> {
        if self.schema != RECEIPT_PROVENANCE_SCHEMA {
            return Err(provenance_error(format!(
                "expected provenance schema `{RECEIPT_PROVENANCE_SCHEMA}`, found an unsupported schema"
            )));
        }
        match &self.subject {
            ProvenanceSubject::Card {
                candidate_sha,
                base_sha,
                card_revision,
                ..
            } => {
                validate_commit_sha(candidate_sha, "candidate_sha")?;
                validate_commit_sha(base_sha, "base_sha")?;
                if *card_revision == 0 {
                    return Err(provenance_error("card_revision must begin at 1".to_owned()));
                }
            }
            ProvenanceSubject::Integration {
                landing_sha,
                base_sha,
                ..
            } => {
                validate_commit_sha(landing_sha, "landing_sha")?;
                validate_commit_sha(base_sha, "base_sha")?;
            }
        }
        for key in self.freshness_dependencies.keys() {
            if key.is_empty()
                || key.len() > 64
                || !key.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || byte == b'.'
                        || byte == b'_'
                        || byte == b'-'
                })
            {
                return Err(provenance_error(
                    "freshness dependency names must be bounded stable identifiers".to_owned(),
                ));
            }
        }
        for fact in &self.lineage {
            if matches!(fact.kind, ReceiptLineageKind::Supersedes)
                && fact.prior_digest != fact.current_digest
            {
                return Err(provenance_error(
                    "a supersession fact must retain the same named freshness digest".to_owned(),
                ));
            }
        }
        Ok(())
    }

    /// True only when every privacy-safe fixed dimension was captured.
    ///
    /// This is a structural check, not the compatibility evaluator in #57.
    #[must_use]
    pub fn has_all_reuse_dimensions(&self) -> bool {
        REUSE_DIMENSIONS
            .iter()
            .all(|dimension| self.dimensions.contains_key(dimension))
    }

    /// Canonical digest used by successor lineage facts.
    ///
    /// # Errors
    ///
    /// Returns an error when canonical serialization fails.
    pub fn digest(&self) -> Result<Digest, HarnessError> {
        Digest::of_canonical(self)
    }

    /// Derives one immutable relationship to a prior fully-versioned receipt.
    ///
    /// It does not mutate or validate the prior receipt. If a prior record
    /// predates this extension, it remains readable but cannot be represented
    /// as a trustworthy lineage predecessor.
    ///
    /// # Errors
    ///
    /// Returns an error when canonical digesting of the prior or subject fails.
    pub fn lineage_from_prior(
        &self,
        prior: &Receipt,
        actor_id: &str,
        recorded_at: Timestamp,
    ) -> Result<Option<ReceiptLineageFact>, HarnessError> {
        let Some(previous) = prior.provenance.as_ref() else {
            return Ok(None);
        };
        let actor_digest = Digest::of_bytes(actor_id.as_bytes());
        let prior_receipt_digest = prior.digest()?;
        if let Some((dimension, prior_digest, current_digest)) =
            changed_subject_dimension(&previous.subject, &self.subject)?
        {
            return Ok(Some(ReceiptLineageFact {
                kind: ReceiptLineageKind::Invalidates,
                prior_receipt_id: prior.receipt_id.clone(),
                prior_receipt_digest,
                dimension,
                prior_digest,
                current_digest,
                actor_digest,
                recorded_at,
            }));
        }
        for (name, current) in &self.freshness_dependencies {
            let Some(previous_value) = previous.freshness_dependencies.get(name) else {
                return Ok(Some(ReceiptLineageFact {
                    kind: ReceiptLineageKind::Invalidates,
                    prior_receipt_id: prior.receipt_id.clone(),
                    prior_receipt_digest,
                    dimension: dependency_dimension(name),
                    prior_digest: Digest::of_bytes(b"missing"),
                    current_digest: current.clone(),
                    actor_digest,
                    recorded_at,
                }));
            };
            if previous_value != current {
                return Ok(Some(ReceiptLineageFact {
                    kind: ReceiptLineageKind::Invalidates,
                    prior_receipt_id: prior.receipt_id.clone(),
                    prior_receipt_digest,
                    dimension: dependency_dimension(name),
                    prior_digest: previous_value.clone(),
                    current_digest: current.clone(),
                    actor_digest,
                    recorded_at,
                }));
            }
        }
        let subject_digest = Digest::of_canonical(&self.subject)?;
        Ok(Some(ReceiptLineageFact {
            kind: ReceiptLineageKind::Supersedes,
            prior_receipt_id: prior.receipt_id.clone(),
            prior_receipt_digest,
            dimension: ProvenanceDimension::Candidate,
            prior_digest: subject_digest.clone(),
            current_digest: subject_digest,
            actor_digest,
            recorded_at,
        }))
    }
}

fn changed_subject_dimension(
    prior: &ProvenanceSubject,
    current: &ProvenanceSubject,
) -> Result<Option<(ProvenanceDimension, Digest, Digest)>, HarnessError> {
    let digest_text = |value: &str| Digest::of_bytes(value.as_bytes());
    let changed = |dimension, prior, current| Some((dimension, prior, current));
    match (prior, current) {
        (
            ProvenanceSubject::Card {
                candidate_sha: prior_candidate,
                base_sha: prior_base,
                cycle_id: prior_cycle,
                card_id: prior_card,
                card_revision: prior_revision,
                card_digest: prior_card_digest,
                lease_id: prior_lease,
            },
            ProvenanceSubject::Card {
                candidate_sha: current_candidate,
                base_sha: current_base,
                cycle_id: current_cycle,
                card_id: current_card,
                card_revision: current_revision,
                card_digest: current_card_digest,
                lease_id: current_lease,
            },
        ) => Ok(if prior_candidate != current_candidate {
            changed(
                ProvenanceDimension::Candidate,
                digest_text(prior_candidate),
                digest_text(current_candidate),
            )
        } else if prior_base != current_base {
            changed(
                ProvenanceDimension::Base,
                digest_text(prior_base),
                digest_text(current_base),
            )
        } else if prior_cycle != current_cycle
            || prior_card != current_card
            || prior_revision != current_revision
            || prior_card_digest != current_card_digest
        {
            changed(
                ProvenanceDimension::Card,
                Digest::of_canonical(prior)?,
                Digest::of_canonical(current)?,
            )
        } else if prior_lease != current_lease {
            changed(
                ProvenanceDimension::Assignment,
                Digest::of_canonical(prior_lease)?,
                Digest::of_canonical(current_lease)?,
            )
        } else {
            None
        }),
        (
            ProvenanceSubject::Integration {
                landing_sha: prior_landing,
                base_sha: prior_base,
                cycle_id: prior_cycle,
                integration_id: prior_integration,
                integration_digest: prior_digest,
            },
            ProvenanceSubject::Integration {
                landing_sha: current_landing,
                base_sha: current_base,
                cycle_id: current_cycle,
                integration_id: current_integration,
                integration_digest: current_digest,
            },
        ) => Ok(if prior_landing != current_landing {
            changed(
                ProvenanceDimension::Candidate,
                digest_text(prior_landing),
                digest_text(current_landing),
            )
        } else if prior_base != current_base {
            changed(
                ProvenanceDimension::Base,
                digest_text(prior_base),
                digest_text(current_base),
            )
        } else if prior_cycle != current_cycle
            || prior_integration != current_integration
            || prior_digest != current_digest
        {
            changed(
                ProvenanceDimension::Integration,
                Digest::of_canonical(prior)?,
                Digest::of_canonical(current)?,
            )
        } else {
            None
        }),
        _ => Ok(changed(
            ProvenanceDimension::Candidate,
            Digest::of_canonical(prior)?,
            Digest::of_canonical(current)?,
        )),
    }
}

fn dependency_dimension(name: &str) -> ProvenanceDimension {
    match name {
        "card" => ProvenanceDimension::Card,
        "gate" => ProvenanceDimension::Gate,
        "lease" => ProvenanceDimension::Assignment,
        "policy" => ProvenanceDimension::Policy,
        "integration" => ProvenanceDimension::Integration,
        _ => ProvenanceDimension::Configuration,
    }
}

fn provenance_error(reason: String) -> HarnessError {
    HarnessError::Control {
        reason,
        code: ErrorCode::GateEvidenceStale,
    }
}

fn validate_commit_sha(value: &str, field: &str) -> Result<(), HarnessError> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(provenance_error(format!(
            "{field} must be a 40-character Git commit SHA"
        )));
    }
    Ok(())
}

impl Termination {
    /// Its stable serialized name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Timeout => "timeout",
            Self::Signal => "signal",
            Self::RunnerError => "runner_error",
        }
    }
}

/// One recorded gate run.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    /// Always [`RECEIPT_SCHEMA`].
    pub schema: String,
    /// Identifies this receipt.
    pub receipt_id: ReceiptId,
    /// The project it belongs to.
    pub project_id: ProjectId,
    /// The cycle it belongs to.
    pub cycle_id: CycleId,
    /// The card whose gate ran, when the run was for one card.
    ///
    /// Absent for a combined integration verification: that gate ran against
    /// the landing tree, which belongs to every member card and to none of
    /// them individually. Attributing it to one card would be a false claim
    /// about what was checked. See D-046.
    #[serde(default)]
    pub card_id: Option<CardId>,
    /// The card digest in force, when the run was for one card.
    #[serde(default)]
    pub card_digest: Option<Digest>,
    /// The integration whose landing commit was verified, when it was one.
    #[serde(default)]
    pub integration_id: Option<IntegrationId>,
    /// The exact commit the gate ran against.
    pub evaluated_sha: String,
    /// The gate that ran.
    pub gate_id: String,
    /// The exact gate definition that ran.
    pub gate_digest: Digest,
    /// The harness version that produced the receipt.
    pub harness_version: String,
    /// What the run environment was.
    pub environment_fingerprint: String,
    /// When the run began.
    pub started_at: Timestamp,
    /// When it ended.
    pub finished_at: Timestamp,
    /// How long it took.
    pub duration_ms: u64,
    /// The process exit code, absent when signalled.
    pub exit_code: Option<i32>,
    /// How the process ended.
    pub termination: Termination,
    /// Digest of captured standard output.
    pub stdout_digest: Digest,
    /// Digest of captured standard error.
    pub stderr_digest: Digest,
    /// Digests of declared artifacts that were produced.
    pub artifact_digests: BTreeMap<String, String>,
    /// Where the logs were written.
    pub log_location: PathBuf,
    /// Which attempt this was, starting at 1.
    pub attempt: u32,
    /// True when this attempt satisfied the gate.
    pub passed: bool,
    /// Whether the worktree matched `evaluated_sha` when the gate ran.
    ///
    /// `Some(true)` is the only value that makes `evaluated_sha` mean what it
    /// says. `Some(false)` records a run against content that was not in the
    /// named commit — a normal thing to do while developing, and never
    /// evidence about the commit. `None` is a receipt written before this was
    /// recorded, which can assert neither; those receipts are treated as
    /// non-evidence rather than trusted, because the check they predate is
    /// exactly the one that would have caught the problem.
    #[serde(default)]
    pub worktree_clean: Option<bool>,
    /// Versioned, privacy-safe provenance added after receipt schema v1.
    ///
    /// Omission preserves legacy receipt parsing, but is deliberately
    /// insufficient for compatible evidence reuse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ReceiptProvenanceV1>,
}

impl Receipt {
    /// Canonical digest of this immutable receipt record.
    ///
    /// # Errors
    ///
    /// Returns an error when canonical serialization fails.
    pub fn digest(&self) -> Result<Digest, HarnessError> {
        Digest::of_canonical(self)
    }

    /// Returns complete, structurally valid reuse material, if any.
    ///
    /// It intentionally does not compare two receipts or schedule a rerun;
    /// that decision belongs to #57.  It establishes the strict floor: old,
    /// incomplete, or malformed provenance cannot be reused accidentally.
    ///
    /// # Errors
    ///
    /// Returns `CH-GATE-EVIDENCE-STALE` for legacy, incomplete, malformed, or
    /// contradictory provenance. It deliberately does not compare receipts.
    pub fn reuse_material(&self) -> Result<&ReceiptProvenanceV1, HarnessError> {
        let Some(provenance) = self.provenance.as_ref() else {
            return Err(provenance_error(
                "receipt predates validation provenance and requires a rerun".to_owned(),
            ));
        };
        provenance.validate()?;
        if !provenance.has_all_reuse_dimensions() {
            return Err(provenance_error(
                "receipt omits privacy-safe validation provenance and requires a rerun".to_owned(),
            ));
        }
        match (&self.card_id, &self.integration_id, &provenance.subject) {
            (
                Some(card_id),
                None,
                ProvenanceSubject::Card {
                    candidate_sha,
                    cycle_id,
                    card_id: provenance_card_id,
                    card_digest,
                    ..
                },
            ) if candidate_sha == &self.evaluated_sha
                && cycle_id == &self.cycle_id
                && provenance_card_id == card_id
                && self.card_digest.as_ref() == Some(card_digest) =>
            {
                Ok(provenance)
            }
            (
                None,
                Some(integration_id),
                ProvenanceSubject::Integration {
                    landing_sha,
                    cycle_id,
                    integration_id: provenance_integration_id,
                    ..
                },
            ) if landing_sha == &self.evaluated_sha
                && cycle_id == &self.cycle_id
                && provenance_integration_id == integration_id =>
            {
                Ok(provenance)
            }
            _ => Err(provenance_error(
                "receipt provenance subject disagrees with receipt identity and requires a rerun"
                    .to_owned(),
            )),
        }
    }
    /// What the run was for, as a short human label.
    ///
    /// Every receipt names a subject: a card for a feature gate, an
    /// integration for a combined verification. One of the two is always set.
    #[must_use]
    pub fn subject(&self) -> String {
        match (&self.card_id, &self.integration_id) {
            (Some(card), _) => format!("card {card}"),
            (None, Some(integration)) => format!("integration {integration}"),
            (None, None) => "an unattributed run".to_owned(),
        }
    }

    /// Relative path of a receipt inside the control repository.
    #[must_use]
    pub fn relative_path(receipt_id: &ReceiptId) -> String {
        format!("{RECEIPT_DIR}/{receipt_id}.json")
    }

    /// True when this receipt still describes the given gate and commit.
    ///
    /// Invariant 7.4.3: a receipt for an earlier SHA is stale and cannot be
    /// reused. The gate digest is checked too, because re-running the same
    /// commit under a changed definition is a different check.
    #[must_use]
    pub fn is_current_for(&self, evaluated_sha: &str, gate_digest: &Digest) -> bool {
        self.evaluated_sha == evaluated_sha && self.gate_digest == *gate_digest
    }

    /// Explains why this receipt does not apply, if it does not.
    #[must_use]
    pub fn staleness(&self, evaluated_sha: &str, gate_digest: &Digest) -> Option<String> {
        if self.evaluated_sha != evaluated_sha {
            return Some(format!(
                "receipt evaluated {} but the candidate is now {evaluated_sha}",
                self.evaluated_sha
            ));
        }
        if self.gate_digest != *gate_digest {
            return Some(format!(
                "receipt used gate definition {} but the registry now holds {gate_digest}",
                self.gate_digest
            ));
        }
        match self.worktree_clean {
            Some(true) => None,
            Some(false) => Some(format!(
                "receipt ran against uncommitted content in the worktree, not against {evaluated_sha}"
            )),
            None => Some(
                "receipt predates the worktree-cleanliness check and cannot say what it ran against"
                    .to_owned(),
            ),
        }
    }
}

/// Where one attempt's logs are written.
///
/// Attempt number is part of the path, so a retry never overwrites the evidence
/// of the attempt before it. Section 14.2 requires every attempt to be
/// recorded, and a log that was overwritten is not a record.
#[must_use]
pub fn attempt_log_paths(log_root: &Path, gate_id: &str, attempt: u32) -> (PathBuf, PathBuf) {
    let directory = log_root.join(gate_id).join(format!("attempt-{attempt}"));
    (directory.join("stdout.log"), directory.join("stderr.log"))
}

/// Decides whether a set of attempts constitutes acceptable evidence.
///
/// Section 14.2: a gate that passes only after an undeclared retry remains
/// failed for acceptance. The rule is deliberately strict, because the whole
/// point of a gate is to distinguish "this works" from "this worked once".
#[must_use]
pub fn evidence_is_acceptable(attempts: &[Receipt], max_attempts: u32) -> bool {
    let Some(passing) = attempts
        .iter()
        .find(|receipt| receipt.passed && receipt.worktree_clean == Some(true))
    else {
        return false;
    };
    passing.attempt <= max_attempts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::clock::{Clock as _, FixedClock};

    fn stamp() -> Timestamp {
        FixedClock::at_unix_seconds(1_785_196_800).unwrap().now()
    }

    fn receipt(attempt: u32, passed: bool) -> Receipt {
        Receipt {
            schema: RECEIPT_SCHEMA.to_owned(),
            receipt_id: format!("R-{attempt:06}").parse().unwrap(),
            project_id: "example".parse().unwrap(),
            cycle_id: "C-001".parse().unwrap(),
            card_id: Some("F-001".parse().unwrap()),
            card_digest: Some(Digest::of_bytes(b"card")),
            integration_id: None,
            evaluated_sha: "a".repeat(40),
            gate_id: "gate.unit".to_owned(),
            gate_digest: Digest::of_bytes(b"gate"),
            harness_version: "0.1.0".to_owned(),
            environment_fingerprint: "os=macos".to_owned(),
            started_at: stamp(),
            finished_at: stamp(),
            duration_ms: 120,
            exit_code: Some(i32::from(!passed)),
            termination: Termination::Completed,
            stdout_digest: Digest::of_bytes(b""),
            stderr_digest: Digest::of_bytes(b""),
            artifact_digests: BTreeMap::new(),
            log_location: PathBuf::from("/logs/gate.unit/attempt-1"),
            attempt,
            passed,
            worktree_clean: Some(true),
            provenance: None,
        }
    }

    /// The same receipt, but earned against content that was not committed.
    fn dirty(attempt: u32, passed: bool) -> Receipt {
        Receipt {
            worktree_clean: Some(false),
            ..receipt(attempt, passed)
        }
    }

    fn complete_provenance() -> ReceiptProvenanceV1 {
        ReceiptProvenanceV1 {
            schema: RECEIPT_PROVENANCE_SCHEMA.to_owned(),
            subject: ProvenanceSubject::Card {
                candidate_sha: "a".repeat(40),
                base_sha: "b".repeat(40),
                cycle_id: "C-001".parse().unwrap(),
                card_id: "F-001".parse().unwrap(),
                card_revision: 1,
                card_digest: Digest::of_bytes(b"card"),
                lease_id: "L-000001".parse().unwrap(),
            },
            gate_definition_digest: Digest::of_bytes(b"gate"),
            argv_digest: Digest::of_bytes(b"argv"),
            policy_digest: Digest::of_bytes(b"policy"),
            proof_map: ProofMapBinding::NotApplicable,
            dimensions: BTreeMap::from([
                (ProvenanceDimension::Environment, Digest::of_bytes(b"env")),
                (
                    ProvenanceDimension::Configuration,
                    Digest::of_bytes(b"config"),
                ),
                (
                    ProvenanceDimension::Toolchain,
                    Digest::of_bytes(b"toolchain"),
                ),
                (ProvenanceDimension::Inputs, Digest::of_bytes(b"inputs")),
                (ProvenanceDimension::Fixtures, Digest::of_bytes(b"fixtures")),
                (ProvenanceDimension::Cache, Digest::of_bytes(b"cache")),
                (ProvenanceDimension::TrustMode, Digest::of_bytes(b"trust")),
            ]),
            freshness_dependencies: BTreeMap::from([
                ("card".to_owned(), Digest::of_bytes(b"card")),
                ("policy".to_owned(), Digest::of_bytes(b"policy")),
            ]),
            lineage: Vec::new(),
            validation_reservation: None,
        }
    }

    #[test]
    fn every_termination_has_a_distinct_name() {
        let names: Vec<&str> = [
            Termination::Completed,
            Termination::Timeout,
            Termination::Signal,
            Termination::RunnerError,
        ]
        .iter()
        .map(|termination| termination.name())
        .collect();
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "{names:?}");
    }

    #[test]
    fn a_receipt_applies_only_to_its_exact_commit_and_gate() {
        let record = receipt(1, true);
        let gate = Digest::of_bytes(b"gate");
        assert!(record.is_current_for(&"a".repeat(40), &gate));
        assert!(!record.is_current_for(&"b".repeat(40), &gate));
        assert!(!record.is_current_for(&"a".repeat(40), &Digest::of_bytes(b"other gate")));
    }

    #[test]
    fn staleness_explains_which_binding_broke() {
        let record = receipt(1, true);
        let gate = Digest::of_bytes(b"gate");

        assert!(record.staleness(&"a".repeat(40), &gate).is_none());

        let moved_candidate = record.staleness(&"b".repeat(40), &gate).unwrap();
        assert!(
            moved_candidate.contains("candidate is now"),
            "{moved_candidate}"
        );

        let moved_gate = record
            .staleness(&"a".repeat(40), &Digest::of_bytes(b"other gate"))
            .unwrap();
        assert!(moved_gate.contains("registry now holds"), "{moved_gate}");
    }

    #[test]
    fn a_pass_earned_on_uncommitted_content_is_not_evidence() {
        // Tier 1, defect 1. `evaluated_sha` is only a claim about the commit if
        // the worktree held that commit when the gate ran.
        assert!(evidence_is_acceptable(&[receipt(1, true)], 1));
        assert!(
            !evidence_is_acceptable(&[dirty(1, true)], 1),
            "a gate that passed on content outside the commit says nothing about it"
        );
    }

    #[test]
    fn a_receipt_predating_the_cleanliness_check_is_not_evidence() {
        // `worktree_clean` is `#[serde(default)]` so old receipts still parse,
        // but None means "cannot say", and the check it predates is exactly the
        // one that would have caught the problem. Trusting it would preserve
        // the defect for every receipt written before the fix.
        let legacy = Receipt {
            worktree_clean: None,
            ..receipt(1, true)
        };
        assert!(!evidence_is_acceptable(&[legacy], 1));
    }

    #[test]
    fn a_clean_pass_after_a_dirty_one_is_still_evidence() {
        // The dirty attempt must not poison the gate. Iterating with
        // uncommitted changes and then committing is the ordinary loop, and the
        // committed run is real evidence.
        assert!(evidence_is_acceptable(
            &[dirty(1, true), receipt(2, true)],
            2
        ));
    }

    #[test]
    fn staleness_distinguishes_a_dirty_run_from_a_missing_one() {
        let gate = Digest::of_bytes(b"gate");
        let reason = dirty(1, true).staleness(&"a".repeat(40), &gate).unwrap();
        assert!(reason.contains("uncommitted"), "{reason}");

        let legacy = Receipt {
            worktree_clean: None,
            ..receipt(1, true)
        };
        let reason = legacy.staleness(&"a".repeat(40), &gate).unwrap();
        assert!(reason.contains("predates"), "{reason}");
    }

    #[test]
    fn attempt_logs_never_overwrite_each_other() {
        let root = Path::new("/logs");
        let (first_out, first_err) = attempt_log_paths(root, "gate.unit", 1);
        let (second_out, second_err) = attempt_log_paths(root, "gate.unit", 2);
        assert_ne!(first_out, second_out);
        assert_ne!(first_err, second_err);
        assert_ne!(first_out, first_err);
    }

    #[test]
    fn a_first_attempt_pass_is_acceptable_evidence() {
        assert!(evidence_is_acceptable(&[receipt(1, true)], 1));
    }

    #[test]
    fn no_passing_attempt_is_never_acceptable() {
        assert!(!evidence_is_acceptable(&[receipt(1, false)], 1));
        assert!(!evidence_is_acceptable(
            &[receipt(1, false), receipt(2, false)],
            3
        ));
        assert!(!evidence_is_acceptable(&[], 1));
    }

    #[test]
    fn a_pass_beyond_the_declared_attempts_is_not_acceptable() {
        // The gate declares one attempt. Passing on the second means something
        // ran twice that was only authorized to run once, and a flaky pass is
        // not the same evidence as a deterministic one.
        assert!(!evidence_is_acceptable(
            &[receipt(1, false), receipt(2, true)],
            1
        ));
    }

    #[test]
    fn a_pass_within_a_declared_retry_budget_is_acceptable() {
        assert!(evidence_is_acceptable(
            &[receipt(1, false), receipt(2, true)],
            2
        ));
    }

    #[test]
    fn a_receipt_round_trips_and_rejects_unknown_fields() {
        let record = receipt(1, true);
        let encoded = serde_json::to_string_pretty(&record).unwrap();
        let decoded: Receipt = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, record);

        let mut value = serde_json::to_value(&record).unwrap();
        value["surprise"] = serde_json::json!(1);
        assert!(serde_json::from_value::<Receipt>(value).is_err());
    }

    #[test]
    fn a_legacy_receipt_is_readable_but_cannot_supply_reuse_material() {
        let legacy = receipt(1, true);
        let encoded = serde_json::to_string(&legacy).unwrap();
        let decoded: Receipt = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.provenance, None);
        let error = decoded.reuse_material().unwrap_err();
        assert_eq!(error.code(), ErrorCode::GateEvidenceStale);
        assert!(error.to_string().contains("requires a rerun"));
    }

    #[test]
    fn complete_safe_provenance_is_reuse_material_for_its_exact_card_subject() {
        let mut record = receipt(1, true);
        record.provenance = Some(complete_provenance());
        assert!(record.reuse_material().is_ok());

        let digest = record.digest().unwrap();
        let encoded = serde_json::to_string(&record).unwrap();
        let decoded: Receipt = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.digest().unwrap(), digest);
    }

    #[test]
    fn missing_safe_dimension_or_raw_sensitive_field_refuses_reuse_material() {
        let mut incomplete = receipt(1, true);
        let mut provenance = complete_provenance();
        provenance.dimensions.remove(&ProvenanceDimension::Inputs);
        incomplete.provenance = Some(provenance);
        let error = incomplete.reuse_material().unwrap_err();
        assert_eq!(error.code(), ErrorCode::GateEvidenceStale);

        // The provenance schema has no raw input slot. `deny_unknown_fields`
        // makes an attempted raw fixture/input a parse refusal, rather than a
        // quietly retained value that a future audit could expose.
        let mut value = serde_json::to_value(complete_provenance()).unwrap();
        value["raw_input"] = serde_json::json!("secret-value-must-not-persist");
        assert!(serde_json::from_value::<ReceiptProvenanceV1>(value).is_err());
    }

    #[test]
    fn provenance_rejects_a_contradictory_subject_and_malformed_sha() {
        let mut wrong_subject = receipt(1, true);
        let mut provenance = complete_provenance();
        if let ProvenanceSubject::Card { card_id, .. } = &mut provenance.subject {
            *card_id = "F-999".parse().unwrap();
        }
        wrong_subject.provenance = Some(provenance);
        assert_eq!(
            wrong_subject.reuse_material().unwrap_err().code(),
            ErrorCode::GateEvidenceStale
        );

        let mut malformed = complete_provenance();
        if let ProvenanceSubject::Card { candidate_sha, .. } = &mut malformed.subject {
            *candidate_sha = "not-a-commit".to_owned();
        }
        assert_eq!(
            malformed.validate().unwrap_err().code(),
            ErrorCode::GateEvidenceStale
        );
    }

    #[test]
    fn provenance_and_lineage_are_deterministic_and_append_only() {
        let first = complete_provenance();
        let mut second = ReceiptProvenanceV1 {
            dimensions: BTreeMap::new(),
            freshness_dependencies: BTreeMap::new(),
            ..first.clone()
        };
        for (dimension, digest) in first.dimensions.iter().rev() {
            second.dimensions.insert(*dimension, digest.clone());
        }
        for (name, digest) in first.freshness_dependencies.iter().rev() {
            second
                .freshness_dependencies
                .insert(name.clone(), digest.clone());
        }
        assert_eq!(first.digest().unwrap(), second.digest().unwrap());

        let prior = receipt(1, true);
        let mut successor = receipt(2, true);
        let mut provenance = complete_provenance();
        provenance.lineage.push(ReceiptLineageFact {
            kind: ReceiptLineageKind::Supersedes,
            prior_receipt_id: prior.receipt_id.clone(),
            prior_receipt_digest: prior.digest().unwrap(),
            dimension: ProvenanceDimension::Configuration,
            prior_digest: Digest::of_bytes(b"same-config"),
            current_digest: Digest::of_bytes(b"same-config"),
            actor_digest: Digest::of_bytes(b"actor"),
            recorded_at: stamp(),
        });
        successor.provenance = Some(provenance);
        assert!(successor.reuse_material().is_ok());
        assert_eq!(
            prior.provenance, None,
            "a successor never rewrites its prior receipt"
        );

        let mut invalid = complete_provenance();
        invalid.lineage.push(ReceiptLineageFact {
            kind: ReceiptLineageKind::Supersedes,
            prior_receipt_id: "R-000001".parse().unwrap(),
            prior_receipt_digest: Digest::of_bytes(b"prior"),
            dimension: ProvenanceDimension::Inputs,
            prior_digest: Digest::of_bytes(b"old"),
            current_digest: Digest::of_bytes(b"new"),
            actor_digest: Digest::of_bytes(b"actor"),
            recorded_at: stamp(),
        });
        assert_eq!(
            invalid.validate().unwrap_err().code(),
            ErrorCode::GateEvidenceStale
        );
    }

    #[test]
    fn successor_lineage_distinguishes_retry_from_changed_freshness() {
        let mut prior = receipt(1, true);
        prior.provenance = Some(complete_provenance());
        let retry = complete_provenance()
            .lineage_from_prior(&prior, "reviewer", stamp())
            .unwrap()
            .unwrap();
        assert_eq!(retry.kind, ReceiptLineageKind::Supersedes);
        assert_eq!(retry.dimension, ProvenanceDimension::Candidate);

        let mut changed = complete_provenance();
        changed
            .freshness_dependencies
            .insert("policy".to_owned(), Digest::of_bytes(b"new-policy"));
        let invalidation = changed
            .lineage_from_prior(&prior, "reviewer", stamp())
            .unwrap()
            .unwrap();
        assert_eq!(invalidation.kind, ReceiptLineageKind::Invalidates);
        assert_eq!(invalidation.dimension, ProvenanceDimension::Policy);
        assert_ne!(
            invalidation.prior_digest, invalidation.current_digest,
            "an invalidation must identify a material state change"
        );

        let mut revised = complete_provenance();
        if let ProvenanceSubject::Card {
            card_revision,
            card_digest,
            ..
        } = &mut revised.subject
        {
            *card_revision = 2;
            *card_digest = Digest::of_bytes(b"revised-card");
        }
        let card_change = revised
            .lineage_from_prior(&prior, "reviewer", stamp())
            .unwrap()
            .unwrap();
        assert_eq!(card_change.kind, ReceiptLineageKind::Invalidates);
        assert_eq!(
            card_change.dimension,
            ProvenanceDimension::Card,
            "a card revision is not a candidate-SHA change"
        );
    }

    #[test]
    fn a_receipt_records_a_failing_attempt_too() {
        // Invariant 7.4.1: passing and failing attempts are both recorded.
        let failed = receipt(1, false);
        assert!(!failed.passed);
        assert_eq!(failed.exit_code, Some(1));
        let encoded = serde_json::to_string(&failed).unwrap();
        assert!(encoded.contains("\"passed\":false"));
    }
}
