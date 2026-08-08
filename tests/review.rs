//! `WP-320` acceptance: independent review.

mod support;

use std::fs;

use serde_json::Value;
use support::Workspace;

/// A card handed off and ready for review.
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
    workspace.work(&[
        "start",
        "--card-id",
        "F-001",
        "--actor-principal-id",
        "implementer-principal",
        "--actor-session-id",
        "implementer-session",
    ]);

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
    (workspace, head)
}

/// Writes a verdict file and returns its path.
fn verdict(workspace: &Workspace, body: &str) -> String {
    let path = workspace.root.join("verdict.yaml");
    let body = if body.contains("decision: approved") && !body.contains("mutation_exemption:") {
        format!(
            "{body}mutation_exemption:\n  code: fixture-no-mutation\n  reason: fixture has no executable mutation\n  approved_by: independent-attestor\n"
        )
    } else {
        body.to_owned()
    };
    fs::write(&path, body).unwrap();
    path.display().to_string()
}

/// A clean approval by a distinct reviewer.
fn approval(reviewer: &str) -> String {
    format!(
        "reviewer_actor_id: {reviewer}\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed each acceptance behavior directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\nmutation_exemption:\n  code: fixture-no-mutation\n  reason: fixture has no executable mutation\n  approved_by: independent-attestor\n"
    )
}

fn error_code(output: &std::process::Output) -> String {
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["code"].as_str().unwrap().to_owned()
}

#[test]
fn begin_emits_the_reviewer_packet() {
    let (workspace, head) = handed_off();
    let envelope = workspace.review_json(&["inspect", "--card-id", "F-001"]);
    assert_eq!(envelope["data"]["candidate_sha"], head);

    // The packet came from `review begin`, which ran in the fixture.
    let begun = workspace
        .events()
        .into_iter()
        .find(|event| event["event_type"] == "review.begun")
        .expect("the review must be recorded as open");
    assert_eq!(begun["next_state"], "review_pending");
    assert_eq!(begun["head_sha"], head);
}

#[test]
fn an_approval_by_a_distinct_actor_is_recorded() {
    let (workspace, head) = handed_off();
    let path = verdict(&workspace, &approval("reviewer-session-a"));

    let envelope = workspace.review_json(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer-session-a",
    ]);
    assert_eq!(envelope["data"]["state"], "approved");
    assert_eq!(envelope["data"]["review"]["decision"], "approved");
    assert_eq!(envelope["data"]["review"]["candidate_sha"], head);
    assert_eq!(
        envelope["data"]["review"]["canonical_algorithm"],
        "harness.canonical-json/v1"
    );
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "approved"
    );
}

#[test]
fn self_review_is_refused() {
    // Invariant 7.3.7. The fixture's handoff actor is `operator`.
    let (workspace, _) = handed_off();
    let path = verdict(&workspace, &approval("operator"));

    let output = workspace.review_raw(&["record", "--card-id", "F-001", "--verdict", &path]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-SELF-REVIEW");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("procedural"),
        "the message must not overclaim what this check proves"
    );
}

#[test]
fn different_actor_in_the_implementer_session_is_refused() {
    let (workspace, _) = handed_off();
    let body = "reviewer_actor_id: reviewer-session-a\nreviewer_kind: agent\nreviewer_provenance:\n  provider: fixture\n  model: agent\n  session_id: implementer-session\n  principal_id: other-principal\ndecision: changes_requested\nfindings:\n  - severity: high\n    location: src/a.rs\n    detail: same session must be refused\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: direct check\n  mutation_evidence:\n    status: exempt\n    reason: non-approval fixture\nresidual_risks: []\nreview_conduct: separate_process\n";
    let output = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict(&workspace, body),
        "--actor",
        "reviewer-session-a",
    ]);
    assert_eq!(error_code(&output), "CH-POLICY-SAME-ACTOR");
}

#[test]
fn self_review_is_refused_through_a_spelling_variant() {
    // The comparison was exact equality, so `Operator` and `operator ` were
    // different people and the check was one keystroke from being bypassed by
    // accident. It still proves nothing about who ran the command — but it
    // should not be defeated by a capital letter.
    for variant in ["Operator", "operator ", " OPERATOR"] {
        let (workspace, _) = handed_off();
        let path = verdict(&workspace, &approval(variant));

        let output = workspace.review_raw(&["record", "--card-id", "F-001", "--verdict", &path]);
        assert_eq!(
            error_code(&output),
            "CH-POLICY-SELF-REVIEW",
            "`{variant}` is the handoff actor"
        );
    }
}

#[test]
fn approving_over_an_open_finding_is_refused() {
    let (workspace, _) = handed_off();
    let body = "reviewer_actor_id: reviewer-session-a\ndecision: approved\nfindings:\n  - severity: critical\n    location: src/a.rs\n    detail: missing guard\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
    let path = verdict(&workspace, body);

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
    assert_eq!(error_code(&output), "CH-POLICY-OPEN-FINDINGS");
}

