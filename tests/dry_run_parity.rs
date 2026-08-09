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

fn review_contract_fixture() -> Workspace {
    let workspace = Workspace::initialized();
    // This local fixture includes valid and invalid exemption-backed verdicts.
    // Opt into their closed policy before cycle creation so the cycle freezes
    // it explicitly; shared setup remains fail-closed.
    workspace.install_fixture_mutation_exemption_policy();
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
    let worktree = workspace.worktrees.join("F-001");
    fs::create_dir_all(worktree.join("src")).unwrap();
    fs::write(worktree.join("src/a.rs"), "fn main() {}\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: add a.rs"]);
    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);
    let head = support::capture(&worktree, &["rev-parse", "HEAD"]);
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
    workspace
}

fn review_contract_verdict(case: &str) -> String {
    let identity = "reviewer_actor_id: reviewer-session-a\nreviewer_kind: agent\nreviewer_provenance:\n  provider: fixture\n  model: fixture\n  session_id: review-session\n  principal_id: reviewer-principal\n";
    let common = "decision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: direct check\n  mutation_evidence:\n    status: exempt\n    reason: no executable mutation\nresidual_risks: []\nreview_conduct: separate_process\n";
    match case {
        "human-no-attestation" => format!(
            "reviewer_actor_id: reviewer-session-a\nreviewer_kind: human\nreviewer_provenance:\n  provider: fixture\n  model: human\n  session_id: review-session\n  principal_id: reviewer-principal\n{common}mutation_exemption:\n  code: fixture\n  reason: no mutation\n  approved_by: independent-attestor\n"
        ),
        "same-principal-attestation" => format!(
            "reviewer_actor_id: reviewer-session-a\nreviewer_kind: human\nreviewer_provenance:\n  provider: fixture\n  model: human\n  session_id: review-session\n  principal_id: reviewer-principal\nhuman_attestation:\n  evidence_id: attestation\n  attestor_actor_id: different-actor\n  attestor_principal_id: reviewer-principal\n  attestor_session_id: attestor-session\n  statement: independent\n  independently_created: true\n{common}mutation_exemption:\n  code: fixture\n  reason: no mutation\n  approved_by: different-approver\n"
        ),
        "invalid-exemption" => format!(
            "{identity}{common}mutation_exemption:\n  code: ''\n  reason: no mutation\n  approved_by: independent-attestor\n"
        ),
        "missing-receipt" => format!("{identity}mutation_receipt_ids: [MR-999]\n{common}"),
        "valid" => format!(
            "reviewer_actor_id: reviewer-session-a\nreviewer_kind: human\nreviewer_provenance:\n  provider: fixture\n  model: human\n  session_id: review-session\n  principal_id: reviewer-principal\nhuman_attestation:\n  evidence_id: attestation\n  attestor_actor_id: independent-attestor\n  attestor_principal_id: attestor-principal\n  attestor_session_id: attestor-session\n  statement: independent\n  independently_created: true\n{common}mutation_exemption:\n  code: fixture\n  reason: no mutation\n  approved_by: independent-approver\n"
        ),
        _ => unreachable!("unknown review contract case"),
    }
}

fn review_begin_identity_fixture() -> Workspace {
    let workspace = active_cycle();
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);
    let worktree = workspace.worktrees.join("F-001");
    fs::create_dir_all(worktree.join("src")).unwrap();
    fs::write(worktree.join("src/a.rs"), "fn main() {}\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: add a.rs"]);
    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);
    let head = support::capture(&worktree, &["rev-parse", "HEAD"]);
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
    workspace
}

fn control_snapshot(workspace: &Workspace) -> (String, usize) {
    let head = support::capture(&workspace.control, &["rev-parse", "HEAD"]);
    let journals = workspace.control.join("journal");
    let count = fs::read_dir(journals).map_or(0, std::iter::Iterator::count);
    (head, count)
}

