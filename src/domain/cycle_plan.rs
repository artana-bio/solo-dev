//! Versioned, deterministic planning facts for a complete cycle.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::error::{ErrorCode, HarnessError};

pub const CYCLE_PLAN_SCHEMA: &str = "harness.cycle-plan/v1";
pub const CYCLE_PLAN_REVISION: u32 = 1;
const MAX_CYCLE_PLAN_ID_LEN: usize = 64;
/// Creation-time policy marker written on every new cycle.
pub const CYCLE_PLAN_POLICY_REQUIRED: &str = "plan_required_v1";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Distribution {
    Parallel,
    Sequential,
    JointIntegration,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PlannedCard {
    pub card_id: String,
    pub card_revision: u32,
    pub scope: Vec<String>,
    /// The authoritative card's excluded scope. An omitted field is the
    /// empty exclusion set for compatibility with the first plan spelling.
    #[serde(default)]
    pub scope_exclude: Vec<String>,
    pub depends_on: Vec<String>,
    pub proof_entries: Vec<String>,
    pub mutation_plan: Vec<String>,
    pub risk: String,
    pub reviewer_requirements: Vec<String>,
    /// Declared actor label for the implementer assignment.
    pub assignment: Option<String>,
    /// Optional only while reading an explicitly migrated historical plan;
    /// newly authored plans must carry both principal and session boundaries.
    #[serde(default)]
    pub assignment_principal_id: Option<String>,
    #[serde(default)]
    pub assignment_session_id: Option<String>,
    pub distribution: Distribution,
    pub acceptance_behaviors: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CyclePlan {
    pub schema: String,
    pub plan_id: String,
    pub cycle_id: String,
    pub objective: String,
    pub cards: Vec<PlannedCard>,
}

impl CyclePlan {
    /// Returns the control-repository path for a validated plan identifier.
    ///
    /// # Errors
    ///
    /// Refuses identifiers that could escape the plans directory or create
    /// filesystem-dependent record names.
    pub fn relative_path(plan_id: &str) -> Result<String, HarnessError> {
        validate_plan_id(plan_id)?;
        Ok(format!("plans/{plan_id}.json"))
    }
    /// The stable digest used to bind this plan into lifecycle records.
    ///
    /// # Errors
    ///
    /// Returns an error if canonical serialization fails.
    pub fn digest(&self) -> Result<crate::domain::digest::Digest, HarnessError> {
        crate::domain::digest::Digest::of_canonical(self)
    }
    /// Deterministically validates the complete plan before distribution.
    ///
    /// # Errors
    ///
    /// Returns a cycle-policy error for missing assignments/evidence,
    /// duplicate or overlapping cards, or invalid dependency graphs.
    pub fn validate(&self) -> Result<(), HarnessError> {
        if self.schema != CYCLE_PLAN_SCHEMA {
            return Err(invalid("plan schema or id is invalid"));
        }
        validate_plan_id(&self.plan_id)?;
        if self.cards.is_empty() {
            return Err(invalid(
                "cycle plan must contain the complete initial card set",
            ));
        }
        let ids: BTreeSet<&str> = self
            .cards
            .iter()
            .map(|card| card.card_id.as_str())
            .collect();
        if ids.len() != self.cards.len() {
            return Err(invalid("cycle plan contains duplicate card ids"));
        }
        for card in &self.cards {
            if card
                .assignment
                .as_deref()
                .is_none_or(|id| id.trim().is_empty())
            {
                return Err(invalid(&format!("card {} is unassigned", card.card_id)));
            }
            if card
                .assignment_principal_id
                .as_deref()
                .is_none_or(|id| id.trim().is_empty())
                || card
                    .assignment_session_id
                    .as_deref()
                    .is_none_or(|id| id.trim().is_empty())
            {
                return Err(invalid(&format!(
                    "card {} has incomplete typed assignment provenance",
                    card.card_id
                )));
            }
            if card.proof_entries.is_empty() || card.mutation_plan.is_empty() {
                return Err(invalid(&format!(
                    "card {} has no evidence plan",
                    card.card_id
                )));
            }
            if card.acceptance_behaviors.is_empty() {
                return Err(invalid(&format!(
                    "card {} has no acceptance behavior",
                    card.card_id
                )));
            }
            if card
                .depends_on
                .iter()
                .any(|dependency| !ids.contains(dependency.as_str()))
            {
                return Err(invalid(&format!(
                    "card {} has a missing dependency",
                    card.card_id
                )));
            }
        }
        for (left, card) in self.cards.iter().enumerate() {
            for other in self.cards.iter().skip(left + 1) {
                if card.scope.iter().any(|path| other.scope.contains(path))
                    && card.distribution == Distribution::Parallel
                    && other.distribution == Distribution::Parallel
                {
                    return Err(invalid(&format!(
                        "parallel cards {} and {} overlap",
                        card.card_id, other.card_id
                    )));
                }
            }
        }
        if has_cycle(&self.cards) {
            return Err(invalid("cycle plan dependencies are circular"));
        }
        Ok(())
    }
}

fn validate_plan_id(plan_id: &str) -> Result<(), HarnessError> {
    if plan_id.is_empty()
        || plan_id.len() > MAX_CYCLE_PLAN_ID_LEN
        || !plan_id.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-' || byte == b'_'
        })
    {
        return Err(invalid(
            "plan id may contain at most 64 ASCII letters, digits, `.`, `-`, and `_`",
        ));
    }
    Ok(())
}