#[test]
fn approving_over_a_dispositioned_finding_is_permitted() {
    // SPIKE-001 F-4: the reviewer must be able to approve while recording that
    // a real problem cannot be fixed within this card's write scope.
    let (workspace, _) = handed_off();
    let body = "reviewer_actor_id: reviewer-session-a\ndecision: approved\nfindings:\n  - severity: high\n    location: tests/\n    detail: no coverage of behavior 4\n    disposition: out_of_scope\ngate_adequacy:\n  gates_observe_acceptance: false\n  unobserved_behaviors: [raises on invalid input]\n  basis: mutation-tested the suite; it passes with the guard removed\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: [behavior 4 stays ungated]\nreview_conduct: separate_process\n";
    let path = verdict(&workspace, body);

    let output = workspace.review(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer-session-a",
    ]);
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["data"]["state"], "approved");
    assert_eq!(
        envelope["data"]["review"]["gate_adequacy"]["gates_observe_acceptance"],
        false
    );
    assert!(
        envelope["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("cannot observe")),
        "an inadequate gate must be surfaced, not buried"
    );
}

#[test]
fn changes_requested_returns_the_card_to_work() {
    let (workspace, _) = handed_off();
    let body = "reviewer_actor_id: reviewer-session-a\ndecision: changes_requested\nfindings:\n  - severity: critical\n    location: src/a.rs\n    detail: missing guard\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
    let path = verdict(&workspace, body);

    let envelope = workspace.review_json(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer-session-a",
    ]);
    assert_eq!(envelope["data"]["state"], "changes_requested");
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "changes_requested"
    );
}

#[test]
fn requesting_changes_without_a_finding_is_refused() {
    let (workspace, _) = handed_off();
    let body = "reviewer_actor_id: reviewer-session-a\ndecision: changes_requested\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
    let path = verdict(&workspace, body);

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
    assert_eq!(error_code(&output), "CH-POLICY-INCOMPLETE-REVIEW");
}

#[test]
fn a_review_must_state_how_gate_adequacy_was_established() {
    let (workspace, _) = handed_off();
    let path = verdict(
        &workspace,
        &approval("reviewer-session-a").replace(
            "basis: probed each acceptance behavior directly",
            "basis: ''",
        ),
    );

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
    assert_eq!(error_code(&output), "CH-POLICY-INCOMPLETE-REVIEW");
}

// #95 gap 1, §10 test 3, exercised through the real CLI rather than only
// `ReviewRecord::validate` directly. `approval()` above already carries a
// `mutation_evidence` block; this strips exactly that block back out, so the
// verdict is otherwise identical to one the CLI already accepts.
//
// Mutation (§11.2): remove the §8.3 enforcement from `ReviewRecord::validate`.
// This test must fail — the stripped verdict would then be accepted.
#[test]
fn a_verdict_with_no_mutation_evidence_is_refused_by_the_cli() {
    let (workspace, _) = handed_off();
    let stripped = approval("reviewer-session-a").replace(
        "  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\n",
        "",
    );
    assert!(
        !stripped.contains("mutation_evidence"),
        "the fixture must actually omit the key, not merely blank a sub-field: {stripped}"
    );
    let path = verdict(&workspace, &stripped);

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
    assert_eq!(error_code(&output), "CH-POLICY-INCOMPLETE-REVIEW");
}

// §10 test 3's other half: a review claiming an exemption is not silently
// dropped or reduced to the bare fact that gates were adequate — the
// exemption itself, and the reviewer's stated reason, are part of the
// recorded, retrievable review.
#[test]
fn a_declared_exemption_is_recorded_and_visible() {
    let (workspace, _) = handed_off();
    let path = verdict(&workspace, &approval("reviewer-session-a"));

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
    let evidence = &envelope["data"]["review"]["gate_adequacy"]["mutation_evidence"];
    assert_eq!(evidence["status"], "exempt");
    assert_eq!(
        evidence["reason"], "fixture verdict for unrelated review behavior; no mutation performed",
        "the exemption's own reason must be visible in the recorded review, not merely a boolean"
    );

    // And still visible on a later read, not only in the write's own reply.
    let inspected = workspace.review_json(&["inspect", "--card-id", "F-001"]);
    let recorded = &inspected["data"]["reviews"][0]["gate_adequacy"]["mutation_evidence"];
    assert_eq!(recorded["status"], "exempt");
    assert_eq!(
        recorded["reason"],
        "fixture verdict for unrelated review behavior; no mutation performed"
    );
}

#[test]
fn a_candidate_change_invalidates_an_approval() {
    let (workspace, _) = handed_off();
    let path = verdict(&workspace, &approval("reviewer-session-a"));
    workspace.review(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer-session-a",
    ]);

    let before = workspace.review_json(&["inspect", "--card-id", "F-001"]);
    assert_eq!(before["data"]["has_current_approval"], true);

    // Section 15.2: a candidate change invalidates approval.
    let worktree = workspace.worktrees.join("F-001");
    fs::write(worktree.join("src/b.rs"), "fn b() {}\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: add b.rs"]);

    let after = workspace.review_json(&["inspect", "--card-id", "F-001"]);
    assert_eq!(after["data"]["has_current_approval"], false);
    assert!(
        after["data"]["reviews"][0]["candidate_sha"] != after["data"]["candidate_sha"],
        "the approval names the old candidate"
    );
}