#[test]
fn review_begin_identity_refusal_has_dry_run_parity_and_no_side_effects() {
    for (actor, principal, session, expected) in [
        (
            "operator",
            "implementer-principal",
            "implementer-session",
            "CH-POLICY-SELF-REVIEW",
        ),
        (
            "reviewer",
            "reviewer-principal",
            "implementer-session",
            "CH-POLICY-SAME-ACTOR",
        ),
    ] {
        let workspace = review_begin_identity_fixture();
        let args = [
            "begin",
            "--card-id",
            "F-001",
            "--actor",
            actor,
            "--actor-principal-id",
            principal,
            "--actor-session-id",
            session,
        ];
        let before = control_snapshot(&workspace);
        let real = workspace.review_raw(&args);
        let after_real = control_snapshot(&workspace);
        let dry_args = [
            "begin",
            "--card-id",
            "F-001",
            "--actor",
            actor,
            "--actor-principal-id",
            principal,
            "--actor-session-id",
            session,
            "--dry-run",
        ];
        let dry = workspace.review_raw(&dry_args);
        let after_dry = control_snapshot(&workspace);
        assert!(!real.status.success());
        assert!(!dry.status.success());
        assert_eq!(code(&real), expected);
        assert_eq!(code(&dry), expected);
        assert_eq!(before, after_real, "real refusal must be side-effect free");
        assert_eq!(
            after_real, after_dry,
            "dry-run refusal must be side-effect free"
        );
    }
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
        "reviewer_actor_id: operator\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\n",
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

// #28 §12 mutation 5. `check_review_conduct` has the same shape defect
// `check_independence` and `validate_mutation_evidence` were each already
// fixed for here: without a call inside `preview_record`, `--dry-run` would
// report success for a same-context-conduct verdict the real command
// refuses. #189 already lists seven checks with this defect; #120 closed one
// by running it ahead of the dry-run branch. This card follows that
// solution — `check_review_conduct` is called directly inside
// `preview_record`, in the same relative position `check_independence` is.
//
// Mutation (§11.5): remove the `check_review_conduct` call from
// `preview_record` (leaving it only in `run_record`'s real transaction, which
// sits textually after the `--dry-run` early return — the literal "move the
// refusal after the `--dry-run` branch" this mutation names). This test must
// fail: the dry run would then report success for the same fixture the real
// command refuses.
#[test]
fn review_record_previews_a_same_context_conduct_refusal() {
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

    // `activate_card`'s fixture declares `review_policy: independent` (the
    // default every workspace helper uses). A distinct reviewer, so this is
    // not the self-review case the test above already pins — the only thing
    // wrong with this verdict is the conduct it declares.
    let verdict = workspace.root.join("verdict.yaml");
    fs::write(
        &verdict,
        "reviewer_actor_id: reviewer-session-a\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: same_context\n",
    )
    .unwrap();
    let path = verdict.display().to_string();

    let real = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer-session-a",
    ]);
    assert_parity(
        "review record",
        &real,
        &workspace.review_raw(&[
            "record",
            "--card-id",
            "F-001",
            "--verdict",
            &path,
            "--actor",
            "reviewer-session-a",
            "--dry-run",
        ]),
    );
    assert_eq!(
        code(&real),
        "CH-POLICY-SELF-REVIEW",
        "the fixture must exercise the same-context-conduct refusal, not something else"
    );
}