fn has_cycle(cards: &[PlannedCard]) -> bool {
    fn visit(
        id: &str,
        cards: &BTreeMap<&str, &PlannedCard>,
        active: &mut BTreeSet<String>,
        done: &mut BTreeSet<String>,
    ) -> bool {
        if active.contains(id) {
            return true;
        }
        if done.contains(id) {
            return false;
        }
        active.insert(id.to_owned());
        let cyclic = cards[id]
            .depends_on
            .iter()
            .any(|dependency| visit(dependency, cards, active, done));
        active.remove(id);
        done.insert(id.to_owned());
        cyclic
    }
    let map: BTreeMap<&str, &PlannedCard> = cards
        .iter()
        .map(|card| (card.card_id.as_str(), card))
        .collect();
    let mut active = BTreeSet::new();
    let mut done = BTreeSet::new();
    map.keys().any(|id| visit(id, &map, &mut active, &mut done))
}

fn invalid(reason: &str) -> HarnessError {
    HarnessError::Control {
        reason: reason.to_owned(),
        code: ErrorCode::PolicyInvalidCycle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str, scope: &str) -> PlannedCard {
        PlannedCard {
            card_id: id.to_owned(),
            card_revision: 1,
            scope: vec![scope.to_owned()],
            scope_exclude: vec![],
            depends_on: vec![],
            proof_entries: vec!["P-001".to_owned()],
            mutation_plan: vec!["remove guard".to_owned()],
            risk: "low".to_owned(),
            reviewer_requirements: vec!["independent".to_owned()],
            assignment: Some("agent-a".to_owned()),
            assignment_principal_id: Some("principal-a".to_owned()),
            assignment_session_id: Some("session-a".to_owned()),
            distribution: Distribution::Parallel,
            acceptance_behaviors: vec!["behavior".to_owned()],
        }
    }

    #[test]
    fn plan_rejects_parallel_scope_overlap() {
        let plan = CyclePlan {
            schema: CYCLE_PLAN_SCHEMA.to_owned(),
            plan_id: "PLAN-001".to_owned(),
            cycle_id: "C-001".to_owned(),
            objective: "objective".to_owned(),
            cards: vec![card("F-001", "src/a.rs"), card("F-002", "src/a.rs")],
        };
        assert!(plan.validate().is_err());
    }

    #[test]
    fn plan_rejects_circular_dependencies() {
        let mut first = card("F-001", "src/a.rs");
        let mut second = card("F-002", "src/b.rs");
        first.depends_on = vec!["F-002".to_owned()];
        second.depends_on = vec!["F-001".to_owned()];
        let plan = CyclePlan {
            schema: CYCLE_PLAN_SCHEMA.to_owned(),
            plan_id: "PLAN-001".to_owned(),
            cycle_id: "C-001".to_owned(),
            objective: "objective".to_owned(),
            cards: vec![first, second],
        };
        assert!(plan.validate().is_err());
    }

    #[test]
    fn plan_rejects_unassigned_or_partially_typed_assignment() {
        let cases = [
            (None, Some("principal-a"), Some("session-a")),
            (Some("agent-a"), None, Some("session-a")),
            (Some("agent-a"), Some("principal-a"), None),
        ];
        for (actor, principal, session) in cases {
            let mut planned = card("F-001", "src/a.rs");
            planned.assignment = actor.map(str::to_owned);
            planned.assignment_principal_id = principal.map(str::to_owned);
            planned.assignment_session_id = session.map(str::to_owned);
            let plan = CyclePlan {
                schema: CYCLE_PLAN_SCHEMA.to_owned(),
                plan_id: "PLAN-001".to_owned(),
                cycle_id: "C-001".to_owned(),
                objective: "objective".to_owned(),
                cards: vec![planned],
            };
            assert!(plan.validate().is_err());
        }
    }
}