#[test]
fn a_card_revision_invalidates_an_approval() {
    let (workspace, _) = handed_off();
    let path = verdict(&workspace, &approval("reviewer-session-a"));
    workspace.review(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer-session-a",
    ]);
    assert_eq!(
        workspace.review_json(&["inspect", "--card-id", "F-001"])["data"]["has_current_approval"],
        true
    );

    workspace.revise_card("F-001", &["src/**"], "requirement changed");

    assert_eq!(
        workspace.review_json(&["inspect", "--card-id", "F-001"])["data"]["has_current_approval"],
        false,
        "a card revision invalidates every review bound to the old digest"
    );
}

#[test]
fn reviewing_a_superseded_handoff_is_refused() {
    let (workspace, _) = handed_off();

    // The branch moves after the handoff was created but before review.
    let worktree = workspace.worktrees.join("F-001");
    fs::write(worktree.join("src/b.rs"), "fn b() {}\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: sneak in b.rs"]);

    let path = verdict(&workspace, &approval("reviewer-session-a"));
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
}

#[test]
fn findings_remain_visible_after_a_later_approval() {
    let (workspace, _) = handed_off();

    // First round: changes requested with a finding.
    let rejection = "reviewer_actor_id: reviewer-session-a\ndecision: changes_requested\nfindings:\n  - severity: critical\n    location: src/a.rs\n    detail: missing guard\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
    workspace.review(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict(&workspace, rejection),
        "--actor",
        "reviewer-session-a",
    ]);

    // Section 11.2: the card must come back to active before it can be handed
    // off again. Resuming is what performs that transition.
    workspace.work(&["resume", "--card-id", "F-001"]);

    // The actor fixes it and hands off again.
    let worktree = workspace.worktrees.join("F-001");
    fs::write(worktree.join("src/a.rs"), "fn main() { /* guarded */ }\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "fix: add guard"]);
    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);
    let head = support::capture(&worktree, &["rev-parse", "HEAD"]);
    let declaration = workspace.root.join("declaration2.yaml");
    fs::write(
        &declaration,
        format!(
            "delivered_sha: {head}\nbehavior_delivered: adds a guard\nimplementation_decisions: [mirrored the sibling]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
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

    // Second round: a different reviewer approves, and must account for the
    // finding the first round left open. This test previously used a blanket
    // approval carrying `findings: []`, which the harness accepted — so it was
    // asserting that a critical open finding can be approved away, under a name
    // claiming the opposite. See `an_approval_may_not_drop_a_prior_open_finding`.
    let resolution = "reviewer_actor_id: reviewer-session-b\ndecision: approved\nfindings:\n  - severity: critical\n    location: src/a.rs\n    detail: missing guard\n    disposition: resolved\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed each acceptance behavior directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
    workspace.review(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict(&workspace, resolution),
        "--actor",
        "reviewer-session-b",
    ]);

    let envelope = workspace.review_json(&["inspect", "--card-id", "F-001"]);
    let reviews = envelope["data"]["reviews"].as_array().unwrap();
    assert_eq!(
        reviews.len(),
        2,
        "a re-review supersedes rather than erases"
    );
    assert_eq!(reviews[0]["decision"], "changes_requested");
    assert_eq!(reviews[0]["findings"][0]["detail"], "missing guard");
    assert_eq!(reviews[1]["decision"], "approved");
    assert_eq!(
        reviews[1]["supersedes"], reviews[0]["review_id"],
        "the later review names the one it supersedes"
    );
    assert_eq!(
        reviews[1]["reviewer_actor_id"], "reviewer-session-b",
        "SPIKE-001 H-04: approval came from a different session"
    );
    assert_eq!(envelope["data"]["has_current_approval"], true);
    assert_eq!(
        reviews[1]["findings"][0]["disposition"], "resolved",
        "the approval had to say what became of the earlier finding"
    );
}

