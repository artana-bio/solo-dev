//! Advisory signals that a card is too big, or is not converging.
//!
//! Both are **report-only** and cannot be made to block. That is the design,
//! not a limitation. Counting rounds and findings is mechanical; deciding to
//! split a card is judgment, and a harness that split cards automatically
//! would be making a product decision from a file count. What it can do is
//! stop the signal being invisible.
//!
//! It was invisible. `F-027` bundled seven unrelated issues across 24 files,
//! ran four review rounds and about seventeen findings before anyone said the
//! word "split", and the control repository held every number needed to say it
//! after round two. Nothing was missing except a line of output.
//!
//! The two checks sit at opposite ends. Scope breadth is *leading* — it asks
//! the question at activation, when the answer is cheap. Convergence is
//! *lagging* — it can only speak once rounds exist, but by then it knows
//! something the first check cannot: whether the work is actually settling.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    config::ConvergencePolicy,
    control::event_store::Event,
    domain::{
        card::Risk,
        digest::Digest,
        ids::{CardId, CycleId, ProjectId},
    },
};

use crate::policy::paths::{self, CaseSensitivity};

/// How wide a card's declared scope is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScopeBreadth {
    /// Distinct paths the card may write.
    ///
    /// Distinct, not the length of the include list. Round 2 of this card's
    /// own review declared one path thirteen times and was told the card
    /// spanned "13 path(s)" — a number that is simply false, and one the
    /// envelope publishes for programs to read.
    pub paths: usize,
    /// Distinct top-level areas those paths fall under.
    pub areas: usize,
    /// The areas themselves, in a stable order, for the message.
    pub area_names: Vec<String>,
}

/// Paths above which a card stops looking like one reviewable outcome.
///
/// Not a rule about repositories in general — a threshold this crate can
/// defend. Every card that has landed here touched fewer than this; the one
/// that did not was `F-027`, at 24, which took eight review rounds and a split.
/// A card at the boundary is asked a question, not refused.
pub const BROAD_PATH_COUNT: usize = 12;

/// Areas above which a card is probably several cards.
///
/// "One independently reviewable outcome" is the plan's phrase. A reviewer
/// holding four unrelated areas in their head at once is not reviewing one
/// outcome, whatever the card says it is about.
pub const BROAD_AREA_COUNT: usize = 4;

/// The area a declared path or a finding location belongs to.
///
/// The first component — `src`, `tests`, `docs` — or the second where the
/// first is `src`, so `src/policy/**` and `src/commands/**` count separately.
/// That is the granularity at which this codebase's cards actually differ.
///
/// One definition, shared by both signals, because this card's own first
/// review round found the trend comparing raw finding *locations* while
/// calling them areas: a card whose findings moved from `src/policy/a.rs` to
/// `src/policy/b.rs` read as spreading into somewhere new when it had not left
/// the area it started in. Two notions of "area" is what made that possible.
///
/// Returns `None` only for a path with no non-empty component at all.
///
/// Takes the canonical spelling first, so that the areas agree with the path
/// counts about which declarations name the same file.
fn area_of(path: &str) -> Option<String> {
    let canonical = paths::canonical(path, CaseSensitivity::host());
    let mut parts = canonical.split('/');
    match (parts.next(), parts.next()) {
        (Some("src"), Some(second)) if !second.contains('*') => Some(format!("src/{second}")),
        (Some(first), _) if !first.is_empty() => Some(first.to_owned()),
        _ => None,
    }
}

impl ScopeBreadth {
    /// Measures a card's include list.
    #[must_use]
    pub fn measure(include: &[String]) -> Self {
        let case = CaseSensitivity::host();
        let distinct: BTreeSet<String> = include
            .iter()
            .map(|path| paths::canonical(path, case))
            .filter(|path| !path.is_empty())
            .collect();
        let areas: BTreeSet<String> = distinct.iter().filter_map(|path| area_of(path)).collect();
        Self {
            paths: distinct.len(),
            areas: areas.len(),
            area_names: areas.into_iter().collect(),
        }
    }

    /// The advisory to show at activation, when there is one.
    ///
    /// Phrased as a question. The card may well be right — a mechanical rename
    /// touches fifty files and is one outcome — and the author is the one who
    /// knows.
    #[must_use]
    pub fn advisory(&self) -> Option<String> {
        // `<=`, not `<`: both constants name the breadth a card may reach and
        // still be one outcome, so the advisory begins one past them. Round 1
        // of this card's review found the strict form firing at exactly 12
        // paths, which the declaration promised would be silent.
        if self.paths <= BROAD_PATH_COUNT && self.areas <= BROAD_AREA_COUNT {
            return None;
        }
        Some(format!(
            "this card declares {} path(s) across {} area(s) ({}); a card is meant to be one independently reviewable outcome, and past that breadth a reviewer is holding several at once. If it is several, splitting now is far cheaper than splitting after the review rounds",
            self.paths,
            self.areas,
            self.area_names.join(", ")
        ))
    }
}

/// One review round, reduced to what the trend needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Round {
    /// How many findings the reviewer left open.
    ///
    /// Findings, not distinct locations. Two open findings naming the same
    /// file are two problems, and a card sitting at three open findings every
    /// round is stuck whether or not they share a line. Counting the set
    /// instead — which this did until round 1 of its own review — reads a
    /// stuck card as converging, which is the one reading that must never
    /// happen silently.
    pub open_findings: usize,
    /// The areas those findings fall under.
    pub areas: BTreeSet<String>,
}

impl Round {
    /// Builds a round from the locations of a review's open findings.
    ///
    /// Takes one item per open finding, duplicates included.
    #[must_use]
    pub fn new<'a>(open_locations: impl IntoIterator<Item = &'a str>) -> Self {
        let mut open_findings = 0;
        let mut areas = BTreeSet::new();
        for location in open_locations {
            open_findings += 1;
            if let Some(area) = area_of(location) {
                areas.insert(area);
            }
        }
        Self {
            open_findings,
            areas,
        }
    }
}

/// What the rounds so far say about whether the card is settling.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Trend {
    /// How many rounds have been recorded, including this one.
    pub rounds: usize,
    /// Open findings in each round, oldest first.
    pub per_round: Vec<usize>,
    /// Areas in this round that no earlier round named.
    pub new_areas: Vec<String>,
}

/// Rounds below which a trend says nothing worth printing.
///
/// Two points is a line through any two numbers. Three is the first round
/// where "still not settling" is a statement rather than an observation.
pub const MIN_ROUNDS_FOR_TREND: usize = 3;

/// The single append-only event type recognized by the v1 projection.
pub const ATTEMPT_RECORDED_EVENT: &str = "convergence.attempt_recorded";

/// Closed attempt classes; callers cannot make up a counter by spelling a new
/// string in event metadata.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum AttemptKind {
    ReviewReturn,
    RepairAttempt,
    GateFailure,
    MaterialScopeRevision,
    IntegrationFailure,
}

impl AttemptKind {
    /// Whether a fact of this class may declare `reason`.
    ///
    /// The one compatibility table for the whole harness. `project` calls this
    /// rather than re-deciding the question from scratch, and so must every
    /// command that accepts a reason before a fact naming it is ever written —
    /// `review record` (71-R2) is the first. A second, hand-written copy of
    /// this table anywhere upstream would only need to disagree with this one
    /// once for a verdict the command accepted to become a fact the projection
    /// then refuses: accepted, then unrecorded, which is worse for an operator
    /// than being refused up front.
    #[must_use]
    pub const fn admits(self, reason: ReasonCategory) -> bool {
        match self {
            Self::ReviewReturn => matches!(
                reason,
                ReasonCategory::AcceptanceDefect
                    | ReasonCategory::Regression
                    | ReasonCategory::SecurityConcern
                    | ReasonCategory::NonBlockingImprovement
            ),
            Self::RepairAttempt => matches!(
                reason,
                ReasonCategory::AcceptanceDefect
                    | ReasonCategory::Regression
                    | ReasonCategory::SecurityConcern
            ),
            Self::GateFailure => matches!(
                reason,
                ReasonCategory::Regression | ReasonCategory::SecurityConcern
            ),
            Self::MaterialScopeRevision => matches!(reason, ReasonCategory::ScopeChange),
            Self::IntegrationFailure => matches!(reason, ReasonCategory::IntegrationConflict),
        }
    }
}

/// The single append-only disposition fact the v1 projection recognizes.
pub const DISPOSITION_RECORDED_EVENT: &str = "convergence.disposition_recorded";

/// Closed set of authorized dispositions. Only the one #74's first card
/// implements exists here; the rest arrive with their own bounded effects.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DispositionKind {
    Renew,
}

/// Why a bounded attempt occurred. This is intentionally descriptive rather
/// than an enforcement decision; #72–#74 consume the counters later.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCategory {
    AcceptanceDefect,
    Regression,
    SecurityConcern,
    ScopeChange,
    IntegrationConflict,
    NonBlockingImprovement,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AttemptMetadata {
    attempt_kind: AttemptKind,
    reason_category: ReasonCategory,
    evidence_ref: String,
    policy_digest: Digest,
}

/// Metadata a disposition fact must declare, validated with the same
/// closed-set rigor as `AttemptMetadata`: an unrecognized `disposition` or
/// `dimension` value, or any field this shape does not name, refuses the
/// fact rather than being silently ignored or defaulted.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DispositionMetadata {
    disposition: DispositionKind,
    dimension: CardDimension,
    rationale: String,
    authorized_by: String,
    policy_digest: Digest,
}

/// A projection error refuses the whole view. Returning partial counters would
/// make an attacker-controlled malformed fact look like unused budget.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectionError {
    pub event_id: String,
    pub reason: String,
}

impl std::fmt::Display for ProjectionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "convergence fact {}: {}",
            self.event_id, self.reason
        )
    }
}

impl std::error::Error for ProjectionError {}