// Repair, post-review: `check_review_conduct` refuses an absent declaration
// on an `independent`-policy card exactly as it refuses a declared
// `same_context` one, and the same #189/#120 defect applies to both arms of
// that one function -- there is no separate call site to forget, but the
// coverage still needs its own pinned fixture, since this exercises a
// different `ErrorCode` than the test above.
//
// Mutation: remove the `check_review_conduct` call from `preview_record`
// (the same edit mutation 5 above names). This test must fail: the dry run
// would report success for a verdict that declares no conduct at all on an
// `independent` card, which the real command refuses.
#[test]
fn review_record_previews_an_undeclared_conduct_refusal() {
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

    // No `review_conduct` key at all -- otherwise a clean, distinct-reviewer
    // approval, exactly as the fixture above except for the field this test
    // exercises.
    let verdict = workspace.root.join("verdict.yaml");
    fs::write(
        &verdict,
        "reviewer_actor_id: reviewer-session-a\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\n",
    )
    .unwrap();
    let path = verdict.display().to_string();

    let real = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer-session-a",
    ]);
    assert_parity(
        "review record",
        &real,
        &workspace.review_raw(&[
            "record",
            "--card-id",
            "F-001",
            "--verdict",
            &path,
            "--actor",
            "reviewer-session-a",
            "--dry-run",
        ]),
    );
    assert_eq!(
        code(&real),
        "CH-POLICY-INCOMPLETE-REVIEW",
        "the fixture must exercise the undeclared-conduct refusal, not something else"
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
        "reviewer_actor_id: reviewer-session-a\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\n",
    )
    .unwrap();
    let path = verdict.display().to_string();

    // #120: `--actor` must agree with the verdict's `reviewer_actor_id`, or
    // `require_actor_agreement` — which runs ahead of every other check on
    // both forms — refuses first, for a reason unrelated to what this
    // fixture exercises.
    let real = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer-session-a",
    ]);
    assert_parity(
        "review record",
        &real,
        &workspace.review_raw(&[
            "record",
            "--card-id",
            "F-001",
            "--verdict",
            &path,
            "--actor",
            "reviewer-session-a",
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
fn review_record_previews_a_stale_self_review_as_stale() {
    // Review round 1 of this exact card: independence pre-existed the
    // staleness call this card adds, and ran first. A verdict that is both a
    // self-review and stale reported CH-POLICY-SELF-REVIEW from the preview
    // and CH-POLICY-STALE-HANDOFF from the real command — parity in
    // presence, not in which reason wins when both apply. `run_record`
    // checks staleness before independence, so the preview must too.
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

    // "operator" is the fixture's handoff actor, so this is also a
    // self-review — both refusals apply at once.
    let verdict = workspace.root.join("verdict.yaml");
    fs::write(
        &verdict,
        "reviewer_actor_id: operator\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\n",
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
        "the real command must resolve this in favor of staleness, not independence"
    );
}

#[test]
fn review_record_previews_a_missing_mutation_evidence_refusal() {
    // #95 gap 1 added ReviewRecord::validate's mutation_evidence check, but
    // validate() has exactly one call site — inside run_record's real
    // transaction, after a full ReviewRecord is built — and preview_record
    // never reaches it. A verdict with no mutation evidence at all was
    // accepted by the dry run and refused by the real command: Tier 3
    // defect 24, reproduced on this card's own field.
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

    // Otherwise a clean, distinct-reviewer approval — every other check this
    // file and `ReviewRecord::validate` enforce is satisfied. The only thing
    // wrong with it is the field this card added: `gate_adequacy` carries
    // none of the three original keys' problems, and no `mutation_evidence`
    // key at all.
    let verdict = workspace.root.join("verdict.yaml");
    fs::write(
        &verdict,
        "reviewer_actor_id: reviewer-session-a\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\nresidual_risks: []\n",
    )
    .unwrap();
    let path = verdict.display().to_string();

    // #120: `--actor` must agree with the verdict's `reviewer_actor_id`, or
    // `require_actor_agreement` — which runs ahead of every other check on
    // both forms — refuses first, for a reason unrelated to what this
    // fixture exercises. Coincidentally the same code
    // (`CH-POLICY-INCOMPLETE-REVIEW`) as the mutation-evidence refusal below,
    // which is exactly why this must be pinned rather than left to chance.
    let real = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer-session-a",
    ]);
    assert_parity(
        "review record",
        &real,
        &workspace.review_raw(&[
            "record",
            "--card-id",
            "F-001",
            "--verdict",
            &path,
            "--actor",
            "reviewer-session-a",
            "--dry-run",
        ]),
    );
    assert_eq!(
        code(&real),
        "CH-POLICY-INCOMPLETE-REVIEW",
        "the fixture must exercise the mutation-evidence refusal, not something else"
    );
}

#[test]
fn review_record_dry_run_matches_record_for_attestation_and_evidence_contracts() {
    let cases = [
        ("human-no-attestation", "CH-POLICY-RISK-REVIEW"),
        ("same-principal-attestation", "CH-POLICY-RISK-REVIEW"),
        ("invalid-exemption", "CH-POLICY-INCOMPLETE-REVIEW"),
        ("missing-receipt", "CH-GATE-EVIDENCE-STALE"),
    ];
    for (case, expected) in cases {
        let workspace = review_contract_fixture();
        let verdict = workspace.root.join(format!("{case}.yaml"));
        fs::write(&verdict, review_contract_verdict(case)).unwrap();
        let path = verdict.display().to_string();
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
        assert_parity(case, &real, &preview);
        assert_eq!(code(&real), expected, "{case} owning refusal changed");
    }

    let real_workspace = review_contract_fixture();
    let real_verdict = real_workspace.root.join("valid.yaml");
    fs::write(&real_verdict, review_contract_verdict("valid")).unwrap();
    let real_output = real_workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &real_verdict.display().to_string(),
        "--actor",
        "reviewer-session-a",
    ]);
    assert!(real_output.status.success());

    let preview_workspace = review_contract_fixture();
    let preview_verdict = preview_workspace.root.join("valid.yaml");
    fs::write(&preview_verdict, review_contract_verdict("valid")).unwrap();
    let before = preview_workspace.control_head();
    let preview_output = preview_workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &preview_verdict.display().to_string(),
        "--actor",
        "reviewer-session-a",
        "--dry-run",
    ]);
    assert!(preview_output.status.success());
    assert_eq!(preview_workspace.control_head(), before);
}

#[test]
fn review_record_previews_an_actor_disagreement_refusal() {
    // #120: `--actor` was accepted on `review record` and never read, so a
    // verdict declaring a different `reviewer_actor_id` was recorded under
    // that declaration, silently. `require_actor_agreement` now refuses the
    // disagreement, called once in `run_record` ahead of the `--dry-run`
    // branch — so the same call answers both forms, and this pins that
    // neither can drift from the other the way the seven gaps #189
    // catalogued did for other checks that were duplicated instead.
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

    // No `--actor` on either `record` call below: it defaults to `operator`,
    // which disagrees with the verdict's declared `reviewer-session-a`.
    let verdict = workspace.root.join("verdict.yaml");
    fs::write(
        &verdict,
        "reviewer_actor_id: reviewer-session-a\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\n",
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
        "CH-POLICY-INCOMPLETE-REVIEW",
        "the fixture must exercise the actor-agreement refusal, not something else"
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
fn cycle_seal_previews_a_repeat_seal_refusal_without_mutation() {
    let workspace = active_cycle();
    workspace.cycle(&["seal", "--cycle-id", "C-001"]);
    let before = workspace.control_head();
    assert_parity(
        "cycle seal",
        &workspace.cycle_raw(&["seal", "--cycle-id", "C-001"]),
        &workspace.cycle_raw(&["seal", "--cycle-id", "C-001", "--dry-run"]),
    );
    assert_eq!(
        workspace.control_head(),
        before,
        "refused previews must not mutate"
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
