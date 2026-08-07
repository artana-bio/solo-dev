//! A verdict that found problems is recordable against the candidate it read.
//!
//! An approval must bind to the exact current candidate — that is the harness's
//! central claim and nothing here weakens it. A `changes_requested` or
//! `blocked` verdict is a different kind of statement: it is true about the
//! candidate the reviewer actually read, and it stays true when the branch
//! moves afterwards. Refusing to file it protects nothing and destroys the
//! reviewer's work.
//!
//! Found by making the mistake four times on one card. Each time a reviewer
//! returned findings, the findings were fixed, the branch moved, and the
//! verdict became unrecordable — three of that card's review rounds survive
//! only as prose inside a `handoff revoke --reason`, where nothing can query
//! them.

mod support;

use std::fs;

use support::Workspace;

/// A card handed off, plus the candidate that was handed off. No review has
/// begun yet — `under_review` below adds that.
fn handed_off() -> (Workspace, String) {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    let path = workspace.worktrees.join("F-001");
    fs::create_dir_all(path.join("src")).unwrap();
    fs::write(path.join("src/a.rs"), "fn main() {}\n").unwrap();
    support::git(&path, &["add", "-A"]);
    support::git(&path, &["commit", "-q", "-m", "feat: add a.rs"]);
    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);

    let reviewed = support::capture(&path, &["rev-parse", "HEAD"]);
    let declaration = workspace.root.join("declaration.yaml");
    fs::write(
        &declaration,
        format!(
            "delivered_sha: {reviewed}\nbehavior_delivered: adds a.rs\nimplementation_decisions: [minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
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
    (workspace, reviewed)
}

/// The same, with a review actually begun. What every test but the
/// review-begin gap test wants.
fn under_review() -> (Workspace, String) {
    let (workspace, reviewed) = handed_off();
    workspace.review(&["begin", "--card-id", "F-001"]);
    (workspace, reviewed)
}

/// Moves the candidate on, the way fixing a finding does.
fn move_the_branch(workspace: &Workspace) -> String {
    let path = workspace.worktrees.join("F-001");
    fs::write(path.join("src/a.rs"), "fn main() { /* fixed */ }\n").unwrap();
    support::git(&path, &["add", "-A"]);
    support::git(&path, &["commit", "-q", "-m", "fix: address the finding"]);
    support::capture(&path, &["rev-parse", "HEAD"])
}

fn verdict_file(workspace: &Workspace, decision: &str) -> String {
    let body = format!(
        "reviewer_actor_id: reviewer-session-a\ndecision: {decision}\nfindings:\n  - severity: critical\n    location: src/a.rs\n    detail: missing guard\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed the guard directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n"
    );
    let path = workspace.root.join("verdict.yaml");
    fs::write(&path, body).unwrap();
    path.display().to_string()
}

fn error_code(output: &std::process::Output) -> String {
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["code"].as_str().unwrap().to_owned()
}

fn error_message(output: &std::process::Output) -> String {
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["message"].as_str().unwrap().to_owned()
}

/// Withdraws the handoff, the way an implementer pulling a delivery back does.
fn revoke_the_handoff(workspace: &Workspace) {
    workspace.handoff(&[
        "revoke",
        "--card-id",
        "F-001",
        "--reason",
        "withdrawn before the verdict was filed",
    ]);
}

/// Revises the card underneath the handoff, changing what it was measured
/// against.
fn revise_the_card(workspace: &Workspace) {
    let body = format!(
        "card_id: F-001\ncycle_id: C-001\ntitle: Implement F-001\ngoal: Deliver F-001 against changed requirements\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [\"src/**\"]\n  exclude: []\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works differently now]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
        base = workspace.authority_head(),
    );
    let path = workspace.root.join("F-001-r2.yaml");
    fs::write(&path, body).unwrap();
    workspace.card(&[
        "revise",
        "--card-id",
        "F-001",
        "--draft",
        &path.display().to_string(),
        "--reason",
        "the requirements changed underneath the handoff",
    ]);
}

#[test]
fn changes_requested_is_recordable_after_the_branch_moves() {
    let (workspace, reviewed) = under_review();
    let moved_on = move_the_branch(&workspace);
    assert_ne!(reviewed, moved_on, "the fixture must move the candidate");

    let path = verdict_file(&workspace, "changes_requested");
    let envelope = workspace.review_json(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer-session-a",
    ]);

    assert_eq!(envelope["status"], "success");
    assert_eq!(
        envelope["data"]["review"]["candidate_sha"], reviewed,
        "the verdict must name the candidate it was reached against, not the current head"
    );
    assert_eq!(envelope["data"]["review"]["decision"], "changes_requested");
}

