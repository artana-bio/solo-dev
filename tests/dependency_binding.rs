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

use std::{
    fs,
    process::{Command, Output},
};

use serde_json::Value;
use support::Workspace;

fn error_code(output: &Output) -> String {
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["code"].as_str().unwrap().to_owned()
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

/// Whether Git can resolve the object, rather than merely parse its SHA.
fn object_exists(workspace: &Workspace, sha: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(&workspace.repository)
        .args(["cat-file", "-e", sha])
        .status()
        .expect("Git should run")
        .success()
}

/// Removes one loose object from the temporary candidate repository.
///
/// The test needs Git's real missing-object path. A made-up 40-character SHA
/// would only prove a malformed record reaches the check; this starts with an
/// object Git created and makes that exact object unavailable.
fn remove_loose_object(workspace: &Workspace, sha: &str) {
    let object_path = workspace
        .repository
        .join(".git/objects")
        .join(&sha[..2])
        .join(&sha[2..]);
    assert!(
        object_path.is_file(),
        "fixture: {sha} must be a loose object before it is made unavailable"
    );
    fs::remove_file(&object_path).expect("remove the fixture's unreachable object");
    assert!(
        !object_exists(workspace, sha),
        "fixture: Git must be unable to resolve {sha}"
    );
}

/// F-001 approved, and F-002 declares it while retaining the cycle baseline.
/// A dependent may declare a prerequisite without incorporating its unlanded
/// candidate; #25 owns any future explicit rebase/incorporation model.
///
/// Returns the workspace and F-001's approved candidate.
fn dependent_built_on_its_dependency() -> (Workspace, String) {
    let workspace = Workspace::initialized();
    workspace.cycle(&["create", "--cycle-id", "C-001", "--objective", "slice"]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/F-001/**"]);
    workspace.approve_card("F-001", "src/F-001/a.rs");

    let first = head_of(&workspace, "F-001");
    workspace.activate_card_depending_on("F-002", &["src/F-002/**"], &["F-001"]);
    workspace.approve_card("F-002", "src/F-002/b.rs");

    // The fixture proves the valid fixed-baseline path. A declaration records
    // scheduling/integration dependency, not an implicit candidate rebase.
    assert!(
        !contains(&workspace, &first, &head_of(&workspace, "F-002")),
        "fixture: F-002 must retain the frozen cycle baseline"
    );
    (workspace, first)
}

#[test]
fn a_rewritten_dependency_does_not_invalidate_a_fixed_baseline_dependent() {
    let (workspace, first) = dependent_built_on_its_dependency();

    let second = workspace.rework_and_reapprove("F-001", "src/F-001/a.rs", true);
    assert_ne!(first, second);
    assert!(
        !contains(&workspace, &first, &second),
        "fixture: the rework must have rewritten history, not extended it"
    );

    // F-002 records a declared dependency but does not carry F-001's
    // unlanded candidate. Rewriting that candidate cannot stale F-002.
    let inspect = workspace.review_json(&["inspect", "--card-id", "F-002"]);
    assert_eq!(
        inspect["data"]["has_current_approval"], true,
        "a fixed-baseline dependent must not inherit an unincorporated rewrite"
    );
    assert_eq!(inspect["data"]["latest_stale_reason"], Value::Null);
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
fn a_missing_handed_off_dependency_commit_is_not_bound() {
    // `resolve_dependency_bindings` treats an ancestry question Git cannot
    // answer as "not incorporated". The earlier handoff is deliberately
    // revoked and made unreachable, so it is still in control history but no
    // longer available to Git when F-002's handoff walks it.
    let workspace = Workspace::initialized();
    workspace.cycle(&["create", "--cycle-id", "C-001", "--objective", "slice"]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/F-001/**"]);
    workspace.approve_card("F-001", "src/F-001/a.rs");
    let unavailable = head_of(&workspace, "F-001");

    let replacement = workspace.rework_and_reapprove("F-001", "src/F-001/a.rs", true);
    assert!(
        !contains(&workspace, &unavailable, &replacement),
        "fixture: the replacement must leave the first handoff unreachable"
    );
    remove_loose_object(&workspace, &unavailable);

    workspace.activate_card_depending_on("F-002", &["src/F-002/**"], &["F-001"]);
    workspace.approve_card("F-002", "src/F-002/b.rs");

    let handoff = workspace.handoff_json(&["inspect", "--card-id", "F-002"]);
    let binding = &handoff["data"]["handoff"]["dependency_bindings"][0];
    assert_eq!(binding["card_id"], "F-001");
    assert_eq!(
        binding["incorporated_sha"],
        Value::Null,
        "a missing object is not evidence that F-002 incorporated it"
    );
}

#[test]
fn a_dependent_cannot_declare_an_unlanded_dependency_candidate_as_its_base() {
    let workspace = Workspace::initialized();
    workspace.cycle(&["create", "--cycle-id", "C-001", "--objective", "slice"]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/F-001/**"]);
    workspace.approve_card("F-001", "src/F-001/a.rs");
    let dependency_candidate = head_of(&workspace, "F-001");
    let draft = workspace.root.join("F-002-wrong-base.yaml");
    fs::write(&draft, format!(
        "card_id: F-002\ncycle_id: C-001\ntitle: dependent\ngoal: dependent\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {dependency_candidate}\nwrite_scope:\n  include: [\"src/F-002/**\"]\n  exclude: []\ndepends_on: [\"F-001\"]\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n"
    )).unwrap();
    workspace.card(&["create", "--draft", draft.to_str().unwrap()]);
    let refused = workspace.card_raw(&["activate", "--card-id", "F-002"]);
    assert_eq!(refused.status.code(), Some(5));
    assert_eq!(error_code(&refused), "CH-POLICY-CYCLE-BASELINE-MISMATCH");
    assert!(!workspace.worktrees.join("F-002").exists());
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
fn a_prepare_keeps_a_valid_fixed_baseline_dependent_after_dependency_rework() {
    // A declaration without candidate incorporation is a scheduling edge, not
    // evidence that F-002 reviewed F-001's old candidate. Both remain ready.
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
    assert_eq!(members, ["F-001", "F-002"]);

    let warnings: Vec<&str> = prepared["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|warning| warning.as_str().unwrap())
        .collect();
    assert!(
        !warnings.iter().any(|warning| warning.contains("F-002")),
        "a valid baseline dependent must not be silently dropped: {warnings:?}"
    );
}
