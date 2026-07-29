//! `WP-430` acceptance: landing commit construction.
//!
//! The landing commit is the thing promotion will publish. Everything here
//! turns on one property: it is fully formed and inspectable *before* anything
//! irreversible happens, and building it moves no branch.

mod support;

use std::fs;

use support::Workspace;

/// A cycle with `count` approved, merged cards, returning the integration id.
fn merged(count: usize) -> (Workspace, String) {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    for index in 1..=count {
        let card = format!("F-{index:03}");
        workspace.activate_card(&card, &[&format!("src/{card}/**")]);
        workspace.approve_card(&card, &format!("src/{card}/a.rs"));
    }
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
    workspace.integration(&[
        "merge",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
    ]);
    (workspace, id)
}

fn error_code(output: &std::process::Output) -> String {
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["code"].as_str().unwrap().to_owned()
}

/// Trailer values under one key, read from the built commit.
fn trailer_values(envelope: &serde_json::Value, key: &str) -> Vec<String> {
    envelope["data"]["trailers"]
        .as_array()
        .expect("trailers")
        .iter()
        .filter(|entry| entry[0] == key)
        .map(|entry| entry[1].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn the_landing_commit_has_the_authority_baseline_as_first_parent() {
    let (workspace, id) = merged(2);
    let authority = workspace.authority_head();

    let envelope =
        workspace.integration_json(&["land", "--integration-id", &id, "--actor-id", "coordinator"]);
    assert_eq!(envelope["data"]["first_parent"], authority);
}

#[test]
fn the_landing_commit_has_the_integration_head_as_second_parent() {
    let (workspace, id) = merged(2);
    let head = workspace.integration_json(&["inspect", "--integration-id", &id])["data"]
        ["integration_head"]
        .as_str()
        .map(ToOwned::to_owned);

    let envelope =
        workspace.integration_json(&["land", "--integration-id", &id, "--actor-id", "coordinator"]);
    assert_eq!(
        envelope["data"]["second_parent"]
            .as_str()
            .map(ToOwned::to_owned),
        head
    );
}

#[test]
fn the_landing_tree_equals_the_verified_integration_tree() {
    let (workspace, id) = merged(2);
    let tree = workspace.integration_json(&["inspect", "--integration-id", &id])["data"]
        ["integration_tree"]
        .as_str()
        .map(ToOwned::to_owned);

    let envelope =
        workspace.integration_json(&["land", "--integration-id", &id, "--actor-id", "coordinator"]);
    assert_eq!(
        envelope["data"]["tree"].as_str().map(ToOwned::to_owned),
        tree,
        "the landing must carry the exact tree that was merged, not a rebuild"
    );
}

#[test]
fn building_the_landing_commit_does_not_move_the_protected_ref() {
    let (workspace, id) = merged(2);
    let authority_before = workspace.authority_head();
    let candidate_before = workspace.candidate_head();

    let envelope =
        workspace.integration_json(&["land", "--integration-id", &id, "--actor-id", "coordinator"]);
    let landing = envelope["data"]["landing_sha"].as_str().unwrap();

    assert_eq!(workspace.authority_head(), authority_before);
    assert_eq!(workspace.candidate_head(), candidate_before);

    // Section 13.5: unreachable from the protected branch until accepted.
    let contains = support::capture(
        &workspace.repository,
        &["branch", "--contains", landing, "--format=%(refname)"],
    );
    assert!(
        contains.is_empty(),
        "the landing commit must not be on any branch yet: {contains}"
    );
}

#[test]
fn the_landing_commit_is_retained_by_a_harness_ref() {
    let (workspace, id) = merged(2);
    let envelope =
        workspace.integration_json(&["land", "--integration-id", &id, "--actor-id", "coordinator"]);
    let landing = envelope["data"]["landing_sha"].as_str().unwrap();
    assert_eq!(
        envelope["data"]["landing_ref"],
        format!("refs/harness/landing/{id}")
    );

    // Nothing else points at it, so without the ref, collection would take it
    // and promotion would have to rebuild and re-verify everything.
    support::git(&workspace.repository, &["gc", "--prune=now", "--quiet"]);
    let resolved = support::capture(&workspace.repository, &["rev-parse", landing]);
    assert_eq!(resolved, landing);
}

#[test]
fn the_subject_names_the_integration_and_is_deterministic() {
    let (workspace, id) = merged(2);
    let envelope =
        workspace.integration_json(&["land", "--integration-id", &id, "--actor-id", "coordinator"]);
    let subject = envelope["data"]["subject"].as_str().unwrap();
    assert!(
        subject.contains(&id),
        "the subject must name the integration: {subject}"
    );
    assert_eq!(subject, "Land INT-001 (2 cards, batch)");
}

#[test]
fn trailers_carry_the_cycle_cards_record_and_receipts() {
    let (workspace, id) = merged(2);
    let envelope =
        workspace.integration_json(&["land", "--integration-id", &id, "--actor-id", "coordinator"]);

    assert_eq!(
        trailer_values(&envelope, "Change-Harness-Integration"),
        [id]
    );
    assert_eq!(trailer_values(&envelope, "Change-Harness-Cycle"), ["C-001"]);

    let cards = trailer_values(&envelope, "Change-Harness-Card");
    assert_eq!(cards.len(), 2);
    assert!(cards[0].starts_with("F-001 r1 "), "unexpected: {cards:?}");
    assert!(cards[0].contains("review RV-"), "unexpected: {cards:?}");

    let digests = trailer_values(&envelope, "Change-Harness-Integration-Digest");
    assert_eq!(digests.len(), 1);
    assert!(digests[0].starts_with("sha256:"), "unexpected: {digests:?}");

    let receipts = trailer_values(&envelope, "Change-Harness-Receipt");
    assert!(
        !receipts.is_empty(),
        "the gates the landing rests on must be named"
    );
    assert!(
        receipts[0].contains("gate.unit"),
        "unexpected: {receipts:?}"
    );
}

#[test]
fn an_unmerged_integration_cannot_be_landed() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
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

    let output =
        workspace.integration_raw(&["land", "--integration-id", &id, "--actor-id", "coordinator"]);
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(error_code(&output), "CH-PRECONDITION-NOT-FOUND");
}

#[test]
fn an_authority_that_moved_after_the_plan_blocks_landing() {
    let (workspace, id) = merged(1);
    // Someone lands directly on the protected branch after the plan was built.
    workspace.advance_authority();

    let output =
        workspace.integration_raw(&["land", "--integration-id", &id, "--actor-id", "coordinator"]);
    assert_eq!(output.status.code(), Some(6));
    assert_eq!(error_code(&output), "CH-CONFLICT-CONTROL-HEAD-MOVED");
}

#[test]
fn a_card_declaring_generated_artifacts_fails_with_the_wp_540_code() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);

    let body = format!(
        "card_id: F-001\ncycle_id: C-001\ntitle: Implement F-001\ngoal: Deliver F-001\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [\"src/F-001/**\"]\n  exclude: []\ngenerated_artifacts: [\"dist/bundle.js\"]\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
        base = workspace.authority_head()
    );
    let path = workspace.root.join("F-001.yaml");
    fs::write(&path, body).unwrap();
    workspace.card(&["create", "--draft", &path.display().to_string()]);
    workspace.card(&["activate", "--card-id", "F-001"]);
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
    workspace.integration(&[
        "merge",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
    ]);

    let output =
        workspace.integration_raw(&["land", "--integration-id", &id, "--actor-id", "coordinator"]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-UNSUPPORTED-UNTIL-WP-540");
}

