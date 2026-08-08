//! The cycle: one bounded integration period with a frozen baseline.
//!
//! Section 10.2 freezes the baseline at activation. That is the whole point of
//! a cycle: every independent card in it starts from one exact commit, so what
//! was reviewed is what gets integrated. A protected-branch change afterwards
//! does not silently move the cycle; the coordinator must decide.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{
    domain::{
        clock::Timestamp,
        digest::Digest,
        ids::{CardId, CycleId},
    },
    error::{ErrorCode, HarnessError},
};

/// Schema identifier for a cycle record.
pub const CYCLE_SCHEMA: &str = "harness.cycle/v1";

/// Directory holding cycle records, relative to the control repository.
pub const CYCLE_DIR: &str = "cycles";

/// Where a cycle sits in its lifecycle.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CycleStatus {
    /// Declared but not yet started; the baseline is not frozen.
    Draft,
    /// Baseline frozen; cards may be declared and worked.
    Active,
    /// Card membership is frozen; existing cards may continue through work and review.
    Sealed,
    /// Candidates are being combined.
    Integrating,
    /// The landing commit was accepted.
    Accepted,
    /// The accepted commit reached the authority branch.
    Landed,
    /// Terminal success.
    Closed,
    /// Progress is halted pending a decision.
    Blocked,
    /// Terminal abandonment.
    Abandoned,
}

impl CycleStatus {
    /// The status a newly created cycle carries.
    pub const INITIAL: Self = Self::Draft;

    /// True when no further transition is permitted.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Closed | Self::Abandoned)
    }

    /// True when the cycle may accept new cards.
    ///
    /// Cards are declared against a frozen baseline, so a draft cycle has
    /// nothing to declare them against and a terminal one must not grow.
    #[must_use]
    pub const fn accepts_cards(self) -> bool {
        matches!(self, Self::Active)
    }

    /// Its stable serialized name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Active => "active",
            Self::Sealed => "sealed",
            Self::Integrating => "integrating",
            Self::Accepted => "accepted",
            Self::Landed => "landed",
            Self::Closed => "closed",
            Self::Blocked => "blocked",
            Self::Abandoned => "abandoned",
        }
    }

    /// Every status this one may transition to.
    ///
    /// Encoded directly from the Section 11.1 diagram. `blocked` returns to the
    /// state that entered it, which a single successor list cannot express, so
    /// it lists both re-entry points and the validator resolves the rest.
    #[must_use]
    pub fn successors(self) -> &'static [Self] {
        match self {
            Self::Draft => &[Self::Active, Self::Abandoned],
            Self::Active => &[
                Self::Sealed,
                Self::Integrating,
                Self::Blocked,
                Self::Abandoned,
            ],
            // This first sealing slice deliberately makes no integration or
            // escalation promise. A sealed cycle can be abandoned, but a
            // later slice must define any further forward transition.
            Self::Sealed => &[Self::Abandoned],
            Self::Integrating => &[Self::Accepted, Self::Blocked, Self::Abandoned],
            Self::Accepted => &[Self::Landed, Self::Abandoned],
            Self::Landed => &[Self::Closed],
            Self::Blocked => &[Self::Active, Self::Integrating, Self::Abandoned],
            Self::Closed | Self::Abandoned => &[],
        }
    }

    /// Checks one transition, returning an error naming both states.
    ///
    /// # Errors
    ///
    /// Returns a policy error when the transition is not in Section 11.1.
    pub fn check_transition(self, next: Self) -> Result<(), HarnessError> {
        if self.successors().contains(&next) {
            return Ok(());
        }
        Err(HarnessError::Control {
            reason: format!(
                "cycle cannot move from `{}` to `{}`; permitted: {}",
                self.name(),
                next.name(),
                if self.successors().is_empty() {
                    "none, this state is terminal".to_owned()
                } else {
                    self.successors()
                        .iter()
                        .map(|state| state.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ),
            code: ErrorCode::PolicyInvalidTransition,
        })
    }
}

impl fmt::Display for CycleStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// A group of cards that must land together or not at all.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AtomicGroup {
    /// Names the group.
    pub name: String,
    /// Cards that share its fate.
    pub card_ids: Vec<CardId>,
}

