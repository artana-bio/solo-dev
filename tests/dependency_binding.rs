//! Invariant 7.3.6: a relevant dependency SHA change invalidates dependent
//! evidence.
//!
//! Section 16's scenario list never enumerated this invariant, so the coverage
//! trace could not be checked for it. These are the scenarios it was missing.
//!
//! "Relevant" is the word the invariant turns on, and every test here exists to
//! draw one edge of it. A dependency change matters to a dependent when the
//! dependent's candidate carries a commit of the dependency that the
//! dependency's standing approval no longer contains. It does not matter when
//! the dependent carries nothing of it, when the dependency merely gained
//! commits on top, or when the dependency landed unchanged.

mod support;

use std::process::Output;

use serde_json::Value;
use support::Workspace;

fn error_code(output: &Output) -> String {
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["code"].as_str().unwrap().to_owned()
}

fn error_reason(output: &Output) -> String {
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["message"].as_str().unwrap().to_owned()
}

/// True when `ancestor` is in `descendant`'s history in the candidate repo.
fn contains(workspace: &Workspace, ancestor: &str, descendant: &str) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(workspace.worktrees.join("F-001"))
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .status()
        .unwrap()
        .success()
}

fn head_of(workspace: &Workspace, card_id: &str) -> String {
    support::capture(&workspace.worktrees.join(card_id), &["rev-parse", "HEAD"])
}

