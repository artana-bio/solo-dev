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
    cycle_id: &'a str,
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
            cycle_id: "C-001",
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

    fn in_cycle(mut self, cycle_id: &'a str) -> Self {
        self.cycle_id = cycle_id;
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
cycle_id: {cycle}
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
            cycle = self.cycle_id,
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
fn overlapping_cards_in_two_active_cycles_cannot_both_activate() {
    // #24: ownership used to be calculated only from the current cycle's
    // membership. This fixture makes both cycles active, then proves the
    // second activation and its dry-run see the first cycle's existing claim.
    let workspace = with_active_cycle();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-002",
        "--objective",
        "Concurrent slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-002"]);

    activate(&workspace, &Draft::new("F-001", &["src/shared.rs"]));

    let second = Draft::new("F-002", &["src/shared.rs"]).in_cycle("C-002");
    let path = second.write(&workspace);
    workspace.card(&["create", "--draft", &path]);

    let preview = workspace.card_raw(&["activate", "--card-id", "F-002", "--dry-run"]);
    assert_eq!(preview.status.code(), Some(5));
    assert_eq!(error_code(&preview), "CH-POLICY-OWNERSHIP-OVERLAP");

    let actual = workspace.card_raw(&["activate", "--card-id", "F-002"]);
    assert_eq!(actual.status.code(), Some(5));
    assert_eq!(error_code(&actual), "CH-POLICY-OWNERSHIP-OVERLAP");

    let cycle = workspace.cycle_json(&["status", "--cycle-id", "C-002"]);
    assert!(
        cycle["data"]["card_ids"].as_array().unwrap().is_empty(),
        "a refused cross-cycle activation must not acquire membership"
    );

    // The second cycle remains usable for independent work. A cross-cycle
    // conflict is localized to the contested path, not a global stop.
    activate(
        &workspace,
        &Draft::new("F-003", &["src/disjoint.rs"]).in_cycle("C-002"),
    );
}

#[test]
fn two_cards_whose_wildcards_meet_cannot_both_activate() {
    // Tier 1, defect 4. Overlap detection asked whether either pattern matched
    // the other as a literal, so two patterns that each carry a wildcard the
    // other must match through were reported disjoint. `src/api_handler.rs`
    // satisfies both, and both cards were granted write scope over it — the one
    // outcome scope ownership exists to prevent, arrived at silently.
    let workspace = with_active_cycle();
    activate(&workspace, &Draft::new("F-001", &["src/api_*.rs"]));

    let output = try_activate(&workspace, &Draft::new("F-002", &["src/*_handler.rs"]));
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-OWNERSHIP-OVERLAP");

    let cycle = workspace.cycle_json(&["status", "--cycle-id", "C-001"]);
    assert_eq!(cycle["data"]["card_ids"].as_array().unwrap().len(), 1);
}

#[test]
fn two_cards_whose_wildcards_cannot_meet_both_activate() {
    // The guard on the fix above. Refusing every pair of patterns that share a
    // directory and a star would make the check useless in the ordinary case —
    // two cards splitting a module by filename is exactly what write scopes are
    // for, and no file satisfies both of these.
    let workspace = with_active_cycle();
    activate(&workspace, &Draft::new("F-001", &["src/api_*.rs"]));
    activate(&workspace, &Draft::new("F-002", &["src/store_*.rs"]));

    let cycle = workspace.cycle_json(&["status", "--cycle-id", "C-001"]);
    assert_eq!(cycle["data"]["card_ids"].as_array().unwrap().len(), 2);
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
    // Prove the fixture actually contends without the exclude: both cards
    // cover src/generated/api.rs.
    let guarded = with_active_cycle();
    activate(&guarded, &Draft::new("F-001", &["src/**"]));
    let collision = try_activate(&guarded, &Draft::new("F-002", &["src/generated/**"]));
    assert_eq!(collision.status.code(), Some(5));
    assert_eq!(
        error_code(&collision),
        "CH-POLICY-OWNERSHIP-OVERLAP",
        "the fixture must overlap before the generated subtree is excluded"
    );

    let workspace = with_active_cycle();
    activate(
        &workspace,
        &Draft::new("F-001", &["src/**"]).excludes(&["src/generated/**"]),
    );
    let admitted = try_activate(&workspace, &Draft::new("F-002", &["src/generated/**"]));
    assert!(
        admitted.status.success(),
        "the exclude must release the generated subtree: {}{}",
        String::from_utf8_lossy(&admitted.stdout),
        String::from_utf8_lossy(&admitted.stderr)
    );

    // Preserve the direct discriminator for effective_includes: when one
    // include is cancelled completely, it cannot contend with a broader card.
    // The recorded mutation restores that cancelled include and is caught
    // here even though the natural subtree carve-out above remains valid.
    let cancelled = with_active_cycle();
    activate(
        &cancelled,
        &Draft::new("F-001", &["src/generated/**"]).excludes(&["src/generated/**"]),
    );
    let broad = try_activate(&cancelled, &Draft::new("F-002", &["src/**"]));
    assert!(
        broad.status.success(),
        "a fully cancelled include must not retain an ownership claim: {}{}",
        String::from_utf8_lossy(&broad.stdout),
        String::from_utf8_lossy(&broad.stderr)
    );
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
