//! Typed, redacted operational projection for one captured control commit.
//!
//! Durable records are read with `git show <captured-head>:<path>`, never from
//! the mutable control worktree. The lock and journal are intentionally kept
//! as a small ephemeral overlay because they describe work in flight rather
//! than authoritative history.

use std::{collections::BTreeMap, fmt::Write as _};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    config::ProjectConfig,
    control::{
        event_store::{EVENT_DIR, Event},
        journal::Journal,
        lock::{LockDiagnosis, ProjectLock},
        repository::ControlRepository,
    },
    domain::{
        card::{CARD_DIR, CardRecord, CardState},
        clock::{Clock, Timestamp},
        cycle::{CYCLE_DIR, CycleRecord, status_from_events},
        digest::Digest,
        handoff::{HANDOFF_DIR, HandoffRecord, HandoffStatus},
        integration::{INTEGRATION_DIR, IntegrationRecord},
        lease::{LEASE_DIR, LeaseRecord},
        review::{Decision, REVIEW_DIR, ReviewRecord},
    },
    error::{ErrorCode, HarnessError},
    git::{
        authority::inspect_authority,
        command::{GitScope, run},
    },
    runner::receipt::{RECEIPT_DIR, Receipt, Termination},
};

/// Stable schema for the operational projection.
pub const PROJECT_SNAPSHOT_SCHEMA: &str = "harness.project-snapshot/v1";