#[test]
fn blocked_is_recordable_after_the_branch_moves() {
    let (workspace, reviewed) = under_review();
    move_the_branch(&workspace);

    let path = verdict_file(&workspace, "blocked");
    let envelope = workspace.review_json(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer-session-a",
    ]);

    assert_eq!(envelope["status"], "success");
    assert_eq!(envelope["data"]["review"]["candidate_sha"], reviewed);
}

#[test]
fn an_approval_after_the_branch_moves_is_still_refused() {
    // The invariant this whole harness exists for. An approval binds to an
    // exact commit; if the branch moved, the approval would attest to code
    // nobody read.
    let (workspace, _) = under_review();
    move_the_branch(&workspace);

    let body = "reviewer_actor_id: reviewer-session-a\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed each acceptance behavior directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
    let path = workspace.root.join("verdict.yaml");
    fs::write(&path, body).unwrap();

    let output = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path.display().to_string(),
        "--actor",
        "reviewer-session-a",
    ]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-STALE-HANDOFF");
}

#[test]
fn an_approval_against_the_current_candidate_still_succeeds() {
    // The other half: nothing about the ordinary path changed.
    let (workspace, reviewed) = under_review();

    let body = "reviewer_actor_id: reviewer-session-a\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed each acceptance behavior directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
    let path = workspace.root.join("verdict.yaml");
    fs::write(&path, body).unwrap();

    let envelope = workspace.review_json(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path.display().to_string(),
        "--actor",
        "reviewer-session-a",
    ]);
    assert_eq!(envelope["data"]["review"]["decision"], "approved");
    assert_eq!(envelope["data"]["review"]["candidate_sha"], reviewed);
}

#[test]
fn the_recorded_verdict_survives_the_sequence_that_used_to_lose_it() {
    // The exact sequence, end to end: review opens, the reviewer finds
    // problems, the implementer fixes and commits before filing, and the
    // verdict is filed afterwards. Previously the last step was impossible and
    // the only way forward was revoking the handoff, which left the findings
    // in a free-text reason.
    let (workspace, reviewed) = under_review();
    move_the_branch(&workspace);

    let path = verdict_file(&workspace, "changes_requested");
    workspace.review(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer-session-a",
    ]);

    let inspected = workspace.review_json(&["inspect", "--card-id", "F-001"]);
    let recorded = inspected["data"]["reviews"]
        .as_array()
        .expect("the review history");
    assert_eq!(recorded.len(), 1, "the verdict is in the record");
    assert_eq!(
        recorded[0]["candidate_sha"], reviewed,
        "and it still names what was actually reviewed"
    );
    assert_eq!(recorded[0]["decision"], "changes_requested");
}

// The relaxation above drops what a non-approval outlives and keeps what it
// does not. Written because the first version of it wrapped the whole check in
// `if decision == approved`, which stopped asking every question — including
// the one about the card the verdict was measured against.

#[test]
fn blocked_against_a_revoked_handoff_is_still_recordable() {
    // Deliberate, and the argument is at `binding_staleness`. `handoff revoke`
    // belongs to the delivery side and `review_pending -> active` is a
    // permitted transition, so an implementer who could make revocation fatal
    // to a verdict could suppress an adverse review by revoking before the
    // reviewer files — the exact failure this file exists to document.
    //
    // Revision 1 of F-030 required the opposite and its own review round
    // rejected it. This test is here so that reinstating the refusal has to
    // argue with a test rather than pass quietly.
    let (workspace, reviewed) = under_review();
    revoke_the_handoff(&workspace);

    let path = verdict_file(&workspace, "blocked");
    let envelope = workspace.review_json(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer-session-a",
    ]);

    assert_eq!(envelope["status"], "success");
    assert_eq!(
        envelope["data"]["review"]["candidate_sha"], reviewed,
        "the verdict names the candidate the reviewer actually read"
    );
}

