//! Read-only composition of the bottleneck policy from authoritative records.

use std::fs;

use serde::{Deserialize, Serialize};

use crate::{
    commands::{card::load_card, review::reviews_for},
    config::ProjectConfig,
    control::{event_store::EventStore, repository::ControlRepository},
    domain::{card::CARD_DIR, ids::CardId},
    error::{ErrorCode, HarnessError},
    policy::{
        bottleneck::{BottleneckProjection, BottleneckSignalKind},
        convergence::{
            CardConvergence, CardCounters, ProjectConvergence, Round, ScopeBreadth, Trend,
            assess_card, project,
        },
    },
};

/// One card's convergence assessment and unified bottleneck projection.
pub struct CardBottleneckAssessment {
    pub convergence: CardConvergence,
    pub bottleneck: BottleneckProjection,
}

/// Project-level entry for one non-clear card.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct ProjectBottleneck {
    pub card_id: CardId,
    pub cycle_id: crate::domain::ids::CycleId,
    pub bottleneck: BottleneckProjection,
}

/// Computes one activated card's read model without mutating control state.
///
/// # Errors
///
/// Returns a control error when reviews or convergence facts are malformed.
pub fn for_card(
    control: &ControlRepository,
    config: &ProjectConfig,
    record: &crate::domain::card::CardRecord,
) -> Result<CardBottleneckAssessment, HarnessError> {
    let policy = config.convergence_policy.as_ref();
    let events = EventStore::new(control).for_cycle(&record.cycle_id)?;
    let view = project(policy, &config.project_id, &record.cycle_id, &events).map_err(|error| {
        HarnessError::Control {
            reason: format!(
                "convergence projection for cycle {} is unusable: {error}",
                record.cycle_id
            ),
            code: ErrorCode::InternalControlCorrupt,
        }
    })?;
    let convergence = assess_card(policy, &view, &record.card_id, record.risk);
    let counters = match &view {
        ProjectConvergence::LegacyUnassessed => None,
        ProjectConvergence::Configured(view) => Some(
            view.cards
                .get(&record.card_id)
                .cloned()
                .unwrap_or_else(CardCounters::default),
        ),
    };

    let reviews = reviews_for(control, &record.card_id)?;
    let review_evidence = reviews
        .iter()
        .map(|review| format!("review:{}", review.review_id))
        .collect::<std::collections::BTreeSet<_>>();
    let rounds = reviews
        .iter()
        .map(|review| {
            Round::new(
                review
                    .findings
                    .iter()
                    .filter(|finding| finding.disposition.blocks_approval())
                    .map(|finding| finding.location.as_str()),
            )
        })
        .collect::<Vec<_>>();
    let breadth = ScopeBreadth::measure(&record.write_scope.include);
    let mut bottleneck = BottleneckProjection::assess(
        &breadth,
        &Trend::measure(&rounds),
        &convergence,
        counters.as_ref(),
    );
    for signal in &mut bottleneck.signals {
        match signal.kind {
            BottleneckSignalKind::BroadScope => {
                signal
                    .evidence
                    .insert(format!("card:{}:r{}", record.card_id, record.revision));
            }
            BottleneckSignalKind::ReviewPlateau | BottleneckSignalKind::ReviewSpread => {
                signal.evidence.extend(review_evidence.iter().cloned());
            }
            BottleneckSignalKind::RepeatedReviewReturns
            | BottleneckSignalKind::RepeatedRepairAttempts
            | BottleneckSignalKind::RepeatedGateFailures
            | BottleneckSignalKind::RepeatedScopeRevisions
            | BottleneckSignalKind::ConvergenceEscalated => {}
        }
    }

    Ok(CardBottleneckAssessment {
        convergence,
        bottleneck,
    })
}

/// Lists every non-terminal activated card with a visible bottleneck signal.
///
/// # Errors
///
/// Fails closed when an activated-card directory or its evidence is malformed.
pub fn for_project(
    control: &ControlRepository,
    config: &ProjectConfig,
) -> Result<Vec<ProjectBottleneck>, HarnessError> {
    let directory = control.path(CARD_DIR);
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(&directory).map_err(|source| HarnessError::ControlIo {
        path: directory.clone(),
        source,
    })?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| HarnessError::ControlIo {
            path: directory.clone(),
            source,
        })?;
        let path = entry.path();
        if entry
            .file_type()
            .map_err(|source| HarnessError::ControlIo {
                path: path.clone(),
                source,
            })?
            .is_dir()
        {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();

    let mut bottlenecks = Vec::new();
    for name in names {
        let state_relative = format!("{CARD_DIR}/{name}/state.json");
        let state_path = control.path(&state_relative);
        if !state_path.exists() {
            continue;
        }
        let card_id: CardId =
            name.parse()
                .map_err(|error: HarnessError| HarnessError::Control {
                    reason: format!("activated card directory {name} is malformed: {error}"),
                    code: ErrorCode::InternalControlCorrupt,
                })?;
        let (record, state) = load_card(control, &card_id)?;
        if state.state.is_terminal() {
            continue;
        }
        let assessment = for_card(control, config, &record)?;
        if assessment.bottleneck.requires_visibility() {
            bottlenecks.push(ProjectBottleneck {
                card_id,
                cycle_id: record.cycle_id,
                bottleneck: assessment.bottleneck,
            });
        }
    }
    Ok(bottlenecks)
}