const CYCLE_STATE_NAMES: &[&str] = &[
    "draft",
    "active",
    "sealed",
    "integrating",
    "accepted",
    "landed",
    "closed",
    "blocked",
    "abandoned",
];
const CARD_STATE_NAMES: &[&str] = &[
    "draft",
    "ready",
    "leased",
    "active",
    "handed_off",
    "review_pending",
    "changes_requested",
    "approved",
    "integrating",
    "accepted",
    "landed",
    "closed",
    "blocked",
    "abandoned",
];

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
        let head = control.head()?.ok_or_else(|| HarnessError::Control {
            reason: "control repository has no commit to snapshot".to_owned(),
            code: ErrorCode::InternalControlCorrupt,
        })?;
        Self::collect_at_head(control, &head, clock)
    }

    /// Collects from an explicitly captured control HEAD and verifies it did
    /// not move while the read-only projection was assembled.
    ///
    /// # Errors
    ///
    /// Returns an error when a required record is malformed or the control HEAD
    /// is no longer the captured commit.
    pub fn collect_at_head(
        control: &ControlRepository,
        control_head: &str,
        clock: &dyn Clock,
    ) -> Result<Self, HarnessError> {
        let config: ProjectConfig = read_json_at(control, control_head, "project/project.json")?;
        let captured_at = clock.now();
        let mut diagnostics = Vec::new();

        let project_head = candidate_head(&config, &mut diagnostics);
        let authority_head = authority_head(&config, &mut diagnostics);
        let cycles: Vec<CycleRecord> = read_json_files(control, control_head, CYCLE_DIR)?;
        let events: Vec<Event> = read_json_files(control, control_head, EVENT_DIR)?;
        let (cards, card_states) = read_cards(control, control_head)?;
        let receipts: Vec<Receipt> = read_json_files(control, control_head, RECEIPT_DIR)?;
        let reviews: Vec<crate::domain::review::ReviewRecord> =
            read_json_files(control, control_head, REVIEW_DIR)?;
        let handoffs: Vec<HandoffRecord> = read_json_files(control, control_head, HANDOFF_DIR)?;
        let integrations: Vec<IntegrationRecord> =
            read_json_files(control, control_head, INTEGRATION_DIR)?;
        let leases: Vec<LeaseRecord> = read_json_files(control, control_head, LEASE_DIR)?;

        let records = SnapshotRecords {
            cycles: &cycles,
            cards: &cards,
            states: &card_states,
            events: &events,
            receipts: &receipts,
            reviews: &reviews,
            handoffs: &handoffs,
            integrations: &integrations,
            leases: &leases,
        };
        validate_subjects(&config, &records, &mut diagnostics);

        let now = captured_at;
        let (cycle_state_counts, cycle_status_diagnostics) = cycle_counts(&cycles, &events);
        diagnostics.extend(cycle_status_diagnostics);
        let card_state_counts = card_counts(&card_states);
        let active_cards = active_cards(&cards, &card_states, &events, &leases, now);
        let gate_metrics = gate_metrics(&receipts);
        let review_metrics = review_metrics(&events);
        let integration = integration_summary(
            &cycles,
            &cards,
            &card_states,
            &integrations,
            &handoffs,
            &reviews,
            &leases,
        );
        let silent_leases = leases
            .iter()
            .filter(|lease| lease.is_silent(now))
            .map(|lease| SilentLeaseSnapshot {
                lease_id: lease.lease_id.to_string(),
                card_id: lease.card_id.to_string(),
                actor_id: lease.actor_id.clone(),
                granted_at: lease.granted_at,
                last_activity_at: lease.last_activity_at(),
                age_seconds: age_seconds(now, lease.last_activity_at()),
            })
            .collect();

        let control_worktree_clean = control.is_clean()?;
        if !control_worktree_clean {
            diagnostics.push("control_worktree_dirty_authoritative_head_used".to_owned());
        }
        let journal = Journal::new(control);
        let unresolved_journal_operations = journal.unresolved()?.len() as u64;
        if unresolved_journal_operations > 0 {
            diagnostics.push("unresolved_journal_operations_are_ephemeral".to_owned());
        }
        let lock_diagnosis = ProjectLock::diagnose(control.root());
        let lock_state = lock_state(&lock_diagnosis);
        if lock_state != "free" {
            diagnostics.push("ephemeral_lock_observed".to_owned());
        }

        let current_head = control.head()?;
        if current_head.as_deref() != Some(control_head) {
            return Err(HarnessError::Control {
                reason: "control head moved while collecting project snapshot".to_owned(),
                code: ErrorCode::ConflictControlHeadMoved,
            });
        }

        Ok(Self {
            schema: PROJECT_SNAPSHOT_SCHEMA.to_owned(),
            project_id: config.project_id.to_string(),
            project_head,
            authority_head,
            control_head: control_head.to_owned(),
            captured_at,
            cycle_state_counts,
            card_state_counts,
            active_cards,
            gate_metrics,
            review_metrics,
            integration,
            silent_leases,
            consistency: ConsistencyDiagnostics {
                authoritative_source: "control_git_object".to_owned(),
                ephemeral_source: "control_worktree_overlay".to_owned(),
                control_worktree_clean,
                control_head_unchanged: true,
                lock_state,
                unresolved_journal_operations,
                diagnostics,
            },
        })
    }

    /// Concise human rendering of this exact typed value.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut text = format!(
            "Project {} snapshot\ncontrol head: {}\nproject head: {}\nauthority head: {}",
            self.project_id,
            self.control_head,
            self.project_head.as_deref().unwrap_or("unavailable"),
            self.authority_head.as_deref().unwrap_or("unavailable"),
        );
        append_counts(&mut text, "cycles", &self.cycle_state_counts);
        append_counts(&mut text, "cards", &self.card_state_counts);
        let _ = write!(
            text,
            "\nactive cards: {}\ngates: {} attempts, {} failures, {} timeouts, {}ms\nreviews: {} returns, {} repair attempts\nintegration: {} ready card(s), {} blocker(s)\nsilent leases: {}",
            self.active_cards.len(),
            self.gate_metrics.attempts,
            self.gate_metrics.failures,
            self.gate_metrics.timeouts,
            self.gate_metrics.duration_ms,
            self.review_metrics.review_returns,
            self.review_metrics.repair_attempts,
            self.integration.ready_card_count,
            self.integration.blockers.len(),
            self.silent_leases.len(),
        );
        for card in &self.active_cards {
            let _ = write!(
                text,
                "\n  {} {} actor={} age={}s last_activity={}",
                card.card_id,
                card.phase,
                card.actor_id.as_deref().unwrap_or("unassigned"),
                card.age_seconds,
                card.last_activity_at
                    .as_ref()
                    .map_or_else(|| "unavailable".to_owned(), Timestamp::to_rfc3339),
            );
        }
        let _ = write!(
            text,
            "\nconsistency: source={} ephemeral={} worktree={} lock={} journal={}",
            self.consistency.authoritative_source,
            self.consistency.ephemeral_source,
            if self.consistency.control_worktree_clean {
                "clean"
            } else {
                "dirty"
            },
            self.consistency.lock_state,
            self.consistency.unresolved_journal_operations,
        );
        text
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