#[test]
fn changes_requested_against_a_revoked_handoff_is_still_recordable() {
    // Review round 2's finding (RV-000055). Revision 2 of this card closed the
    // suppression route for `blocked` and left it open for this verdict: the
    // staleness check passed and `active -> changes_requested` was then refused
    // by the transition table, so the review was never written. Whichever
    // verdict the reviewer reached, revoking must not decide it for them.
    let (workspace, reviewed) = under_review();
    revoke_the_handoff(&workspace);

    let path = verdict_file(&workspace, "changes_requested");
    let envelope = workspace.review_json(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer-session-a",
    ]);

    assert_eq!(envelope["status"], "success");
    assert_eq!(envelope["data"]["review"]["decision"], "changes_requested");
    assert_eq!(
        envelope["data"]["review"]["candidate_sha"], reviewed,
        "the verdict names the candidate the reviewer actually read"
    );

    let inspected = workspace.review_json(&["inspect", "--card-id", "F-001"]);
    let recorded = inspected["data"]["reviews"]
        .as_array()
        .expect("the review history");
    assert_eq!(recorded.len(), 1, "the verdict is in the record");
    assert_eq!(
        recorded[0]["findings"][0]["location"], "src/a.rs",
        "and the findings came with it"
    );
}

#[test]
fn an_active_card_with_no_handoff_at_all_cannot_be_reviewed() {
    // The only guard the new transition leans on, and it is narrower than it
    // sounds: it establishes that a card nobody handed off cannot be reviewed,
    // and nothing more. A *revoked* handoff satisfies it — see the test below,
    // which says so plainly rather than letting this one's name imply
    // otherwise. Review round 3 (RV-000056) caught exactly that overclaim.
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    let path = verdict_file(&workspace, "changes_requested");
    let output = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer-session-a",
    ]);

    assert_ne!(
        output.status.code(),
        Some(0),
        "a card nobody handed off has nothing to review"
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("an error envelope");
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no handoff"),
        "refused for the absent handoff: {}",
        envelope["error"]["message"]
    );
}

#[test]
fn recording_a_verdict_now_requires_that_a_review_began() {
    // Closes the gap `recording_a_verdict_does_not_yet_require...` used to
    // pin deliberately: `review record` previously never checked that
    // `review begin` happened, so a handoff created and then revoked with no
    // review round at any point was enough to file a verdict. Reachable
    // through `blocked` and `changes_requested`, both of which `active`
    // admits directly.
    //
    // The signal is a `review.begun` event tagged with the exact handoff id,
    // which survives the state overwrite `handoff revoke` performs — the
    // card's current state alone cannot answer whether a review ever began.
    // Not an authority bypass: `check_independence` already refused a
    // reviewer who is the handoff actor, unrelated to this.
    for decision in ["blocked", "changes_requested"] {
        let (workspace, _) = handed_off();
        workspace.handoff(&[
            "revoke",
            "--card-id",
            "F-001",
            "--reason",
            "withdrawn before any review began",
        ]);

        let path = verdict_file(&workspace, decision);
        let output = workspace.review_raw(&[
            "record",
            "--card-id",
            "F-001",
            "--verdict",
            &path,
            "--actor",
            "reviewer-session-a",
        ]);
        assert_eq!(
            output.status.code(),
            Some(5),
            "{decision} must now be refused with no review round open"
        );
        let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(envelope["error"]["code"], "CH-POLICY-REVIEW-NOT-BEGUN");

        let inspected = workspace.review_json(&["inspect", "--card-id", "F-001"]);
        assert!(
            inspected["data"]["reviews"].as_array().unwrap().is_empty(),
            "{decision}: a refused verdict must not reach the record"
        );
    }
}

