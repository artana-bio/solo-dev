//! `WP-220` acceptance: ownership, contracts, resources, and dependencies.

mod support;

use std::fs;

use serde_json::Value;
use support::Workspace;

fn with_active_cycle() -> Workspace {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace
}

/// Builds a draft with configurable ownership claims.
struct Draft<'a> {
    card_id: &'a str,
    include: Vec<&'a str>,
    exclude: Vec<&'a str>,
    contract_changes: Vec<&'a str>,
    exclusive_resources: Vec<&'a str>,
    depends_on: Vec<&'a str>,
}

impl<'a> Draft<'a> {
    fn new(card_id: &'a str, include: &[&'a str]) -> Self {
        Self {
            card_id,
            include: include.to_vec(),
            exclude: vec![],
            contract_changes: vec![],
            exclusive_resources: vec![],
            depends_on: vec![],
        }
    }

    fn contracts(mut self, domains: &[&'a str]) -> Self {
        self.contract_changes = domains.to_vec();
        self
    }

    fn resources(mut self, resources: &[&'a str]) -> Self {
        self.exclusive_resources = resources.to_vec();
        self
    }

    fn depends(mut self, cards: &[&'a str]) -> Self {
        self.depends_on = cards.to_vec();
        self
    }

    fn excludes(mut self, patterns: &[&'a str]) -> Self {
        self.exclude = patterns.to_vec();
        self
    }

    /// Writes the draft into the workspace and returns its path.
    fn write(&self, workspace: &Workspace) -> String {
        let list = |values: &[&str]| {
            values
                .iter()
                .map(|value| format!("\"{value}\""))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let body = format!(
            "\
card_id: {id}
cycle_id: C-001
title: Implement {id}
goal: Deliver the {id} behavior
non_goals: []
risk: low
change_kind: feature
base_sha: {baseline}
write_scope:
  include: [{include}]
  exclude: [{exclude}]
contract_changes: [{contracts}]
exclusive_resources: [{resources}]
depends_on: [{depends}]
named_gates:
  feature: [gate.unit]
  review: []
  integration: [gate.all]
acceptance:
  behaviors: [it works]
  regressions: []
review_policy: independent
rollback_strategy: revert the commit
",
            id = self.card_id,
            baseline = workspace.authority_head(),
            include = list(&self.include),
            exclude = list(&self.exclude),
            contracts = list(&self.contract_changes),
            resources = list(&self.exclusive_resources),
            depends = list(&self.depends_on),
        );
        let path = workspace.root.join(format!("{}.yaml", self.card_id));
        fs::write(&path, body).unwrap();
        path.display().to_string()
    }
}

/// Creates and activates a card, asserting success.
fn activate(workspace: &Workspace, draft: &Draft<'_>) {
    let path = draft.write(workspace);
    workspace.card(&["create", "--draft", &path]);
    workspace.card(&["activate", "--card-id", draft.card_id]);
}

/// Creates a card and attempts activation, returning the raw output.
fn try_activate(workspace: &Workspace, draft: &Draft<'_>) -> std::process::Output {
    let path = draft.write(workspace);
    workspace.card(&["create", "--draft", &path]);
    workspace.card_raw(&["activate", "--card-id", draft.card_id])
}

fn error_code(output: &std::process::Output) -> String {
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["code"].as_str().unwrap().to_owned()
}

#[test]
fn disjoint_cards_both_activate() {
    let workspace = with_active_cycle();
    activate(&workspace, &Draft::new("F-001", &["src/temperature.rs"]));
    activate(&workspace, &Draft::new("F-002", &["src/currency.rs"]));

    let cycle = workspace.cycle_json(&["status", "--cycle-id", "C-001"]);
    assert_eq!(cycle["data"]["card_ids"].as_array().unwrap().len(), 2);
}

#[test]
fn overlapping_cards_cannot_both_activate() {
    let workspace = with_active_cycle();
    activate(&workspace, &Draft::new("F-001", &["src/**"]));

    let output = try_activate(&workspace, &Draft::new("F-002", &["src/shared.rs"]));
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-OWNERSHIP-OVERLAP");

    // The refused card must not have been declared in the cycle.
    let cycle = workspace.cycle_json(&["status", "--cycle-id", "C-001"]);
    assert_eq!(cycle["data"]["card_ids"].as_array().unwrap().len(), 1);
}

#[test]
fn the_overlap_error_names_both_patterns_and_the_conflicting_card() {
    let workspace = with_active_cycle();
    activate(&workspace, &Draft::new("F-001", &["src/**"]));
    let output = try_activate(&workspace, &Draft::new("F-002", &["src/shared.rs"]));

    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(message.contains("F-001"), "{message}");
    assert!(message.contains("src/**"), "{message}");
    assert!(message.contains("src/shared.rs"), "{message}");
}

#[test]
fn contract_overlap_is_refused_even_when_paths_are_disjoint() {
    let workspace = with_active_cycle();
    activate(
        &workspace,
        &Draft::new("F-001", &["src/a.rs"]).contracts(&["api.v1"]),
    );

    let output = try_activate(
        &workspace,
        &Draft::new("F-002", &["src/b.rs"]).contracts(&["api.v1"]),
    );
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-CONTRACT-OVERLAP");
}

#[test]
fn distinct_contract_domains_are_admissible() {
    let workspace = with_active_cycle();
    activate(
        &workspace,
        &Draft::new("F-001", &["src/a.rs"]).contracts(&["api.v1"]),
    );
    activate(
        &workspace,
        &Draft::new("F-002", &["src/b.rs"]).contracts(&["api.v2"]),
    );
}

#[test]
fn an_exclusive_resource_cannot_be_double_booked() {
    let workspace = with_active_cycle();
    activate(
        &workspace,
        &Draft::new("F-001", &["src/a.rs"]).resources(&["port:8080"]),
    );

    let output = try_activate(
        &workspace,
        &Draft::new("F-002", &["src/b.rs"]).resources(&["port:8080"]),
    );
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-RESOURCE-CONFLICT");
}

#[test]
fn a_dependency_on_an_undeclared_card_is_refused() {
    let workspace = with_active_cycle();
    let output = try_activate(
        &workspace,
        &Draft::new("F-001", &["src/a.rs"]).depends(&["F-999"]),
    );
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-DEPENDENCY-UNSATISFIED");
}

#[test]
fn a_satisfied_dependency_is_admissible() {
    let workspace = with_active_cycle();
    activate(&workspace, &Draft::new("F-001", &["src/a.rs"]));
    activate(
        &workspace,
        &Draft::new("F-002", &["src/b.rs"]).depends(&["F-001"]),
    );
}

#[test]
fn a_dependency_cycle_is_refused_with_an_explanatory_path() {
    let workspace = with_active_cycle();
    activate(&workspace, &Draft::new("F-001", &["src/a.rs"]));
    activate(
        &workspace,
        &Draft::new("F-002", &["src/b.rs"]).depends(&["F-001"]),
    );

    // Revise F-001 so it depends back on F-002, closing the loop.
    let looping = Draft::new("F-001", &["src/a.rs"]).depends(&["F-002"]);
    let path = looping.write(&workspace);
    let output = workspace.card_raw(&[
        "revise",
        "--card-id",
        "F-001",
        "--draft",
        &path,
        "--reason",
        "introduce a cycle",
    ]);

    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-DEPENDENCY-CYCLE");
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("->"),
        "the path must be explanatory: {message}"
    );
    assert!(
        message.contains("F-001") && message.contains("F-002"),
        "{message}"
    );
}

#[test]
fn an_exclude_lets_two_cards_share_a_directory() {
    let workspace = with_active_cycle();
    // F-001 owns src/** except the generated subtree; F-002 owns exactly that
    // subtree. Without excludes taking effect these would collide.
    activate(
        &workspace,
        &Draft::new("F-001", &["src/handwritten.rs"]).excludes(&["src/generated/**"]),
    );
    activate(&workspace, &Draft::new("F-002", &["src/generated/api.rs"]));
}

#[test]
fn an_abandoned_cards_claims_are_released() {
    let workspace = with_active_cycle();
    activate(&workspace, &Draft::new("F-001", &["src/**"]));

    // While F-001 holds src/**, a second claim is refused.
    let blocked = try_activate(&workspace, &Draft::new("F-002", &["src/a.rs"]));
    assert_eq!(blocked.status.code(), Some(5));

    // Section 11.2 lets a non-landed card be abandoned, which releases its
    // claims; the same card may then activate.
    workspace.tamper_card_state("F-001", "abandoned");
    workspace.card(&["activate", "--card-id", "F-002"]);
}

#[test]
fn a_revision_that_widens_scope_into_a_conflict_is_refused() {
    let workspace = with_active_cycle();
    activate(&workspace, &Draft::new("F-001", &["src/a.rs"]));
    activate(&workspace, &Draft::new("F-002", &["src/b.rs"]));

    // Widening F-002 to the whole tree now collides with F-001.
    let widened = Draft::new("F-002", &["src/**"]);
    let path = widened.write(&workspace);
    let output = workspace.card_raw(&[
        "revise",
        "--card-id",
        "F-002",
        "--draft",
        &path,
        "--reason",
        "widen scope",
    ]);

    assert_eq!(
        output.status.code(),
        Some(5),
        "allocation must be re-checked on revision, not assumed from activation"
    );
    assert_eq!(error_code(&output), "CH-POLICY-OWNERSHIP-OVERLAP");
}

#[test]
fn a_refused_activation_leaves_control_untouched() {
    let workspace = with_active_cycle();
    activate(&workspace, &Draft::new("F-001", &["src/**"]));
    let before = workspace.control_head();

    let output = try_activate(&workspace, &Draft::new("F-002", &["src/a.rs"]));
    assert_eq!(output.status.code(), Some(5));

    // `card create` committed the draft, so control advanced once for that; the
    // refused activation itself must add nothing further.
    let after_create_and_refusal = workspace.control_head();
    let output_again = workspace.card_raw(&["activate", "--card-id", "F-002"]);
    assert_eq!(output_again.status.code(), Some(5));
    assert_eq!(
        workspace.control_head(),
        after_create_and_refusal,
        "a repeated refusal must not advance control"
    );
    assert_ne!(before, "");
}