/// Metrics derived only from structured gate receipts.
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

#[derive(Clone, Debug, Deserialize)]
struct StoredCardState {
    #[allow(dead_code)]
    schema: String,
    card_id: crate::domain::ids::CardId,
    state: CardState,
    current_revision: u32,
    #[allow(dead_code)]
    current_digest: Digest,
    #[allow(dead_code)]
    canonical_algorithm: String,
}

fn read_json_at<T: DeserializeOwned>(
    control: &ControlRepository,
    head: &str,
    relative: &str,
) -> Result<T, HarnessError> {
    let object = format!("{head}:{relative}");
    let output = run(&control.scope(), ["show", object.as_str()])?;
    if !output.success() {
        return Err(HarnessError::Control {
            reason: "required control record is missing from the captured commit".to_owned(),
            code: ErrorCode::InternalControlCorrupt,
        });
    }
    serde_json::from_slice(&output.stdout_bytes).map_err(|_| HarnessError::Control {
        reason: "a control record in the captured commit is malformed".to_owned(),
        code: ErrorCode::InternalControlCorrupt,
    })
}

fn read_json_files<T: DeserializeOwned>(
    control: &ControlRepository,
    head: &str,
    prefix: &str,
) -> Result<Vec<T>, HarnessError> {
    let object_names = list_files(control, head, prefix)?;
    object_names
        .iter()
        .filter(|name| {
            std::path::Path::new(name)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        })
        .map(|name| read_json_at(control, head, name))
        .collect()
}

fn list_files(
    control: &ControlRepository,
    head: &str,
    prefix: &str,
) -> Result<Vec<String>, HarnessError> {
    let output = run(
        &control.scope(),
        ["ls-tree", "-r", "--name-only", head, "--", prefix],
    )?;
    if !output.success() {
        return Err(HarnessError::Control {
            reason: "could not enumerate the captured control commit".to_owned(),
            code: ErrorCode::InternalControlCorrupt,
        });
    }
    Ok(output.trimmed_stdout().lines().map(str::to_owned).collect())
}

fn read_cards(
    control: &ControlRepository,
    head: &str,
) -> Result<(Vec<CardRecord>, Vec<StoredCardState>), HarnessError> {
    let mut cards = Vec::new();
    let mut states = Vec::new();
    for relative in list_files(control, head, CARD_DIR)? {
        if !std::path::Path::new(&relative)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        {
            continue;
        }
        if relative.ends_with("/state.json") {
            states.push(read_json_at(control, head, &relative)?);
        } else if relative.contains("/r") {
            cards.push(read_json_at(control, head, &relative)?);
        }
    }
    Ok((cards, states))
}

fn candidate_head(config: &ProjectConfig, diagnostics: &mut Vec<String>) -> Option<String> {
    let output = run(
        &GitScope::work_tree(&config.repository),
        ["rev-parse", "--verify", "HEAD"],
    )
    .ok()?;
    if output.success() {
        Some(output.trimmed_stdout().to_owned())
    } else {
        diagnostics.push("project_head_unavailable".to_owned());
        None
    }
}

fn authority_head(config: &ProjectConfig, diagnostics: &mut Vec<String>) -> Option<String> {
    if let Ok(state) = inspect_authority(&config.authority_repository, &config.protected_branch) {
        state.protected_sha
    } else {
        diagnostics.push("authority_head_unavailable".to_owned());
        None
    }
}

fn card_counts(states: &[StoredCardState]) -> BTreeMap<String, u64> {
    let mut counts = zeroed_counts(CARD_STATE_NAMES);
    for state in states {
        *counts.entry(state.state.name().to_owned()).or_default() += 1;
    }
    counts
}