/// One bounded dimension's count and the evidence behind it.
///
/// `count` is authoritative, not `evidence`: two facts may cite the same
/// reference, so `evidence.len()` can be less than `count`, and that is
/// expected rather than a defect. Evidence is retained at all because #72
/// has to publish exact evidence for each blocking return; before this type
/// existed, `evidence_ref` was validated on every fact and then discarded —
/// a malformed reference still failed the whole projection, but a fact that
/// passed left no trace of which receipt justified which count.
#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct DimensionCount {
    pub count: u32,
    /// Distinct evidence references of the counted facts, sorted.
    /// `count` is authoritative: two facts may cite one reference.
    pub evidence: BTreeSet<String>,
    /// Budgets granted back by authorized renewal dispositions.
    ///
    /// Not evidence and not a count of facts: this is how many times an
    /// authorized actor decided to grant the configured budget again.
    pub renewals: u32,
}

/// Counters derived for a card, accumulated over every fact recorded against
/// it in this cycle, whichever revision each fact names.
///
/// Not "for one exact revision", which is what this held until this change.
/// `material_scope_revisions` counts a card moving from one revision to
/// another, so it can never share a single revision with the attempts
/// around it; counting it per revision would hold it at 1 forever and call
/// that a count. Worse, resetting a card's counters whenever its facts
/// crossed into a new revision would make revising the card the exact
/// bypass #72 exists to close: "a raw retry cannot bypass the escalation"
/// stops being true the moment resubmitting under a new revision clears the
/// budget that was tracking it. Each fact still binds exactly to the
/// revision, digest, and head it names — see the per-fact checks in
/// `project` — only the tally reaches across revisions, not the identity of
/// any one fact.
#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct CardCounters {
    pub review_returns: DimensionCount,
    pub repair_attempts: DimensionCount,
    pub gate_failures: DimensionCount,
    pub material_scope_revisions: DimensionCount,
}

/// Counters derived for a whole exact cycle.
#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct CycleCounters {
    pub integration_failures: DimensionCount,
}

/// The pure, reproducible v1 view. `BTreeMap` makes output independent of event
/// discovery order.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct ConvergenceProjection {
    pub policy_digest: Digest,
    pub cycle: CycleCounters,
    pub cards: BTreeMap<CardId, CardCounters>,
}

/// A legacy project has no implicit budget. It is visible as unassessed, not
/// silently counted under a policy an operator never configured.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProjectConvergence {
    LegacyUnassessed,
    Configured(ConvergenceProjection),
}

/// Projects only events bound to the supplied project, cycle and policy.
///
/// Any malformed, duplicate, foreign or unbound recognized fact refuses the
/// entire projection. Events of other types are not convergence facts and are
/// deliberately ignored.
#[allow(clippy::too_many_lines)] // One fail-closed fold keeps partial state impossible.
/// # Errors
///
/// Returns a refusal for any recognized fact that cannot be bound exactly to
/// this policy, project, cycle, and subject.
pub fn project(
    policy: Option<&ConvergencePolicy>,
    project_id: &ProjectId,
    cycle_id: &CycleId,
    events: &[Event],
) -> Result<ProjectConvergence, ProjectionError> {
    let Some(policy) = policy else {
        return Ok(ProjectConvergence::LegacyUnassessed);
    };
    let policy_digest = policy.digest().map_err(|error| ProjectionError {
        event_id: "<policy>".to_owned(),
        reason: error.to_string(),
    })?;
    let mut result = ConvergenceProjection {
        policy_digest: policy_digest.clone(),
        cycle: CycleCounters::default(),
        cards: BTreeMap::new(),
    };
    let mut seen = BTreeSet::new();

    for event in events
        .iter()
        .filter(|event| event.event_type == ATTEMPT_RECORDED_EVENT)
    {
        let event_id = event.event_id.as_str().to_owned();
        if !seen.insert(event_id.clone()) {
            return Err(ProjectionError {
                event_id,
                reason: "duplicate event identifier".to_owned(),
            });
        }
        if &event.project_id != project_id || event.cycle_id.as_ref() != Some(cycle_id) {
            return Err(ProjectionError {
                event_id,
                reason: "fact is not bound to this project and cycle".to_owned(),
            });
        }
        let metadata: AttemptMetadata = serde_json::from_value(serde_json::Value::Object(
            event.metadata.clone().into_iter().collect(),
        ))
        .map_err(|error| ProjectionError {
            event_id: event_id.clone(),
            reason: format!("malformed metadata: {error}"),
        })?;
        if metadata.evidence_ref.trim().is_empty() {
            return Err(ProjectionError {
                event_id,
                reason: "evidence_ref must not be empty".to_owned(),
            });
        }
        if metadata.policy_digest != policy_digest {
            return Err(ProjectionError {
                event_id,
                reason: "fact names a foreign policy digest".to_owned(),
            });
        }

        match metadata.attempt_kind {
            AttemptKind::IntegrationFailure => {
                if event.card_id.is_some()
                    || event.card_revision.is_some()
                    || event.card_digest.is_some()
                    || !event.head_sha.as_deref().is_some_and(is_exact_sha)
                {
                    return Err(ProjectionError {
                        event_id,
                        reason: "integration failure must be cycle-only".to_owned(),
                    });
                }
                if !metadata.attempt_kind.admits(metadata.reason_category) {
                    return Err(ProjectionError {
                        event_id,
                        reason: "integration failure has incompatible reason category".to_owned(),
                    });
                }
                result.cycle.integration_failures.count = result
                    .cycle
                    .integration_failures
                    .count
                    .checked_add(1)
                    .ok_or_else(|| ProjectionError {
                        event_id: event_id.clone(),
                        reason: "counter overflow".to_owned(),
                    })?;
                result
                    .cycle
                    .integration_failures
                    .evidence
                    .insert(metadata.evidence_ref);
            }
            kind => {
                // Each fact still must name its own exact card_id,
                // card_revision, card_digest, and head — refused right here
                // if it does not — regardless of what any other fact for
                // this card names. What is gone is the older requirement
                // that every fact toward one card agree on the *same*
                // revision, digest, and head as the others: that combined a
                // real rule (a fact must bind to reality) with a false
                // assumption (a card has one revision for its whole life).
                // `MaterialScopeRevision` exists because that assumption is
                // false; see `CardCounters` for why the tally spans
                // revisions even though each fact's own binding stays
                // exact.
                let (Some(card_id), Some(_), Some(_), Some(head)) = (
                    event.card_id.as_ref(),
                    event.card_revision,
                    event.card_digest.as_ref(),
                    event.head_sha.as_ref(),
                ) else {
                    return Err(ProjectionError {
                        event_id,
                        reason: "card attempt lacks exact card revision, digest, or head binding"
                            .to_owned(),
                    });
                };
                if !is_exact_sha(head) {
                    return Err(ProjectionError {
                        event_id,
                        reason: "card attempt head is not an exact commit SHA".to_owned(),
                    });
                }
                if !kind.admits(metadata.reason_category) {
                    return Err(ProjectionError {
                        event_id,
                        reason: "card attempt has incompatible reason category".to_owned(),
                    });
                }
                let counters = result.cards.entry(card_id.clone()).or_default();
                let dimension = match kind {
                    AttemptKind::ReviewReturn => &mut counters.review_returns,
                    AttemptKind::RepairAttempt => &mut counters.repair_attempts,
                    AttemptKind::GateFailure => &mut counters.gate_failures,
                    AttemptKind::MaterialScopeRevision => &mut counters.material_scope_revisions,
                    AttemptKind::IntegrationFailure => unreachable!(),
                };
                dimension.count =
                    dimension
                        .count
                        .checked_add(1)
                        .ok_or_else(|| ProjectionError {
                            event_id,
                            reason: "counter overflow".to_owned(),
                        })?;
                dimension.evidence.insert(metadata.evidence_ref);
            }
        }
    }

    // A disposition fact binds with exactly the same rigor as an attempt
    // fact — project/cycle, duplicate identifier, policy digest, and exact
    // card revision/digest/head — with one addition: `rationale` and
    // `authorized_by` must both be non-blank, because a decision with no
    // declared reason and no declared actor is not a decision. Unlike
    // `IntegrationFailure`, a disposition has no cycle-only shape: it is
    // always about one card in one exact state, so the card binding below
    // is unconditional rather than selected by a match arm.
    for event in events
        .iter()
        .filter(|event| event.event_type == DISPOSITION_RECORDED_EVENT)
    {
        let event_id = event.event_id.as_str().to_owned();
        if !seen.insert(event_id.clone()) {
            return Err(ProjectionError {
                event_id,
                reason: "duplicate event identifier".to_owned(),
            });
        }
        if &event.project_id != project_id || event.cycle_id.as_ref() != Some(cycle_id) {
            return Err(ProjectionError {
                event_id,
                reason: "fact is not bound to this project and cycle".to_owned(),
            });
        }
        let metadata: DispositionMetadata = serde_json::from_value(serde_json::Value::Object(
            event.metadata.clone().into_iter().collect(),
        ))
        .map_err(|error| ProjectionError {
            event_id: event_id.clone(),
            reason: format!("malformed metadata: {error}"),
        })?;
        if metadata.rationale.trim().is_empty() || metadata.authorized_by.trim().is_empty() {
            return Err(ProjectionError {
                event_id,
                reason: "a disposition needs a declared rationale and an authorizing actor"
                    .to_owned(),
            });
        }
        if metadata.policy_digest != policy_digest {
            return Err(ProjectionError {
                event_id,
                reason: "fact names a foreign policy digest".to_owned(),
            });
        }
        let (Some(card_id), Some(_), Some(_), Some(head)) = (
            event.card_id.as_ref(),
            event.card_revision,
            event.card_digest.as_ref(),
            event.head_sha.as_ref(),
        ) else {
            return Err(ProjectionError {
                event_id,
                reason: "disposition lacks exact card revision, digest, or head binding".to_owned(),
            });
        };
        if !is_exact_sha(head) {
            return Err(ProjectionError {
                event_id,
                reason: "disposition head is not an exact commit SHA".to_owned(),
            });
        }

        let counters = result.cards.entry(card_id.clone()).or_default();
        let dimension = match metadata.dimension {
            CardDimension::ReviewReturns => &mut counters.review_returns,
            CardDimension::RepairAttempts => &mut counters.repair_attempts,
            CardDimension::GateFailures => &mut counters.gate_failures,
            CardDimension::MaterialScopeRevisions => &mut counters.material_scope_revisions,
        };
        match metadata.disposition {
            // Exactly one renewal grant per fact, hard-coded rather than
            // read from any amount this metadata could declare — it
            // declares none. `continue` must never silently expand a
            // budget (#74's acceptance criterion): if a disposition could
            // name its own amount, "renew" would be an unlimited budget
            // with extra steps, since nothing would stop a `+1000` behind
            // a plausible-sounding rationale. Granting back exactly the
            // configured limit, once per fact, means widening a budget
            // further always costs another authorized decision with its
            // own rationale, actor, and trail — never a bigger number on
            // this one.
            DispositionKind::Renew => {
                dimension.renewals =
                    dimension
                        .renewals
                        .checked_add(1)
                        .ok_or_else(|| ProjectionError {
                            event_id,
                            reason: "counter overflow".to_owned(),
                        })?;
            }
        }
    }

    Ok(ProjectConvergence::Configured(result))
}

