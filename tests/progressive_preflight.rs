//! Acceptance tests for the read-only progressive-validation projection.

mod support;

use std::fs;

use change_harness::{commands::card::CardStateRecord, domain::card::CardRecord};
use serde_json::Value;
use support::Workspace;

fn active_cycle() -> Workspace {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Progressive validation",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace
}

#[derive(Clone, Copy)]
struct CardSpec<'a> {
    risk: &'a str,
    include: &'a str,
    feature: &'a str,
    review: &'a str,
    integration: &'a str,
    proof_map: bool,
}

fn activate(workspace: &Workspace, card_id: &str, spec: CardSpec<'_>) {
    let proof = if spec.proof_map {
        "proof_map:\n  schema: harness.proof-map/v1\n  entries:\n    - invariant: behavior remains true\n      precondition: focused fixture exists\n      assertion: check observes behavior\n      mutation: bypass makes check fail\n  claim_boundary: only this named behavior\n"
    } else {
        ""
    };
    let raw = format!(
        "card_id: {card_id}\ncycle_id: C-001\ntitle: {card_id}\ngoal: Deliver {card_id}\nnon_goals: []\nrisk: {}\nchange_kind: feature\nbase_sha: {}\nwrite_scope:\n  include: [{}]\n  exclude: []\nnamed_gates:\n  feature: [{}]\n  review: [{}]\n  integration: [{}]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert\n{proof}",
        spec.risk,
        workspace.authority_head(),
        spec.include,
        spec.feature,
        spec.review,
        spec.integration,
    );
    let path = workspace.root.join(format!("{card_id}.yaml"));
    fs::write(&path, raw).unwrap();
    workspace.card(&["create", "--draft", &path.display().to_string()]);
    workspace.card(&["activate", "--card-id", card_id]);
}

fn stage_names(plan: &Value) -> Vec<&str> {
    plan["stages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|stage| stage["stage"].as_str().unwrap())
        .collect()
}

fn error_code(output: &std::process::Output) -> String {
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("error envelope");
    envelope["error"]["code"].as_str().unwrap().to_owned()
}

fn start_committed_candidate(workspace: &Workspace, card_id: &str, path: &str) {
    workspace.work(&["start", "--card-id", card_id]);
    let worktree = workspace.worktrees.join(card_id);
    let file = worktree.join(path);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "// candidate\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: candidate"]);
}

fn handoff_and_approve(workspace: &Workspace, card_id: &str) {
    let worktree = workspace.worktrees.join(card_id);
    let head = support::capture(&worktree, &["rev-parse", "HEAD"]);
    let declaration = workspace.root.join(format!("{card_id}-declaration.yaml"));
    fs::write(
        &declaration,
        format!(
            "delivered_sha: {head}\nbehavior_delivered: scoped change\nimplementation_decisions: [minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
        ),
    )
    .unwrap();
    workspace.handoff(&[
        "create",
        "--card-id",
        card_id,
        "--declaration",
        &declaration.display().to_string(),
    ]);
    workspace.review(&["begin", "--card-id", card_id]);
    let verdict = workspace.root.join(format!("{card_id}-verdict.yaml"));
    fs::write(
        &verdict,
        "reviewer_actor_id: reviewer-session\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: direct proof\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\n",
    )
    .unwrap();
    workspace.review(&[
        "record",
        "--card-id",
        card_id,
        "--verdict",
        &verdict.display().to_string(),
        "--actor",
        "reviewer-session",
    ]);
}