fn cycle_counts(cycles: &[CycleRecord], events: &[Event]) -> (BTreeMap<String, u64>, Vec<String>) {
    let mut counts = zeroed_counts(CYCLE_STATE_NAMES);
    let mut diagnostics = Vec::new();
    for cycle in cycles {
        let event_states = events
            .iter()
            .filter(|event| {
                event.cycle_id.as_ref() == Some(&cycle.cycle_id)
                    && event.card_id.is_none()
                    && event.event_type.starts_with("cycle.")
            })
            .filter_map(|event| event.next_state.as_deref());
        let derived = status_from_events(event_states);
        if derived != cycle.status {
            diagnostics.push("cycle_stored_state_disagrees_with_events".to_owned());
        }
        *counts.entry(derived.name().to_owned()).or_default() += 1;
    }
    (counts, diagnostics)
}

fn active_cards(
    cards: &[CardRecord],
    states: &[StoredCardState],
    events: &[Event],
    leases: &[LeaseRecord],
    now: Timestamp,
) -> Vec<ActiveCardSnapshot> {
    let card_records: BTreeMap<_, _> = cards
        .iter()
        .map(|card| ((card.card_id.clone(), card.revision), card))
        .collect();
    let mut snapshots = Vec::new();
    for state in states {
        if matches!(
            state.state,
            CardState::Draft | CardState::Ready | CardState::Closed | CardState::Abandoned
        ) {
            continue;
        }
        let Some(card) = card_records.get(&(state.card_id.clone(), state.current_revision)) else {
            continue;
        };
        let lease = leases
            .iter()
            .find(|lease| lease.card_id == state.card_id && lease.is_held());
        let latest_event = events
            .iter()
            .filter(|event| event.card_id.as_ref() == Some(&state.card_id))
            .max_by_key(|event| event.occurred_at);
        let lease_activity = lease.map(LeaseRecord::last_activity_at);
        let event_activity = latest_event.map(|event| event.occurred_at);
        let last_activity_at = [lease_activity, event_activity].into_iter().flatten().max();
        let actor_id = lease
            .map(|lease| lease.actor_id.clone())
            .or_else(|| latest_event.map(|event| event.actor_id.clone()));
        let phase_started_at = events
            .iter()
            .filter(|event| {
                event.card_id.as_ref() == Some(&state.card_id)
                    && event.next_state.as_deref() == Some(state.state.name())
            })
            .max_by_key(|event| event.occurred_at)
            .map(|event| event.occurred_at)
            .or(last_activity_at)
            .or(Some(card.created_at));
        snapshots.push(ActiveCardSnapshot {
            card_id: state.card_id.to_string(),
            cycle_id: card.cycle_id.to_string(),
            phase: state.state.name().to_owned(),
            actor_id,
            age_seconds: phase_started_at.map_or(0, |at| age_seconds(now, at)),
            last_activity_at,
        });
    }
    snapshots.sort_by(|left, right| left.card_id.cmp(&right.card_id));
    snapshots
}

fn gate_metrics(receipts: &[Receipt]) -> GateMetrics {
    let mut metrics = GateMetrics::default();
    for receipt in receipts {
        metrics.attempts += 1;
        metrics.duration_ms = metrics.duration_ms.saturating_add(receipt.duration_ms);
        if !receipt.passed {
            metrics.failures += 1;
        }
        if receipt.termination == Termination::Timeout {
            metrics.timeouts += 1;
        }
        let gate = metrics.by_gate.entry(receipt.gate_id.clone()).or_default();
        gate.attempts += 1;
        gate.duration_ms = gate.duration_ms.saturating_add(receipt.duration_ms);
        if !receipt.passed {
            gate.failures += 1;
        }
        if receipt.termination == Termination::Timeout {
            gate.timeouts += 1;
        }
    }
    metrics
}

fn review_metrics(events: &[Event]) -> ReviewMetrics {
    let mut metrics = ReviewMetrics::default();
    for event in events {
        let Some(card_id) = event.card_id.as_ref() else {
            continue;
        };
        let Some(kind) = event
            .metadata
            .get("attempt_kind")
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let card = metrics.by_card.entry(card_id.to_string()).or_default();
        match kind {
            "review_return" => {
                metrics.review_returns += 1;
                card.review_returns += 1;
            }
            "repair_attempt" => {
                metrics.repair_attempts += 1;
                card.repair_attempts += 1;
            }
            _ => {}
        }
    }
    metrics
}