#[test]
fn an_approval_may_not_drop_a_prior_open_finding() {
    // Tier 1, defect 2. `WP-320` required this and it was never implemented.
    // A re-review carrying `findings: []` approved away a prior critical open
    // finding: the earlier record stayed on disk, stayed open, and nothing
    // consulted it again. Supersession without this check is only filing.
    let (workspace, _) = handed_off();

    let rejection = "reviewer_actor_id: reviewer-session-a\ndecision: changes_requested\nfindings:\n  - severity: critical\n    location: src/a.rs\n    detail: missing guard\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
    workspace.review(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict(&workspace, rejection),
        "--actor",
        "reviewer-session-a",
    ]);
    workspace.work(&["resume", "--card-id", "F-001"]);

    let worktree = workspace.worktrees.join("F-001");
    fs::write(worktree.join("src/a.rs"), "fn main() { /* guarded */ }\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "fix: add guard"]);
    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);
    let head = support::capture(&worktree, &["rev-parse", "HEAD"]);
    let declaration = workspace.root.join("declaration2.yaml");
    fs::write(
        &declaration,
        format!(
            "delivered_sha: {head}\nbehavior_delivered: adds a guard\nimplementation_decisions: [mirrored the sibling]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
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

    // The fixture has to be able to succeed, or a refusal below proves nothing
    // about the finding: the same approval with the finding dispositioned is
    // accepted, and only the silent one is refused.
    let silent = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict(&workspace, &approval("reviewer-session-b")),
        "--actor",
        "reviewer-session-b",
    ]);
    assert!(
        !silent.status.success(),
        "an approval carrying no findings must not clear a prior critical one"
    );
    assert_eq!(error_code(&silent), "CH-POLICY-OPEN-FINDINGS");
    let envelope: Value = serde_json::from_slice(&silent.stdout).expect("an error envelope");
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("src/a.rs") && message.contains("critical"),
        "the refusal must name the finding it is protecting: {message}"
    );

    let dispositioned = "reviewer_actor_id: reviewer-session-b\ndecision: approved\nfindings:\n  - severity: critical\n    location: src/a.rs\n    detail: missing guard\n    disposition: resolved\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed each acceptance behavior directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
    workspace.review(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict(&workspace, dispositioned),
        "--actor",
        "reviewer-session-b",
    ]);
    assert_eq!(
        workspace.review_json(&["inspect", "--card-id", "F-001"])["data"]["has_current_approval"],
        true,
        "dispositioning it is the way through, and it must work"
    );
}

/// Moves the card on one round: resume, change something, gate, hand off, and
/// open the next review. Leaves it ready for another verdict.
fn next_round(workspace: &Workspace, note: &str) {
    workspace.work(&["resume", "--card-id", "F-001"]);
    let worktree = workspace.worktrees.join("F-001");
    fs::write(
        worktree.join("src/a.rs"),
        format!("fn main() {{ /* {note} */ }}\n"),
    )
    .unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "fix: partial"]);
    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);

    let head = support::capture(&worktree, &["rev-parse", "HEAD"]);
    let declaration = workspace.root.join(format!("declaration-{note}.yaml"));
    fs::write(
        &declaration,
        format!(
            "delivered_sha: {head}\nbehavior_delivered: {note}\nimplementation_decisions: [minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
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
}

#[test]
fn a_finding_can_be_carried_forward_as_still_open_and_still_refuses_an_approval() {
    // `artana-bio/solo-dev#14`. A re-review had to disposition every prior
    // open finding as resolved, accepted or out of scope, and the common shape
    // of a long-running card is none of those: worked on, partly closed, still
    // there. Recording `RV-000039` on `F-027` hit exactly this and was refused.
    let (workspace, _) = handed_off();

    let opened = "reviewer_actor_id: reviewer-session-a\ndecision: changes_requested\nfindings:\n  - severity: critical\n    location: src/a.rs\n    detail: the comparison loses to a case variant\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
    workspace.review(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict(&workspace, opened),
        "--actor",
        "reviewer-session-a",
    ]);

    next_round(&workspace, "narrowed");

    // The verdict that used to be unrecordable.
    let carried = "reviewer_actor_id: reviewer-session-a\ndecision: changes_requested\nfindings:\n  - severity: critical\n    location: src/a.rs\n    detail: the demonstrated character is closed; the class survives at another\n    disposition: still_open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
    workspace.review(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict(&workspace, carried),
        "--actor",
        "reviewer-session-a",
    ]);

    // It is in the record as carried rather than freshly raised, which is what
    // makes "round three of the same defect" legible to a reader.
    let reviews = workspace.review_json(&["inspect", "--card-id", "F-001"]);
    let second = &reviews["data"]["reviews"][1];
    assert_eq!(second["findings"][0]["disposition"], "still_open");
    assert_eq!(second["findings"][0]["location"], "src/a.rs");

    // And it settles nothing: the next round cannot approve over it.
    next_round(&workspace, "still-not-fixed");
    let premature = "reviewer_actor_id: reviewer-session-b\ndecision: approved\nfindings:\n  - severity: critical\n    location: src/a.rs\n    detail: the class survives\n    disposition: still_open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed each acceptance behavior directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
    let refused = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict(&workspace, premature),
        "--actor",
        "reviewer-session-b",
    ]);
    assert!(
        !refused.status.success(),
        "carrying a finding forward must not clear it"
    );
    assert_eq!(error_code(&refused), "CH-POLICY-OPEN-FINDINGS");

    // Which refusal it is matters. Round 2 of this card's own review dropped
    // `StillOpen` from `blocks_approval` and this test stayed green: with the
    // prior carried finding no longer counting as open, the invented-carry
    // check refused the approval instead — a different rule, reaching the same
    // error code. The code alone cannot tell them apart, so assert the reason.
    let envelope: Value = serde_json::from_slice(&refused.stdout).expect("an error envelope");
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("cannot approve with open findings"),
        "the approval must be refused for the open finding, not by some other \
         rule that happens to share the code: {message}"
    );

    // The fixture has to be able to succeed, or the refusal proves nothing:
    // the same approval with the finding actually settled goes through.
    let settled = "reviewer_actor_id: reviewer-session-b\ndecision: approved\nfindings:\n  - severity: critical\n    location: src/a.rs\n    detail: the class is closed\n    disposition: resolved\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed each acceptance behavior directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
    workspace.review(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict(&workspace, settled),
        "--actor",
        "reviewer-session-b",
    ]);
    assert_eq!(
        workspace.review_json(&["inspect", "--card-id", "F-001"])["data"]["has_current_approval"],
        true,
        "settling it is still the way through"
    );
}

