//! Typed, redacted operational projection for one captured control commit.

#[path = "project_snapshot_collect.rs"]
mod project_snapshot_collect;
#[path = "project_snapshot_metrics.rs"]
mod project_snapshot_metrics;
#[path = "project_snapshot_observation.rs"]
mod project_snapshot_observation;
#[path = "project_snapshot_render.rs"]
mod project_snapshot_render;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    control::repository::ControlRepository,
    domain::clock::{Clock, Timestamp},
    error::HarnessError,
    runner::receipt::TestResultSummary,
};

/// Stable schema for the operational projection.
pub const PROJECT_SNAPSHOT_SCHEMA: &str = "harness.project-snapshot/v1";

/// One complete, redacted operational snapshot.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProjectSnapshot {
    /// Always [`PROJECT_SNAPSHOT_SCHEMA`].
    pub schema: String,
    /// Project identifier from the captured project document.
    pub project_id: String,
    /// Candidate repository HEAD, when it can be read.
    pub project_head: Option<String>,
    /// Protected authority branch HEAD, when it can be read.
    pub authority_head: Option<String>,
    /// The one control commit used for every durable record below.
    pub control_head: String,
    /// Instant used for all wall-clock ages in this value.
    pub captured_at: Timestamp,
    /// Cycle lifecycle counts.
    pub cycle_state_counts: BTreeMap<String, u64>,
    /// Card lifecycle counts.
    pub card_state_counts: BTreeMap<String, u64>,
    /// Cards currently in an operational phase.
    pub active_cards: Vec<ActiveCardSnapshot>,
    /// Aggregate and per-gate receipt metrics.
    pub gate_metrics: GateMetrics,
    /// Aggregate structured test counts from exact subject-bound receipts.
    pub test_metrics: TestResultSummary,
    /// Existing review-return and repair-attempt facts.
    pub review_metrics: ReviewMetrics,
    /// Readiness and blockers visible from the captured lifecycle records.
    pub integration: IntegrationSummary,
    /// Held leases whose last recorded activity crossed the silence threshold.
    pub silent_leases: Vec<SilentLeaseSnapshot>,
    /// Source boundaries and consistency facts.
    pub consistency: ConsistencyDiagnostics,
}

impl ProjectSnapshot {
    /// Captures the current control HEAD once, then collects from that object.
    ///
    /// # Errors
    ///
    /// Returns an error when the control repository is unavailable, malformed,
    /// or moves while the projection is being collected.
    pub fn collect(control: &ControlRepository, clock: &dyn Clock) -> Result<Self, HarnessError> {
        project_snapshot_collect::collect(control, clock)
    }

    /// Collects from an explicitly captured control HEAD and verifies it did
    /// not move while the read-only projection was assembled.
    ///
    /// # Errors
    ///
    /// Returns an error when a required record is malformed, evidence is
    /// inconsistent, or the control HEAD is no longer the captured commit.
    pub fn collect_at_head(
        control: &ControlRepository,
        control_head: &str,
        clock: &dyn Clock,
    ) -> Result<Self, HarnessError> {
        project_snapshot_collect::collect_at_head(control, control_head, clock)
    }

    /// Concise human rendering of this exact typed value.
    #[must_use]
    pub fn to_text(&self) -> String {
        project_snapshot_render::render(self)
    }
}

/// One card currently occupying an operational lifecycle phase.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct ActiveCardSnapshot {
    pub card_id: String,
    pub cycle_id: String,
    pub phase: String,
    pub actor_id: Option<String>,
    pub age_seconds: i64,
    pub last_activity_at: Option<Timestamp>,
}

/// Metrics derived only from validated structured gate receipts.
#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct GateMetrics {
    pub attempts: u64,
    pub failures: u64,
    pub timeouts: u64,
    pub duration_ms: u64,
    pub by_gate: BTreeMap<String, GateMetric>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct GateMetric {
    pub attempts: u64,
    pub failures: u64,
    pub timeouts: u64,
    pub duration_ms: u64,
}

/// Existing convergence facts, kept separate from gate evidence.
#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct ReviewMetrics {
    pub review_returns: u64,
    pub repair_attempts: u64,
    pub by_card: BTreeMap<String, ReviewMetric>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct ReviewMetric {
    pub review_returns: u64,
    pub repair_attempts: u64,
}

/// Readiness projection without carrying free-form command explanations.
#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct IntegrationSummary {
    pub ready: bool,
    pub ready_card_count: u64,
    pub blockers: Vec<IntegrationBlocker>,
    pub open_integrations: Vec<OpenIntegration>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct IntegrationBlocker {
    pub cycle_id: Option<String>,
    pub card_id: Option<String>,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct OpenIntegration {
    pub integration_id: String,
    pub cycle_id: String,
    pub status: String,
    pub member_count: u64,
}

/// A silent lease with no worktree locator or free-form checkpoint text.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct SilentLeaseSnapshot {
    pub lease_id: String,
    pub card_id: String,
    pub actor_id: String,
    pub granted_at: Timestamp,
    pub last_activity_at: Timestamp,
    pub age_seconds: i64,
}

/// Where each part of the snapshot came from and what could disagree.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct ConsistencyDiagnostics {
    pub authoritative_source: String,
    pub ephemeral_source: String,
    pub control_worktree_clean: bool,
    pub control_head_unchanged: bool,
    pub lock_state: String,
    pub unresolved_journal_operations: u64,
    pub diagnostics: Vec<String>,
}