fn current_candidate(
    card_id: &crate::domain::ids::CardId,
    handoff: Option<&HandoffRecord>,
    leases: &[LeaseRecord],
) -> Option<String> {
    let lease = leases
        .iter()
        .find(|lease| lease.card_id == *card_id && lease.is_held());
    if let Some(lease) = lease {
        let output = run(
            &GitScope::work_tree(&lease.worktree_path),
            ["rev-parse", "--verify", "HEAD"],
        )
        .ok()?;
        return output.success().then(|| output.trimmed_stdout().to_owned());
    }
    handoff.map(|handoff| handoff.candidate_sha.clone())
}

fn approved_card_blocker(
    card_id: &crate::domain::ids::CardId,
    state: &StoredCardState,
    handoffs: &[HandoffRecord],
    reviews: &[ReviewRecord],
    leases: &[LeaseRecord],
) -> Option<&'static str> {
    let handoff = handoffs
        .iter()
        .filter(|handoff| handoff.card_id == *card_id && handoff.status == HandoffStatus::Active)
        .max_by_key(|handoff| handoff.created_at);
    let candidate = current_candidate(card_id, handoff, leases);
    let approval = reviews
        .iter()
        .filter(|review| {
            review.card_id == *card_id
                && review.decision == Decision::Approved
                && review.card_digest == state.current_digest
                && candidate
                    .as_deref()
                    .is_some_and(|sha| review.candidate_sha == sha)
        })
        .max_by_key(|review| review.reviewed_at);
    match (handoff, candidate, approval) {
        (Some(handoff), Some(candidate), Some(_)) if handoff.candidate_sha == candidate => None,
        (None, _, _) => Some("handoff_missing"),
        (Some(_), None, _) => Some("candidate_unavailable"),
        (Some(handoff), Some(candidate), _) if handoff.candidate_sha != candidate => {
            Some("candidate_changed_since_approval")
        }
        (Some(_), Some(_), _) => Some("approval_missing_or_stale"),
    }
}

fn integration_summary(
    cycles: &[CycleRecord],
    cards: &[CardRecord],
    states: &[StoredCardState],
    integrations: &[IntegrationRecord],
    handoffs: &[HandoffRecord],
    reviews: &[ReviewRecord],
    leases: &[LeaseRecord],
) -> IntegrationSummary {
    let state_by_card: BTreeMap<_, _> =
        states.iter().map(|state| (&state.card_id, state)).collect();
    let card_by_id: BTreeMap<_, _> = cards.iter().map(|card| (&card.card_id, card)).collect();
    let mut summary = IntegrationSummary::default();
    for cycle in cycles {
        if cycle.status == crate::domain::cycle::CycleStatus::Blocked {
            summary.blockers.push(IntegrationBlocker {
                cycle_id: Some(cycle.cycle_id.to_string()),
                card_id: None,
                reason: "cycle_blocked".to_owned(),
            });
        }
        for card_id in &cycle.card_ids {
            let Some(state) = state_by_card.get(card_id) else {
                summary.blockers.push(IntegrationBlocker {
                    cycle_id: Some(cycle.cycle_id.to_string()),
                    card_id: Some(card_id.to_string()),
                    reason: "card_state_missing".to_owned(),
                });
                continue;
            };
            if state.state == CardState::Approved {
                if let Some(reason) =
                    approved_card_blocker(card_id, state, handoffs, reviews, leases)
                {
                    summary.blockers.push(IntegrationBlocker {
                        cycle_id: Some(cycle.cycle_id.to_string()),
                        card_id: Some(card_id.to_string()),
                        reason: reason.to_owned(),
                    });
                } else {
                    summary.ready_card_count += 1;
                }
            } else if !state.state.is_terminal() {
                summary.blockers.push(IntegrationBlocker {
                    cycle_id: Some(cycle.cycle_id.to_string()),
                    card_id: Some(card_id.to_string()),
                    reason: format!("card_state_{}", state.state.name()),
                });
            }
            if !card_by_id.contains_key(card_id) {
                summary.blockers.push(IntegrationBlocker {
                    cycle_id: Some(cycle.cycle_id.to_string()),
                    card_id: Some(card_id.to_string()),
                    reason: "card_record_missing".to_owned(),
                });
            }
        }
    }
    for integration in integrations {
        if integration.status.holds_lease() {
            if integration.status == crate::domain::integration::IntegrationStatus::Blocked {
                summary.blockers.push(IntegrationBlocker {
                    cycle_id: Some(integration.cycle_id.to_string()),
                    card_id: None,
                    reason: "integration_blocked".to_owned(),
                });
            }
            summary.open_integrations.push(OpenIntegration {
                integration_id: integration.integration_id.to_string(),
                cycle_id: integration.cycle_id.to_string(),
                status: integration.status.name().to_owned(),
                member_count: integration.members.len() as u64,
            });
        }
    }
    summary.ready = summary.ready_card_count > 0;
    summary
}

