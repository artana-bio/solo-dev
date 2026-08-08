//! Pure, deterministic projection of recorded card bottleneck signals.
//!
//! This module decides only what the existing facts say. It does not compare
//! natural-language hypotheses, launch agents, split cards, or authorize a
//! disposition. Those remain coordinator decisions.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::convergence::{
    CardConvergence, CardCounters, MIN_ROUNDS_FOR_TREND, NextPermittedAction, ScopeBreadth, Trend,
};

/// Stable schema for the projection embedded in status commands.
pub const BOTTLENECK_PROJECTION_SCHEMA: &str = "harness.bottleneck-projection/v1";

/// Reaching two attempts is the earliest honest repeated-attempt signal.
pub const REPEATED_ATTEMPT_THRESHOLD: u32 = 2;

/// Whether attempt-based detection had authoritative counters to inspect.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AttemptCoverage {
    Configured,
    LegacyUnassessed,
}

/// Overall urgency of the projected signals.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum BottleneckStatus {
    Clear,
    Advisory,
    AttentionRequired,
    StopRequired,
}

impl BottleneckStatus {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Clear => "clear",
            Self::Advisory => "advisory",
            Self::AttentionRequired => "attention_required",
            Self::StopRequired => "stop_required",
        }
    }
}

/// Stable classes a model or coordinator may match without parsing prose.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BottleneckSignalKind {
    BroadScope,
    ReviewPlateau,
    ReviewSpread,
    RepeatedReviewReturns,
    RepeatedRepairAttempts,
    RepeatedGateFailures,
    RepeatedScopeRevisions,
    ConvergenceEscalated,
}

impl BottleneckSignalKind {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::BroadScope => "broad_scope",
            Self::ReviewPlateau => "review_plateau",
            Self::ReviewSpread => "review_spread",
            Self::RepeatedReviewReturns => "repeated_review_returns",
            Self::RepeatedRepairAttempts => "repeated_repair_attempts",
            Self::RepeatedGateFailures => "repeated_gate_failures",
            Self::RepeatedScopeRevisions => "repeated_scope_revisions",
            Self::ConvergenceEscalated => "convergence_escalated",
        }
    }
}

/// Urgency of one signal.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum BottleneckSeverity {
    Advisory,
    Attention,
    Stop,
}

/// What the coordinator should do with this projection.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BottleneckAction {
    ConsiderCardSplit,
    ConveneBottleneckGroup,
}

impl BottleneckAction {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::ConsiderCardSplit => "consider_card_split",
            Self::ConveneBottleneckGroup => "convene_bottleneck_group",
        }
    }
}

/// One deterministic signal and the evidence that supports it.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct BottleneckSignal {
    pub kind: BottleneckSignalKind,
    pub severity: BottleneckSeverity,
    pub count: Option<u64>,
    pub threshold: Option<u64>,
    pub evidence: BTreeSet<String>,
    pub detail: String,
}

/// Unified read model for agents and operators.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct BottleneckProjection {
    pub schema: String,
    pub status: BottleneckStatus,
    pub attempt_coverage: AttemptCoverage,
    pub signals: Vec<BottleneckSignal>,
    pub recommended_action: Option<BottleneckAction>,
    pub authority_action: Option<NextPermittedAction>,
}

impl BottleneckProjection {
    /// Projects breadth, review history, and convergence facts into one result.
    #[must_use]
    pub fn assess(
        breadth: &ScopeBreadth,
        trend: &Trend,
        convergence: &CardConvergence,
        counters: Option<&CardCounters>,
    ) -> Self {
        let mut signals = Vec::new();
        add_scope_signal(&mut signals, breadth);
        add_trend_signals(&mut signals, trend);
        if let Some(counters) = counters {
            add_repeated_attempt_signals(&mut signals, counters);
        }
        let authority_action = add_escalation_signal(&mut signals, convergence);

        let status = signals
            .iter()
            .map(|signal| match signal.severity {
                BottleneckSeverity::Advisory => BottleneckStatus::Advisory,
                BottleneckSeverity::Attention => BottleneckStatus::AttentionRequired,
                BottleneckSeverity::Stop => BottleneckStatus::StopRequired,
            })
            .max()
            .unwrap_or(BottleneckStatus::Clear);
        let recommended_action = match status {
            BottleneckStatus::Clear => None,
            BottleneckStatus::Advisory => Some(BottleneckAction::ConsiderCardSplit),
            BottleneckStatus::AttentionRequired | BottleneckStatus::StopRequired => {
                Some(BottleneckAction::ConveneBottleneckGroup)
            }
        };

        Self {
            schema: BOTTLENECK_PROJECTION_SCHEMA.to_owned(),
            status,
            attempt_coverage: if counters.is_some() {
                AttemptCoverage::Configured
            } else {
                AttemptCoverage::LegacyUnassessed
            },
            signals,
            recommended_action,
            authority_action,
        }
    }

    /// Whether this projection belongs in the project-level attention list.
    #[must_use]
    pub const fn requires_visibility(&self) -> bool {
        !matches!(self.status, BottleneckStatus::Clear)
    }
}