#[test]
fn low_risk_preflight_is_deterministic_and_names_only_narrow_then_final_stages() {
    let workspace = active_cycle();
    activate(
        &workspace,
        "F-001",
        CardSpec {
            risk: "low",
            include: "src/low.rs",
            feature: "gate.unit",
            review: "",
            integration: "gate.all",
            proof_map: false,
        },
    );

    let first = workspace.gate_json(&["preflight", "--card-id", "F-001"]);
    let second = workspace.gate_json(&["preflight", "--card-id", "F-001"]);
    assert_eq!(first, second, "same frozen records must produce one plan");
    let plan = &first["data"];
    assert_eq!(plan["schema"], "harness.validation-plan/v1");
    assert_eq!(plan["risk"], "low");
    assert!(plan["card_digest"].as_str().unwrap().starts_with("sha256:"));
    assert!(
        plan["policy_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    assert!(plan["proof_map_digest"].is_null());
    assert_eq!(stage_names(plan), ["narrow", "final_integration"]);
    assert_eq!(plan["next_permitted_stage"], "narrow");
    assert_eq!(
        plan["stages"][0]["checks"][0]["receipt_schema"],
        "harness.receipt/v1"
    );
}

#[test]
fn medium_and_high_preflight_require_and_project_the_full_registered_ladder() {
    let workspace = active_cycle();
    workspace.register_gate("gate.review", &["true"]);
    for (card_id, risk, include) in [
        ("F-001", "medium", "src/medium.rs"),
        ("F-002", "high", "src/high.rs"),
    ] {
        activate(
            &workspace,
            card_id,
            CardSpec {
                risk,
                include,
                feature: "gate.unit",
                review: "gate.review",
                integration: "gate.all",
                proof_map: true,
            },
        );
        let plan = workspace.gate_json(&["preflight", "--card-id", card_id]);
        assert_eq!(
            stage_names(&plan["data"]),
            ["narrow", "handoff", "final_integration"]
        );
        assert!(
            plan["data"]["proof_map_digest"]
                .as_str()
                .unwrap()
                .starts_with("sha256:")
        );
        assert_eq!(plan["data"]["next_permitted_stage"], "narrow");
    }
}

#[test]
fn required_missing_stage_gate_and_reordered_gate_are_refused_before_any_execution() {
    let missing = active_cycle();
    activate(
        &missing,
        "F-001",
        CardSpec {
            risk: "high",
            include: "src/high.rs",
            feature: "gate.unit",
            review: "",
            integration: "gate.all",
            proof_map: true,
        },
    );
    let output = missing.gate_raw(&["preflight", "--card-id", "F-001"]);
    assert_eq!(output.status.code(), Some(5));
    assert!(String::from_utf8_lossy(&output.stdout).contains("`handoff` stage"));

    let reordered = active_cycle();
    activate(
        &reordered,
        "F-001",
        CardSpec {
            risk: "low",
            include: "src/reordered.rs",
            feature: "gate.unit",
            review: "gate.unit",
            integration: "gate.all",
            proof_map: false,
        },
    );
    let output = reordered.gate_raw(&["preflight", "--card-id", "F-001"]);
    assert_eq!(output.status.code(), Some(5));
    assert!(String::from_utf8_lossy(&output.stdout).contains("more than one validation stage"));
}

#[test]
fn stale_base_or_project_policy_refuses_before_a_plan_is_returned() {
    let stale_base = active_cycle();
    activate(
        &stale_base,
        "F-001",
        CardSpec {
            risk: "low",
            include: "src/stale.rs",
            feature: "gate.unit",
            review: "",
            integration: "gate.all",
            proof_map: false,
        },
    );
    let card_path = stale_base.control.join("cards/F-001/r1.json");
    let mut card: CardRecord =
        serde_json::from_str(&fs::read_to_string(&card_path).unwrap()).unwrap();
    card.base_sha = "f".repeat(40);
    fs::write(
        &card_path,
        format!("{}\n", serde_json::to_string_pretty(&card).unwrap()),
    )
    .unwrap();
    let state_path = stale_base.control.join("cards/F-001/state.json");
    let mut state: CardStateRecord =
        serde_json::from_str(&fs::read_to_string(&state_path).unwrap()).unwrap();
    state.current_digest = card.digest().unwrap();
    fs::write(
        &state_path,
        format!("{}\n", serde_json::to_string_pretty(&state).unwrap()),
    )
    .unwrap();
    let output = stale_base.gate_raw(&["preflight", "--card-id", "F-001"]);
    assert_eq!(output.status.code(), Some(5));
    assert!(String::from_utf8_lossy(&output.stdout).contains("CH-POLICY-CYCLE-BASELINE-MISMATCH"));

    let stale_policy = active_cycle();
    activate(
        &stale_policy,
        "F-001",
        CardSpec {
            risk: "low",
            include: "src/policy.rs",
            feature: "gate.unit",
            review: "",
            integration: "gate.all",
            proof_map: false,
        },
    );
    let project_path = stale_policy.control.join("project/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
    project["validation_policy"]["stage_requirements"]["low"] = serde_json::json!(["narrow"]);
    fs::write(
        &project_path,
        format!("{}\n", serde_json::to_string_pretty(&project).unwrap()),
    )
    .unwrap();
    let output = stale_policy.gate_raw(&["preflight", "--card-id", "F-001"]);
    assert_eq!(output.status.code(), Some(5));
    assert!(String::from_utf8_lossy(&output.stdout).contains("CH-POLICY-INVALID-CYCLE"));
}

#[test]
fn progressive_execution_allows_only_the_next_gate_and_reserves_final_for_integration() {
    let workspace = active_cycle();
    workspace.register_gate("gate.review", &["true"]);
    activate(
        &workspace,
        "F-001",
        CardSpec {
            risk: "medium",
            include: "src/progressive.rs",
            feature: "gate.unit",
            review: "gate.review",
            integration: "gate.all",
            proof_map: true,
        },
    );
    start_committed_candidate(&workspace, "F-001", "src/progressive.rs");

    // A later handoff gate may not leap over the narrow proof. Preview and
    // real use the same evaluator and therefore refuse identically.
    for args in [
        vec![
            "run",
            "--card-id",
            "F-001",
            "--gate-id",
            "gate.review",
            "--dry-run",
        ],
        vec!["run", "--card-id", "F-001", "--gate-id", "gate.review"],
    ] {
        let output = workspace.gate_raw(&args);
        assert_eq!(output.status.code(), Some(5));
        assert_eq!(error_code(&output), "CH-POLICY-INVALID-TRANSITION");
    }

    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);
    let after_narrow = workspace.gate_json(&["preflight", "--card-id", "F-001"]);
    assert_eq!(after_narrow["data"]["next_permitted_stage"], "handoff");
    assert_eq!(after_narrow["data"]["next_permitted_gate"], "gate.review");

    // The full/integration check cannot be scheduled on an individual card.
    let output = workspace.gate_raw(&["run", "--card-id", "F-001", "--gate-id", "gate.all"]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-INVALID-TRANSITION");

    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.review"]);
    let final_only = workspace.gate_json(&["preflight", "--card-id", "F-001"]);
    assert_eq!(
        final_only["data"]["next_permitted_stage"],
        "final_integration"
    );
    assert_eq!(final_only["data"]["next_permitted_gate"], "gate.all");
}

#[test]
fn an_approved_card_cannot_schedule_final_integration_until_handoff_evidence_is_current() {
    let workspace = active_cycle();
    workspace.register_gate("gate.review", &["true"]);
    activate(
        &workspace,
        "F-001",
        CardSpec {
            risk: "medium",
            include: "src/integration.rs",
            feature: "gate.unit",
            review: "gate.review",
            integration: "gate.all",
            proof_map: true,
        },
    );
    start_committed_candidate(&workspace, "F-001", "src/integration.rs");
    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);
    handoff_and_approve(&workspace, "F-001");

    let ready = workspace.integration_json(&["ready", "--cycle-id", "C-001"]);
    assert!(ready["data"]["ready"].as_array().unwrap().is_empty());
    let waiting = &ready["data"]["not_ready"][0];
    assert_eq!(waiting["card_id"], "F-001");
    assert!(
        waiting["reason"]
            .as_str()
            .unwrap()
            .contains("required validation before integration"),
        "final integration must be blocked by the missing handoff gate: {waiting}"
    );
}