struct SnapshotRecords<'a> {
    cycles: &'a [CycleRecord],
    cards: &'a [CardRecord],
    states: &'a [StoredCardState],
    events: &'a [Event],
    receipts: &'a [Receipt],
    reviews: &'a [crate::domain::review::ReviewRecord],
    handoffs: &'a [HandoffRecord],
    integrations: &'a [IntegrationRecord],
    leases: &'a [LeaseRecord],
}

fn validate_subjects(
    config: &ProjectConfig,
    records: &SnapshotRecords<'_>,
    diagnostics: &mut Vec<String>,
) {
    let cycle_ids: std::collections::BTreeSet<_> =
        records.cycles.iter().map(|cycle| &cycle.cycle_id).collect();
    let card_ids: std::collections::BTreeSet<_> =
        records.cards.iter().map(|card| &card.card_id).collect();
    if records.cycles.iter().any(|cycle| {
        cycle.project_revision
            != Digest::of_canonical(config).unwrap_or_else(|_| cycle.project_revision.clone())
    }) {
        diagnostics.push("cycle_project_revision_mismatch".to_owned());
    }
    for event in records.events {
        if event.project_id != config.project_id {
            diagnostics.push("event_project_mismatch".to_owned());
        }
        if event
            .cycle_id
            .as_ref()
            .is_some_and(|id| !cycle_ids.contains(id))
        {
            diagnostics.push("event_cycle_missing".to_owned());
        }
        if event
            .card_id
            .as_ref()
            .is_some_and(|id| !card_ids.contains(id))
        {
            diagnostics.push("event_card_missing".to_owned());
        }
    }
    for receipt in records.receipts {
        if receipt.project_id != config.project_id {
            diagnostics.push("receipt_project_mismatch".to_owned());
        }
    }
    for review in records.reviews {
        if !card_ids.contains(&review.card_id) {
            diagnostics.push("review_card_missing".to_owned());
        }
    }
    for handoff in records.handoffs {
        if !card_ids.contains(&handoff.card_id) {
            diagnostics.push("handoff_card_missing".to_owned());
        }
    }
    for integration in records.integrations {
        if !cycle_ids.contains(&integration.cycle_id) {
            diagnostics.push("integration_cycle_missing".to_owned());
        }
    }
    for lease in records.leases {
        if !card_ids.contains(&lease.card_id) {
            diagnostics.push("lease_card_missing".to_owned());
        }
    }
    for state in records.states {
        if !card_ids.contains(&state.card_id) {
            diagnostics.push("card_state_record_missing".to_owned());
        }
    }
}

fn zeroed_counts(names: &[&str]) -> BTreeMap<String, u64> {
    names.iter().map(|name| ((*name).to_owned(), 0)).collect()
}

fn append_counts(text: &mut String, label: &str, counts: &BTreeMap<String, u64>) {
    let rendered = counts
        .iter()
        .filter(|(_, count)| **count > 0)
        .map(|(name, count)| format!("{name}={count}"))
        .collect::<Vec<_>>()
        .join(", ");
    let _ = write!(
        text,
        "\n{label}: {}",
        if rendered.is_empty() {
            "none"
        } else {
            &rendered
        }
    );
}

fn age_seconds(now: Timestamp, then: Timestamp) -> i64 {
    now.unix_seconds()
        .saturating_sub(then.unix_seconds())
        .max(0)
}

fn lock_state(diagnosis: &LockDiagnosis) -> String {
    match diagnosis {
        LockDiagnosis::Free => "free",
        LockDiagnosis::Held(_) => "held",
        LockDiagnosis::Stale { .. } => "stale",
        LockDiagnosis::Ambiguous { .. } => "ambiguous",
    }
    .to_owned()
}