fn add_scope_signal(signals: &mut Vec<BottleneckSignal>, breadth: &ScopeBreadth) {
    if breadth.advisory().is_none() {
        return;
    }
    let (count, threshold) = if breadth.paths > super::convergence::BROAD_PATH_COUNT {
        (breadth.paths, super::convergence::BROAD_PATH_COUNT)
    } else {
        (breadth.areas, super::convergence::BROAD_AREA_COUNT)
    };
    signals.push(BottleneckSignal {
        kind: BottleneckSignalKind::BroadScope,
        severity: BottleneckSeverity::Advisory,
        count: Some(count as u64),
        threshold: Some(threshold as u64),
        evidence: BTreeSet::new(),
        detail: format!(
            "declared scope spans {} path(s) across {} area(s): {}",
            breadth.paths,
            breadth.areas,
            breadth.area_names.join(", ")
        ),
    });
}

fn add_trend_signals(signals: &mut Vec<BottleneckSignal>, trend: &Trend) {
    if trend.rounds < MIN_ROUNDS_FOR_TREND {
        return;
    }
    if trend.is_flat() {
        signals.push(BottleneckSignal {
            kind: BottleneckSignalKind::ReviewPlateau,
            severity: BottleneckSeverity::Attention,
            count: Some(trend.rounds as u64),
            threshold: Some(MIN_ROUNDS_FOR_TREND as u64),
            evidence: BTreeSet::new(),
            detail: format!(
                "open findings per review round are not improving: {}",
                trend
                    .per_round
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" -> ")
            ),
        });
    }
    if !trend.new_areas.is_empty() {
        signals.push(BottleneckSignal {
            kind: BottleneckSignalKind::ReviewSpread,
            severity: BottleneckSeverity::Attention,
            count: Some(trend.new_areas.len() as u64),
            threshold: Some(1),
            evidence: BTreeSet::new(),
            detail: format!(
                "latest review found problems in new area(s): {}",
                trend.new_areas.join(", ")
            ),
        });
    }
}

fn add_repeated_attempt_signals(signals: &mut Vec<BottleneckSignal>, counters: &CardCounters) {
    for (kind, count, label) in [
        (
            BottleneckSignalKind::RepeatedReviewReturns,
            &counters.review_returns,
            "review returns",
        ),
        (
            BottleneckSignalKind::RepeatedRepairAttempts,
            &counters.repair_attempts,
            "repair attempts",
        ),
        (
            BottleneckSignalKind::RepeatedGateFailures,
            &counters.gate_failures,
            "gate failures",
        ),
        (
            BottleneckSignalKind::RepeatedScopeRevisions,
            &counters.material_scope_revisions,
            "material scope revisions",
        ),
    ] {
        if count.count < REPEATED_ATTEMPT_THRESHOLD {
            continue;
        }
        signals.push(BottleneckSignal {
            kind,
            severity: BottleneckSeverity::Attention,
            count: Some(u64::from(count.count)),
            threshold: Some(u64::from(REPEATED_ATTEMPT_THRESHOLD)),
            evidence: count.evidence.clone(),
            detail: format!("recorded {} {label}", count.count),
        });
    }
}

fn add_escalation_signal(
    signals: &mut Vec<BottleneckSignal>,
    convergence: &CardConvergence,
) -> Option<NextPermittedAction> {
    let CardConvergence::Escalated {
        exhausted,
        next_permitted_action,
    } = convergence
    else {
        return None;
    };
    let evidence = exhausted
        .iter()
        .flat_map(|dimension| dimension.evidence.iter().cloned())
        .collect();
    signals.push(BottleneckSignal {
        kind: BottleneckSignalKind::ConvergenceEscalated,
        severity: BottleneckSeverity::Stop,
        count: Some(exhausted.len() as u64),
        threshold: None,
        evidence,
        detail: format!(
            "{} convergence dimension(s) exhausted; an authorized disposition is required",
            exhausted.len()
        ),
    });
    Some(*next_permitted_action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::convergence::{CardDimension, DimensionCount, ExhaustedDimension};

    #[test]
    fn two_recorded_repairs_require_attention_before_budget_exhaustion() {
        let mut counters = CardCounters::default();
        counters.repair_attempts.count = 2;
        counters.repair_attempts.evidence =
            BTreeSet::from(["handoff:HO-001".to_owned(), "handoff:HO-002".to_owned()]);
        let projection = BottleneckProjection::assess(
            &ScopeBreadth::measure(&[]),
            &Trend::measure(&[]),
            &CardConvergence::Within,
            Some(&counters),
        );

        assert_eq!(projection.status, BottleneckStatus::AttentionRequired);
        assert_eq!(
            projection.recommended_action,
            Some(BottleneckAction::ConveneBottleneckGroup)
        );
        assert_eq!(
            projection.signals[0].kind,
            BottleneckSignalKind::RepeatedRepairAttempts
        );
    }

    #[test]
    fn hard_escalation_preserves_the_authoritative_next_action() {
        let convergence = CardConvergence::Escalated {
            exhausted: vec![ExhaustedDimension {
                dimension: CardDimension::RepairAttempts,
                count: 2,
                limit: 2,
                evidence: BTreeSet::from(["handoff:HO-002".to_owned()]),
            }],
            next_permitted_action: NextPermittedAction::RecordAuthorizedDisposition,
        };
        let projection = BottleneckProjection::assess(
            &ScopeBreadth::measure(&[]),
            &Trend::measure(&[]),
            &convergence,
            Some(&CardCounters {
                repair_attempts: DimensionCount {
                    count: 2,
                    evidence: BTreeSet::from(["handoff:HO-002".to_owned()]),
                    ..DimensionCount::default()
                },
                ..CardCounters::default()
            }),
        );

        assert_eq!(projection.status, BottleneckStatus::StopRequired);
        assert_eq!(
            projection.authority_action,
            Some(NextPermittedAction::RecordAuthorizedDisposition)
        );
    }
}
