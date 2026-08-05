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

/// Whether `ancestor` is `descendant` itself or reachable from it.
///
/// `git merge-base --is-ancestor` treats a commit as its own ancestor, which
/// is exactly the "or equal to" half of the relationship the anchor trailer
/// is required to have with the control head after land completes.
fn is_ancestor(repository: &std::path::Path, ancestor: &str, descendant: &str) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .status()
        .expect("git should run")
        .success()
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

    // The ref has to actually exist and name this commit. Asserting the name
    // the envelope reported only proves the envelope is consistent with itself
    // — deleting `landing::retain` outright leaves that assertion passing.
    assert_eq!(
        support::capture(
            &workspace.repository,
            &["rev-parse", &format!("refs/harness/landing/{id}")]
        ),
        landing,
        "the retention ref must exist in the repository, not just in the report"
    );

    // And the commit must survive collection. Checked with `cat-file -e`, not
    // `rev-parse`: given a 40-character hex string `rev-parse` echoes it back
    // and exits 0 whether or not the object exists, so the assertion this test
    // used to make — that `rev-parse <landing>` equals `<landing>` — held
    // whatever the repository contained.
    support::git(&workspace.repository, &["gc", "--prune=now", "--quiet"]);
    assert!(
        support::object_exists(&workspace.repository, landing),
        "the landing commit must survive collection"
    );
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
fn a_landing_commit_anchors_the_control_head() {
    let (workspace, id) = merged(2);
    // Read before invoking `land`: nothing else touches control between this
    // call and the `land` process opening it, so this is exactly the head
    // that process will observe while building the trailer.
    let control_head_before_land = workspace.control_head();

    let envelope =
        workspace.integration_json(&["land", "--integration-id", &id, "--actor-id", "coordinator"]);

    // The actual value, not its shape: a mutation anchoring some other real
    // SHA (the authority head, say) would still produce 40 hex characters and
    // pass a shape check, but must fail this one.
    assert_eq!(
        trailer_values(&envelope, "Change-Harness-Control"),
        [control_head_before_land],
        "the trailer must carry the exact control head the landing was built from"
    );
}

#[test]
fn the_anchored_head_is_the_control_state_the_landing_was_built_from() {
    let (workspace, id) = merged(2);

    let envelope =
        workspace.integration_json(&["land", "--integration-id", &id, "--actor-id", "coordinator"]);
    let anchored = trailer_values(&envelope, "Change-Harness-Control");
    assert_eq!(anchored.len(), 1, "exactly one control anchor trailer");
    let anchored = &anchored[0];

    // Land's own control commit — recording the landing SHA into the
    // integration record and the `integration.landing-built` event — lands
    // after the trailer is built, so the control head has moved past the
    // anchor by the time land returns. Never equal: this is the relationship
    // #88's ancestor check depends on, so it must fail if a mutation makes
    // the anchor equal to (or beyond) the post-land head instead of a strict
    // ancestor of it.
    let control_after = workspace.control_head();
    assert_ne!(
        anchored, &control_after,
        "the anchored head must be strictly behind the control head land itself advances to"
    );
    assert!(
        is_ancestor(&workspace.control, anchored, &control_after),
        "anchored control head {anchored} must be an ancestor of the post-land control head {control_after}"
    );
}

#[test]
fn the_existing_five_trailers_are_unchanged_by_the_new_anchor() {
    let (workspace, id) = merged(2);
    let envelope =
        workspace.integration_json(&["land", "--integration-id", &id, "--actor-id", "coordinator"]);

    // Same keys, same values as `trailers_carry_the_cycle_cards_record_and_receipts`
    // establishes independently — reasserted here, on a commit that also
    // carries the new anchor, so a mutation that let building the anchor
    // disturb one of these (an off-by-one in the vec, an accidental
    // overwrite) fails here even if it happened to dodge the other test.
    assert_eq!(
        trailer_values(&envelope, "Change-Harness-Integration"),
        [id]
    );
    assert_eq!(trailer_values(&envelope, "Change-Harness-Cycle"), ["C-001"]);

    let cards = trailer_values(&envelope, "Change-Harness-Card");
    assert_eq!(cards.len(), 2);
    assert!(cards[0].starts_with("F-001 r1 "), "unexpected: {cards:?}");
    assert!(cards[1].starts_with("F-002 r1 "), "unexpected: {cards:?}");

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

    let control = trailer_values(&envelope, "Change-Harness-Control");
    assert_eq!(control.len(), 1, "exactly one control anchor trailer");

    // Every trailer in the commit falls under one of these six known keys —
    // proving the card added exactly one trailer and altered no other. A
    // mutation that dropped one of the five while still adding the anchor
    // (or duplicated a trailer under a stray key) changes this sum without
    // necessarily changing any single key's count above.
    let all = envelope["data"]["trailers"].as_array().expect("trailers");
    assert_eq!(
        all.len(),
        1 + 1 + cards.len() + digests.len() + receipts.len() + control.len(),
        "trailer count must be exactly the five existing plus the new anchor: {all:?}"
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
fn a_card_declaring_a_shared_artifact_cannot_land_yet() {
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
        "card_id: F-001\ncycle_id: C-001\ntitle: Implement F-001\ngoal: Deliver F-001\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [\"src/F-001/**\"]\n  exclude: []\ngenerated_artifacts:\n  - path: dist/bundle.js\n    class: shared\n    generator: gate.bundle\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
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
