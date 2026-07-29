//! `WP-540` acceptance: generated-artifact classification and ownership.
//!
//! Generated files are where ownership goes wrong quietly. Two cards run the
//! same generator, both commit the output, and the conflict arrives at merge as
//! a diff nobody wrote — or one side silently wins. These tests are about
//! deciding the owner before anyone writes the file.

mod support;

use std::fs;

use support::Workspace;

fn error_code(output: &std::process::Output) -> String {
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["code"].as_str().unwrap().to_owned()
}

/// Writes a draft declaring the given generated-artifact YAML block.
fn draft(workspace: &Workspace, card: &str, include: &str, artifacts: &str) -> String {
    let body = format!(
        "card_id: {card}\ncycle_id: C-001\ntitle: Implement {card}\ngoal: Deliver {card}\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [{include}]\n  exclude: []\ngenerated_artifacts:\n{artifacts}named_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
        base = workspace.authority_head()
    );
    let path = workspace.root.join(format!("{card}.yaml"));
    fs::write(&path, body).unwrap();
    path.display().to_string()
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
fn one_path_declared_under_two_classes_is_refused() {
    let workspace = active_cycle();
    let path = draft(
        &workspace,
        "F-001",
        "\"src/**\"",
        "  - path: src/generated.rs\n    class: transient\n  - path: src/generated.rs\n    class: shared\n    generator: gate.all\n",
    );
    let output = workspace.card_raw(&["validate", "--draft", &path]);
    assert!(!output.status.success());
    assert_eq!(error_code(&output), "CH-POLICY-INVALID-ARTIFACT");
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(message.contains("transient"), "unexpected: {message}");
    assert!(message.contains("shared"), "unexpected: {message}");
}

#[test]
fn a_per_card_artifact_generated_from_sources_outside_the_scope_is_refused() {
    let workspace = active_cycle();
    let path = draft(
        &workspace,
        "F-001",
        "\"src/**\"",
        "  - path: src/generated.rs\n    class: per_card\n    generator: gate.all\n    sources: [\"schema/**\"]\n",
    );
    let output = workspace.card_raw(&["validate", "--draft", &path]);
    assert!(!output.status.success());
    assert_eq!(error_code(&output), "CH-POLICY-INVALID-ARTIFACT");
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("go stale"),
        "the reason must say why it matters: {envelope}"
    );
}

#[test]
fn a_per_card_artifact_generated_from_owned_sources_is_accepted() {
    let workspace = active_cycle();
    let path = draft(
        &workspace,
        "F-001",
        "\"src/**\"",
        "  - path: src/generated.rs\n    class: per_card\n    generator: gate.all\n    sources: [\"src/**\"]\n",
    );
    workspace.card(&["validate", "--draft", &path]);
}

#[test]
fn a_card_claiming_a_shared_artifact_in_its_own_scope_is_refused() {
    let workspace = active_cycle();
    let path = draft(
        &workspace,
        "F-001",
        "\"src/**\", \"dist/bundle.js\"",
        "  - path: dist/bundle.js\n    class: shared\n    generator: gate.all\n",
    );
    let output = workspace.card_raw(&["validate", "--draft", &path]);
    assert!(!output.status.success());
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("two owners"),
        "unexpected: {envelope}"
    );
}

#[test]
fn a_serialized_artifact_without_an_allocated_identifier_is_refused() {
    let workspace = active_cycle();
    let path = draft(
        &workspace,
        "F-001",
        "\"migrations/**\"",
        "  - path: migrations/0007.sql\n    class: serialized\n",
    );
    let output = workspace.card_raw(&["validate", "--draft", &path]);
    assert!(!output.status.success());
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("allocated before work"),
        "the reason must name the race it prevents: {envelope}"
    );
}

#[test]
fn a_serialized_artifact_with_its_identifier_activates() {
    let workspace = active_cycle();
    let path = draft(
        &workspace,
        "F-001",
        "\"migrations/**\"",
        "  - path: migrations/0007.sql\n    class: serialized\n    identifier: \"0007\"\n",
    );
    workspace.card(&["create", "--draft", &path]);
    workspace.card(&["activate", "--card-id", "F-001"]);
}

#[test]
fn committing_a_transient_path_fails_verification() {
    let workspace = active_cycle();
    let path = draft(
        &workspace,
        "F-001",
        "\"src/**\"",
        "  - path: src/build-output.js\n    class: transient\n",
    );
    workspace.card(&["create", "--draft", &path]);
    workspace.card(&["activate", "--card-id", "F-001"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    let worktree = workspace.worktrees.join("F-001");
    fs::create_dir_all(worktree.join("src")).unwrap();
    fs::write(worktree.join("src/build-output.js"), "// built\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: commit the build"]);

    let output = workspace.work_raw(&["verify", "--card-id", "F-001"]);
    assert!(
        !output.status.success(),
        "transient output belongs to nobody and must not be committed"
    );
    // The envelope carries the summary code; the finding's own wording is what
    // reaches the operator, so that is what this asserts.
    assert_eq!(error_code(&output), "CH-POLICY-CANDIDATE-OUT-OF-SCOPE");
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("declared transient and must never be committed"),
        "the reason must say why: {envelope}"
    );
}

#[test]
fn committing_a_shared_path_fails_verification() {
    let workspace = active_cycle();
    let path = draft(
        &workspace,
        "F-001",
        "\"src/**\"",
        "  - path: src/shared.gen.rs\n    class: shared\n    generator: gate.all\n",
    );
    workspace.card(&["create", "--draft", &path]);
    workspace.card(&["activate", "--card-id", "F-001"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    let worktree = workspace.worktrees.join("F-001");
    fs::create_dir_all(worktree.join("src")).unwrap();
    fs::write(worktree.join("src/shared.gen.rs"), "// generated\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(
        &worktree,
        &["commit", "-q", "-m", "feat: commit the shared file"],
    );

    let output = workspace.work_raw(&["verify", "--card-id", "F-001"]);
    assert!(
        !output.status.success(),
        "integration owns a shared artifact, so no card may commit it"
    );
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("integration generates it"),
        "the reason must name who owns it: {envelope}"
    );
}

#[test]
fn committing_a_per_card_artifact_verifies() {
    // The regression: classification must not make ordinary generated work
    // impossible, only unowned work.
    let workspace = active_cycle();
    let path = draft(
        &workspace,
        "F-001",
        "\"src/**\"",
        "  - path: src/generated.rs\n    class: per_card\n    generator: gate.all\n    sources: [\"src/**\"]\n",
    );
    workspace.card(&["create", "--draft", &path]);
    workspace.card(&["activate", "--card-id", "F-001"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    let worktree = workspace.worktrees.join("F-001");
    fs::create_dir_all(worktree.join("src")).unwrap();
    fs::write(worktree.join("src/generated.rs"), "// generated\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: regenerate"]);

    workspace.work(&["verify", "--card-id", "F-001"]);
}

#[test]
fn a_card_declaring_nothing_generated_is_unaffected() {
    let workspace = active_cycle();
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);
    let worktree = workspace.worktrees.join("F-001");
    fs::create_dir_all(worktree.join("src")).unwrap();
    fs::write(worktree.join("src/a.rs"), "// work\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: work"]);
    workspace.work(&["verify", "--card-id", "F-001"]);
}