#[test]
fn an_integration_cannot_be_landed_twice() {
    let (workspace, id) = merged(1);
    workspace.integration(&["land", "--integration-id", &id, "--actor-id", "coordinator"]);

    let output =
        workspace.integration_raw(&["land", "--integration-id", &id, "--actor-id", "coordinator"]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-INVALID-TRANSITION");
}

#[test]
fn a_land_dry_run_reports_the_plan_and_builds_nothing() {
    let (workspace, id) = merged(2);
    let before = workspace.control_head();

    let envelope = workspace.integration_json(&[
        "land",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
        "--dry-run",
    ]);
    assert_eq!(envelope["data"]["dry_run"], true);
    assert_eq!(envelope["data"]["first_parent"], workspace.authority_head());

    assert_eq!(workspace.control_head(), before);
    let refs = support::capture(
        &workspace.repository,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/harness/landing",
        ],
    );
    assert!(refs.is_empty(), "a dry run must retain nothing: {refs}");
}

#[test]
fn the_landing_is_recorded_so_promotion_can_reload_it() {
    let (workspace, id) = merged(2);
    let envelope =
        workspace.integration_json(&["land", "--integration-id", &id, "--actor-id", "coordinator"]);
    let landing = envelope["data"]["landing_sha"].as_str().unwrap();

    // Promotion reloads through `inspect`, so the landing must be visible there.
    let inspected = workspace.integration_json(&["inspect", "--integration-id", &id]);
    assert_eq!(inspected["data"]["landing_sha"], landing);

    let event = workspace
        .events()
        .into_iter()
        .find(|event| event["event_type"] == "integration.landing-built")
        .expect("the landing must be recorded");
    assert_eq!(event["head_sha"], landing);
    assert_eq!(event["metadata"]["landing_sha"], landing);
}

#[test]
fn scenario_30_a_landing_tree_that_no_longer_matches_the_merge_is_refused() {
    let (workspace, id) = merged(1);

    // Rewrite the integration head so it carries a different tree than the one
    // the merge recorded. This is what a hand-edited or amended integration
    // looks like from the outside, and it must not reach a landing commit.
    let head = workspace.integration_json(&["inspect", "--integration-id", &id])["data"]
        ["integration_head"]
        .as_str()
        .unwrap()
        .to_owned();
    let tampered = tamper_tree(&workspace, &head);
    assert_ne!(tampered, head);
    set_integration_head(&workspace, &id, &tampered);

    let output =
        workspace.integration_raw(&["land", "--integration-id", &id, "--actor-id", "coordinator"]);
    assert_eq!(output.status.code(), Some(6));
    assert_eq!(error_code(&output), "CH-CONFLICT-MERGE-FAILED");
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("recorded tree"),
        "the mismatch must be named: {envelope}"
    );
}

/// Builds a commit with the same parents as `head` but a different tree.
fn tamper_tree(workspace: &Workspace, head: &str) -> String {
    let parent = support::capture(&workspace.repository, &["rev-parse", &format!("{head}^1")]);
    let other_tree = support::capture(
        &workspace.repository,
        &["rev-parse", &format!("{parent}^{{tree}}")],
    );
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(&workspace.repository)
        .args(["commit-tree", &other_tree, "-p", &parent, "-m", "tampered"])
        .output()
        .expect("git should run");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Rewrites the recorded integration head, simulating an external edit.
fn set_integration_head(workspace: &Workspace, id: &str, head: &str) {
    let path = workspace.control.join(format!("integrations/{id}.json"));
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    value["integration_head"] = serde_json::json!(head);
    fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&value).unwrap()),
    )
    .unwrap();
}