#[test]
fn one_resolution_cannot_approve_away_two_findings_at_one_location() {
    // Round 1 of this card's own review, driven through the real CLI exactly
    // as the reviewer drove it. Accounting was existential by location, so one
    // `resolved` discharged every prior finding there: two criticals at
    // `src/a.rs:10`, one resolved, and the approval was recorded with the
    // second gone. A real defect disappearing between rounds is the single
    // thing this rule exists to prevent.
    let (workspace, _) = handed_off();

    let two = "reviewer_actor_id: reviewer-session-a\ndecision: changes_requested\nfindings:\n  - severity: critical\n    location: \"src/a.rs:10\"\n    detail: authorization bypass\n    disposition: open\n  - severity: critical\n    location: \"src/a.rs:10\"\n    detail: destructive operation is not bounded\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
    workspace.review(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict(&workspace, two),
        "--actor",
        "reviewer-session-a",
    ]);

    next_round(&workspace, "fixed-one");

    let half = "reviewer_actor_id: reviewer-session-b\ndecision: approved\nfindings:\n  - severity: critical\n    location: \"src/a.rs:10\"\n    detail: authorization bypass fixed\n    disposition: resolved\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed only the authorization bypass\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
    let refused = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict(&workspace, half),
        "--actor",
        "reviewer-session-b",
    ]);
    assert!(
        !refused.status.success(),
        "one resolution must not answer for two findings"
    );
    assert_eq!(error_code(&refused), "CH-POLICY-OPEN-FINDINGS");
    let envelope: Value = serde_json::from_slice(&refused.stdout).expect("an error envelope");
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(message.contains("1 of 2 accounted for"), "{message}");
    assert!(
        message.contains("critical"),
        "the refusal still names what it is protecting: {message}"
    );

    // The fixture has to be able to succeed: one entry per finding goes
    // through, and the record then says what became of each.
    let both = "reviewer_actor_id: reviewer-session-b\ndecision: approved\nfindings:\n  - severity: critical\n    location: \"src/a.rs:10\"\n    detail: authorization bypass fixed\n    disposition: resolved\n  - severity: critical\n    location: \"src/a.rs:10\"\n    detail: the destructive operation is now bounded\n    disposition: resolved\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed each acceptance behavior directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
    workspace.review(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict(&workspace, both),
        "--actor",
        "reviewer-session-b",
    ]);
    assert_eq!(
        workspace.review_json(&["inspect", "--card-id", "F-001"])["data"]["has_current_approval"],
        true,
        "answering for each finding is the way through, and it must work"
    );
}

#[test]
fn carrying_forward_a_finding_no_earlier_review_raised_is_refused() {
    // Otherwise the disposition manufactures a history of persistence: a
    // first-time observation filed as though three rounds had already seen it.
    let (workspace, _) = handed_off();

    let opened = "reviewer_actor_id: reviewer-session-a\ndecision: changes_requested\nfindings:\n  - severity: critical\n    location: src/a.rs\n    detail: missing guard\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
    workspace.review(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict(&workspace, opened),
        "--actor",
        "reviewer-session-a",
    ]);

    next_round(&workspace, "elsewhere");

    let invented = "reviewer_actor_id: reviewer-session-a\ndecision: changes_requested\nfindings:\n  - severity: critical\n    location: src/a.rs\n    detail: fixed\n    disposition: resolved\n  - severity: high\n    location: src/never-seen.rs\n    detail: pretending this has been here for rounds\n    disposition: still_open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
    let refused = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict(&workspace, invented),
        "--actor",
        "reviewer-session-a",
    ]);
    assert!(!refused.status.success());
    assert_eq!(error_code(&refused), "CH-POLICY-OPEN-FINDINGS");
    let envelope: Value = serde_json::from_slice(&refused.stdout).expect("an error envelope");
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(message.contains("src/never-seen.rs"), "{message}");
}

#[test]
fn the_first_review_of_a_card_cannot_carry_a_finding_forward() {
    // `check_supersedes` only runs when a predecessor exists, so the very
    // first review is the one place a fabricated carry-forward would not meet
    // it. Refused where the record is built instead.
    let (workspace, _) = handed_off();

    let carried = "reviewer_actor_id: reviewer-session-a\ndecision: changes_requested\nfindings:\n  - severity: critical\n    location: src/a.rs\n    detail: claiming this survived a round that never happened\n    disposition: still_open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
    let refused = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict(&workspace, carried),
        "--actor",
        "reviewer-session-a",
    ]);
    assert!(!refused.status.success());
    assert_eq!(error_code(&refused), "CH-POLICY-OPEN-FINDINGS");
    let envelope: Value = serde_json::from_slice(&refused.stdout).expect("an error envelope");
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(message.contains("first review"), "{message}");
}