/// F-001 approved, and F-002 declaring it and branched from its candidate.
///
/// Returns the workspace and F-001's approved candidate.
fn dependent_built_on_its_dependency() -> (Workspace, String) {
    let workspace = Workspace::initialized();
    workspace.cycle(&["create", "--cycle-id", "C-001", "--objective", "slice"]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/F-001/**"]);
    workspace.approve_card("F-001", "src/F-001/a.rs");

    let first = head_of(&workspace, "F-001");
    workspace.activate_card_depending_on_at("F-002", &["src/F-002/**"], &["F-001"], &first);
    workspace.approve_card("F-002", "src/F-002/b.rs");

    // The fixture must actually have built one on the other, or every
    // assertion below passes for a reason that has nothing to do with
    // dependencies.
    assert!(
        contains(&workspace, &first, &head_of(&workspace, "F-002")),
        "fixture: F-002's candidate must contain F-001's approved commit"
    );
    (workspace, first)
}

#[test]
fn a_rewritten_dependency_invalidates_the_dependent_built_on_it() {
    let (workspace, first) = dependent_built_on_its_dependency();

    let second = workspace.rework_and_reapprove("F-001", "src/F-001/a.rs", true);
    assert_ne!(first, second);
    assert!(
        !contains(&workspace, &first, &second),
        "fixture: the rework must have rewritten history, not extended it"
    );

    // The enforcement assertion first: the approval must have stopped
    // standing. A test that led with the recorded field would report the
    // record being written as the defect being fixed.
    let inspect = workspace.review_json(&["inspect", "--card-id", "F-002"]);
    assert_eq!(
        inspect["data"]["has_current_approval"], false,
        "F-002's approval covers a version of F-001 that is no longer approved"
    );
    let reason = inspect["data"]["latest_stale_reason"]
        .as_str()
        .expect("a stale reason")
        .to_owned();
    assert!(
        reason.contains("F-001"),
        "must name the dependency: {reason}"
    );
    assert!(
        reason.contains(&first) && reason.contains(&second),
        "must name both commits: {reason}"
    );

    let refused = workspace.integration_raw(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
        "--card-id",
        "F-001",
        "--card-id",
        "F-002",
    ]);
    assert_eq!(refused.status.code(), Some(5), "integration must refuse");
    assert_eq!(error_code(&refused), "CH-POLICY-NOT-INTEGRABLE");
    let refusal = error_reason(&refused);
    assert!(
        refusal.contains("F-001") && refusal.contains(&first),
        "the refusal must name the dependency and the superseded commit: {refusal}"
    );

    // And the record carries the binding the check reads, so a holder of the
    // evidence can reach the same conclusion without the harness.
    let handoff = workspace.handoff_json(&["inspect", "--card-id", "F-002"]);
    let bindings = handoff["data"]["handoff"]["dependency_bindings"]
        .as_array()
        .expect("dependency bindings");
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0]["card_id"], "F-001");
    assert_eq!(bindings[0]["incorporated_sha"], first.as_str());
}

#[test]
fn a_dependency_that_only_gained_commits_does_not_invalidate_its_dependent() {
    // The overcorrection this check is one line away from. F-001 is
    // re-approved at a commit that still contains what F-002 built on, so
    // nothing F-002 carries is superseded and nothing lands twice. A fix
    // comparing the bound SHA to the approved SHA for equality refuses here,
    // and refusing here makes every review-and-fix round on a dependency void
    // every dependent's evidence.
    let (workspace, first) = dependent_built_on_its_dependency();

    let second = workspace.rework_and_reapprove("F-001", "src/F-001/a.rs", false);
    assert_ne!(first, second);
    assert!(
        contains(&workspace, &first, &second),
        "fixture: the rework must have extended history, not rewritten it"
    );

    let inspect = workspace.review_json(&["inspect", "--card-id", "F-002"]);
    assert_eq!(
        inspect["data"]["has_current_approval"], true,
        "F-002 still holds exactly the version of F-001 that is approved, as a prefix"
    );

    let prepared = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
        "--card-id",
        "F-001",
        "--card-id",
        "F-002",
    ]);
    assert_eq!(prepared["data"]["members"].as_array().unwrap().len(), 2);
}

#[test]
fn a_dependency_rewrite_does_not_invalidate_a_dependent_that_does_not_incorporate_it() {
    // The other overcorrection: binding a dependent to whatever commit its
    // dependency stands approved at, rather than to what the dependent
    // actually holds. F-002 declares F-001 and branched from the cycle
    // baseline, so F-001 can be rewritten as often as it likes without
    // changing a line of what F-002's reviewer read.
    let workspace = Workspace::initialized();
    workspace.cycle(&["create", "--cycle-id", "C-001", "--objective", "slice"]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/F-001/**"]);
    workspace.activate_card_depending_on("F-002", &["src/F-002/**"], &["F-001"]);
    workspace.approve_card("F-001", "src/F-001/a.rs");
    workspace.approve_card("F-002", "src/F-002/b.rs");

    let first = head_of(&workspace, "F-001");
    assert!(
        !contains(&workspace, &first, &head_of(&workspace, "F-002")),
        "fixture: F-002 must not contain F-001's candidate"
    );

    let second = workspace.rework_and_reapprove("F-001", "src/F-001/a.rs", true);
    assert!(!contains(&workspace, &first, &second));

    let inspect = workspace.review_json(&["inspect", "--card-id", "F-002"]);
    assert_eq!(
        inspect["data"]["has_current_approval"], true,
        "F-002 incorporates nothing of F-001; its evidence is untouched"
    );
    let binding = &inspect["data"]["reviews"][0]["dependency_bindings"][0];
    assert_eq!(binding["card_id"], "F-001");
    assert_eq!(
        binding["incorporated_sha"],
        Value::Null,
        "the binding must record that nothing was incorporated, not the dependency's own SHA"
    );

    let prepared = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
        "--card-id",
        "F-001",
        "--card-id",
        "F-002",
    ]);
    assert_eq!(prepared["data"]["members"].as_array().unwrap().len(), 2);
}

#[test]
fn landing_a_dependency_does_not_invalidate_its_dependent() {
    // The single most likely wrong fix, because "which commit is the
    // dependency at" reads as "what is on main". The landing commit is a merge
    // nobody reviewed as the dependency's candidate; binding to it would void
    // every dependent at the exact moment nothing about the dependency
    // changed, and no multi-card cycle could ever land in two batches.
    let (workspace, first) = dependent_built_on_its_dependency();

    let id = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
        "--card-id",
        "F-001",
    ])["data"]["integration_id"]
        .as_str()
        .unwrap()
        .to_owned();
    for step in ["merge", "land"] {
        workspace.integration(&[step, "--integration-id", &id, "--actor-id", "coordinator"]);
    }
    workspace.integration(&["verify", "--integration-id", &id, "--actor-id", "verifier"]);
    workspace.integration(&[
        "review",
        "--integration-id",
        &id,
        "--reviewer-actor-id",
        "reviewer",
    ]);
    workspace.acceptance(&[
        "record",
        "--integration-id",
        &id,
        "--acceptance-owner",
        "owner",
    ]);
    workspace.integration(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);

    // The fixture must actually have moved the authority, or "still current"
    // proves nothing.
    let authority = workspace.authority_head();
    assert_ne!(
        authority, first,
        "fixture: F-001 must have landed at a commit other than its candidate"
    );

    // Asserted on F-002, which still holds its lease: `review inspect` reports
    // no current approval for a card with no allocation, for reasons having
    // nothing to do with dependencies.
    let inspect = workspace.review_json(&["inspect", "--card-id", "F-002"]);
    assert_eq!(
        inspect["data"]["has_current_approval"], true,
        "landing F-001 changed nothing F-002's reviewer looked at"
    );

    let ready = workspace.integration_json(&["ready", "--cycle-id", "C-001"]);
    let ready_ids: Vec<&str> = ready["data"]["ready"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["card_id"].as_str().unwrap())
        .collect();
    assert_eq!(ready_ids, ["F-002"], "F-002 must still be integrable");
}

#[test]
fn a_dependent_may_be_handed_off_before_its_dependency_is_approved() {
    // Recording `None` is the point: it is a fact about what the candidate
    // holds, not an error. Refusing here would forbid a dependent from
    // reaching handoff until its dependency finished, which is the
    // serialization this harness exists to avoid.
    let workspace = Workspace::initialized();
    workspace.cycle(&["create", "--cycle-id", "C-001", "--objective", "slice"]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/F-001/**"]);
    workspace.activate_card_depending_on("F-002", &["src/F-002/**"], &["F-001"]);
    workspace.approve_card("F-002", "src/F-002/b.rs");

    let handoff = workspace.handoff_json(&["inspect", "--card-id", "F-002"]);
    assert_eq!(handoff["data"]["is_current"], true);
    let bindings = handoff["data"]["handoff"]["dependency_bindings"]
        .as_array()
        .expect("dependency bindings");
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0]["card_id"], "F-001");
    assert_eq!(bindings[0]["incorporated_sha"], Value::Null);

    let inspect = workspace.review_json(&["inspect", "--card-id", "F-002"]);
    assert_eq!(inspect["data"]["has_current_approval"], true);
}

#[test]
fn a_prepare_that_takes_everything_ready_names_what_it_left_out() {
    // `select` drops a blocked card when the coordinator names none, which is
    // right — a cycle almost always holds unfinished work. Doing it silently
    // is what was wrong: the batch shipped fewer cards than the coordinator
    // believed, with an empty `warnings` array.
    let (workspace, _) = dependent_built_on_its_dependency();
    workspace.rework_and_reapprove("F-001", "src/F-001/a.rs", true);

    let prepared = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
    ]);
    let members: Vec<&str> = prepared["data"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|member| member["card_id"].as_str().unwrap())
        .collect();
    assert_eq!(members, ["F-001"], "F-002 is not integrable");

    let warnings: Vec<&str> = prepared["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|warning| warning.as_str().unwrap())
        .collect();
    assert!(
        warnings.iter().any(|warning| warning.contains("F-002")),
        "the plan must say which cards it left out: {warnings:?}"
    );
}
