//! Global allocation: what an active card may claim, and what stops it.
//!
//! Invariant 7.3.2: active cards may not overlap write scopes or exclusive
//! resources. This module answers "may this card activate given everything
//! already active", and it deliberately compares against *active* cards only —
//! a closed or abandoned card's claims are released.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    domain::{
        card::{CardRecord, CardState},
        ids::CardId,
    },
    error::{ErrorCode, HarnessError},
};

use super::paths::Scope;

/// One card's claims, as seen by the allocator.
#[derive(Clone, Debug)]
pub struct Claim {
    /// The card making the claim.
    pub card_id: CardId,
    /// Where the card currently sits.
    pub state: CardState,
    /// Paths it may write.
    pub include: Vec<String>,
    /// Paths carved back out.
    pub exclude: Vec<String>,
    /// Contract domains it changes.
    pub contract_changes: Vec<String>,
    /// Resources it needs exclusively.
    pub exclusive_resources: Vec<String>,
    /// Cards it depends on.
    pub depends_on: Vec<CardId>,
}

impl Claim {
    /// Builds a claim from an activated card and its current state.
    #[must_use]
    pub fn from_record(record: &CardRecord, state: CardState) -> Self {
        Self {
            card_id: record.card_id.clone(),
            state,
            include: record.write_scope.include.clone(),
            exclude: record.write_scope.exclude.clone(),
            contract_changes: record.contract_changes.clone(),
            exclusive_resources: record.exclusive_resources.clone(),
            depends_on: record.depends_on.clone(),
        }
    }

    /// True when this card still holds its claims.
    ///
    /// A card that has landed, closed, or been abandoned has released whatever
    /// it owned, so it must not block a new card over the same region.
    #[must_use]
    pub const fn holds_claims(&self) -> bool {
        !matches!(
            self.state,
            CardState::Closed | CardState::Abandoned | CardState::Landed | CardState::Draft
        )
    }

    /// The path scope this claim covers.
    #[must_use]
    pub fn scope(&self) -> Scope<'_> {
        Scope::new(&self.include, &self.exclude)
    }
}

/// Checks one candidate claim against everything currently active.
///
/// # Errors
///
/// Returns a policy error naming the conflicting card and the exact reason.
pub fn check_admissible(candidate: &Claim, existing: &[Claim]) -> Result<(), HarnessError> {
    for other in existing {
        if other.card_id == candidate.card_id || !other.holds_claims() {
            continue;
        }

        if let Some((left, right)) = candidate.scope().overlaps(&other.scope()) {
            return Err(HarnessError::Control {
                reason: format!(
                    "card {} claims `{left}`, which overlaps active card {} claiming `{right}`",
                    candidate.card_id, other.card_id
                ),
                code: ErrorCode::PolicyOwnershipOverlap,
            });
        }

        // Contract domains are checked independently of paths. Two cards can
        // edit different files and still both change the same public contract,
        // which a path comparison cannot see.
        let shared_contracts: Vec<&String> = candidate
            .contract_changes
            .iter()
            .filter(|domain| other.contract_changes.contains(domain))
            .collect();
        if let Some(domain) = shared_contracts.first() {
            return Err(HarnessError::Control {
                reason: format!(
                    "card {} changes contract domain `{domain}`, which active card {} also changes",
                    candidate.card_id, other.card_id
                ),
                code: ErrorCode::PolicyContractOverlap,
            });
        }

        let shared_resources: Vec<&String> = candidate
            .exclusive_resources
            .iter()
            .filter(|resource| other.exclusive_resources.contains(resource))
            .collect();
        if let Some(resource) = shared_resources.first() {
            return Err(HarnessError::Control {
                reason: format!(
                    "card {} reserves `{resource}`, which active card {} already holds exclusively",
                    candidate.card_id, other.card_id
                ),
                code: ErrorCode::PolicyResourceConflict,
            });
        }
    }
    Ok(())
}