#[test]
fn a_re_review_requesting_changes_may_not_drop_a_prior_finding_either() {
    // The check applies to every re-review, not only approvals. A
    // `changes_requested` that omits the earlier finding leaves the next
    // reviewer looking at a clean predecessor, which is the same defect one
    // step removed: review 1 opens it, review 2 drops it, review 3 approves
    // over a prior review that no longer shows anything open.
    let (workspace, _) = handed_off();

    let first = "reviewer_actor_id: reviewer-session-a\ndecision: changes_requested\nfindings:\n  - severity: critical\n    location: src/a.rs\n    detail: missing guard\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
    workspace.review(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict(&workspace, first),
        "--actor",
        "reviewer-session-a",
    ]);
    workspace.work(&["resume", "--card-id", "F-001"]);

    let worktree = workspace.worktrees.join("F-001");
    fs::write(worktree.join("src/a.rs"), "fn main() { /* partial */ }\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "wip: partial"]);
    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);
    let head = support::capture(&worktree, &["rev-parse", "HEAD"]);
    let declaration = workspace.root.join("declaration2.yaml");
    fs::write(
        &declaration,
        format!(
            "delivered_sha: {head}\nbehavior_delivered: partial\nimplementation_decisions: [wip]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
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

    // A second round about a different problem entirely, silent on the first.
    let second = "reviewer_actor_id: reviewer-session-b\ndecision: changes_requested\nfindings:\n  - severity: medium\n    location: src/b.rs\n    detail: naming\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
    let output = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict(&workspace, second),
        "--actor",
        "reviewer-session-b",
    ]);
    assert!(
        !output.status.success(),
        "a re-review about something else must still account for what is open"
    );
    assert_eq!(error_code(&output), "CH-POLICY-OPEN-FINDINGS");
}

#[test]
fn the_review_is_versioned_and_recorded_as_an_event() {
    let (workspace, head) = handed_off();
    let path = verdict(&workspace, &approval("reviewer-session-a"));
    let envelope = workspace.review_json(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer-session-a",
    ]);
    let review_id = envelope["data"]["review"]["review_id"].as_str().unwrap();

    let tracked = workspace.control_tracked_files();
    assert!(
        tracked.contains(&format!("reviews/{review_id}.json")),
        "{tracked:?}"
    );

    let recorded = workspace
        .events()
        .into_iter()
        .find(|event| event["event_type"] == "review.recorded")
        .expect("the review must be recorded");
    assert_eq!(recorded["metadata"]["decision"], "approved");
    assert_eq!(recorded["actor_id"], "reviewer-session-a");
    assert_eq!(recorded["head_sha"], head);
    assert_eq!(recorded["next_state"], "approved");
}

#[test]
fn a_dry_run_records_nothing() {
    let (workspace, _) = handed_off();
    let path = verdict(&workspace, &approval("reviewer-session-a"));
    let before = workspace.control_head();

    let envelope = workspace.review_json(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer-session-a",
        "--dry-run",
    ]);
    assert_eq!(envelope["data"]["dry_run"], true);
    assert_eq!(workspace.control_head(), before);
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "review_pending"
    );
}

#[test]
fn a_reviewer_can_record_that_a_card_is_blocked() {
    // Tier 3, defect 21. `Decision::Blocked` exists, parses, validates, and is
    // one of three verdicts the schema offers — and Section 11.2's table has no
    // `review_pending → blocked` edge, so recording it failed the transition
    // check and aborted the whole transaction. No review record was written at
    // all: the reviewer's judgement, and their gate-adequacy finding with it,
    // was discarded rather than filed.
    let (workspace, _) = handed_off();

    let blocked = "reviewer_actor_id: reviewer-session-a\ndecision: blocked\nfindings:\n  - severity: high\n    location: src/a.rs\n    detail: needs a decision outside my remit\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: false\n  unobserved_behaviors: [the failure path]\n  basis: read the gate definitions\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
    workspace.review(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict(&workspace, blocked),
        "--actor",
        "reviewer-session-a",
    ]);

    let envelope = workspace.review_json(&["inspect", "--card-id", "F-001"]);
    let reviews = envelope["data"]["reviews"].as_array().unwrap();
    assert_eq!(
        reviews.len(),
        1,
        "the judgement must be filed, not discarded"
    );
    assert_eq!(reviews[0]["decision"], "blocked");
    assert_eq!(
        reviews[0]["gate_adequacy"]["gates_observe_acceptance"], false,
        "and the gate-adequacy finding filed with it"
    );

    // `blocked` already resumes to active, so the card is not stranded by it.
    workspace.work(&["resume", "--card-id", "F-001"]);
}

