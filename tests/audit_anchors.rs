//! `#88` acceptance: verifying every anchored control head is still an
//! ancestor of the control record.
//!
//! `#87` writes the control repository's head into every landing commit as a
//! `Change-Harness-Control` trailer. An anchor nothing checks is just a
//! string in a commit message — every test here is about the one thing that
//! turns it into a check: does the anchor still hold, told apart correctly
//! from "it never held" and from "there is nothing here to worry about".
//!
//! `#89` acceptance too, from here to the end of the file: the same check,
//! called at the one boundary where it is load-bearing rather than optional.
//! `integration promote` must refuse before the protected branch moves, a
//! dry run must refuse identically, an intact record must promote exactly as
//! it always did, and every read-only command an operator needs in order to
//! diagnose the refusal must keep working.

mod support;

use std::fs;

use support::Workspace;

fn error_code(output: &std::process::Output) -> String {
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["code"].as_str().unwrap().to_owned()
}

/// Trailer values under one key, read from a command's JSON envelope.
///
/// Copied from `landing_commit.rs`'s helper of the same name and shape.
/// Fixture helpers are not shared across files in this suite (see also
/// `promotion.rs`'s `reviewed`/`accepted` versus `landing_commit.rs`'s
/// `merged`), so this is a deliberate, small duplication rather than an
/// oversight.
fn trailer_values(envelope: &serde_json::Value, key: &str) -> Vec<String> {
    envelope["data"]["trailers"]
        .as_array()
        .expect("trailers")
        .iter()
        .filter(|entry| entry[0] == key)
        .map(|entry| entry[1].as_str().unwrap().to_owned())
        .collect()
}

/// Drives one complete cycle to a promoted landing commit on an existing
/// workspace, and returns the integration id, the landing commit's own SHA,
/// and the control head it anchored.
///
/// Mirrors `lifecycle.rs`'s `run_cycle` fixture (itself proof that a second,
/// independent cycle composes correctly against a protected branch the first
/// one already moved), minus archiving — this file has no use for archive
/// state. `card`/`cycle` are parameterized, unlike `support::Workspace`'s own
/// `activate_card`, which hardcodes `cycle_id: C-001` into the card body it
/// writes; a second landing under its own cycle needs the real one.
fn landed(workspace: &Workspace, cycle: &str, card: &str, file: &str) -> (String, String, String) {
    workspace.cycle(&[
        "create",
        "--cycle-id",
        cycle,
        "--objective",
        "Deliver one bounded change",
    ]);
    workspace.cycle(&["activate", "--cycle-id", cycle]);

    let body = format!(
        "card_id: {card}\ncycle_id: {cycle}\ntitle: Implement {card}\ngoal: Deliver {card}\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [\"src/{card}/**\"]\n  exclude: []\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
        base = workspace.authority_head()
    );
    let draft = workspace.root.join(format!("{card}.yaml"));
    fs::write(&draft, body).unwrap();
    workspace.card(&["create", "--draft", &draft.display().to_string()]);
    workspace.card(&["activate", "--card-id", card]);

    workspace.work(&["start", "--card-id", card]);
    let worktree = workspace.worktrees.join(card);
    fs::create_dir_all(worktree.join(format!("src/{card}"))).unwrap();
    fs::write(worktree.join(file), format!("// {card}\n")).unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", &format!("feat: {card}")]);
    workspace.gate(&["run", "--card-id", card, "--gate-id", "gate.unit"]);

    let delivered = support::capture(&worktree, &["rev-parse", "HEAD"]);
    let declaration = workspace.root.join(format!("{card}-declaration.yaml"));
    fs::write(
        &declaration,
        format!(
            "delivered_sha: {delivered}\nbehavior_delivered: adds {file}\nimplementation_decisions: [minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
        ),
    )
    .unwrap();
    workspace.handoff(&[
        "create",
        "--card-id",
        card,
        "--declaration",
        &declaration.display().to_string(),
    ]);

    workspace.review(&["begin", "--card-id", card]);
    let verdict = workspace.root.join(format!("{card}-verdict.yaml"));
    fs::write(
        &verdict,
        "reviewer_actor_id: reviewer-session\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed each acceptance behavior directly\nresidual_risks: []\n",
    )
    .unwrap();
    workspace.review(&[
        "record",
        "--card-id",
        card,
        "--verdict",
        &verdict.display().to_string(),
    ]);

    let id =
        workspace.integration_json(&["prepare", "--cycle-id", cycle, "--actor-id", "coordinator"])
            ["data"]["integration_id"]
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
    let land_envelope =
        workspace.integration_json(&["land", "--integration-id", &id, "--actor-id", "coordinator"]);
    let anchor = trailer_values(&land_envelope, "Change-Harness-Control")
        .into_iter()
        .next()
        .expect("a landing commit must carry exactly one control anchor trailer");

    workspace.integration(&["verify", "--integration-id", &id, "--actor-id", "verifier"]);
    workspace.integration(&[
        "review",
        "--integration-id",
        &id,
        "--reviewer-actor-id",
        "integration-reviewer",
    ]);
    workspace.acceptance(&[
        "record",
        "--integration-id",
        &id,
        "--acceptance-owner",
        "acceptance-owner",
    ]);
    workspace.integration(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);

    let landing_sha = workspace.authority_head();
    (id, landing_sha, anchor)
}