/// Validates that dependencies form a directed acyclic graph.
///
/// # Errors
///
/// Returns a policy error naming the exact cycle path, or a missing dependency.
pub fn check_dependencies(claims: &[Claim]) -> Result<(), HarnessError> {
    let by_id: BTreeMap<&CardId, &Claim> =
        claims.iter().map(|claim| (&claim.card_id, claim)).collect();

    for claim in claims {
        for dependency in &claim.depends_on {
            if !by_id.contains_key(dependency) {
                return Err(HarnessError::Control {
                    reason: format!(
                        "card {} depends on {dependency}, which is not declared in this cycle",
                        claim.card_id
                    ),
                    code: ErrorCode::PolicyDependencyUnsatisfied,
                });
            }
        }
    }

    // Depth-first search recording the active path, so a detected cycle can be
    // reported as the route through it rather than as a bare "cycle exists".
    let mut settled: BTreeSet<&CardId> = BTreeSet::new();
    let mut path: Vec<&CardId> = Vec::new();

    for claim in claims {
        if let Some(cycle) = visit(&claim.card_id, &by_id, &mut settled, &mut path) {
            let rendered: Vec<String> = cycle.iter().map(ToString::to_string).collect();
            return Err(HarnessError::Control {
                reason: format!("dependency cycle: {}", rendered.join(" -> ")),
                code: ErrorCode::PolicyDependencyCycle,
            });
        }
    }
    Ok(())
}