#[test]
fn a_card_whose_candidate_moved_during_review_can_be_taken_back() {
    // Tier 3, defect 20. `review begin` moves the card to `review_pending`,
    // whose only exits were `approved`, `changes_requested` and `abandoned`. If
    // the branch moves after the handoff — which is exactly what
    // `reviewing_a_superseded_handoff_is_refused` proves is refused — no
    // verdict can be recorded at all, and the card has no way back to work. The
    // only escape was abandoning it, or `card revise`, which is defect 19.
    // `handed_off` already begins the review, so the card is `review_pending`.
    let (workspace, _) = handed_off();

    let worktree = workspace.worktrees.join("F-001");
    fs::write(worktree.join("src/b.rs"), "fn b() {}\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: sneak in b.rs"]);

    // The fixture must genuinely be stuck: no verdict is recordable now.
    let refused = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict(&workspace, &approval("reviewer-session-a")),
        "--actor",
        "reviewer-session-a",
    ]);
    assert_eq!(error_code(&refused), "CH-POLICY-STALE-HANDOFF");

    // Taking the work back is the way out, the same way a handoff can be
    // revoked before review starts.
    workspace.work(&["resume", "--card-id", "F-001"]);
    let status = workspace.card_json(&["status", "--card-id", "F-001"]);
    assert_eq!(status["data"]["state"], "active");
}

#[test]
fn approving_a_high_risk_card_requires_a_declared_human_reviewer() {
    // Tier 3, defect 22. `Risk::requires_human_review` exists, is unit-tested,
    // and was called from nowhere. Section 15.3 requires a human reviewer for
    // `high` and `critical` cards, and a critical-risk card received exactly
    // the treatment a low-risk one did: the policy was documented, modelled,
    // and never enforced.
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_risk("F-001", &["src/**"], "high");
    workspace.work(&["start", "--card-id", "F-001"]);

    let path = workspace.worktrees.join("F-001");
    fs::create_dir_all(path.join("src")).unwrap();
    fs::write(path.join("src/a.rs"), "fn main() {}\n").unwrap();
    support::git(&path, &["add", "-A"]);
    support::git(&path, &["commit", "-q", "-m", "feat: add a.rs"]);
    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);
    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.review"]);
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

    let legacy_human = "reviewer_actor_id: reviewer-session-a\nhuman_reviewer: true\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: legacy self-certification\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
    let legacy_output = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict(&workspace, legacy_human),
        "--actor",
        "reviewer-session-a",
    ]);
    assert_eq!(error_code(&legacy_output), "CH-POLICY-RISK-REVIEW");

    let output = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict(&workspace, &approval("reviewer-session-a")),
        "--actor",
        "reviewer-session-a",
    ]);
    assert!(
        !output.status.success(),
        "a high-risk card must not be approved without a human reviewer"
    );
    assert_eq!(error_code(&output), "CH-POLICY-RISK-REVIEW");
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("an error envelope");
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("declared"),
        "the message must not claim the harness can tell: {message}"
    );

    // Declaring it is the way through. Like every other identity in this
    // harness it is a claim, not a proof — D-013 — so this must be recorded on
    // the review rather than checked.
    let human = "reviewer_actor_id: reviewer-session-a\nreviewer_kind: human\nreviewer_provenance:\n  provider: fixture\n  model: human\n  session_id: review-session\n  principal_id: reviewer-principal\nhuman_attestation:\n  evidence_id: attestation-1\n  attestor_actor_id: independent-attestor\n  attestor_principal_id: attestor-principal\n  attestor_session_id: attestor-session\n  statement: I independently attest this is a human review\n  independently_created: true\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed each acceptance behavior directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\nmutation_exemption:\n  code: fixture-no-mutation\n  reason: fixture review has no executable mutation\n  approved_by: independent-attestor\n";
    let envelope = workspace.review_json(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict(&workspace, human),
        "--actor",
        "reviewer-session-a",
    ]);
    assert_eq!(envelope["data"]["state"], "approved");
    assert_eq!(
        envelope["data"]["review"]["reviewer_kind"], "human",
        "the typed claim must be on the record, so an auditor can see who made it"
    );
}

#[test]
fn a_low_risk_card_needs_no_human_declaration() {
    // The guard: the check must key on the card's risk, not become a blanket
    // requirement that makes every agent review impossible.
    let (workspace, _) = handed_off();
    let envelope = workspace.review_json(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict(&workspace, &approval("reviewer-session-a")),
        "--actor",
        "reviewer-session-a",
    ]);
    assert_eq!(envelope["data"]["state"], "approved");
    assert_eq!(envelope["data"]["review"]["human_reviewer"], false);
}

#[test]
fn every_new_low_risk_approval_needs_typed_mutation_evidence() {
    let (workspace, _) = handed_off();
    let path = workspace.root.join("unbound-approval.yaml");
    fs::write(
        &path,
        "reviewer_actor_id: reviewer-session-a\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: direct check\n  mutation_evidence:\n    status: exempt\n    reason: prose only\nresidual_risks: []\nreview_conduct: separate_process\n",
    )
    .unwrap();
    let output = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path.display().to_string(),
        "--actor",
        "reviewer-session-a",
    ]);
    assert_eq!(error_code(&output), "CH-POLICY-INCOMPLETE-REVIEW");
}