/// One bounded integration period.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CycleRecord {
    /// Always [`CYCLE_SCHEMA`].
    pub schema: String,
    /// Identifies this cycle.
    pub cycle_id: CycleId,
    /// What the cycle is for.
    pub objective: String,
    /// Its current status.
    pub status: CycleStatus,
    /// The exact authority commit frozen at activation.
    ///
    /// `None` while the cycle is a draft. Once set it never changes, which is
    /// what "frozen" means.
    pub baseline_sha: Option<String>,
    /// The harness version that created the record.
    pub harness_version: String,
    /// Digest of the project configuration in force.
    ///
    /// Section 10.2 calls this `project_revision`. A digest is used rather than
    /// a counter because it changes exactly when the configuration changes and
    /// needs no separate bookkeeping to stay honest.
    pub project_revision: Digest,
    /// Conditions the cycle must satisfy to be accepted.
    pub release_invariants: Vec<String>,
    /// The active distribution plan bound to this cycle, when migrated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_digest: Option<Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_revision: Option<u32>,
    /// Explicit provenance for a pre-plan cycle admitted through migration.
    /// New cycles never receive this implicitly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_migration_provenance: Option<String>,
    /// Cards declared against this cycle.
    pub card_ids: Vec<CardId>,
    /// Groups of cards that land together.
    pub atomic_groups: Vec<AtomicGroup>,
    /// Who created it. Declared, not proven.
    pub created_by: String,
    /// When it was created.
    pub created_at: Timestamp,
    /// When its baseline was frozen.
    pub activated_at: Option<Timestamp>,
}

impl CycleRecord {
    /// Relative path of this record inside the control repository.
    #[must_use]
    pub fn relative_path(cycle_id: &CycleId) -> String {
        format!("{CYCLE_DIR}/{cycle_id}.json")
    }

    /// Validates internal consistency.
    ///
    /// # Errors
    ///
    /// Returns a policy error when membership or baseline state is incoherent.
    pub fn validate(&self) -> Result<(), HarnessError> {
        if self.objective.trim().is_empty() {
            return Err(HarnessError::Control {
                reason: "a cycle must state an objective".to_owned(),
                code: ErrorCode::PolicyInvalidCycle,
            });
        }

        // An active cycle without a frozen baseline would let cards start from
        // whatever the branch happens to be, which is the failure a cycle exists
        // to prevent.
        let needs_baseline = !matches!(self.status, CycleStatus::Draft | CycleStatus::Abandoned);
        if needs_baseline && self.baseline_sha.is_none() {
            return Err(HarnessError::Control {
                reason: format!(
                    "cycle {} is `{}` but has no frozen baseline",
                    self.cycle_id, self.status
                ),
                code: ErrorCode::PolicyInvalidCycle,
            });
        }

        let mut seen = std::collections::BTreeSet::new();
        for card_id in &self.card_ids {
            if !seen.insert(card_id) {
                return Err(HarnessError::Control {
                    reason: format!(
                        "card {card_id} is declared twice in cycle {}",
                        self.cycle_id
                    ),
                    code: ErrorCode::PolicyInvalidCycle,
                });
            }
        }

        let mut grouped = std::collections::BTreeSet::new();
        for group in &self.atomic_groups {
            if group.card_ids.is_empty() {
                return Err(HarnessError::Control {
                    reason: format!("atomic group `{}` is empty", group.name),
                    code: ErrorCode::PolicyInvalidCycle,
                });
            }
            for card_id in &group.card_ids {
                if !self.card_ids.contains(card_id) {
                    return Err(HarnessError::Control {
                        reason: format!(
                            "atomic group `{}` names card {card_id}, which is not in cycle {}",
                            group.name, self.cycle_id
                        ),
                        code: ErrorCode::PolicyInvalidCycle,
                    });
                }
                // A card in two groups would have two different sets of cards
                // sharing its fate, which cannot both be honored.
                if !grouped.insert(card_id) {
                    return Err(HarnessError::Control {
                        reason: format!("card {card_id} belongs to more than one atomic group"),
                        code: ErrorCode::PolicyInvalidCycle,
                    });
                }
            }
        }
        Ok(())
    }

    /// Adds a card, refusing when the cycle cannot accept one.
    ///
    /// # Errors
    ///
    /// Returns a policy error when the cycle is not active or already holds the
    /// card.
    pub fn declare_card(&mut self, card_id: CardId) -> Result<(), HarnessError> {
        if !self.status.accepts_cards() {
            return Err(HarnessError::Control {
                reason: format!(
                    "cycle {} is `{}` and cannot accept new cards",
                    self.cycle_id, self.status
                ),
                code: ErrorCode::PolicyInvalidTransition,
            });
        }
        if self.card_ids.contains(&card_id) {
            return Err(HarnessError::Control {
                reason: format!(
                    "card {card_id} is already declared in cycle {}",
                    self.cycle_id
                ),
                code: ErrorCode::PolicyInvalidCycle,
            });
        }
        self.card_ids.push(card_id);
        Ok(())
    }
}