fn is_exact_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Which bounded card dimension a budget covers.
///
/// Declaration order is significant: `assess_card` reports exhausted
/// dimensions in this order, not discovery order, so the same card always
/// lists its exhausted dimensions the same way regardless of which facts a
/// projection happened to fold first.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum CardDimension {
    ReviewReturns,
    RepairAttempts,
    GateFailures,
    MaterialScopeRevisions,
}

/// One dimension whose budget is spent, and what spent it.
///
/// `evidence` is copied from the matching `DimensionCount`, so the same
/// "count is authoritative, evidence may undercount because two facts can
/// cite one reference" property documented there holds here too.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct ExhaustedDimension {
    pub dimension: CardDimension,
    pub count: u32,
    /// The effective budget this dimension was actually compared against —
    /// the configured limit already multiplied by `renewals + 1` — never
    /// the bare configuration limit. Reporting the base limit here would
    /// read as "no renewal was ever granted" even when several were; this
    /// is the number `count` was really measured against.
    pub limit: u32,
    pub evidence: BTreeSet<String>,
}

/// What the recorded facts say about one card's budget.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CardConvergence {
    /// No policy configured: nothing to enforce, and no budget implied.
    LegacyUnassessed,
    /// A policy is configured and every dimension still has budget.
    Within,
    /// At least one registered budget is spent.
    Escalated {
        exhausted: Vec<ExhaustedDimension>,
        next_permitted_action: NextPermittedAction,
    },
}

/// The only thing that may happen next to an escalated card.
///
/// A single-variant enum on purpose: the next action is a closed value
/// programs compare against, not a prose sentence whose wording can drift
/// between call sites. #74 adds the command that satisfies it; until then
/// this names what is required, not what already exists.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum NextPermittedAction {
    RecordAuthorizedDisposition,
}

/// Assesses one card against the budget its declared risk selects.
///
/// A missing policy, or a project still in `LegacyUnassessed`, answers
/// `LegacyUnassessed` rather than inferring a budget nobody configured. The
/// opposite mistake matters just as much: a card the projection never
/// mentions has no entry in `view.cards`, and that absence is silence about
/// *attempts*, not absence of *budget* — such a card is `Within`, exactly as
/// if it had been recorded with every counter sitting at zero.
///
/// A dimension is exhausted at `count >= effective budget`, not
/// `count > effective budget`: a limit of 2 grants two attempts, and it is
/// the second one — not some third attempt that was never going to be
/// permitted anyway — that spends the budget. The effective budget is the
/// configured limit multiplied by `renewals + 1`, computed with checked
/// arithmetic; see the loop below for why every renewal is worth exactly
/// one more configured budget, never an actor-chosen amount, and for why an
/// overflow there fails closed instead of wrapping into an enormous one.
///
/// Every exhausted dimension is reported, in `CardDimension`'s declared
/// order, not only the first one found. An operator who fixes the one
/// dimension a partial answer happened to mention, then meets a second
/// dimension hiding behind it on the next attempt, has lost a whole review
/// cycle to a check that already knew about both.
#[must_use]
pub fn assess_card(
    policy: Option<&ConvergencePolicy>,
    view: &ProjectConvergence,
    card_id: &CardId,
    risk: Risk,
) -> CardConvergence {
    let Some(policy) = policy else {
        return CardConvergence::LegacyUnassessed;
    };
    let ProjectConvergence::Configured(view) = view else {
        return CardConvergence::LegacyUnassessed;
    };
    let limits = policy.card_limits.for_risk(risk);
    // A card the projection never mentions has no `CardCounters` entry at
    // all. That is silence, not a zero-attempt fact; `Default` gives exactly
    // the all-zero counters that silence implies, without inventing an
    // entry in a map that is supposed to mean "facts were recorded here".
    let empty_counters = CardCounters::default();
    let counters = view.cards.get(card_id).unwrap_or(&empty_counters);

    // Fixed order, not discovery order: this list mirrors `CardDimension`'s
    // own declaration so the two can never silently drift apart.
    let budgets = [
        (
            CardDimension::ReviewReturns,
            &counters.review_returns,
            limits.review_returns,
        ),
        (
            CardDimension::RepairAttempts,
            &counters.repair_attempts,
            limits.repair_attempts,
        ),
        (
            CardDimension::GateFailures,
            &counters.gate_failures,
            limits.gate_failures,
        ),
        (
            CardDimension::MaterialScopeRevisions,
            &counters.material_scope_revisions,
            limits.material_scope_revisions,
        ),
    ];

    let mut exhausted = Vec::new();
    for (dimension, count, base_limit) in budgets {
        // The effective budget is the configured limit granted again once
        // per authorized renewal, never an actor-chosen amount: `renewals`
        // only ever moves by exactly 1 per fact (see `project`), so
        // multiplying the base limit by `renewals + 1` is the whole grant a
        // renewal can ever be worth. A card with zero renewals is assessed
        // against exactly the configured limit, unchanged from before this
        // dimension existed.
        //
        // Both the `+ 1` and the multiplication are checked, chained with
        // `and_then` so an overflow in either step — not only the
        // multiplication — is caught. Fail closed, never open: an overflow
        // must never wrap into a small or enormous budget by accident, so
        // it forces this dimension exhausted outright, regardless of
        // `count`.
        let effective_budget = count
            .renewals
            .checked_add(1)
            .and_then(|multiplier| base_limit.checked_mul(multiplier));
        let overflowed = effective_budget.is_none();
        let effective_budget = effective_budget.unwrap_or(u32::MAX);
        // `>=`, not `>`: the boundary this function exists to get right.
        // See the doc comment above. An overflowed budget is exhausted
        // unconditionally: there is no safe number left to compare `count`
        // against, so escalating is the only fail-closed answer.
        if overflowed || count.count >= effective_budget {
            exhausted.push(ExhaustedDimension {
                dimension,
                count: count.count,
                limit: effective_budget,
                evidence: count.evidence.clone(),
            });
        }
    }

    if exhausted.is_empty() {
        CardConvergence::Within
    } else {
        CardConvergence::Escalated {
            exhausted,
            next_permitted_action: NextPermittedAction::RecordAuthorizedDisposition,
        }
    }
}