/// Depth-first visit returning the cycle path when one is found.
fn visit<'a>(
    card_id: &'a CardId,
    by_id: &BTreeMap<&'a CardId, &'a Claim>,
    settled: &mut BTreeSet<&'a CardId>,
    path: &mut Vec<&'a CardId>,
) -> Option<Vec<&'a CardId>> {
    if settled.contains(card_id) {
        return None;
    }
    if let Some(position) = path.iter().position(|entry| *entry == card_id) {
        let mut cycle: Vec<&CardId> = path[position..].to_vec();
        cycle.push(card_id);
        return Some(cycle);
    }

    path.push(card_id);
    if let Some(claim) = by_id.get(card_id) {
        for dependency in &claim.depends_on {
            let resolved = by_id.get_key_value(dependency).map(|(key, _)| *key);
            if let Some(next) = resolved
                && let Some(cycle) = visit(next, by_id, settled, path)
            {
                return Some(cycle);
            }
        }
    }
    path.pop();
    settled.insert(card_id);
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(card_id: &str, include: &[&str]) -> Claim {
        Claim {
            card_id: card_id.parse().unwrap(),
            state: CardState::Active,
            include: include.iter().map(|s| (*s).to_owned()).collect(),
            exclude: vec![],
            contract_changes: vec![],
            exclusive_resources: vec![],
            depends_on: vec![],
        }
    }

    #[test]
    fn disjoint_cards_are_admissible() {
        let existing = vec![claim("F-001", &["src/a.rs"])];
        check_admissible(&claim("F-002", &["src/b.rs"]), &existing).expect("no conflict");
    }

    #[test]
    fn overlapping_write_scopes_are_refused() {
        let existing = vec![claim("F-001", &["src/**"])];
        let error =
            check_admissible(&claim("F-002", &["src/a.rs"]), &existing).expect_err("must refuse");
        assert_eq!(error.code(), ErrorCode::PolicyOwnershipOverlap);
        assert!(error.to_string().contains("F-001"));
        assert!(error.to_string().contains("src/**"));
    }

    #[test]
    fn a_card_does_not_conflict_with_itself() {
        let existing = vec![claim("F-001", &["src/**"])];
        check_admissible(&claim("F-001", &["src/**"]), &existing)
            .expect("a card must not block its own revision");
    }

    #[test]
    fn released_claims_do_not_block_new_cards() {
        for released in [
            CardState::Closed,
            CardState::Abandoned,
            CardState::Landed,
            CardState::Draft,
        ] {
            let mut held = claim("F-001", &["src/**"]);
            held.state = released;
            check_admissible(&claim("F-002", &["src/a.rs"]), &[held])
                .unwrap_or_else(|error| panic!("{released} must release its claims: {error}"));
        }
    }

    #[test]
    fn in_flight_states_still_hold_their_claims() {
        for held_state in [
            CardState::Ready,
            CardState::Leased,
            CardState::Active,
            CardState::HandedOff,
            CardState::ReviewPending,
            CardState::Approved,
            CardState::Integrating,
            CardState::Accepted,
        ] {
            let mut held = claim("F-001", &["src/**"]);
            held.state = held_state;
            assert!(
                check_admissible(&claim("F-002", &["src/a.rs"]), &[held]).is_err(),
                "{held_state} must still hold its claims"
            );
        }
    }

    #[test]
    fn contract_overlap_is_refused_even_when_paths_differ() {
        let mut first = claim("F-001", &["src/a.rs"]);
        first.contract_changes = vec!["api.v1".to_owned()];
        let mut second = claim("F-002", &["src/b.rs"]);
        second.contract_changes = vec!["api.v1".to_owned()];

        // The paths are disjoint, so only the contract check can catch this.
        assert!(second.scope().overlaps(&first.scope()).is_none());
        let error = check_admissible(&second, &[first]).expect_err("must refuse");
        assert_eq!(error.code(), ErrorCode::PolicyContractOverlap);
        assert!(error.to_string().contains("api.v1"));
    }

    #[test]
    fn exclusive_resources_cannot_be_double_booked() {
        let mut first = claim("F-001", &["src/a.rs"]);
        first.exclusive_resources = vec!["port:8080".to_owned()];
        let mut second = claim("F-002", &["src/b.rs"]);
        second.exclusive_resources = vec!["port:8080".to_owned()];

        let error = check_admissible(&second, &[first]).expect_err("must refuse");
        assert_eq!(error.code(), ErrorCode::PolicyResourceConflict);
        assert!(error.to_string().contains("port:8080"));
    }

    #[test]
    fn distinct_resources_are_admissible() {
        let mut first = claim("F-001", &["src/a.rs"]);
        first.exclusive_resources = vec!["port:8080".to_owned()];
        let mut second = claim("F-002", &["src/b.rs"]);
        second.exclusive_resources = vec!["port:8081".to_owned()];
        check_admissible(&second, &[first]).expect("different ports do not contend");
    }

    #[test]
    fn a_linear_dependency_chain_is_valid() {
        let mut first = claim("F-001", &["a"]);
        let mut second = claim("F-002", &["b"]);
        let third = claim("F-003", &["c"]);
        first.depends_on = vec![second.card_id.clone()];
        second.depends_on = vec![third.card_id.clone()];
        check_dependencies(&[first, second, third]).expect("a chain is acyclic");
    }

    #[test]
    fn a_dependency_cycle_reports_its_path() {
        let mut first = claim("F-001", &["a"]);
        let mut second = claim("F-002", &["b"]);
        let mut third = claim("F-003", &["c"]);
        first.depends_on = vec![second.card_id.clone()];
        second.depends_on = vec![third.card_id.clone()];
        third.depends_on = vec![first.card_id.clone()];

        let error = check_dependencies(&[first, second, third]).expect_err("must refuse");
        assert_eq!(error.code(), ErrorCode::PolicyDependencyCycle);
        let message = error.to_string();
        assert!(message.contains("F-001"), "{message}");
        assert!(message.contains("F-002"), "{message}");
        assert!(message.contains("F-003"), "{message}");
        assert!(
            message.contains("->"),
            "the path must be explanatory: {message}"
        );
    }

    #[test]
    fn a_self_dependency_is_a_cycle() {
        let mut only = claim("F-001", &["a"]);
        only.depends_on = vec![only.card_id.clone()];
        let error = check_dependencies(&[only]).expect_err("must refuse");
        assert_eq!(error.code(), ErrorCode::PolicyDependencyCycle);
    }

    #[test]
    fn a_dependency_outside_the_cycle_is_refused() {
        let mut only = claim("F-001", &["a"]);
        only.depends_on = vec!["F-999".parse().unwrap()];
        let error = check_dependencies(&[only]).expect_err("must refuse");
        assert_eq!(error.code(), ErrorCode::PolicyDependencyUnsatisfied);
        assert!(error.to_string().contains("F-999"));
    }

    #[test]
    fn a_diamond_is_not_a_cycle() {
        let mut top = claim("F-001", &["a"]);
        let mut left = claim("F-002", &["b"]);
        let mut right = claim("F-003", &["c"]);
        let bottom = claim("F-004", &["d"]);
        top.depends_on = vec![left.card_id.clone(), right.card_id.clone()];
        left.depends_on = vec![bottom.card_id.clone()];
        right.depends_on = vec![bottom.card_id.clone()];
        check_dependencies(&[top, left, right, bottom]).expect("a diamond is acyclic");
    }

    #[test]
    fn an_empty_allocation_is_valid() {
        check_dependencies(&[]).expect("nothing to check");
        check_admissible(&claim("F-001", &["src/a.rs"]), &[]).expect("nothing to conflict with");
    }
}