/// A fresh project with a single landed, promoted integration.
fn single_landed() -> (Workspace, String, String, String) {
    let workspace = Workspace::initialized();
    let (id, landing_sha, anchor) = landed(&workspace, "C-001", "F-001", "src/F-001/a.rs");
    (workspace, id, landing_sha, anchor)
}

fn audit_anchors_raw(workspace: &Workspace) -> std::process::Output {
    Workspace::run(&[
        "audit".into(),
        "anchors".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--output".into(),
        "json".into(),
    ])
}

fn audit_anchors_json(workspace: &Workspace) -> serde_json::Value {
    let output = audit_anchors_raw(workspace);
    assert!(
        output.status.success(),
        "audit anchors failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("the JSON envelope")
}

#[test]
fn a_healthy_project_reports_no_anchor_discrepancy() {
    let (workspace, _id, _landing_sha, _anchor) = single_landed();

    let envelope = audit_anchors_json(&workspace);
    assert_eq!(envelope["schema"], "harness.command-result/v1");
    assert_eq!(envelope["command"], "audit.anchors");
    assert_eq!(
        envelope["data"]["discrepancies"].as_array().unwrap().len(),
        0
    );
    // Exact counts, not just "some": a check that examined nothing would also
    // report zero discrepancies, and that must not read as healthy.
    assert_eq!(envelope["data"]["landing_commits_examined"], 1);
    assert_eq!(envelope["data"]["anchors_checked"], 1);
}

#[test]
fn a_rewritten_control_history_is_detected() {
    let workspace = Workspace::initialized();
    let (_first_id, first_landing_sha, first_anchor) =
        landed(&workspace, "C-001", "F-001", "src/F-001/a.rs");
    // A second landing, so the orphaned anchor sits on an OLDER landing than
    // the newest one reachable from the protected branch. An enumeration that
    // only inspected the newest landing would never see the corrupted one and
    // this test would pass for the wrong reason.
    let (_second_id, _second_landing_sha, second_anchor) =
        landed(&workspace, "C-002", "F-002", "src/F-002/a.rs");
    assert_ne!(
        first_anchor, second_anchor,
        "sanity: the two landings must anchor different control heads"
    );

    // Narrowly corrupt exactly the first anchor: a sibling commit with the
    // same tree (so `project/project.json`, and everything else the control
    // repository needs in order to stay readable, survives intact) built on
    // the anchor's own parent, so the anchor itself is no longer reachable
    // from `main` — the way #85's fixtures corrupt exactly one thing rather
    // than rebuilding history wholesale.
    let parent = support::capture(
        &workspace.control,
        &["rev-parse", &format!("{first_anchor}^")],
    );
    let tree = support::capture(
        &workspace.control,
        &["rev-parse", &format!("{first_anchor}^{{tree}}")],
    );
    let rewritten = support::capture(
        &workspace.control,
        &[
            "commit-tree",
            &tree,
            "-p",
            &parent,
            "-m",
            "test: rewritten control history",
        ],
    );
    support::git(&workspace.control, &["reset", "--hard", &rewritten]);

    // Sanity: the fixture must actually have built the case it claims to —
    // the object still present, just no longer an ancestor of the new head.
    assert!(support::object_exists(&workspace.control, &first_anchor));
    let still_ancestor = std::process::Command::new("git")
        .arg("-C")
        .arg(&workspace.control)
        .args(["merge-base", "--is-ancestor", &first_anchor, &rewritten])
        .status()
        .expect("git should run")
        .success();
    assert!(
        !still_ancestor,
        "the fixture must actually orphan the anchor"
    );

    let output = audit_anchors_raw(&workspace);
    assert!(!output.status.success(), "a rewritten anchor must not pass");
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-AUDIT-DISCREPANCY");
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(
        message.contains(&first_anchor),
        "the orphaned anchor's SHA must be named: {message}"
    );
    assert!(
        message.contains(&first_landing_sha),
        "the landing commit that claimed it must be named: {message}"
    );
}

#[test]
fn appending_to_the_control_record_never_reports_a_discrepancy() {
    let workspace = Workspace::initialized();
    landed(&workspace, "C-001", "F-001", "src/F-001/a.rs");

    // Several further control-mutating commands, none of them another
    // landing — proving the check tolerates ordinary continued work, not
    // just more landings.
    workspace.register_gate("gate.extra", &["true"]);
    workspace.activate_card("F-002", &["src/F-002/**"]);
    workspace.approve_card("F-002", "src/F-002/a.rs");
    workspace.configure_convergence_policy(5, 5);

    // And a second landing too: growth by landing again must be exactly as
    // clean as growth by anything else.
    landed(&workspace, "C-002", "F-003", "src/F-003/a.rs");

    let envelope = audit_anchors_json(&workspace);
    assert!(
        envelope["data"]["discrepancies"]
            .as_array()
            .unwrap()
            .is_empty(),
        "ordinary further work must never be reported as tampering: {envelope}"
    );
    assert_eq!(envelope["data"]["landing_commits_examined"], 2);
    assert_eq!(envelope["data"]["anchors_checked"], 2);
}

#[test]
fn an_anchored_head_absent_from_the_control_repository_is_a_discrepancy_not_a_crash() {
    let (workspace, _id, _landing_sha, anchor) = single_landed();

    // Make the anchored control commit genuinely gone, not merely
    // unreachable. `reset --hard` alone leaves it reachable through the
    // reflog, which `gc --prune=now` respects (verified directly: a fresh
    // reflog entry survives `--prune=now` on its own), so the reflog is
    // expired first. `object_exists` below is not decoration; it is what
    // tells this test it actually built the case it claims to.
    support::git(
        &workspace.control,
        &["reset", "--hard", &format!("{anchor}^")],
    );
    support::git(
        &workspace.control,
        &["reflog", "expire", "--expire=now", "--all"],
    );
    support::git(&workspace.control, &["gc", "--prune=now", "--quiet"]);
    assert!(
        !support::object_exists(&workspace.control, &anchor),
        "the fixture must actually remove the anchored commit"
    );

    let output = audit_anchors_raw(&workspace);
    assert!(!output.status.success(), "an absent anchor must not pass");
    assert_eq!(
        output.status.code(),
        Some(5),
        "a discrepancy, not a crash or an unrelated tool failure, must be reported: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(error_code(&output), "CH-POLICY-AUDIT-DISCREPANCY");
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(
        message.contains(&anchor),
        "the missing anchor must be named: {message}"
    );
    assert!(
        !message.contains("ancestor"),
        "case 3 (absent entirely) must read differently from case 2 (orphaned by a \
         rewrite), even though both are tampering: {message}"
    );
}

#[test]
fn the_command_changes_nothing() {
    let (workspace, _id, _landing_sha, _anchor) = single_landed();

    let authority_before = workspace.authority_head();
    let control_before = workspace.control_head();

    audit_anchors_json(&workspace);

    assert_eq!(
        workspace.authority_head(),
        authority_before,
        "the protected branch must not move"
    );
    assert_eq!(
        workspace.control_head(),
        control_before,
        "the control head must not move"
    );
    assert_eq!(
        support::capture(&workspace.control, &["status", "--porcelain"]),
        "",
        "the control worktree must stay clean"
    );
}

// --- #89: the same check, called at the promotion boundary -----------------

/// Same shape as `landed`, stopped one command short of `promote`: the
/// integration reaches `accepted` and no further. `#89`'s tests need a
/// pending promotion to run against a control record that a *different*,
/// already-promoted integration corrupted, and `landed` always promotes —
/// it cannot build that half of the fixture on its own.
///
/// The cycle/card setup below is copied from `landed`'s own inline block
/// rather than `support::Workspace::activate_card`, for the reason its own
/// comment gives: `activate_card` hardcodes `cycle_id: C-001`, and a second
/// cycle needs the real one. The work-through-review stretch has no such
/// constraint, so it reuses `approve_card` directly instead of copying that
/// part too.
fn accepted_but_not_promoted(workspace: &Workspace, cycle: &str, card: &str, file: &str) -> String {
    workspace.cycle(&[
        "create",
        "--cycle-id",
        cycle,
        "--objective",
        "Deliver one bounded change",
    ]);
    workspace.cycle(&["activate", "--cycle-id", cycle]);

    let body = format!(
        "card_id: {card}\ncycle_id: {cycle}\ntitle: Implement {card}\ngoal: Deliver {card}\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [\"src/{card}/**\"]\n  exclude: []\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
        base = workspace.authority_head()
    );
    let draft = workspace.root.join(format!("{card}.yaml"));
    fs::write(&draft, body).unwrap();
    workspace.card(&["create", "--draft", &draft.display().to_string()]);
    workspace.card(&["activate", "--card-id", card]);

    workspace.approve_card(card, file);

    let id =
        workspace.integration_json(&["prepare", "--cycle-id", cycle, "--actor-id", "coordinator"])
            ["data"]["integration_id"]
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
    workspace.integration(&["land", "--integration-id", &id, "--actor-id", "coordinator"]);
    workspace.integration(&["verify", "--integration-id", &id, "--actor-id", "verifier"]);
    workspace.integration(&[
        "review",
        "--integration-id",
        &id,
        "--reviewer-actor-id",
        "integration-reviewer",
    ]);
    workspace.acceptance(&[
        "record",
        "--integration-id",
        &id,
        "--acceptance-owner",
        "acceptance-owner",
    ]);
    id
}

/// The landing commit an integration's promotion would use, read before
/// promoting it. `landed` reads the same fact the other way around —
/// `workspace.authority_head()` *after* promotion moves the branch there —
/// which only works once promotion has already happened. Mirrors
/// `promotion.rs`'s helper of the same name and shape.
fn landing_of(workspace: &Workspace, id: &str) -> String {
    workspace.integration_json(&["inspect", "--integration-id", id])["data"]["landing_sha"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Rewrites control history so `orphaned` — an anchor from an earlier,
/// already-promoted landing — is no longer an ancestor of control head,
/// while the *content* control head points at is untouched: one synthetic
/// commit reuses the real current head's tree wholesale, parented on a
/// sibling of `orphaned` instead of on `orphaned` itself.
///
/// Deliberately not `a_rewritten_control_history_is_detected`'s technique,
/// which resets straight to the sibling and abandons everything recorded
/// since. That is fine for auditing, which only ever reads git objects and
/// the authority's trailers — but `#89`'s check runs inside `check_promotion`
/// *after* `load_integration` has already read the pending integration's own
/// record out of the control tree, so a fixture that made that record
/// disappear would refuse promotion for the wrong reason before the check
/// under test ever ran.
fn orphan_an_earlier_anchor(workspace: &Workspace, orphaned: &str) {
    let parent = support::capture(&workspace.control, &["rev-parse", &format!("{orphaned}^")]);
    let orphaned_tree = support::capture(
        &workspace.control,
        &["rev-parse", &format!("{orphaned}^{{tree}}")],
    );
    let sibling = support::capture(
        &workspace.control,
        &[
            "commit-tree",
            &orphaned_tree,
            "-p",
            &parent,
            "-m",
            "test: rewritten control history",
        ],
    );

    let current_head = workspace.control_head();
    let current_tree = support::capture(
        &workspace.control,
        &["rev-parse", &format!("{current_head}^{{tree}}")],
    );
    let rewritten = support::capture(
        &workspace.control,
        &[
            "commit-tree",
            &current_tree,
            "-p",
            &sibling,
            "-m",
            "test: fast-forward the rewrite to the real current state",
        ],
    );
    support::git(&workspace.control, &["reset", "--hard", &rewritten]);

    // Sanity: the fixture must actually have built the case it claims to —
    // the anchor still present as an object, just no longer reachable.
    assert!(support::object_exists(&workspace.control, orphaned));
    let still_ancestor = std::process::Command::new("git")
        .arg("-C")
        .arg(&workspace.control)
        .args(["merge-base", "--is-ancestor", orphaned, &rewritten])
        .status()
        .expect("git should run")
        .success();
    assert!(
        !still_ancestor,
        "the fixture must actually orphan the anchor"
    );
}

/// The scenario `#89` exists for: one integration landed and promoted, its
/// control anchor then orphaned by a history rewrite, and a second
/// integration carried all the way to `accepted` and no further — so every
/// test below drives the exact same `integration promote` call against it.
///
/// Returns the workspace, the orphaned anchor, the landing commit that
/// claimed it, and the pending integration's id.
fn corrupted_first_anchor_pending_second() -> (Workspace, String, String, String) {
    let workspace = Workspace::initialized();
    let (_first_id, first_landing_sha, first_anchor) =
        landed(&workspace, "C-001", "F-001", "src/F-001/a.rs");
    let second_id = accepted_but_not_promoted(&workspace, "C-002", "F-002", "src/F-002/a.rs");

    orphan_an_earlier_anchor(&workspace, &first_anchor);

    (workspace, first_anchor, first_landing_sha, second_id)
}

#[test]
fn a_rewritten_control_record_refuses_promotion() {
    let (workspace, first_anchor, first_landing_sha, second_id) =
        corrupted_first_anchor_pending_second();

    let refused = workspace.integration_raw(&[
        "promote",
        "--integration-id",
        &second_id,
        "--actor-id",
        "promoter",
    ]);
    assert!(
        !refused.status.success(),
        "a rewritten control record must not promote"
    );
    assert_eq!(error_code(&refused), "CH-POLICY-AUDIT-DISCREPANCY");

    let envelope: serde_json::Value = serde_json::from_slice(&refused.stdout).unwrap();
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(
        message.contains(&first_anchor),
        "the orphaned anchor's SHA must be named: {message}"
    );
    assert!(
        message.contains(&first_landing_sha),
        "the landing commit that claimed it must be named: {message}"
    );

    // The remedy, not just the diagnosis — a refusal with no way forward is
    // exactly what #79 and #73 already made this repository pay for once
    // each. Anchored on substance, not the full sentence, so a rewording
    // that keeps the meaning still passes: "restore", "backup", and
    // "anchored commit" are each load-bearing nouns from
    // `require_control_anchors_intact`'s own established remedy wording
    // ("restore the control repository from a `backup create` archive that
    // contains the anchored commit") and, unlike "restore" alone — which
    // also appears earlier in this same message describing the *cause*
    // ("an out-of-band restore to an earlier control state") — the
    // three-way conjunction only holds while the remedy sentence itself is
    // present.
    assert!(
        message.contains("restore")
            && message.contains("backup")
            && message.contains("anchored commit"),
        "the remedy — restore the control repository from a backup containing the \
         anchored commit — must be named: {message}"
    );
    // "deliberately" is the term of art this codebase already uses for this
    // exact point (see `require_control_anchors_intact`'s own doc comment:
    // "No override, deliberately."), so a rewording that keeps the meaning
    // is expected to keep the word; requiring it alongside "override" pins
    // that the refusal is permanent by design, not merely that the word
    // "override" appears somewhere.
    assert!(
        message.contains("override") && message.contains("deliberately"),
        "the refusal must say there is deliberately no override, so nobody reads its \
         absence as an oversight and adds one: {message}"
    );
}

#[test]
fn promotion_is_unaffected_when_every_anchor_holds() {
    let workspace = Workspace::initialized();
    landed(&workspace, "C-001", "F-001", "src/F-001/a.rs");
    let second_id = accepted_but_not_promoted(&workspace, "C-002", "F-002", "src/F-002/a.rs");
    let second_landing = landing_of(&workspace, &second_id);

    let promoted = workspace.integration_json(&[
        "promote",
        "--integration-id",
        &second_id,
        "--actor-id",
        "promoter",
    ]);
    assert_eq!(promoted["data"]["status"], "promoted");
    assert_eq!(
        workspace.authority_head(),
        second_landing,
        "an intact anchor history must not block an otherwise-clean promotion"
    );
}

#[test]
fn the_protected_branch_does_not_move_when_promotion_is_refused() {
    let (workspace, _first_anchor, _first_landing_sha, second_id) =
        corrupted_first_anchor_pending_second();
    let before = workspace.authority_head();

    let refused = workspace.integration_raw(&[
        "promote",
        "--integration-id",
        &second_id,
        "--actor-id",
        "promoter",
    ]);
    assert!(!refused.status.success());
    assert_eq!(error_code(&refused), "CH-POLICY-AUDIT-DISCREPANCY");

    assert_eq!(
        workspace.authority_head(),
        before,
        "the protected branch must not move when promotion is refused — the property this \
         card exists for"
    );
}

#[test]
fn the_dry_run_refuses_the_same_way() {
    let (workspace, _first_anchor, _first_landing_sha, second_id) =
        corrupted_first_anchor_pending_second();

    let preview = workspace.integration_raw(&[
        "promote",
        "--integration-id",
        &second_id,
        "--actor-id",
        "promoter",
        "--dry-run",
    ]);
    assert_eq!(
        error_code(&preview),
        "CH-POLICY-AUDIT-DISCREPANCY",
        "a preview must never promise what the real command rejects"
    );

    let real = workspace.integration_raw(&[
        "promote",
        "--integration-id",
        &second_id,
        "--actor-id",
        "promoter",
    ]);
    assert_eq!(
        error_code(&preview),
        error_code(&real),
        "the dry run must refuse exactly the way the real command does"
    );
}

#[test]
fn read_only_commands_still_work_on_a_broken_anchor() {
    let (workspace, _first_anchor, _first_landing_sha, second_id) =
        corrupted_first_anchor_pending_second();

    // The refusal itself is the previous tests' job; this one is about what
    // stays usable after it.
    let refused = workspace.integration_raw(&[
        "promote",
        "--integration-id",
        &second_id,
        "--actor-id",
        "promoter",
    ]);
    assert!(!refused.status.success());

    // `audit anchors` is the check `#89` calls, not a command it gates — it
    // still runs and reports exactly what it always would for this input.
    let audit = audit_anchors_raw(&workspace);
    assert!(
        !audit.status.success(),
        "the discrepancy is still reported, not hidden by the new refusal"
    );
    assert_eq!(error_code(&audit), "CH-POLICY-AUDIT-DISCREPANCY");

    // And a status command: an operator diagnosing the refusal must still be
    // able to see the record `#89` declined to build on.
    let inspect = workspace.integration_json(&["inspect", "--integration-id", &second_id]);
    assert_eq!(inspect["data"]["status"], "accepted");
}