impl Trend {
    /// Computes the trend across every round recorded for a card.
    ///
    /// `rounds` is oldest first and includes the round being recorded.
    #[must_use]
    pub fn measure(rounds: &[Round]) -> Self {
        let seen: BTreeSet<&String> = rounds
            .iter()
            .rev()
            .skip(1)
            .flat_map(|round| round.areas.iter())
            .collect();
        let new_areas = rounds
            .last()
            .map(|latest| {
                latest
                    .areas
                    .iter()
                    .filter(|area| !seen.contains(area))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        Self {
            rounds: rounds.len(),
            per_round: rounds.iter().map(|round| round.open_findings).collect(),
            new_areas,
        }
    }

    /// True when this round did not improve on the best any earlier round
    /// reached.
    ///
    /// Volume alone is not the signal. A round of twelve findings that becomes
    /// six then two is a card being finished. Four, then four, then five, is a
    /// card whose bottom nobody has found.
    ///
    /// Measured against the lowest earlier round rather than the first one.
    /// Round 2 of this card's own review found the first-round comparison
    /// reading `5 → 3 → 3` as converging, because 3 is still below 5 — a card
    /// that made early progress and then stopped, which is the most common
    /// shape a stuck card actually has, and precisely what this exists to
    /// catch. Against the running best it is flat, correctly.
    ///
    /// Using the best rather than the immediately preceding round also keeps
    /// `3 → 4 → 3` flat: recovering ground already held is not progress.
    #[must_use]
    pub fn is_flat(&self) -> bool {
        let Some((last, earlier)) = self.per_round.split_last() else {
            return false;
        };
        if *last == 0 {
            // An approval closes the card. Zero open is the end state, not a
            // plateau, however many rounds it took to get there.
            return false;
        }
        earlier.iter().min().is_some_and(|best| last >= best)
    }

    /// The advisory to show after recording, when there is one.
    ///
    /// Silent until there is enough history to mean anything, and silent when
    /// the card is converging — a card that is nearly done should not be
    /// nagged about its size.
    #[must_use]
    pub fn advisory(&self) -> Option<String> {
        if self.rounds < MIN_ROUNDS_FOR_TREND {
            return None;
        }
        let spreading = !self.new_areas.is_empty();
        if !self.is_flat() && !spreading {
            return None;
        }

        // Clauses joined rather than written into a `String`: `write!` into a
        // `String` needs an `expect` to discharge its `Result`, and a panic
        // site — however unreachable — on the one code path whose entire claim
        // is that it cannot change what a command does is the wrong shape.
        let mut clauses: Vec<String> = Vec::new();
        if self.is_flat() {
            clauses.push("open findings are not falling".to_owned());
        }
        if spreading {
            clauses.push(format!(
                "round {} raised {} finding(s) in area(s) no earlier round named ({})",
                self.rounds,
                self.new_areas.len(),
                self.new_areas.join(", ")
            ));
        }
        let counts: Vec<String> = self.per_round.iter().map(ToString::to_string).collect();
        Some(format!(
            "this card is on review round {} with open findings per round of {}; {}. Findings that keep appearing in new places usually mean the card is several cards. Consider splitting it — this is a signal, not a refusal, and the judgment is yours",
            self.rounds,
            counts.join(" → "),
            clauses.join(" and ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, str::FromStr};

    use crate::{
        config::{CardConvergenceLimits, CycleConvergenceLimits, RiskConvergenceLimits},
        control::event_store::Event,
        domain::{clock::Timestamp, ids::EventId},
    };

    fn paths(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn policy() -> ConvergencePolicy {
        let limits = CardConvergenceLimits {
            review_returns: 2,
            repair_attempts: 3,
            gate_failures: 2,
            material_scope_revisions: 1,
        };
        ConvergencePolicy {
            version: crate::config::CONVERGENCE_POLICY_V1.to_owned(),
            card_limits: RiskConvergenceLimits {
                low: limits.clone(),
                medium: limits.clone(),
                high: limits.clone(),
                critical: limits,
            },
            cycle_limits: CycleConvergenceLimits {
                integration_failures: 2,
            },
        }
    }

    fn event(id: u64, kind: AttemptKind) -> Event {
        let policy = policy();
        let reason = match kind {
            AttemptKind::ReviewReturn => ReasonCategory::AcceptanceDefect,
            AttemptKind::RepairAttempt | AttemptKind::GateFailure => ReasonCategory::Regression,
            AttemptKind::MaterialScopeRevision => ReasonCategory::ScopeChange,
            AttemptKind::IntegrationFailure => ReasonCategory::IntegrationConflict,
        };
        let mut metadata = BTreeMap::new();
        metadata.insert(
            "attempt_kind".to_owned(),
            serde_json::to_value(kind).unwrap(),
        );
        metadata.insert(
            "reason_category".to_owned(),
            serde_json::to_value(reason).unwrap(),
        );
        metadata.insert(
            "evidence_ref".to_owned(),
            serde_json::json!(format!("receipt:{id}")),
        );
        metadata.insert(
            "policy_digest".to_owned(),
            serde_json::json!(policy.digest().unwrap().as_str()),
        );
        let integration = kind == AttemptKind::IntegrationFailure;
        Event {
            schema: crate::control::event_store::EVENT_SCHEMA.to_owned(),
            event_id: format!("E-{id:06}").parse::<EventId>().unwrap(),
            project_id: "example".parse().unwrap(),
            cycle_id: Some("C-001".parse().unwrap()),
            card_id: (!integration).then(|| "F-001".parse().unwrap()),
            card_revision: (!integration).then_some(1),
            card_digest: (!integration).then(|| Digest::of_bytes(b"card")),
            event_type: ATTEMPT_RECORDED_EVENT.to_owned(),
            actor_id: "luna".to_owned(),
            occurred_at: Timestamp::from_unix_seconds(1).unwrap(),
            previous_state: None,
            next_state: None,
            head_sha: Some("0123456789012345678901234567890123456789".to_owned()),
            metadata,
        }
    }

    /// Builds a valid `renew` disposition fact for `F-001`, naming the given
    /// dimension. Tests that need an invalid or differently bound
    /// disposition mutate the returned event.
    fn disposition_event(id: u64, dimension: CardDimension) -> Event {
        let policy = policy();
        let mut metadata = BTreeMap::new();
        metadata.insert(
            "disposition".to_owned(),
            serde_json::to_value(DispositionKind::Renew).unwrap(),
        );
        metadata.insert(
            "dimension".to_owned(),
            serde_json::to_value(dimension).unwrap(),
        );
        metadata.insert(
            "rationale".to_owned(),
            serde_json::json!(format!("renewal:{id}")),
        );
        metadata.insert("authorized_by".to_owned(), serde_json::json!("luna"));
        metadata.insert(
            "policy_digest".to_owned(),
            serde_json::json!(policy.digest().unwrap().as_str()),
        );
        Event {
            schema: crate::control::event_store::EVENT_SCHEMA.to_owned(),
            event_id: format!("E-{id:06}").parse::<EventId>().unwrap(),
            project_id: "example".parse().unwrap(),
            cycle_id: Some("C-001".parse().unwrap()),
            card_id: Some("F-001".parse().unwrap()),
            card_revision: Some(1),
            card_digest: Some(Digest::of_bytes(b"card")),
            event_type: DISPOSITION_RECORDED_EVENT.to_owned(),
            actor_id: "luna".to_owned(),
            occurred_at: Timestamp::from_unix_seconds(1).unwrap(),
            previous_state: None,
            next_state: None,
            head_sha: Some("0123456789012345678901234567890123456789".to_owned()),
            metadata,
        }
    }

    #[test]
    fn an_ordinary_card_draws_no_scope_advisory() {
        // Every card that landed cleanly in this repository is this shape.
        let narrow = ScopeBreadth::measure(&paths(&["src/policy/actors.rs", "tests/promotion.rs"]));
        assert_eq!(narrow.areas, 2);
        assert!(narrow.advisory().is_none());
    }

    #[test]
    fn the_card_this_check_exists_for_is_flagged() {
        // F-027's declared scope, verbatim: seven unrelated issues, 24 paths,
        // eight review rounds, and a split that should have happened at round
        // two. The check has to catch this one or it catches nothing.
        let f027 = ScopeBreadth::measure(&paths(&[
            ".claude/skills/change-harness/SKILL.md",
            "docs/IMPLEMENTATION_PLAN.md",
            "src/cli/output.rs",
            "src/commands/acceptance.rs",
            "src/commands/gate.rs",
            "src/commands/integration.rs",
            "src/control/repository.rs",
            "src/domain/acceptance.rs",
            "src/domain/card.rs",
            "src/domain/gate.rs",
            "src/domain/handoff.rs",
            "src/domain/review.rs",
            "src/error.rs",
            "src/main.rs",
            "src/policy/actors.rs",
            "src/policy/hygiene.rs",
            "src/policy/mod.rs",
            "src/runner/mod.rs",
            "tests/audit.rs",
            "tests/authority.rs",
            "tests/gate_registry.rs",
            "tests/promotion.rs",
            "tests/record_hygiene.rs",
            "tests/review.rs",
        ]));
        assert_eq!(f027.paths, 24);

        let advisory = f027.advisory().expect("F-027 must be flagged");
        assert!(advisory.contains("24 path(s)"), "{advisory}");
        assert!(
            advisory.contains("splitting now is far cheaper"),
            "the advisory has to say what to do about it: {advisory}"
        );
    }

    #[test]
    fn the_path_threshold_is_exclusive_and_decides_on_paths_alone() {
        // Both sides of the boundary, in one test, so neither can drift on its
        // own. Round 1 of this card's own review found `<` where the
        // declaration promised "more than", so exactly the threshold fired.
        let at: Vec<String> = (0..BROAD_PATH_COUNT)
            .map(|index| format!("src/domain/f{index}.rs"))
            .collect();
        let breadth = ScopeBreadth::measure(&at);
        assert_eq!(breadth.paths, BROAD_PATH_COUNT);
        assert_eq!(breadth.areas, 1, "all one area, so paths decide this");
        assert!(
            breadth.advisory().is_none(),
            "exactly {BROAD_PATH_COUNT} paths is the widest a card may be and stay quiet"
        );

        let mut over = at;
        over.push("src/domain/one_more.rs".to_owned());
        assert!(
            ScopeBreadth::measure(&over).advisory().is_some(),
            "one path past it is not"
        );
    }

    #[test]
    fn the_area_threshold_is_exclusive_and_decides_on_areas_alone() {
        let at = paths(&[
            "src/policy/a.rs",
            "src/runner/b.rs",
            "tests/c.rs",
            "docs/d.md",
        ]);
        let breadth = ScopeBreadth::measure(&at);
        assert_eq!(breadth.areas, BROAD_AREA_COUNT);
        assert!(
            breadth.paths <= BROAD_PATH_COUNT,
            "the path count must not be what decides this"
        );
        assert!(
            breadth.advisory().is_none(),
            "exactly {BROAD_AREA_COUNT} areas is the widest a card may be and stay quiet"
        );

        let mut over = at;
        over.push(".claude/skills/e.md".to_owned());
        let wider = ScopeBreadth::measure(&over);
        assert_eq!(wider.areas, BROAD_AREA_COUNT + 1);
        assert!(wider.advisory().is_some(), "one area past it is not");
    }

    #[test]
    fn a_glob_under_src_counts_as_its_own_area() {
        let breadth = ScopeBreadth::measure(&paths(&["src/policy/**", "src/commands/**"]));
        assert_eq!(breadth.area_names, vec!["src/commands", "src/policy"]);
        let bare = ScopeBreadth::measure(&paths(&["src/**"]));
        assert_eq!(bare.area_names, vec!["src"], "a bare src glob is one area");
    }

    #[test]
    fn a_trend_says_nothing_before_it_can_mean_anything() {
        // Two points is a line through any two numbers.
        for rounds in 1..MIN_ROUNDS_FOR_TREND {
            let history: Vec<Round> = (0..rounds).map(|_| Round::new(["src/a.rs"])).collect();
            assert!(
                Trend::measure(&history).advisory().is_none(),
                "{rounds} round(s) is not a trend"
            );
        }
    }

    #[test]
    fn a_card_that_is_settling_is_left_alone() {
        // Twelve findings becoming six becoming one is a card being finished,
        // and nagging it about its size would train people to ignore this.
        let history = [
            Round::new(["src/a.rs", "src/b.rs", "src/c.rs"]),
            Round::new(["src/a.rs", "src/b.rs"]),
            Round::new(["src/a.rs"]),
        ];
        let trend = Trend::measure(&history);
        assert_eq!(trend.per_round, vec![3, 2, 1]);
        assert!(!trend.is_flat());
        assert!(trend.advisory().is_none());
    }

    #[test]
    fn findings_that_stay_flat_are_flagged() {
        let history = [
            Round::new(["src/a.rs", "src/b.rs"]),
            Round::new(["src/a.rs", "src/b.rs"]),
            Round::new(["src/a.rs", "src/b.rs"]),
        ];
        let advisory = Trend::measure(&history)
            .advisory()
            .expect("flat is a signal");
        assert!(advisory.contains("2 → 2 → 2"), "{advisory}");
        assert!(advisory.contains("not falling"), "{advisory}");
        assert!(
            advisory.contains("not a refusal"),
            "an advisory has to say it is advisory: {advisory}"
        );
    }

    #[test]
    fn findings_that_keep_moving_to_new_areas_are_flagged_even_while_falling() {
        // The F-027 shape, and the reason volume alone is the wrong measure:
        // the count came down while every round found a defect somewhere the
        // last round had not looked.
        let history = [
            Round::new(["src/policy/hygiene.rs", "src/domain/card.rs", "src/main.rs"]),
            Round::new(["src/control/repository.rs", "src/policy/hygiene.rs"]),
            Round::new(["tests/authority.rs"]),
        ];
        let trend = Trend::measure(&history);
        assert!(!trend.is_flat(), "the count is falling");
        assert_eq!(trend.new_areas, vec!["tests"], "an area, not a location");

        let advisory = trend.advisory().expect("spreading is a signal on its own");
        assert!(advisory.contains("no earlier round named"), "{advisory}");
    }

    #[test]
    fn an_area_named_two_rounds_ago_is_not_new() {
        // Round 5 of this card's own review: nothing pinned "no *earlier*
        // round" as meaning all of them rather than the last one. Restricting
        // the comparison to the previous round alone passed every committed
        // test, because each history either held its areas throughout or
        // introduced one no round had ever named.
        //
        // Here round 3 returns to `tests`, which round 1 named and round 2 did
        // not. A card revisiting ground it has already been over is not
        // spreading.
        let history = [
            Round::new(["tests/a.rs", "tests/b.rs", "tests/c.rs"]),
            Round::new(["docs/x.md", "docs/y.md"]),
            Round::new(["tests/d.rs"]),
        ];
        let trend = Trend::measure(&history);
        assert_eq!(trend.per_round, vec![3, 2, 1]);
        assert!(
            trend.new_areas.is_empty(),
            "round 1 named `tests`, so round 3 is not new ground: {:?}",
            trend.new_areas
        );
        assert!(trend.advisory().is_none());
    }

    #[test]
    fn a_new_file_inside_an_area_already_named_is_not_spreading() {
        // Round 1 of this card's own review. The trend compared raw finding
        // locations while calling them areas, so a card being worked through
        // one module file by file — the most ordinary shape a converging card
        // has — was told it was spreading somewhere new every round.
        let history = [
            Round::new(["src/policy/a.rs", "src/policy/b.rs", "src/policy/c.rs"]),
            Round::new(["src/policy/a.rs", "src/policy/b.rs"]),
            Round::new(["src/policy/d.rs"]),
        ];
        let trend = Trend::measure(&history);
        assert_eq!(trend.per_round, vec![3, 2, 1]);
        assert!(
            trend.new_areas.is_empty(),
            "d.rs is new, src/policy is not: {:?}",
            trend.new_areas
        );
        assert!(
            trend.advisory().is_none(),
            "a card converging inside one area must not be nagged"
        );
    }

    #[test]
    fn a_card_that_fell_and_then_stopped_is_flat() {
        // Round 2 of this card's own review. Measured against the *first*
        // round, 5 → 3 → 3 looked like progress because 3 is below 5, so a
        // card that made early headway and then stalled — the most common
        // shape a stuck card has — was told it was converging.
        let history = [
            Round::new(["src/policy/a.rs"; 5]),
            Round::new(["src/policy/a.rs"; 3]),
            Round::new(["src/policy/a.rs"; 3]),
        ];
        let trend = Trend::measure(&history);
        assert_eq!(trend.per_round, vec![5, 3, 3]);
        assert!(
            trend.new_areas.is_empty(),
            "nothing new, so the flat count is the only thing that can speak"
        );
        assert!(trend.is_flat(), "3 is no better than the 3 before it");

        let advisory = trend.advisory().expect("a plateau is a signal");
        assert!(advisory.contains("5 → 3 → 3"), "{advisory}");
        assert!(advisory.contains("not falling"), "{advisory}");
    }

    #[test]
    fn regaining_ground_already_held_is_not_progress() {
        // 3 → 4 → 3 is back where it started. Comparing against the round
        // immediately before would call this falling.
        let history = [
            Round::new(["src/a.rs"; 3]),
            Round::new(["src/a.rs"; 4]),
            Round::new(["src/a.rs"; 3]),
        ];
        assert!(Trend::measure(&history).is_flat());
    }

    #[test]
    fn a_long_card_making_steady_progress_is_never_nagged() {
        // Every round a new low. The check must stay quiet for as long as
        // that holds, however many rounds it takes.
        let history: Vec<Round> = [10, 9, 8, 7, 6, 5]
            .into_iter()
            .map(|count| Round::new(std::iter::repeat_n("src/a.rs", count)))
            .collect();
        let trend = Trend::measure(&history);
        assert_eq!(trend.per_round, vec![10, 9, 8, 7, 6, 5]);
        assert!(!trend.is_flat());
        assert!(trend.advisory().is_none());
    }

    #[test]
    fn one_path_declared_many_times_is_one_path() {
        // Round 2 of this card's own review: `include.len()` counted entries,
        // so thirteen copies of one file were reported as "13 path(s)" — a
        // number that is false, and that the envelope publishes.
        let repeated = paths(&["src/policy/actors.rs"; 13]);
        let breadth = ScopeBreadth::measure(&repeated);
        assert_eq!(breadth.paths, 1, "one distinct path");
        assert_eq!(breadth.areas, 1);
        assert!(
            breadth.advisory().is_none(),
            "and so nothing to say about it"
        );
    }

    #[test]
    fn one_path_spelled_many_ways_is_one_path() {
        // Round 3 of this card's own review, verbatim: the same file written
        // thirteen ways. Round 2 had fixed identical strings, which is the
        // narrower half of the same defect — the count was still comparing
        // spellings rather than paths.
        let aliased = paths(&[
            "src/a.rs",
            "./src/a.rs",
            ".//src/a.rs",
            "././src/a.rs",
            "src//a.rs",
            "src///a.rs",
            "src/./a.rs",
            "src/.//a.rs",
            "src//./a.rs",
            "src/a.rs/",
            "src/a.rs//",
            "./src/a.rs/",
            ".//src//a.rs",
        ]);
        let breadth = ScopeBreadth::measure(&aliased);
        assert_eq!(breadth.paths, 1, "thirteen spellings of one file");
        assert_eq!(breadth.areas, 1, "and therefore one area, not two");
        assert!(breadth.advisory().is_none());
    }

    #[test]
    fn finding_locations_are_compared_as_paths_too() {
        // `measure` canonicalizes before it asks for areas, so the scope tests
        // cannot see whether `area_of` does it as well. Finding locations come
        // straight from a reviewer's verdict and are never canonicalized on
        // the way in, which makes this the only place that property is
        // observable — and a reviewer writing `./src/policy/b.rs` in round 3
        // must not read as the card spreading somewhere new.
        let history = [
            Round::new(["src/policy/a.rs", "src/policy/b.rs", "src/policy/c.rs"]),
            Round::new(["src/policy/a.rs", "src/policy/b.rs"]),
            Round::new(["./src/./policy//d.rs"]),
        ];
        let trend = Trend::measure(&history);
        assert!(
            trend.new_areas.is_empty(),
            "a differently spelled path is not a new area: {:?}",
            trend.new_areas
        );
        assert!(trend.advisory().is_none(), "and so there is nothing to say");
    }

    #[test]
    fn case_aliases_follow_the_host() {
        // Not an exotic input — a typo in one include entry. On macOS these
        // are the same file, and the harness already treats them as the same
        // path everywhere else; this asks that the count agree.
        let mixed = paths(&["src/policy/a.rs", "SRC/Policy/a.rs"]);
        let breadth = ScopeBreadth::measure(&mixed);
        let expected = if CaseSensitivity::host() == CaseSensitivity::Insensitive {
            1
        } else {
            2
        };
        assert_eq!(
            breadth.paths, expected,
            "path identity must match the host the harness is running on"
        );
        assert_eq!(breadth.areas, expected);
    }

    #[test]
    fn a_round_with_nothing_open_is_not_flat() {
        // An approval closes the card. Zero open findings is the end state, not
        // a plateau, however many rounds it took to get there.
        let history = [
            Round::new(["src/a.rs"]),
            Round::new(["src/a.rs"]),
            Round::new([]),
        ];
        let trend = Trend::measure(&history);
        assert!(!trend.is_flat());
        assert!(trend.advisory().is_none());
    }

    #[test]
    fn several_open_findings_at_one_location_count_separately() {
        // Round 1 of this card's own review, and the worse half of the same
        // mistake: counting the *set* of locations read a card stuck at three
        // open findings as converging 3 → 2 → 1 and said nothing. Two findings
        // in one file are two problems.
        let round = Round::new(["src/a.rs", "src/a.rs", "src/b.rs"]);
        assert_eq!(round.open_findings, 3, "three findings");
        assert_eq!(round.areas.len(), 2, "but two areas");

        let stuck = [
            Round::new(["src/a.rs", "src/b.rs", "src/c.rs"]),
            Round::new(["src/a.rs", "src/a.rs", "src/b.rs"]),
            Round::new(["src/a.rs", "src/a.rs", "src/a.rs"]),
        ];
        let trend = Trend::measure(&stuck);
        assert_eq!(
            trend.per_round,
            vec![3, 3, 3],
            "three open findings in every round"
        );
        assert!(trend.is_flat());
        assert!(
            trend.advisory().is_some(),
            "a card stuck at three open findings must not read as converging"
        );
    }

    #[test]
    fn omitted_convergence_policy_is_legacy_unassessed() {
        let project_id = ProjectId::from_str("example").unwrap();
        let cycle = CycleId::from_str("C-001").unwrap();
        let view = project(
            None,
            &project_id,
            &cycle,
            &[event(1, AttemptKind::RepairAttempt)],
        )
        .unwrap();
        assert_eq!(view, ProjectConvergence::LegacyUnassessed);
        assert_eq!(
            serde_json::to_string(&view).unwrap(),
            r#"{"status":"legacy_unassessed"}"#
        );
    }

    #[test]
    fn configured_convergence_policy_validates_and_has_a_stable_digest() {
        let policy = policy();
        policy.validate().unwrap();
        assert_eq!(policy.digest().unwrap(), policy.digest().unwrap());
        let mut invalid = policy.clone();
        invalid.card_limits.low.gate_failures = 0;
        assert!(invalid.validate().is_err());
        let mut unsupported = policy.clone();
        unsupported.version = "harness.convergence-policy/v99".to_owned();
        assert!(unsupported.validate().is_err());
        let mut changed = policy.clone();
        changed.cycle_limits.integration_failures = 4;
        assert_ne!(policy.digest().unwrap(), changed.digest().unwrap());
    }

    #[test]
    fn fixed_attempt_facts_project_deterministic_card_and_cycle_counters() {
        let project_id = ProjectId::from_str("example").unwrap();
        let cycle = CycleId::from_str("C-001").unwrap();
        let facts = vec![
            event(1, AttemptKind::ReviewReturn),
            event(2, AttemptKind::RepairAttempt),
            event(3, AttemptKind::GateFailure),
            event(4, AttemptKind::MaterialScopeRevision),
            event(5, AttemptKind::IntegrationFailure),
        ];
        let first = project(Some(&policy()), &project_id, &cycle, &facts).unwrap();
        let mut shuffled = facts;
        shuffled.reverse();
        let second = project(Some(&policy()), &project_id, &cycle, &shuffled).unwrap();
        // Equality on the whole projection compares each `DimensionCount`'s
        // `evidence` as a `BTreeSet`, so this also proves the recorded
        // evidence does not depend on the order the facts arrived in.
        assert_eq!(first, second);
        let ProjectConvergence::Configured(view) = first else {
            panic!("configured")
        };
        assert_eq!(view.cycle.integration_failures.count, 1);
        assert_eq!(
            view.cards[&CardId::from_str("F-001").unwrap()],
            CardCounters {
                review_returns: DimensionCount {
                    count: 1,
                    evidence: BTreeSet::from(["receipt:1".to_owned()]),
                    renewals: 0,
                },
                repair_attempts: DimensionCount {
                    count: 1,
                    evidence: BTreeSet::from(["receipt:2".to_owned()]),
                    renewals: 0,
                },
                gate_failures: DimensionCount {
                    count: 1,
                    evidence: BTreeSet::from(["receipt:3".to_owned()]),
                    renewals: 0,
                },
                material_scope_revisions: DimensionCount {
                    count: 1,
                    evidence: BTreeSet::from(["receipt:4".to_owned()]),
                    renewals: 0,
                },
            }
        );
        assert_eq!(
            serde_json::to_string(&ProjectConvergence::Configured(view.clone())).unwrap(),
            format!(
                r#"{{"status":"configured","policy_digest":"{}","cycle":{{"integration_failures":{{"count":1,"evidence":["receipt:5"],"renewals":0}}}},"cards":{{"F-001":{{"review_returns":{{"count":1,"evidence":["receipt:1"],"renewals":0}},"repair_attempts":{{"count":1,"evidence":["receipt:2"],"renewals":0}},"gate_failures":{{"count":1,"evidence":["receipt:3"],"renewals":0}},"material_scope_revisions":{{"count":1,"evidence":["receipt:4"],"renewals":0}}}}}}}}"#,
                view.policy_digest
            )
        );
    }

    #[test]
    fn malformed_duplicate_or_unbound_attempt_facts_fail_closed_without_partial_counts() {
        let project_id = ProjectId::from_str("example").unwrap();
        let cycle = CycleId::from_str("C-001").unwrap();
        let good = event(1, AttemptKind::RepairAttempt);
        let mut malformed = event(2, AttemptKind::GateFailure);
        malformed.metadata.remove("evidence_ref");
        assert!(
            project(
                Some(&policy()),
                &project_id,
                &cycle,
                &[good.clone(), malformed]
            )
            .is_err()
        );
        assert!(project(Some(&policy()), &project_id, &cycle, &[good.clone(), good]).is_err());
        let mut unbound = event(3, AttemptKind::ReviewReturn);
        unbound.head_sha = None;
        assert!(project(Some(&policy()), &project_id, &cycle, &[unbound]).is_err());
    }

    #[test]
    fn foreign_policy_fact_is_refused_instead_of_counted() {
        let project_id = ProjectId::from_str("example").unwrap();
        let cycle = CycleId::from_str("C-001").unwrap();
        let mut foreign = event(1, AttemptKind::RepairAttempt);
        foreign.metadata.insert(
            "policy_digest".to_owned(),
            serde_json::json!(Digest::of_bytes(b"foreign").as_str()),
        );
        assert!(project(Some(&policy()), &project_id, &cycle, &[foreign]).is_err());
    }

    #[test]
    fn an_integration_failure_without_an_exact_head_is_refused() {
        // The half of the old `mixed_card_revisions_and_missing_integration_
        // head_fail_closed` that is still a rejection: mixed card revisions
        // moved to `attempts_across_card_revisions_accumulate_into_one_
        // budget`, where mixing revisions is the ordinary case, not a
        // refusal.
        let project_id = ProjectId::from_str("example").unwrap();
        let cycle = CycleId::from_str("C-001").unwrap();
        let mut integration = event(1, AttemptKind::IntegrationFailure);
        integration.head_sha = None;
        assert!(project(Some(&policy()), &project_id, &cycle, &[integration]).is_err());
    }

    #[test]
    fn attempts_across_card_revisions_accumulate_into_one_budget() {
        // The shape a revised card actually produces: the same card, the
        // same dimension, two different revisions each with their own
        // digest and head. Before this change, the second fact's differing
        // binding made the whole projection fail closed forever — so a card
        // could never be revised more than once and keep a working budget.
        let project_id = ProjectId::from_str("example").unwrap();
        let cycle = CycleId::from_str("C-001").unwrap();
        let first = event(1, AttemptKind::RepairAttempt);
        let mut second = event(2, AttemptKind::RepairAttempt);
        second.card_revision = Some(2);
        second.card_digest = Some(Digest::of_bytes(b"card-revision-2"));
        second.head_sha = Some("ab".repeat(20));

        let view = project(Some(&policy()), &project_id, &cycle, &[first, second]).unwrap();
        let ProjectConvergence::Configured(view) = view else {
            panic!("configured")
        };
        let counters = &view.cards[&CardId::from_str("F-001").unwrap()];
        assert_eq!(
            counters.repair_attempts.count, 2,
            "both revisions' attempts count toward the one budget"
        );
        assert_eq!(
            counters.repair_attempts.evidence,
            BTreeSet::from(["receipt:1".to_owned(), "receipt:2".to_owned()]),
            "the evidence behind each revision's attempt is retained"
        );
    }

    #[test]
    fn a_material_scope_revision_is_counted_alongside_earlier_attempts() {
        // The real flow, not a contrived one: a card is returned in review
        // once, then its scope is revised before the next attempt.
        // `MaterialScopeRevision` records that a new revision now exists, so
        // it can never share a revision with the `ReviewReturn` before it —
        // that pairing is not an edge case, it is what recording a scope
        // change looks like. Before this change, that ordinary sequence left
        // the whole projection in permanent error.
        let project_id = ProjectId::from_str("example").unwrap();
        let cycle = CycleId::from_str("C-001").unwrap();
        let review_return = event(1, AttemptKind::ReviewReturn);
        let mut scope_revision = event(2, AttemptKind::MaterialScopeRevision);
        scope_revision.card_revision = Some(2);
        scope_revision.card_digest = Some(Digest::of_bytes(b"card-revision-2"));
        scope_revision.head_sha = Some("cd".repeat(20));

        let view = project(
            Some(&policy()),
            &project_id,
            &cycle,
            &[review_return, scope_revision],
        )
        .unwrap();
        let ProjectConvergence::Configured(view) = view else {
            panic!("configured")
        };
        let counters = &view.cards[&CardId::from_str("F-001").unwrap()];
        assert_eq!(counters.review_returns.count, 1);
        assert_eq!(counters.material_scope_revisions.count, 1);
    }

    #[test]
    fn every_counted_dimension_retains_its_evidence_reference() {
        // One fact of each class, spread across two cards so both halves of
        // the claim show up in one run: `F-001` shows a dimension holds
        // exactly the reference of the fact that hit it, and no other
        // dimension's reference leaks in. `F-002`, which only ever receives
        // a `ReviewReturn`, shows that a dimension no fact ever touched is
        // still present in the output at its zero default — `count: 0` and
        // empty evidence — rather than missing.
        let project_id = ProjectId::from_str("example").unwrap();
        let cycle = CycleId::from_str("C-001").unwrap();
        let review_return = event(1, AttemptKind::ReviewReturn);
        let repair_attempt = event(2, AttemptKind::RepairAttempt);
        let gate_failure = event(3, AttemptKind::GateFailure);
        let scope_revision = event(4, AttemptKind::MaterialScopeRevision);
        let integration_failure = event(5, AttemptKind::IntegrationFailure);
        let mut second_card = event(6, AttemptKind::ReviewReturn);
        second_card.card_id = Some(CardId::from_str("F-002").unwrap());

        let view = project(
            Some(&policy()),
            &project_id,
            &cycle,
            &[
                review_return,
                repair_attempt,
                gate_failure,
                scope_revision,
                integration_failure,
                second_card,
            ],
        )
        .unwrap();
        let ProjectConvergence::Configured(view) = view else {
            panic!("configured")
        };

        let f001 = &view.cards[&CardId::from_str("F-001").unwrap()];
        assert_eq!(
            f001.review_returns.evidence,
            BTreeSet::from(["receipt:1".to_owned()])
        );
        assert_eq!(
            f001.repair_attempts.evidence,
            BTreeSet::from(["receipt:2".to_owned()])
        );
        assert_eq!(
            f001.gate_failures.evidence,
            BTreeSet::from(["receipt:3".to_owned()])
        );
        assert_eq!(
            f001.material_scope_revisions.evidence,
            BTreeSet::from(["receipt:4".to_owned()])
        );
        assert_eq!(
            view.cycle.integration_failures.evidence,
            BTreeSet::from(["receipt:5".to_owned()])
        );

        let f002 = &view.cards[&CardId::from_str("F-002").unwrap()];
        assert_eq!(
            f002.review_returns,
            DimensionCount {
                count: 1,
                evidence: BTreeSet::from(["receipt:6".to_owned()]),
                renewals: 0,
            },
        );
        assert_eq!(f002.repair_attempts, DimensionCount::default());
        assert_eq!(f002.gate_failures, DimensionCount::default());
        assert_eq!(f002.material_scope_revisions, DimensionCount::default());
    }

    #[test]
    fn a_card_attempt_without_an_exact_revision_or_digest_is_refused() {
        // With the cross-fact bindings map gone, `card_revision` and
        // `card_digest` have no safety net left but this per-fact check.
        // Losing it silently would let an attempt with no revision, or no
        // digest, at all still count toward a card's budget.
        let project_id = ProjectId::from_str("example").unwrap();
        let cycle = CycleId::from_str("C-001").unwrap();
        let mut missing_revision = event(1, AttemptKind::RepairAttempt);
        missing_revision.card_revision = None;
        assert!(project(Some(&policy()), &project_id, &cycle, &[missing_revision]).is_err());

        let mut missing_digest = event(2, AttemptKind::RepairAttempt);
        missing_digest.card_digest = None;
        assert!(project(Some(&policy()), &project_id, &cycle, &[missing_digest]).is_err());
    }

    #[test]
    fn a_dimension_exactly_at_its_limit_is_exhausted() {
        // The frontier this card exists to get right: two review returns
        // against a limit of two is the budget spent, not one short of
        // spent.
        let project_id = ProjectId::from_str("example").unwrap();
        let cycle = CycleId::from_str("C-001").unwrap();
        let card_id = CardId::from_str("F-001").unwrap();
        let facts = vec![
            event(1, AttemptKind::ReviewReturn),
            event(2, AttemptKind::ReviewReturn),
        ];
        let view = project(Some(&policy()), &project_id, &cycle, &facts).unwrap();

        assert_eq!(
            assess_card(Some(&policy()), &view, &card_id, Risk::Medium),
            CardConvergence::Escalated {
                exhausted: vec![ExhaustedDimension {
                    dimension: CardDimension::ReviewReturns,
                    count: 2,
                    limit: 2,
                    evidence: BTreeSet::from(["receipt:1".to_owned(), "receipt:2".to_owned()]),
                }],
                next_permitted_action: NextPermittedAction::RecordAuthorizedDisposition,
            }
        );
    }

    #[test]
    fn a_dimension_one_below_its_limit_still_has_budget() {
        // The other half of the same frontier, in its own test so neither
        // side can drift without the other noticing.
        let project_id = ProjectId::from_str("example").unwrap();
        let cycle = CycleId::from_str("C-001").unwrap();
        let card_id = CardId::from_str("F-001").unwrap();
        let facts = vec![event(1, AttemptKind::ReviewReturn)];
        let view = project(Some(&policy()), &project_id, &cycle, &facts).unwrap();

        assert_eq!(
            assess_card(Some(&policy()), &view, &card_id, Risk::Medium),
            CardConvergence::Within,
            "one review return against a limit of two is one attempt still available"
        );
    }

    #[test]
    fn every_exhausted_dimension_is_reported_not_only_the_first() {
        // Review returns and gate failures both spent; repair attempts and
        // material scope revisions both untouched. An operator who only
        // learns about review_returns here, fixes it, and then meets
        // gate_failures on the next attempt has lost a cycle for nothing.
        let project_id = ProjectId::from_str("example").unwrap();
        let cycle = CycleId::from_str("C-001").unwrap();
        let card_id = CardId::from_str("F-001").unwrap();
        let facts = vec![
            event(1, AttemptKind::ReviewReturn),
            event(2, AttemptKind::ReviewReturn),
            event(3, AttemptKind::GateFailure),
            event(4, AttemptKind::GateFailure),
        ];
        let view = project(Some(&policy()), &project_id, &cycle, &facts).unwrap();

        let CardConvergence::Escalated { exhausted, .. } =
            assess_card(Some(&policy()), &view, &card_id, Risk::Medium)
        else {
            panic!("two spent dimensions must escalate the card");
        };
        assert_eq!(
            exhausted
                .iter()
                .map(|item| item.dimension)
                .collect::<Vec<_>>(),
            vec![CardDimension::ReviewReturns, CardDimension::GateFailures],
            "both exhausted dimensions must appear, in CardDimension's declared order"
        );
    }

    #[test]
    fn the_budget_follows_the_cards_declared_risk() {
        // Same one recorded review return, two risk tiers with different
        // review_returns limits: low risk still has budget, high risk does
        // not. If this read a fixed tier instead of the card's own declared
        // risk, both assertions below would see the same answer.
        let low_limits = CardConvergenceLimits {
            review_returns: 5,
            repair_attempts: 5,
            gate_failures: 5,
            material_scope_revisions: 5,
        };
        let high_limits = CardConvergenceLimits {
            review_returns: 1,
            repair_attempts: 5,
            gate_failures: 5,
            material_scope_revisions: 5,
        };
        let tiered_policy = ConvergencePolicy {
            version: crate::config::CONVERGENCE_POLICY_V1.to_owned(),
            card_limits: RiskConvergenceLimits {
                low: low_limits,
                medium: high_limits.clone(),
                high: high_limits.clone(),
                critical: high_limits,
            },
            cycle_limits: CycleConvergenceLimits {
                integration_failures: 2,
            },
        };
        let card_id = CardId::from_str("F-001").unwrap();
        let view = ProjectConvergence::Configured(ConvergenceProjection {
            policy_digest: tiered_policy.digest().unwrap(),
            cycle: CycleCounters::default(),
            cards: BTreeMap::from([(
                card_id.clone(),
                CardCounters {
                    review_returns: DimensionCount {
                        count: 1,
                        evidence: BTreeSet::from(["receipt:1".to_owned()]),
                        renewals: 0,
                    },
                    ..CardCounters::default()
                },
            )]),
        });

        assert_eq!(
            assess_card(Some(&tiered_policy), &view, &card_id, Risk::Low),
            CardConvergence::Within,
            "low risk grants five review returns; one is nowhere near spent"
        );
        assert_eq!(
            assess_card(Some(&tiered_policy), &view, &card_id, Risk::High),
            CardConvergence::Escalated {
                exhausted: vec![ExhaustedDimension {
                    dimension: CardDimension::ReviewReturns,
                    count: 1,
                    limit: 1,
                    evidence: BTreeSet::from(["receipt:1".to_owned()]),
                }],
                next_permitted_action: NextPermittedAction::RecordAuthorizedDisposition,
            },
            "high risk grants only one review return, and it is already spent"
        );
    }

    #[test]
    fn a_card_with_no_recorded_facts_is_within_budget() {
        let project_id = ProjectId::from_str("example").unwrap();
        let cycle = CycleId::from_str("C-001").unwrap();
        let card_id = CardId::from_str("F-001").unwrap();
        let view = project(Some(&policy()), &project_id, &cycle, &[]).unwrap();

        assert_eq!(
            assess_card(Some(&policy()), &view, &card_id, Risk::Medium),
            CardConvergence::Within,
            "absence of facts is not absence of budget"
        );
    }

    #[test]
    fn an_unconfigured_policy_assesses_as_legacy_unassessed() {
        let project_id = ProjectId::from_str("example").unwrap();
        let cycle = CycleId::from_str("C-001").unwrap();
        let card_id = CardId::from_str("F-001").unwrap();
        let facts = vec![
            event(1, AttemptKind::ReviewReturn),
            event(2, AttemptKind::ReviewReturn),
        ];
        let configured_view = project(Some(&policy()), &project_id, &cycle, &facts).unwrap();

        assert_eq!(
            assess_card(None, &configured_view, &card_id, Risk::Medium),
            CardConvergence::LegacyUnassessed,
            "no configured policy means nothing to enforce, even though these \
             facts would otherwise exhaust the budget"
        );
        assert_eq!(
            assess_card(
                Some(&policy()),
                &ProjectConvergence::LegacyUnassessed,
                &card_id,
                Risk::Medium
            ),
            CardConvergence::LegacyUnassessed,
            "a legacy project has no implicit budget even once a policy exists"
        );
    }

    #[test]
    fn only_the_named_card_is_assessed() {
        let project_id = ProjectId::from_str("example").unwrap();
        let cycle = CycleId::from_str("C-001").unwrap();
        let exhausted_card = CardId::from_str("F-001").unwrap();
        let quiet_card = CardId::from_str("F-002").unwrap();
        let mut third = event(3, AttemptKind::ReviewReturn);
        third.card_id = Some(quiet_card.clone());
        let facts = vec![
            event(1, AttemptKind::ReviewReturn),
            event(2, AttemptKind::ReviewReturn),
            third,
        ];
        let view = project(Some(&policy()), &project_id, &cycle, &facts).unwrap();

        assert!(
            matches!(
                assess_card(Some(&policy()), &view, &exhausted_card, Risk::Medium),
                CardConvergence::Escalated { .. }
            ),
            "F-001 has spent its review_returns budget"
        );
        assert_eq!(
            assess_card(Some(&policy()), &view, &quiet_card, Risk::Medium),
            CardConvergence::Within,
            "F-002 has one review return against a limit of two and must not \
             inherit F-001's exhaustion"
        );
    }

    #[test]
    fn the_three_shapes_serialize_exactly() {
        assert_eq!(
            serde_json::to_string(&CardConvergence::LegacyUnassessed).unwrap(),
            r#"{"status":"legacy_unassessed"}"#
        );
        assert_eq!(
            serde_json::to_string(&CardConvergence::Within).unwrap(),
            r#"{"status":"within"}"#
        );

        let project_id = ProjectId::from_str("example").unwrap();
        let cycle = CycleId::from_str("C-001").unwrap();
        let card_id = CardId::from_str("F-001").unwrap();
        let mut first = event(1, AttemptKind::ReviewReturn);
        first.metadata.insert(
            "evidence_ref".to_owned(),
            serde_json::json!("review:RV-000001"),
        );
        let mut second = event(2, AttemptKind::ReviewReturn);
        second.metadata.insert(
            "evidence_ref".to_owned(),
            serde_json::json!("review:RV-000002"),
        );
        let view = project(Some(&policy()), &project_id, &cycle, &[first, second]).unwrap();

        let escalated = assess_card(Some(&policy()), &view, &card_id, Risk::Medium);
        assert_eq!(
            serde_json::to_string(&escalated).unwrap(),
            r#"{"status":"escalated","exhausted":[{"dimension":"review_returns","count":2,"limit":2,"evidence":["review:RV-000001","review:RV-000002"]}],"next_permitted_action":"record_authorized_disposition"}"#
        );
    }

    #[test]
    fn a_renewal_grants_exactly_one_more_budget() {
        // The central test. Limit 2, spent twice, is escalated on its own —
        // register exactly one authorized renewal against the same facts
        // and the card is `Within` again.
        let project_id = ProjectId::from_str("example").unwrap();
        let cycle = CycleId::from_str("C-001").unwrap();
        let card_id = CardId::from_str("F-001").unwrap();
        let facts = vec![
            event(1, AttemptKind::ReviewReturn),
            event(2, AttemptKind::ReviewReturn),
        ];
        let view = project(Some(&policy()), &project_id, &cycle, &facts).unwrap();
        assert!(
            matches!(
                assess_card(Some(&policy()), &view, &card_id, Risk::Medium),
                CardConvergence::Escalated { .. }
            ),
            "two review returns against a limit of two, with no renewal, must escalate"
        );

        let mut renewed = facts;
        renewed.push(disposition_event(3, CardDimension::ReviewReturns));
        let renewed_view = project(Some(&policy()), &project_id, &cycle, &renewed).unwrap();
        assert_eq!(
            assess_card(Some(&policy()), &renewed_view, &card_id, Risk::Medium),
            CardConvergence::Within,
            "one authorized renewal grants the configured limit again"
        );
    }

    #[test]
    fn a_renewed_budget_escalates_again_once_it_is_spent() {
        // limit 2, one renewal -> effective budget 2 * (1 + 1) = 4. Four
        // review returns spend all of it, and the reported `limit` is the
        // effective budget actually compared against, not the base 2.
        let project_id = ProjectId::from_str("example").unwrap();
        let cycle = CycleId::from_str("C-001").unwrap();
        let card_id = CardId::from_str("F-001").unwrap();
        let mut facts: Vec<Event> = (1u64..=4)
            .map(|id| event(id, AttemptKind::ReviewReturn))
            .collect();
        facts.push(disposition_event(5, CardDimension::ReviewReturns));
        let view = project(Some(&policy()), &project_id, &cycle, &facts).unwrap();

        let CardConvergence::Escalated { exhausted, .. } =
            assess_card(Some(&policy()), &view, &card_id, Risk::Medium)
        else {
            panic!("four review returns against an effective budget of four must escalate");
        };
        assert_eq!(exhausted.len(), 1);
        assert_eq!(exhausted[0].dimension, CardDimension::ReviewReturns);
        assert_eq!(exhausted[0].count, 4);
        assert_eq!(
            exhausted[0].limit, 4,
            "limit reports the effective budget (2 * (1 renewal + 1)), not the base 2"
        );
    }

    #[test]
    fn two_renewals_grant_two_budgets_and_no_more() {
        // limit 2, two renewals -> effective budget 2 * (2 + 1) = 6. Six
        // review returns spend it exactly. This is the test that catches a
        // renewal being treated as "unlimited" instead of "one more
        // configured budget".
        let project_id = ProjectId::from_str("example").unwrap();
        let cycle = CycleId::from_str("C-001").unwrap();
        let card_id = CardId::from_str("F-001").unwrap();
        let mut facts: Vec<Event> = (1u64..=6)
            .map(|id| event(id, AttemptKind::ReviewReturn))
            .collect();
        facts.push(disposition_event(7, CardDimension::ReviewReturns));
        facts.push(disposition_event(8, CardDimension::ReviewReturns));
        let view = project(Some(&policy()), &project_id, &cycle, &facts).unwrap();

        let CardConvergence::Escalated { exhausted, .. } =
            assess_card(Some(&policy()), &view, &card_id, Risk::Medium)
        else {
            panic!(
                "two renewals grant exactly two extra budgets (6 total); six review returns must spend all of it"
            );
        };
        assert_eq!(
            exhausted[0].limit, 6,
            "two renewals means 2 * (2 + 1), not an unbounded budget"
        );
    }

    #[test]
    fn a_renewal_only_affects_the_dimension_it_names() {
        // A review_returns renewal must not rescue an exhausted
        // gate_failures dimension on the same card.
        let project_id = ProjectId::from_str("example").unwrap();
        let cycle = CycleId::from_str("C-001").unwrap();
        let card_id = CardId::from_str("F-001").unwrap();
        let facts = vec![
            event(1, AttemptKind::GateFailure),
            event(2, AttemptKind::GateFailure),
            disposition_event(3, CardDimension::ReviewReturns),
        ];
        let view = project(Some(&policy()), &project_id, &cycle, &facts).unwrap();

        let CardConvergence::Escalated { exhausted, .. } =
            assess_card(Some(&policy()), &view, &card_id, Risk::Medium)
        else {
            panic!("gate_failures must still be exhausted; the renewal named review_returns");
        };
        assert_eq!(
            exhausted
                .iter()
                .map(|item| item.dimension)
                .collect::<Vec<_>>(),
            vec![CardDimension::GateFailures],
            "only gate_failures is exhausted; review_returns was never spent"
        );
    }

    #[test]
    fn a_renewal_only_affects_the_card_it_names() {
        let project_id = ProjectId::from_str("example").unwrap();
        let cycle = CycleId::from_str("C-001").unwrap();
        let exhausted_card = CardId::from_str("F-001").unwrap();
        let other_card = CardId::from_str("F-002").unwrap();

        let mut renewal_for_other_card = disposition_event(3, CardDimension::ReviewReturns);
        renewal_for_other_card.card_id = Some(other_card);

        let facts = vec![
            event(1, AttemptKind::ReviewReturn),
            event(2, AttemptKind::ReviewReturn),
            renewal_for_other_card,
        ];
        let view = project(Some(&policy()), &project_id, &cycle, &facts).unwrap();

        assert!(
            matches!(
                assess_card(Some(&policy()), &view, &exhausted_card, Risk::Medium),
                CardConvergence::Escalated { .. }
            ),
            "F-001 never received a renewal; one recorded against F-002 must not rescue it"
        );
    }

    #[test]
    fn a_disposition_without_a_rationale_or_an_authorizing_actor_is_refused() {
        let project_id = ProjectId::from_str("example").unwrap();
        let cycle = CycleId::from_str("C-001").unwrap();

        let mut blank_rationale = disposition_event(1, CardDimension::ReviewReturns);
        blank_rationale
            .metadata
            .insert("rationale".to_owned(), serde_json::json!("   "));
        assert!(
            project(Some(&policy()), &project_id, &cycle, &[blank_rationale]).is_err(),
            "a decision with no declared reason is not a decision"
        );

        let mut blank_actor = disposition_event(2, CardDimension::ReviewReturns);
        blank_actor
            .metadata
            .insert("authorized_by".to_owned(), serde_json::json!(""));
        assert!(
            project(Some(&policy()), &project_id, &cycle, &[blank_actor]).is_err(),
            "a decision with no declared authorizing actor is not a decision"
        );
    }

    #[test]
    fn a_disposition_bound_to_a_foreign_policy_or_no_card_is_refused() {
        let project_id = ProjectId::from_str("example").unwrap();
        let cycle = CycleId::from_str("C-001").unwrap();

        let mut foreign = disposition_event(1, CardDimension::ReviewReturns);
        foreign.metadata.insert(
            "policy_digest".to_owned(),
            serde_json::json!(Digest::of_bytes(b"foreign").as_str()),
        );
        assert!(project(Some(&policy()), &project_id, &cycle, &[foreign]).is_err());

        let mut no_revision = disposition_event(2, CardDimension::ReviewReturns);
        no_revision.card_revision = None;
        assert!(project(Some(&policy()), &project_id, &cycle, &[no_revision]).is_err());

        let mut no_digest = disposition_event(3, CardDimension::ReviewReturns);
        no_digest.card_digest = None;
        assert!(project(Some(&policy()), &project_id, &cycle, &[no_digest]).is_err());

        let mut no_head = disposition_event(4, CardDimension::ReviewReturns);
        no_head.head_sha = None;
        assert!(project(Some(&policy()), &project_id, &cycle, &[no_head]).is_err());
    }

    #[test]
    fn renewals_are_independent_of_event_order() {
        let project_id = ProjectId::from_str("example").unwrap();
        let cycle = CycleId::from_str("C-001").unwrap();
        let facts = vec![
            event(1, AttemptKind::ReviewReturn),
            event(2, AttemptKind::ReviewReturn),
            disposition_event(3, CardDimension::ReviewReturns),
            disposition_event(4, CardDimension::GateFailures),
            event(5, AttemptKind::GateFailure),
        ];
        let first = project(Some(&policy()), &project_id, &cycle, &facts).unwrap();
        let mut shuffled = facts;
        shuffled.reverse();
        let second = project(Some(&policy()), &project_id, &cycle, &shuffled).unwrap();
        assert_eq!(
            first, second,
            "the same facts in a different order must project the same view"
        );
    }

    #[test]
    fn the_projection_json_carries_renewals() {
        let project_id = ProjectId::from_str("example").unwrap();
        let cycle = CycleId::from_str("C-001").unwrap();
        let facts = vec![
            event(1, AttemptKind::ReviewReturn),
            disposition_event(2, CardDimension::ReviewReturns),
        ];
        let view = project(Some(&policy()), &project_id, &cycle, &facts).unwrap();
        let ProjectConvergence::Configured(configured) = view else {
            panic!("configured")
        };
        let dimension = &configured.cards[&CardId::from_str("F-001").unwrap()].review_returns;
        assert_eq!(
            serde_json::to_string(dimension).unwrap(),
            r#"{"count":1,"evidence":["receipt:1"],"renewals":1}"#
        );
    }
}