/// Derives a cycle's status from its authoritative events.
///
/// `WP-200` acceptance requires status to come from events rather than from a
/// mutable field, so a stored record that drifts from history is detectable
/// rather than authoritative.
#[must_use]
pub fn status_from_events<'a>(events: impl IntoIterator<Item = &'a str>) -> CycleStatus {
    let mut status = CycleStatus::INITIAL;
    for next_state in events {
        if let Some(parsed) = parse_status(next_state) {
            status = parsed;
        }
    }
    status
}

/// Parses a serialized status name.
#[must_use]
pub fn parse_status(name: &str) -> Option<CycleStatus> {
    Some(match name {
        "draft" => CycleStatus::Draft,
        "active" => CycleStatus::Active,
        "sealed" => CycleStatus::Sealed,
        "integrating" => CycleStatus::Integrating,
        "accepted" => CycleStatus::Accepted,
        "landed" => CycleStatus::Landed,
        "closed" => CycleStatus::Closed,
        "blocked" => CycleStatus::Blocked,
        "abandoned" => CycleStatus::Abandoned,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::clock::{Clock as _, FixedClock};

    fn sample() -> CycleRecord {
        CycleRecord {
            schema: CYCLE_SCHEMA.to_owned(),
            cycle_id: "C-001".parse().unwrap(),
            objective: "Ship the first slice".to_owned(),
            status: CycleStatus::Draft,
            baseline_sha: None,
            harness_version: "0.1.0".to_owned(),
            project_revision: Digest::of_bytes(b"project"),
            release_invariants: vec![],
            plan_id: None,
            plan_digest: None,
            plan_revision: None,
            plan_migration_provenance: None,
            card_ids: vec![],
            atomic_groups: vec![],
            created_by: "alvaro".to_owned(),
            created_at: FixedClock::at_unix_seconds(1_785_196_800).unwrap().now(),
            activated_at: None,
        }
    }

    #[test]
    fn the_documented_transitions_are_permitted() {
        for (from, to) in [
            (CycleStatus::Draft, CycleStatus::Active),
            (CycleStatus::Active, CycleStatus::Sealed),
            (CycleStatus::Sealed, CycleStatus::Abandoned),
            (CycleStatus::Active, CycleStatus::Integrating),
            (CycleStatus::Integrating, CycleStatus::Accepted),
            (CycleStatus::Accepted, CycleStatus::Landed),
            (CycleStatus::Landed, CycleStatus::Closed),
            (CycleStatus::Active, CycleStatus::Blocked),
            (CycleStatus::Blocked, CycleStatus::Active),
            (CycleStatus::Integrating, CycleStatus::Blocked),
            (CycleStatus::Blocked, CycleStatus::Integrating),
        ] {
            from.check_transition(to)
                .unwrap_or_else(|error| panic!("{from} -> {to} should be legal: {error}"));
        }
    }

    #[test]
    fn undocumented_transitions_fail_and_name_the_alternatives() {
        let error = CycleStatus::Draft
            .check_transition(CycleStatus::Landed)
            .expect_err("must reject");
        assert_eq!(error.code(), ErrorCode::PolicyInvalidTransition);
        assert!(error.to_string().contains("active"));
    }

    #[test]
    fn terminal_states_permit_nothing() {
        for terminal in [CycleStatus::Closed, CycleStatus::Abandoned] {
            assert!(terminal.is_terminal());
            assert!(terminal.successors().is_empty());
            let error = terminal
                .check_transition(CycleStatus::Active)
                .expect_err("must reject");
            assert!(error.to_string().contains("terminal"));
        }
    }

    #[test]
    fn a_landed_cycle_cannot_be_abandoned() {
        // Section 11.2 makes this explicit for cards, and the same reasoning
        // applies here: what has reached authority cannot be un-landed.
        assert!(
            CycleStatus::Landed
                .check_transition(CycleStatus::Abandoned)
                .is_err()
        );
    }

    #[test]
    fn only_an_active_cycle_accepts_cards() {
        for status in [
            CycleStatus::Draft,
            CycleStatus::Sealed,
            CycleStatus::Integrating,
            CycleStatus::Accepted,
            CycleStatus::Landed,
            CycleStatus::Closed,
            CycleStatus::Blocked,
            CycleStatus::Abandoned,
        ] {
            assert!(!status.accepts_cards(), "{status} must not accept cards");
        }
        assert!(CycleStatus::Active.accepts_cards());
    }

    #[test]
    fn declaring_a_card_on_an_abandoned_cycle_fails() {
        let mut cycle = CycleRecord {
            status: CycleStatus::Abandoned,
            ..sample()
        };
        let error = cycle
            .declare_card("F-001".parse().unwrap())
            .expect_err("must reject");
        assert_eq!(error.code(), ErrorCode::PolicyInvalidTransition);
    }

    #[test]
    fn declaring_the_same_card_twice_fails() {
        let mut cycle = CycleRecord {
            status: CycleStatus::Active,
            baseline_sha: Some("abc".into()),
            ..sample()
        };
        cycle.declare_card("F-001".parse().unwrap()).unwrap();
        assert!(cycle.declare_card("F-001".parse().unwrap()).is_err());
    }

    #[test]
    fn an_active_cycle_without_a_baseline_is_invalid() {
        let cycle = CycleRecord {
            status: CycleStatus::Active,
            ..sample()
        };
        let error = cycle.validate().expect_err("must reject");
        assert_eq!(error.code(), ErrorCode::PolicyInvalidCycle);
        assert!(error.to_string().contains("frozen baseline"));
    }

    #[test]
    fn a_draft_cycle_without_a_baseline_is_valid() {
        sample()
            .validate()
            .expect("a draft has not frozen anything yet");
    }

    #[test]
    fn an_empty_objective_is_rejected() {
        let cycle = CycleRecord {
            objective: "   ".to_owned(),
            ..sample()
        };
        assert!(cycle.validate().is_err());
    }

    #[test]
    fn an_atomic_group_naming_an_undeclared_card_is_rejected() {
        let cycle = CycleRecord {
            status: CycleStatus::Active,
            baseline_sha: Some("abc".into()),
            card_ids: vec!["F-001".parse().unwrap()],
            atomic_groups: vec![AtomicGroup {
                name: "pair".to_owned(),
                card_ids: vec!["F-002".parse().unwrap()],
            }],
            ..sample()
        };
        let error = cycle.validate().expect_err("must reject");
        assert!(error.to_string().contains("not in cycle"));
    }

    #[test]
    fn a_card_in_two_atomic_groups_is_rejected() {
        let cycle = CycleRecord {
            status: CycleStatus::Active,
            baseline_sha: Some("abc".into()),
            card_ids: vec!["F-001".parse().unwrap(), "F-002".parse().unwrap()],
            atomic_groups: vec![
                AtomicGroup {
                    name: "one".to_owned(),
                    card_ids: vec!["F-001".parse().unwrap()],
                },
                AtomicGroup {
                    name: "two".to_owned(),
                    card_ids: vec!["F-001".parse().unwrap(), "F-002".parse().unwrap()],
                },
            ],
            ..sample()
        };
        let error = cycle.validate().expect_err("must reject");
        assert!(error.to_string().contains("more than one atomic group"));
    }

    #[test]
    fn an_empty_atomic_group_is_rejected() {
        let cycle = CycleRecord {
            status: CycleStatus::Active,
            baseline_sha: Some("abc".into()),
            atomic_groups: vec![AtomicGroup {
                name: "empty".to_owned(),
                card_ids: vec![],
            }],
            ..sample()
        };
        assert!(cycle.validate().is_err());
    }

    #[test]
    fn status_is_derived_by_folding_events() {
        assert_eq!(status_from_events(Vec::<&str>::new()), CycleStatus::Draft);
        assert_eq!(status_from_events(vec!["active"]), CycleStatus::Active);
        assert_eq!(
            status_from_events(vec!["active", "blocked", "active", "integrating"]),
            CycleStatus::Integrating
        );
        // Unrecognized transitions are ignored rather than resetting state.
        assert_eq!(
            status_from_events(vec!["active", "not-a-state"]),
            CycleStatus::Active
        );
    }

    #[test]
    fn a_record_round_trips_through_json() {
        let cycle = CycleRecord {
            status: CycleStatus::Active,
            baseline_sha: Some("abc123".into()),
            card_ids: vec!["F-001".parse().unwrap()],
            ..sample()
        };
        let encoded = serde_json::to_string_pretty(&cycle).unwrap();
        let decoded: CycleRecord = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, cycle);
    }

    #[test]
    fn an_unknown_field_is_rejected_on_load() {
        let mut value = serde_json::to_value(sample()).unwrap();
        value["surprise"] = serde_json::json!(1);
        assert!(serde_json::from_value::<CycleRecord>(value).is_err());
    }
}