#[test]
fn an_approval_against_a_revoked_never_reviewed_handoff_still_reports_stale() {
    // Review round 1 of this exact card: the new review-not-begun check
    // originally ran unconditionally, ahead of staleness, for every
    // decision. An approval can never legitimately reach this gap — Approved
    // is only admitted from review_pending, which review begin is what
    // reaches — but running the check anyway silently swapped the reported
    // reason for the one case where both apply: revoked AND never reviewed.
    // F-030 already established that an approval against a revoked handoff
    // must report CH-POLICY-STALE-HANDOFF; this asserts that guarantee holds
    // even when review never began at all, in both the real command and its
    // dry-run preview.
    let (workspace, _) = handed_off();
    workspace.handoff(&[
        "revoke",
        "--card-id",
        "F-001",
        "--reason",
        "withdrawn before any review began",
    ]);

    let body = "reviewer_actor_id: reviewer-session-a\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
    let path = workspace.root.join("verdict.yaml");
    fs::write(&path, body).unwrap();
    let path = path.display().to_string();

    let real = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer-session-a",
    ]);
    let preview = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer-session-a",
        "--dry-run",
    ]);
    assert_eq!(error_code(&real), "CH-POLICY-STALE-HANDOFF");
    assert_eq!(
        error_code(&preview),
        error_code(&real),
        "the preview must refuse for the same reason as the real command"
    );
}

#[test]
fn a_dry_run_previews_the_review_not_begun_refusal() {
    // Dry-run parity for the new check, in the same fixture as above.
    let (workspace, _) = handed_off();
    workspace.handoff(&[
        "revoke",
        "--card-id",
        "F-001",
        "--reason",
        "withdrawn before any review began",
    ]);
    let path = verdict_file(&workspace, "blocked");
    let real = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer-session-a",
    ]);
    let preview = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer-session-a",
        "--dry-run",
    ]);
    let real_env: serde_json::Value = serde_json::from_slice(&real.stdout).unwrap();
    let preview_env: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(
        preview_env["error"]["code"], real_env["error"]["code"],
        "the preview must refuse for the same reason as the real command"
    );
    assert_eq!(real_env["error"]["code"], "CH-POLICY-REVIEW-NOT-BEGUN");
}

#[test]
fn the_verdict_against_a_revoked_handoff_reaches_the_record() {
    // The exit code alone would pass even if the verdict were accepted and
    // then dropped. What matters is that the findings are queryable afterwards
    // — that is the whole reason the refusal was rejected.
    let (workspace, _) = under_review();
    revoke_the_handoff(&workspace);

    let path = verdict_file(&workspace, "blocked");
    workspace.review(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer-session-a",
    ]);

    let inspected = workspace.review_json(&["inspect", "--card-id", "F-001"]);
    let recorded = inspected["data"]["reviews"]
        .as_array()
        .expect("the review history");
    assert_eq!(recorded.len(), 1, "the verdict is in the record");
    assert_eq!(recorded[0]["decision"], "blocked");
    assert_eq!(
        recorded[0]["findings"][0]["location"], "src/a.rs",
        "and the findings survived with it, rather than as prose in a reason"
    );
}

#[test]
fn a_non_approval_against_a_revised_card_is_refused_for_the_true_reason() {
    // `card revise` returns the card to `ready`, from which the transition
    // table refuses `blocked` anyway — so this case was never a hole in the
    // record. What it was is a refusal that named the wrong thing: the
    // operator was told a card cannot move from `ready` to `blocked`, when
    // what actually happened is that the card was revised out from under the
    // handoff the reviewer was holding. This asserts the accurate one.
    let (workspace, _) = under_review();
    revise_the_card(&workspace);

    let path = verdict_file(&workspace, "blocked");
    let output = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer-session-a",
    ]);

    assert_eq!(output.status.code(), Some(5));
    assert_eq!(
        error_code(&output),
        "CH-POLICY-STALE-HANDOFF",
        "not CH-POLICY-INVALID-TRANSITION, which is the state machine \
         answering a question nobody asked"
    );
    let message = error_message(&output);
    assert!(
        message.contains("card digest"),
        "the refusal must name the binding that broke: {message}"
    );
}

