//! Tier 3, defect 24: `--dry-run` must refuse whatever the real command refuses.
//!
//! A preview that skips checks is worse than no preview, because it tells an
//! operator a command will succeed when it will not — and the operator then
//! runs the real one having been told it is safe. `archive close` was the
//! extreme case, performing the destruction it was asked to preview, and the
//! reviewers found seven further paths that skip checks.
//!
//! Every test here builds a state the real command refuses, runs both forms,
//! and requires the same error code. The pairing is the point: asserting only
//! that the dry run fails would pass for a dry run that fails for its own
//! unrelated reason.

mod support;

use std::fs;

use support::Workspace;

fn code(output: &std::process::Output) -> String {
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "an envelope, got {}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    envelope["error"]["code"]
        .as_str()
        .unwrap_or("<no code>")
        .to_owned()
}

/// Runs a command with and without `--dry-run` and requires the same refusal.
fn assert_parity(label: &str, real: &std::process::Output, preview: &std::process::Output) {
    assert!(
        !real.status.success(),
        "{label}: the fixture must be one the real command refuses"
    );
    assert!(
        !preview.status.success(),
        "{label}: the dry run reported success for something the real command refuses: {}",
        String::from_utf8_lossy(&preview.stdout)
    );
    assert_eq!(
        code(preview),
        code(real),
        "{label}: the dry run must refuse for the same reason"
    );
}

fn active_cycle() -> Workspace {
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

#[test]
fn work_start_previews_a_held_lease_refusal() {
    let workspace = active_cycle();
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    // Starting the same card twice is refused: it already holds a lease.
    assert_parity(
        "work start",
        &workspace.work_raw(&["start", "--card-id", "F-001"]),
        &workspace.work_raw(&["start", "--card-id", "F-001", "--dry-run"]),
    );
}

#[test]
fn work_start_previews_an_existing_branch_refusal() {
    let workspace = active_cycle();
    workspace.activate_card("F-001", &["src/**"]);
    support::git(&workspace.repository, &["branch", "card/F-001"]);

    let real = workspace.work_raw(&["start", "--card-id", "F-001"]);
    assert_parity(
        "work start",
        &real,
        &workspace.work_raw(&["start", "--card-id", "F-001", "--dry-run"]),
    );
    assert_eq!(
        code(&real),
        "CH-PRECONDITION-BRANCH-EXISTS",
        "the fixture must exercise the existing-branch refusal"
    );
}

#[test]
fn review_record_previews_a_self_review_refusal() {
    let workspace = active_cycle();
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);
    let path = workspace.worktrees.join("F-001");
    fs::create_dir_all(path.join("src")).unwrap();
    fs::write(path.join("src/a.rs"), "fn main() {}\n").unwrap();
    support::git(&path, &["add", "-A"]);
    support::git(&path, &["commit", "-q", "-m", "feat: add a.rs"]);
    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);
    let head = support::capture(&path, &["rev-parse", "HEAD"]);
    let declaration = workspace.root.join("declaration.yaml");
    fs::write(
        &declaration,
        format!(
            "delivered_sha: {head}\nbehavior_delivered: adds a.rs\nimplementation_decisions: [minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
        ),
    )
    .unwrap();
    workspace.handoff(&[
        "create",
        "--card-id",
        "F-001",
        "--declaration",
        &declaration.display().to_string(),
    ]);
    workspace.review(&["begin", "--card-id", "F-001"]);

    // The fixture's handoff actor is `operator`, so this is a self-review.
    let verdict = workspace.root.join("verdict.yaml");
    fs::write(
        &verdict,
        "reviewer_actor_id: operator\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\nresidual_risks: []\n",
    )
    .unwrap();
    let path = verdict.display().to_string();

    assert_parity(
        "review record",
        &workspace.review_raw(&["record", "--card-id", "F-001", "--verdict", &path]),
        &workspace.review_raw(&[
            "record",
            "--card-id",
            "F-001",
            "--verdict",
            &path,
            "--dry-run",
        ]),
    );
}

#[test]
fn review_record_previews_a_staleness_refusal() {
    // `preview_record` called `check_independence` but never
    // `require_current_handoff`, so a dry run reported success for a verdict
    // the real command refused on staleness grounds — for every decision,
    // approvals included. This pins the approval case: revoking the handoff
    // refuses an approval with `CH-POLICY-STALE-HANDOFF`, and the preview
    // must agree before this fix and does not.
    let workspace = active_cycle();
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);
    let path = workspace.worktrees.join("F-001");
    fs::create_dir_all(path.join("src")).unwrap();
    fs::write(path.join("src/a.rs"), "fn main() {}\n").unwrap();
    support::git(&path, &["add", "-A"]);
    support::git(&path, &["commit", "-q", "-m", "feat: add a.rs"]);
    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);
    let head = support::capture(&path, &["rev-parse", "HEAD"]);
    let declaration = workspace.root.join("declaration.yaml");
    fs::write(
        &declaration,
        format!(
            "delivered_sha: {head}\nbehavior_delivered: adds a.rs\nimplementation_decisions: [minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
        ),
    )
    .unwrap();
    workspace.handoff(&[
        "create",
        "--card-id",
        "F-001",
        "--declaration",
        &declaration.display().to_string(),
    ]);
    workspace.review(&["begin", "--card-id", "F-001"]);
    workspace.handoff(&["revoke", "--card-id", "F-001", "--reason", "withdrawn"]);

    // The staleness check runs before the card-transition check, so the
    // card sitting in `active` (not `review_pending`) afterward doesn't
    // matter here — both forms should refuse on the handoff, not the state.
    let verdict = workspace.root.join("verdict.yaml");
    fs::write(
        &verdict,
        "reviewer_actor_id: reviewer-session-a\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\nresidual_risks: []\n",
    )
    .unwrap();
    let path = verdict.display().to_string();

    let real = workspace.review_raw(&["record", "--card-id", "F-001", "--verdict", &path]);
    assert_parity(
        "review record",
        &real,
        &workspace.review_raw(&[
            "record",
            "--card-id",
            "F-001",
            "--verdict",
            &path,
            "--dry-run",
        ]),
    );
    assert_eq!(
        code(&real),
        "CH-POLICY-STALE-HANDOFF",
        "the fixture must exercise the staleness refusal, not something else"
    );
}