#[test]
fn human_approval_rejects_blank_attestor_identity() {
    let (workspace, _) = handed_off();
    let path = workspace.root.join("blank-attestor.yaml");
    fs::write(
        &path,
        "reviewer_actor_id: reviewer-session-a\nreviewer_kind: human\nreviewer_provenance:\n  provider: fixture\n  model: human\n  session_id: review-session\n  principal_id: reviewer-principal\nhuman_attestation:\n  evidence_id: attestation-blank\n  attestor_actor_id: ' '\n  attestor_principal_id: attestor-principal\n  attestor_session_id: attestor-session\n  statement: independent attestation\n  independently_created: true\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: direct check\n  mutation_evidence:\n    status: exempt\n    reason: fixture\nresidual_risks: []\nreview_conduct: separate_process\nmutation_exemption:\n  code: fixture\n  reason: no mutation\n  approved_by: independent-attestor\n",
    )
    .unwrap();
    let output = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path.display().to_string(),
        "--actor",
        "reviewer-session-a",
    ]);
    assert_eq!(error_code(&output), "CH-POLICY-RISK-REVIEW");
}

#[test]
fn human_attestation_requires_a_distinct_principal_and_session_boundary() {
    let cases = [
        (
            "same-reviewer-principal",
            "reviewer-principal",
            "attestor-session",
        ),
        (
            "same-reviewer-session",
            "attestor-principal",
            "review-session",
        ),
        (
            "same-implementer-principal",
            "implementer-principal",
            "attestor-session",
        ),
        (
            "same-implementer-session",
            "attestor-principal",
            "implementer-session",
        ),
        ("blank-provenance", " ", " "),
    ];
    for (label, principal, session) in cases {
        let (workspace, _) = handed_off();
        let path = workspace.root.join(format!("{label}.yaml"));
        fs::write(
            &path,
            format!(
                "reviewer_actor_id: reviewer-session-a\nreviewer_kind: human\nreviewer_provenance:\n  provider: fixture\n  model: human\n  session_id: review-session\n  principal_id: reviewer-principal\nhuman_attestation:\n  evidence_id: {label}\n  attestor_actor_id: different-attestor\n  attestor_principal_id: '{principal}'\n  attestor_session_id: '{session}'\n  statement: independent\n  independently_created: true\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: direct check\n  mutation_evidence:\n    status: exempt\n    reason: fixture\nresidual_risks: []\nreview_conduct: separate_process\nmutation_exemption:\n  code: fixture\n  reason: no mutation\n  approved_by: independent-approver\n"
            ),
        )
        .unwrap();
        let output = workspace.review_raw(&[
            "record",
            "--card-id",
            "F-001",
            "--verdict",
            &path.display().to_string(),
            "--actor",
            "reviewer-session-a",
        ]);
        assert!(
            !output.status.success(),
            "{label} unexpectedly succeeded: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(error_code(&output), "CH-POLICY-RISK-REVIEW", "{label}");
    }

    let (workspace, _) = handed_off();
    let path = workspace.root.join("missing-attestor-provenance.yaml");
    fs::write(
        &path,
        "reviewer_actor_id: reviewer-session-a\nreviewer_kind: human\nreviewer_provenance:\n  provider: fixture\n  model: human\n  session_id: review-session\n  principal_id: reviewer-principal\nhuman_attestation:\n  evidence_id: missing-provenance\n  attestor_actor_id: independent-attestor\n  statement: independent\n  independently_created: true\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: direct check\n  mutation_evidence:\n    status: exempt\n    reason: fixture\nresidual_risks: []\nreview_conduct: separate_process\nmutation_exemption:\n  code: fixture\n  reason: no mutation\n  approved_by: independent-approver\n",
    )
    .unwrap();
    let output = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path.display().to_string(),
        "--actor",
        "reviewer-session-a",
    ]);
    assert_eq!(error_code(&output), "CH-POLICY-RISK-REVIEW");

    let (workspace, _) = handed_off();
    let path = workspace.root.join("valid-distinct-attestation.yaml");
    fs::write(
        &path,
        "reviewer_actor_id: reviewer-session-a\nreviewer_kind: human\nreviewer_provenance:\n  provider: fixture\n  model: human\n  session_id: review-session\n  principal_id: reviewer-principal\nhuman_attestation:\n  evidence_id: distinct-attestation\n  attestor_actor_id: independent-attestor\n  attestor_principal_id: attestor-principal\n  attestor_session_id: attestor-session\n  statement: independent\n  independently_created: true\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: direct check\n  mutation_evidence:\n    status: exempt\n    reason: fixture\nresidual_risks: []\nreview_conduct: separate_process\nmutation_exemption:\n  code: fixture\n  reason: no mutation\n  approved_by: independent-approver\n",
    )
    .unwrap();
    let output = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path.display().to_string(),
        "--actor",
        "reviewer-session-a",
    ]);
    assert!(
        output.status.success(),
        "valid distinct attestation refused: {output:?}"
    );
}