#[test]
fn an_approval_against_a_revoked_handoff_is_still_refused() {
    // Unchanged by this card, and asserted so that narrowing the scope for
    // non-approvals cannot quietly narrow it for approvals too.
    let (workspace, _) = under_review();
    revoke_the_handoff(&workspace);

    let body = "reviewer_actor_id: reviewer-session-a\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed each acceptance behavior directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
    let path = workspace.root.join("verdict.yaml");
    fs::write(&path, body).unwrap();

    let output = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path.display().to_string(),
        "--actor",
        "reviewer-session-a",
    ]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-STALE-HANDOFF");
}

#[test]
fn a_revised_card_refuses_a_verdict_even_when_the_handoff_was_revoked() {
    // The questions that are dropped must not drag down the one that is kept.
    // Revoking and revising together still refuses, and for the revision.
    let (workspace, _) = under_review();
    revoke_the_handoff(&workspace);
    revise_the_card(&workspace);

    let path = verdict_file(&workspace, "blocked");
    let output = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer-session-a",
    ]);

    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-STALE-HANDOFF");
    let message = error_message(&output);
    assert!(
        message.contains("card digest"),
        "the surviving question is the one about the card: {message}"
    );
}

// #95 gap 1, §10 test 4: "write, read back, compare", taken literally rather
// than as an in-memory `serde` round trip. `review record` writes
// `reviews/RV-000001.json` into the control repository; this reads that exact
// file back off disk — not through `review inspect`, a second code path this
// test does not want to depend on for its own soundness — and compares its
// `mutation_evidence` against what the verdict actually declared.
#[test]
fn a_recorded_review_s_mutation_evidence_writes_and_reads_back_off_disk() {
    let (workspace, _) = under_review();

    let body = "reviewer_actor_id: reviewer-session-a\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed each acceptance behavior directly\n  mutation_evidence:\n    status: demonstrated\n    mutation: removed the guard at src/a.rs:1\n    failing_test: rejects_the_missing_guard\n    oracle: gate.unit\n    authorship: reviewer_devised\nresidual_risks: []\nreview_conduct: separate_process\n";
    let path = workspace.root.join("verdict.yaml");
    fs::write(&path, body).unwrap();

    let envelope = workspace.review_json(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path.display().to_string(),
        "--actor",
        "reviewer-session-a",
    ]);
    assert_eq!(envelope["status"], "success");
    let review_id = envelope["data"]["review"]["review_id"]
        .as_str()
        .expect("the recorded review names its own id")
        .to_owned();

    // Read back: not the JSON envelope the write returned, and not `review
    // inspect` — the stored file itself, straight off disk.
    let stored_path = workspace
        .control
        .join("reviews")
        .join(format!("{review_id}.json"));
    let stored_raw = fs::read_to_string(&stored_path).unwrap_or_else(|error| {
        panic!("stored review file must exist at {stored_path:?}: {error}")
    });
    let stored: serde_json::Value =
        serde_json::from_str(&stored_raw).expect("the stored file is well-formed JSON");

    // Compare against what the verdict actually declared, not against the
    // envelope `review record` already echoed back — a defect that recorded
    // one thing and reported another would pass a comparison against the
    // envelope and still be a real defect in what landed on disk.
    let evidence = &stored["gate_adequacy"]["mutation_evidence"];
    assert_eq!(evidence["status"], "demonstrated");
    assert_eq!(evidence["mutation"], "removed the guard at src/a.rs:1");
    assert_eq!(evidence["failing_test"], "rejects_the_missing_guard");
    assert_eq!(evidence["oracle"], "gate.unit");
    // #28: the same fixture also declares the two fields that card adds, so
    // this proves they write and read back off disk exactly as the three
    // original keys above do.
    assert_eq!(evidence["authorship"], "reviewer_devised");
    assert_eq!(stored["review_conduct"], "separate_process");
}

