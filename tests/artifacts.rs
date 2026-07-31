//! `WP-540` acceptance: generated-artifact classification and ownership.
//!
//! Generated files are where ownership goes wrong quietly. Two cards run the
//! same generator, both commit the output, and the conflict arrives at merge as
//! a diff nobody wrote — or one side silently wins. These tests are about
//! deciding the owner before anyone writes the file.

mod support;

use change_harness::policy::paths::{CaseSensitivity, Scope, matches};
use std::fs;

use support::Workspace;

fn error_code(output: &std::process::Output) -> String {
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["code"].as_str().unwrap().to_owned()
}

/// Writes a draft declaring the given generated-artifact YAML block.
fn draft(workspace: &Workspace, card: &str, include: &str, artifacts: &str) -> String {
    draft_excluding(workspace, card, include, "", artifacts)
}

/// The same, with an explicit exclude list.
///
/// Carving a shared artifact out of the scope is the only legal way to declare
/// one now, so several fixtures need it.
fn draft_excluding(
    workspace: &Workspace,
    card: &str,
    include: &str,
    exclude: &str,
    artifacts: &str,
) -> String {
    let body = format!(
        "card_id: {card}\ncycle_id: C-001\ntitle: Implement {card}\ngoal: Deliver {card}\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [{include}]\n  exclude: [{exclude}]\ngenerated_artifacts:\n{artifacts}named_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
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
    let include = ["src/a/**".to_owned()];
    let scope = Scope::new(&include, &[]);
    assert!(scope.allows("src/a/owned.rs"));
    assert!(
        !scope.allows("src/b/unowned.rs"),
        "the fixture's source region must contain an actually unowned path"
    );

    let workspace = active_cycle();
    let path = draft(
        &workspace,
        "F-001",
        "\"src/a/**\"",
        "  - path: src/a/generated.rs\n    class: per_card\n    generator: gate.all\n    sources: [\"src/b/**\"]\n",
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
        // A real source path, not the include pattern repeated. This test used
        // `sources: ["src/**"]` against `include: ["src/**"]`, so it passed
        // because the two strings were identical — it certified `==`, not
        // ownership, and could not tell the two implementations apart.
        "  - path: src/generated.rs\n    class: per_card\n    generator: gate.all\n    sources: [\"src/schema.toml\"]\n",
    );
    workspace.card(&["validate", "--draft", &path]);
}

#[test]
fn a_card_claiming_a_shared_artifact_in_its_own_scope_is_refused() {
    let case = CaseSensitivity::host();
    assert!(
        matches("dist/*.js", "dist/bundle.js", case)
            && matches("dist/bundle.*", "dist/bundle.js", case),
        "dist/bundle.js must witness real overlap between the two fixture globs"
    );

    let workspace = active_cycle();
    let path = draft(
        &workspace,
        "F-001",
        "\"src/**\", \"dist/*.js\"",
        "  - path: dist/bundle.*\n    class: shared\n    generator: gate.all\n",
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
    // The shared path is carved out of the scope. That is now the only legal
    // way to declare one — a card whose include covers the artifact is refused
    // at activation — and this test needs a legal card to reach the verify-side
    // rule at all.
    let path = draft_excluding(
        &workspace,
        "F-001",
        "\"src/**\"",
        "\"src/shared.gen.rs\"",
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

#[test]
fn a_shared_artifact_covered_by_a_glob_include_is_refused() {
    // Tier 4. The check compared write-scope *patterns* to artifact *paths*
    // with `==`, so it only ever fired when the include literally spelled the
    // artifact. A glob include covering the same path sailed through: one path
    // with two owners, which is the whole thing the shared class exists to
    // prevent.
    let workspace = active_cycle();
    let path = draft(
        &workspace,
        "F-001",
        "\"src/**\"",
        "  - path: src/generated/api.rs\n    class: shared\n    generator: gate.all\n",
    );
    let output = workspace.card_raw(&["validate", "--draft", &path]);
    assert!(
        !output.status.success(),
        "a glob include covering a shared artifact must be refused: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "CH-POLICY-INVALID-ARTIFACT");
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("two owners"),
        "{envelope}"
    );
}

#[test]
fn a_per_card_source_covered_by_a_glob_include_is_accepted() {
    // The other direction of the same defect: `==` refused a source the card
    // plainly owns, because the include was a glob rather than the literal
    // path. This failed closed, which is the more annoying half — a correct
    // card could not be written at all.
    let workspace = active_cycle();
    let path = draft(
        &workspace,
        "F-001",
        "\"src/**\"",
        "  - path: src/generated.rs\n    class: per_card\n    generator: gate.all\n    sources: [\"src/schema.toml\"]\n",
    );
    workspace.card(&["validate", "--draft", &path]);
}

#[test]
fn a_shared_artifact_excluded_from_the_scope_is_accepted() {
    // The guard that stops the shared arm refusing everything. Carving the
    // integration-owned path out of the card's own scope is the only way to
    // declare a shared artifact at all — and a card must declare it to get the
    // protection at verify time. An arm that ignored excludes would make the
    // whole Shared class undeclarable by any card whose scope globs the tree.
    let workspace = active_cycle();
    let path = draft_excluding(
        &workspace,
        "F-001",
        "\"src/**\"",
        "\"src/generated/api.rs\"",
        "  - path: src/generated/api.rs\n    class: shared\n    generator: gate.all\n",
    );
    workspace.card(&["validate", "--draft", &path]);
}

#[test]
fn a_shared_artifact_glob_only_partly_excluded_from_the_scope_is_refused() {
    let include = ["src/**".to_owned()];
    let exclude = ["src/*".to_owned()];
    let scope = Scope::new(&include, &exclude);
    assert!(!scope.allows("src/one.rs"));
    assert!(
        scope.allows("src/deep/file.rs")
            && matches("src/**", "src/deep/file.rs", CaseSensitivity::host()),
        "src/deep/file.rs must witness shared ownership left behind by the partial exclude"
    );

    let workspace = active_cycle();
    let path = draft_excluding(
        &workspace,
        "F-001",
        "\"src/**\"",
        "\"src/*\"",
        "  - path: src/**\n    class: shared\n    generator: gate.all\n",
    );
    let output = workspace.card_raw(&["validate", "--draft", &path]);
    assert!(
        !output.status.success(),
        "a partial exclude must not hide the remaining two-owner region: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(error_code(&output), "CH-POLICY-INVALID-ARTIFACT");
}

#[test]
fn a_shared_artifact_glob_fully_excluded_from_the_scope_is_accepted() {
    let workspace = active_cycle();
    let path = draft_excluding(
        &workspace,
        "F-001",
        "\"src/**\"",
        "\"src/generated/**\"",
        "  - path: src/generated/**\n    class: shared\n    generator: gate.all\n",
    );
    workspace.card(&["validate", "--draft", &path]);
}

#[test]
fn a_shared_artifact_outside_the_scope_is_accepted() {
    let workspace = active_cycle();
    let path = draft(
        &workspace,
        "F-001",
        "\"src/**\"",
        "  - path: dist/bundle.js\n    class: shared\n    generator: gate.all\n",
    );
    workspace.card(&["validate", "--draft", &path]);
}

#[test]
fn a_per_card_source_broader_than_the_scope_is_refused() {
    // The guard that stops the sources arm going loose. Reaching for
    // intersection here — because the shared arm uses it — would let a card
    // owning `src/a/**` declare `src/**` as a source, which is exactly the
    // staleness the rule exists to prevent.
    let workspace = active_cycle();
    let path = draft(
        &workspace,
        "F-001",
        "\"src/a/**\"",
        "  - path: src/a/generated.rs\n    class: per_card\n    generator: gate.all\n    sources: [\"src/**\"]\n",
    );
    let output = workspace.card_raw(&["validate", "--draft", &path]);
    assert!(!output.status.success());
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("go stale"),
        "{envelope}"
    );
}

#[test]
fn a_per_card_source_excluded_from_the_scope_is_refused() {
    // Containment must honour excludes. `==` ignored them entirely.
    let workspace = active_cycle();
    let path = draft_excluding(
        &workspace,
        "F-001",
        "\"src/**\"",
        "\"src/schema.toml\"",
        "  - path: src/generated.rs\n    class: per_card\n    generator: gate.all\n    sources: [\"src/schema.toml\"]\n",
    );
    let output = workspace.card_raw(&["validate", "--draft", &path]);
    assert!(
        !output.status.success(),
        "an excluded source is not owned: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn a_per_card_source_glob_broader_than_a_single_segment_scope_is_refused() {
    let include = ["src/*".to_owned()];
    let scope = Scope::new(&include, &[]);
    assert!(scope.allows("src/one.rs"));
    assert!(
        !scope.allows("src/nested/two.rs"),
        "the source glob must contain a concrete path beyond the card's scope"
    );

    let workspace = active_cycle();
    let path = draft(
        &workspace,
        "F-001",
        "\"src/*\"",
        "  - path: src/generated.rs\n    class: per_card\n    generator: gate.all\n    sources: [\"src/**\"]\n",
    );
    let output = workspace.card_raw(&["validate", "--draft", &path]);
    assert!(
        !output.status.success(),
        "src/** includes nested paths that src/* does not own: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(error_code(&output), "CH-POLICY-INVALID-ARTIFACT");
}

#[test]
fn a_per_card_source_glob_intersecting_an_exclude_is_refused() {
    let include = ["src/**".to_owned()];
    let exclude = ["src/b/private/**".to_owned()];
    let scope = Scope::new(&include, &exclude);
    assert!(scope.allows("src/b/public.rs"));
    assert!(
        !scope.allows("src/b/private/secret.rs"),
        "the source glob must contain a concrete path removed by the exclude"
    );

    let workspace = active_cycle();
    let path = draft_excluding(
        &workspace,
        "F-001",
        "\"src/**\"",
        "\"src/b/private/**\"",
        "  - path: src/generated.rs\n    class: per_card\n    generator: gate.all\n    sources: [\"src/b/**\"]\n",
    );
    let output = workspace.card_raw(&["validate", "--draft", &path]);
    assert!(
        !output.status.success(),
        "the source glob includes paths the card explicitly excludes: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(error_code(&output), "CH-POLICY-INVALID-ARTIFACT");
}

#[test]
fn a_nested_per_card_source_glob_with_a_disjoint_exclude_is_accepted() {
    let workspace = active_cycle();
    let path = draft_excluding(
        &workspace,
        "F-001",
        "\"src/**\"",
        "\"docs/**\"",
        "  - path: src/generated.rs\n    class: per_card\n    generator: gate.all\n    sources: [\"src/b/**\"]\n",
    );
    workspace.card(&["validate", "--draft", &path]);
}

#[test]
fn a_generated_source_pattern_cannot_traverse_outside_the_repository() {
    let workspace = active_cycle();
    let path = draft(
        &workspace,
        "F-001",
        "\"src/**\"",
        "  - path: src/generated.rs\n    class: per_card\n    generator: gate.all\n    sources: [\"src/../secret/**\"]\n",
    );
    let output = workspace.card_raw(&["validate", "--draft", &path]);
    assert!(
        !output.status.success(),
        "generated-source patterns must not escape their apparent scope: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("must not traverse upward"),
        "the path validator must own the refusal: {envelope}"
    );
}

#[test]
fn a_generated_artifact_path_cannot_traverse_outside_the_repository() {
    let workspace = active_cycle();
    let path = draft(
        &workspace,
        "F-001",
        "\"src/**\"",
        "  - path: src/../generated.rs\n    class: per_card\n    generator: gate.all\n    sources: [\"src/schema.toml\"]\n",
    );
    let output = workspace.card_raw(&["validate", "--draft", &path]);
    assert!(!output.status.success());
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        envelope["error"]["message"].as_str().unwrap().contains(
            "generated artifact path pattern `src/../generated.rs` must not traverse upward"
        ),
        "the artifact path must use the same repository-boundary validator: {envelope}"
    );
}

#[test]
fn a_generated_source_pattern_cannot_name_git_internals_through_dot_segments() {
    let workspace = active_cycle();
    let path = draft(
        &workspace,
        "F-001",
        "\"**\"",
        "  - path: generated.rs\n    class: per_card\n    generator: gate.all\n    sources: [\"./.git/**\"]\n",
    );
    let output = workspace.card_raw(&["validate", "--draft", &path]);
    assert!(
        !output.status.success(),
        "normalized generated-source patterns must not name Git internals: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("must not name Git internals"),
        "the repository-path validator must own the refusal: {envelope}"
    );
}

#[test]
fn a_generated_artifact_path_cannot_name_case_varied_git_internals() {
    let workspace = active_cycle();
    let path = draft(
        &workspace,
        "F-001",
        "\"**\"",
        "  - path: ./.GIT/generated.rs\n    class: per_card\n    generator: gate.all\n    sources: [\"src/schema.toml\"]\n",
    );
    let output = workspace.card_raw(&["validate", "--draft", &path]);
    assert!(!output.status.success());
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("must not name Git internals"),
        "Git-internal rejection must be deterministic across host case rules: {envelope}"
    );
}