#[test]
fn handoff_create_previews_a_rewritten_branch_refusal() {
    let workspace = active_cycle();
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);
    let path = workspace.worktrees.join("F-001");
    fs::create_dir_all(path.join("src")).unwrap();
    fs::write(path.join("src/a.rs"), "fn main() {}\n").unwrap();
    support::git(&path, &["add", "-A"]);
    support::git(&path, &["commit", "-q", "-m", "feat: add a.rs"]);
    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);
    let head = support::capture(&path, &["rev-parse", "HEAD"]);

    // The branch moves after the actor decided what they delivered — the
    // SPIKE-001 F-1 case the whole exact-SHA binding exists for.
    fs::write(path.join("src/b.rs"), "fn b() {}\n").unwrap();
    support::git(&path, &["add", "-A"]);
    support::git(&path, &["commit", "-q", "-m", "feat: sneak in b.rs"]);

    let declaration = workspace.root.join("declaration.yaml");
    fs::write(
        &declaration,
        format!(
            "delivered_sha: {head}\nbehavior_delivered: adds a.rs\nimplementation_decisions: [minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
        ),
    )
    .unwrap();
    let path = declaration.display().to_string();

    assert_parity(
        "handoff create",
        &workspace.handoff_raw(&["create", "--card-id", "F-001", "--declaration", &path]),
        &workspace.handoff_raw(&[
            "create",
            "--card-id",
            "F-001",
            "--declaration",
            &path,
            "--dry-run",
        ]),
    );
}

#[test]
fn cycle_activate_previews_a_repeat_activation_refusal() {
    let workspace = active_cycle();
    assert_parity(
        "cycle activate",
        &workspace.cycle_raw(&["activate", "--cycle-id", "C-001"]),
        &workspace.cycle_raw(&["activate", "--cycle-id", "C-001", "--dry-run"]),
    );
}

#[test]
fn card_activate_previews_an_overlapping_scope_refusal() {
    let workspace = active_cycle();
    workspace.activate_card("F-001", &["src/**"]);

    let draft = workspace.root.join("F-002.yaml");
    fs::write(
        &draft,
        format!(
            "card_id: F-002\ncycle_id: C-001\ntitle: Implement F-002\ngoal: Deliver F-002\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [src/shared.rs]\n  exclude: []\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
            base = workspace.authority_head()
        ),
    )
    .unwrap();
    workspace.card(&["create", "--draft", &draft.display().to_string()]);

    assert_parity(
        "card activate",
        &workspace.card_raw(&["activate", "--card-id", "F-002"]),
        &workspace.card_raw(&["activate", "--card-id", "F-002", "--dry-run"]),
    );
}

#[test]
fn acceptance_record_previews_an_unreviewed_integration_refusal() {
    let workspace = active_cycle();
    workspace.activate_card("F-001", &["src/F-001/**"]);
    workspace.approve_card("F-001", "src/F-001/a.rs");
    let id = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
    ])["data"]["integration_id"]
        .as_str()
        .unwrap()
        .to_owned();

    // Never merged, landed, verified, or reviewed.
    assert_parity(
        "acceptance record",
        &workspace.acceptance_raw(&[
            "record",
            "--integration-id",
            &id,
            "--acceptance-owner",
            "owner",
        ]),
        &workspace.acceptance_raw(&[
            "record",
            "--integration-id",
            &id,
            "--acceptance-owner",
            "owner",
            "--dry-run",
        ]),
    );
}

#[test]
fn integration_prepare_previews_a_nothing_approved_refusal() {
    let workspace = active_cycle();
    workspace.activate_card("F-001", &["src/F-001/**"]);

    assert_parity(
        "integration prepare",
        &workspace.integration_raw(&[
            "prepare",
            "--cycle-id",
            "C-001",
            "--actor-id",
            "coordinator",
        ]),
        &workspace.integration_raw(&[
            "prepare",
            "--cycle-id",
            "C-001",
            "--actor-id",
            "coordinator",
            "--dry-run",
        ]),
    );
}

#[test]
fn gate_run_previews_an_unallocated_card_refusal() {
    let workspace = active_cycle();
    workspace.activate_card("F-001", &["src/**"]);

    // No `work start`, so there is no worktree to run a gate in.
    assert_parity(
        "gate run",
        &workspace.gate_raw(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]),
        &workspace.gate_raw(&[
            "run",
            "--card-id",
            "F-001",
            "--gate-id",
            "gate.unit",
            "--dry-run",
        ]),
    );
}