// #120: `--actor` was accepted on `review record` and never read — attribution
// came entirely from the verdict's `reviewer_actor_id`, so `record --actor
// reviewer-b` against a verdict declaring `reviewer_actor_id: implementer-a`
// silently attributed the review to `implementer-a`. The three tests below
// pin the fix: `--actor` and the verdict's declared reviewer must agree, in
// both directions, normalized the same way `check_independence` already
// normalizes a self-review comparison.

/// A clean approval naming `reviewer`, for the three tests below — distinct
/// from `verdict_file`, which hardcodes `reviewer-session-a`: these tests
/// need to control the declared reviewer themselves.
fn approval_by(reviewer: &str) -> String {
    format!(
        "reviewer_actor_id: {reviewer}\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed each acceptance behavior directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n"
    )
}

#[test]
fn record_refuses_when_an_explicit_actor_disagrees_with_the_verdict() {
    // Neither name is `operator` (the fixture's feature actor), so
    // `check_independence` has nothing to say here — only the new agreement
    // check can refuse this. Reproduces #120's own motivating example
    // verbatim: `--actor reviewer-b` against a verdict naming someone else.
    let (workspace, _) = under_review();
    let path = workspace.root.join("verdict.yaml");
    fs::write(&path, approval_by("implementer-a")).unwrap();
    let path = path.display().to_string();

    let output = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer-b",
    ]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-INCOMPLETE-REVIEW");
    let message = error_message(&output);
    assert!(
        message.contains("reviewer-b") && message.contains("implementer-a"),
        "the refusal must name both disagreeing declarations: {message}"
    );

    // No review may have been recorded: `implementer-a` must not be attributed
    // the review `--actor reviewer-b` was asked for, and neither declaration
    // gets to win silently.
    let inspected = workspace.review_json(&["inspect", "--card-id", "F-001"]);
    assert_eq!(
        inspected["data"]["reviews"]
            .as_array()
            .expect("the review history")
            .len(),
        0,
        "a disagreement must not be resolved by recording either declaration"
    );
}

#[test]
fn record_refuses_when_the_default_actor_disagrees_with_the_verdict() {
    // The same disagreement, reached by omitting `--actor` rather than typing
    // a wrong one. `--actor` defaults to `operator`, and this fixture's
    // verdict deliberately does not name `operator`, so this exercises the
    // agreement check on the flag's default value, not only its explicit
    // one — an implementer who checks agreement only when the flag is
    // written out would pass every test above and still leave the common
    // case (an operator who never passes `--actor` at all) exactly as silent
    // as #120 found it.
    let (workspace, _) = under_review();
    let path = workspace.root.join("verdict.yaml");
    fs::write(&path, approval_by("reviewer-b")).unwrap();
    let path = path.display().to_string();

    let output = workspace.review_raw(&["record", "--card-id", "F-001", "--verdict", &path]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-INCOMPLETE-REVIEW");
    let message = error_message(&output);
    assert!(
        message.contains("operator") && message.contains("reviewer-b"),
        "the refusal must name both the default actor and the verdict's declared reviewer: {message}"
    );
}

#[test]
fn record_accepts_an_actor_and_verdict_that_agree_only_after_normalization() {
    // The positive control for the two refusals above, and the pin for #120
    // §9 mutation 4: whitespace and case must not defeat the comparison, the
    // same normalization `check_independence` already applies via
    // `policy::actors::same`. An operator who types `Reviewer-B` for a
    // verdict declaring `reviewer-b` must get an accepted review, not an
    // accidental refusal.
    let (workspace, reviewed) = under_review();
    let path = workspace.root.join("verdict.yaml");
    fs::write(&path, approval_by("reviewer-b")).unwrap();
    let path = path.display().to_string();

    let envelope = workspace.review_json(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        " Reviewer-B\t",
    ]);
    assert_eq!(envelope["status"], "success");
    assert_eq!(envelope["data"]["review"]["decision"], "approved");
    assert_eq!(envelope["data"]["review"]["candidate_sha"], reviewed);
    assert_eq!(
        envelope["data"]["review"]["reviewer_actor_id"],
        "reviewer-b"
    );
}
