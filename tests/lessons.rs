//! Governed-lessons acceptance coverage.

mod support;

use std::fs;

use serde_json::Value;
use support::Workspace;

fn install_lesson_authorizer(workspace: &Workspace, actor: &str) {
    let path = workspace.root.join("final-authorization-policy.json");
    fs::write(
        &path,
        serde_json::to_string_pretty(&serde_json::json!({
            "version": "harness.final-authorization-policy/v1",
            "authorization_unit": "sealed_cycle",
            "authorizer_actor_ids": [actor]
        }))
        .unwrap(),
    )
    .unwrap();
    let output = Workspace::run(&[
        "project".into(),
        "set-final-authorization-policy".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--policy".into(),
        path.display().to_string(),
        "--output".into(),
        "json".into(),
    ]);
    assert!(
        output.status.success(),
        "failed to install lesson authorizer: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn lesson_command(
    workspace: &Workspace,
    action: &str,
    lesson_id: &str,
    actor: &str,
    dry_run: bool,
) -> std::process::Output {
    let mut args = vec![
        "lesson".to_owned(),
        action.to_owned(),
        "--control".to_owned(),
        workspace.control.display().to_string(),
        "--lesson-id".to_owned(),
        lesson_id.to_owned(),
        "--actor".to_owned(),
        actor.to_owned(),
        "--output".to_owned(),
        "json".to_owned(),
    ];
    if dry_run {
        args.push("--dry-run".to_owned());
    }
    Workspace::run(&args)
}

fn lesson_definition(workspace: &Workspace) -> String {
    let path = workspace.root.join("lesson.yaml");
    fs::write(
        &path,
        "title: Carry review lessons forward\nrule: Read every applicable lesson before handoff\nrationale: Fresh agents otherwise repeat known errors\nselectors:\n  paths: [src/**]\n  contracts: []\n  change_kinds: []\n  minimum_risk: null\nenforcement: required\nobligations:\n  feature_gates: []\n  integration_gates: []\n  review_checks: [lesson-read]\nprovenance:\n  source_kind: review\n  source_id: RV-000001\n  evidence: Prior review found an omitted regression\n",
    )
    .unwrap();
    path.display().to_string()
}

fn feature_lesson_definition(workspace: &Workspace) -> String {
    let path = workspace.root.join("feature-lesson.yaml");
    fs::write(
        &path,
        "title: Preserve the feature gate preference\nrule: Run the named feature gate before handoff\nrationale: Prior work found that this gate catches the recurring regression\nselectors:\n  paths: [src/**]\n  contracts: []\n  change_kinds: []\n  minimum_risk: null\nenforcement: required\nobligations:\n  feature_gates: [gate.unit]\n  integration_gates: []\n  review_checks: []\nprovenance:\n  source_kind: review\n  source_id: RV-000002\n  evidence: Prior review found the feature gate was skipped\n",
    )
    .unwrap();
    path.display().to_string()
}

fn propose_and_activate(workspace: &Workspace, definition: String) -> String {
    let proposed = Workspace::run(&[
        "lesson".into(),
        "propose".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--definition".into(),
        definition,
        "--output".into(),
        "json".into(),
    ]);
    assert!(proposed.status.success());
    let proposed: Value = serde_json::from_slice(&proposed.stdout).unwrap();
    let lesson_id = proposed["data"]["lesson"]["lesson_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let activated = Workspace::run(&[
        "lesson".into(),
        "activate".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--lesson-id".into(),
        lesson_id.clone(),
        "--actor".into(),
        "owner".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert!(activated.status.success());
    lesson_id
}

fn prepare_candidate(workspace: &Workspace) -> std::path::PathBuf {
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
            "delivered_sha: {head}\nbehavior_delivered: adds a.rs\nimplementation_decisions: [kept it minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert the commit\n"
        ),
    )
    .unwrap();
    declaration
}

fn packet_digest(workspace: &Workspace) -> String {
    let packet = workspace.work_json(&["packet", "--card-id", "F-001"]);
    let digest = packet["data"]["manifest_digest"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        packet["data"]["reporting_contract"]["handoff_argument"],
        serde_json::json!(["--lesson-manifest-digest", digest])
    );
    digest
}

#[test]
fn lessons_are_proposed_activated_and_retired_without_rewriting_history() {
    let workspace = Workspace::initialized();
    install_lesson_authorizer(&workspace, "owner");
    let definition = lesson_definition(&workspace);
    let proposed = Workspace::run(&[
        "lesson".into(),
        "propose".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--definition".into(),
        definition,
        "--output".into(),
        "json".into(),
    ]);
    assert!(
        proposed.status.success(),
        "{}",
        String::from_utf8_lossy(&proposed.stderr)
    );
    let proposed: Value = serde_json::from_slice(&proposed.stdout).unwrap();
    assert_eq!(proposed["data"]["lesson"]["status"], "proposed");
    let lesson_id = proposed["data"]["lesson"]["lesson_id"].as_str().unwrap();

    let activated = Workspace::run(&[
        "lesson".into(),
        "activate".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--lesson-id".into(),
        lesson_id.into(),
        "--actor".into(),
        "owner".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert!(
        activated.status.success(),
        "{}",
        String::from_utf8_lossy(&activated.stderr)
    );
    let activated: Value = serde_json::from_slice(&activated.stdout).unwrap();
    assert_eq!(activated["data"]["lesson"]["status"], "active");
    assert_eq!(activated["data"]["lesson"]["revision"], 2);

    let listed = Workspace::run(&[
        "lesson".into(),
        "list".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--output".into(),
        "json".into(),
    ]);
    let listed: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(listed["data"]["lessons"].as_array().unwrap().len(), 1);
    assert_eq!(listed["data"]["lessons"][0]["status"], "active");

    let retired = Workspace::run(&[
        "lesson".into(),
        "retire".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--lesson-id".into(),
        lesson_id.into(),
        "--actor".into(),
        "owner".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert!(
        retired.status.success(),
        "{}",
        String::from_utf8_lossy(&retired.stderr)
    );
    let retired: Value = serde_json::from_slice(&retired.stdout).unwrap();
    assert_eq!(retired["data"]["lesson"]["status"], "retired");

    let entries = fs::read_dir(workspace.control.join("lessons").join(lesson_id))
        .unwrap()
        .count();
    assert_eq!(
        entries, 3,
        "propose/activate/retire must preserve immutable revisions"
    );
}

#[test]
fn a_required_feature_lesson_blocks_handoff_until_its_receipt_exists() {
    let workspace = Workspace::initialized();
    install_lesson_authorizer(&workspace, "owner");
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Enforce a governed lesson",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gates("F-001", &["src/**"], &["gate.unit"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    propose_and_activate(&workspace, feature_lesson_definition(&workspace));
    let manifest_digest = packet_digest(&workspace);

    let worktree = workspace.worktrees.join("F-001");
    fs::create_dir_all(worktree.join("src")).unwrap();
    fs::write(worktree.join("src/a.rs"), "fn main() {}\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: add a.rs"]);
    let head = support::capture(&worktree, &["rev-parse", "HEAD"]);
    let declaration = workspace.root.join("declaration.yaml");
    fs::write(
        &declaration,
        format!(
            "delivered_sha: {head}\nbehavior_delivered: adds a.rs\nimplementation_decisions: [kept it minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert the commit\n"
        ),
    )
    .unwrap();

    let refused = workspace.handoff_raw(&[
        "create",
        "--card-id",
        "F-001",
        "--declaration",
        &declaration.display().to_string(),
        "--lesson-manifest-digest",
        &manifest_digest,
    ]);
    let refused_json: Value = serde_json::from_slice(&refused.stdout).unwrap();
    // The card's named feature gate is already a base handoff requirement, so
    // the missing receipt is refused at that shared evidence boundary before
    // the lesson-specific receipt check runs.
    assert_eq!(refused_json["error"]["code"], "CH-GATE-EVIDENCE-STALE");

    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);
    let handed_off = workspace.handoff_json(&[
        "create",
        "--card-id",
        "F-001",
        "--declaration",
        &declaration.display().to_string(),
        "--lesson-manifest-digest",
        &manifest_digest,
    ]);
    assert_eq!(handed_off["status"], "success");
    assert!(
        handed_off["data"]["lesson_manifest_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
}

#[test]
fn a_governed_project_refuses_handoff_without_the_packet_digest_in_preview_and_reality() {
    let workspace = Workspace::initialized();
    install_lesson_authorizer(&workspace, "owner");
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Bind the implementation packet",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gates("F-001", &["src/**"], &["gate.unit"]);
    workspace.work(&["start", "--card-id", "F-001"]);
    propose_and_activate(&workspace, lesson_definition(&workspace));
    let declaration = prepare_candidate(&workspace);

    for dry_run in [true, false] {
        let mut args = vec![
            "create",
            "--card-id",
            "F-001",
            "--declaration",
            declaration.to_str().unwrap(),
        ];
        if dry_run {
            args.push("--dry-run");
        }
        let refused = workspace.handoff_raw(&args);
        let envelope: Value = serde_json::from_slice(&refused.stdout).unwrap();
        assert_eq!(envelope["error"]["code"], "CH-POLICY-LESSON-MANIFEST-STALE");
        assert!(
            envelope["error"]["message"]
                .as_str()
                .unwrap()
                .contains("missing the lesson manifest digest")
        );
    }
    assert!(!workspace.control.join("handoffs").exists());
}

#[test]
fn retiring_a_lesson_after_packet_generation_refuses_handoff_in_preview_and_reality() {
    let workspace = Workspace::initialized();
    install_lesson_authorizer(&workspace, "owner");
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Keep retired lessons in the packet binding",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gates("F-001", &["src/**"], &["gate.unit"]);
    workspace.work(&["start", "--card-id", "F-001"]);
    let lesson_id = propose_and_activate(&workspace, lesson_definition(&workspace));
    let stale_digest = packet_digest(&workspace);

    let retired = Workspace::run(&[
        "lesson".into(),
        "retire".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--lesson-id".into(),
        lesson_id,
        "--actor".into(),
        "owner".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert!(retired.status.success());
    let declaration = prepare_candidate(&workspace);

    for dry_run in [true, false] {
        let mut args = vec![
            "create",
            "--card-id",
            "F-001",
            "--declaration",
            declaration.to_str().unwrap(),
            "--lesson-manifest-digest",
            &stale_digest,
        ];
        if dry_run {
            args.push("--dry-run");
        }
        let refused = workspace.handoff_raw(&args);
        let envelope: Value = serde_json::from_slice(&refused.stdout).unwrap();
        assert_eq!(envelope["error"]["code"], "CH-POLICY-LESSON-MANIFEST-STALE");
        let message = envelope["error"]["message"].as_str().unwrap();
        assert!(message.contains(&stale_digest));
        assert!(message.contains("implementation packet is stale"));
    }
    assert!(!workspace.control.join("handoffs").exists());
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "active"
    );
}

#[test]
fn lesson_policy_changes_require_a_configured_authorizer_on_preview_and_execution() {
    let workspace = Workspace::initialized();
    let definition = lesson_definition(&workspace);
    let proposed = Workspace::run(&[
        "lesson".into(),
        "propose".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--definition".into(),
        definition,
        "--actor".into(),
        "proposer".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert!(proposed.status.success());
    let proposed: Value = serde_json::from_slice(&proposed.stdout).unwrap();
    let lesson_id = proposed["data"]["lesson"]["lesson_id"].as_str().unwrap();

    for dry_run in [true, false] {
        let refused = lesson_command(&workspace, "activate", lesson_id, "owner", dry_run);
        assert!(!refused.status.success());
        let envelope: Value = serde_json::from_slice(&refused.stdout).unwrap();
        assert_eq!(envelope["error"]["code"], "CH-POLICY-NOT-ACCEPTED");
    }
    assert_eq!(
        fs::read_dir(workspace.control.join("lessons").join(lesson_id))
            .unwrap()
            .count(),
        1,
        "a refused activation must not allocate a revision"
    );

    install_lesson_authorizer(&workspace, "owner");
    let outsider = lesson_command(&workspace, "activate", lesson_id, "reviewer", false);
    assert!(!outsider.status.success());
    let outsider: Value = serde_json::from_slice(&outsider.stdout).unwrap();
    assert_eq!(outsider["error"]["code"], "CH-POLICY-NOT-ACCEPTED");

    assert!(
        lesson_command(&workspace, "activate", lesson_id, "owner", false)
            .status
            .success()
    );
    let repeated = lesson_command(&workspace, "activate", lesson_id, "owner", false);
    assert!(!repeated.status.success());
    let repeated: Value = serde_json::from_slice(&repeated.stdout).unwrap();
    assert_eq!(repeated["error"]["code"], "CH-POLICY-LESSON-INVALID");

    assert!(
        lesson_command(&workspace, "retire", lesson_id, "owner", false)
            .status
            .success()
    );
    let reactivate = lesson_command(&workspace, "activate", lesson_id, "owner", false);
    assert!(!reactivate.status.success());
    let reactivate: Value = serde_json::from_slice(&reactivate.stdout).unwrap();
    assert_eq!(reactivate["error"]["code"], "CH-POLICY-LESSON-INVALID");
}

#[test]
fn registry_loader_refuses_gaps_and_broken_supersedes_links() {
    let workspace = Workspace::initialized();
    install_lesson_authorizer(&workspace, "owner");
    let definition = lesson_definition(&workspace);
    let proposed = Workspace::run(&[
        "lesson".into(),
        "propose".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--definition".into(),
        definition,
        "--output".into(),
        "json".into(),
    ]);
    let proposed: Value = serde_json::from_slice(&proposed.stdout).unwrap();
    let lesson_id = proposed["data"]["lesson"]["lesson_id"].as_str().unwrap();
    assert!(
        lesson_command(&workspace, "activate", lesson_id, "owner", false)
            .status
            .success()
    );

    let revision_two = workspace
        .control
        .join("lessons")
        .join(lesson_id)
        .join("r2.json");
    let original = fs::read_to_string(&revision_two).unwrap();
    let mut record: Value = serde_json::from_str(&original).unwrap();
    record["supersedes"] = Value::Null;
    fs::write(
        &revision_two,
        format!("{}\n", serde_json::to_string_pretty(&record).unwrap()),
    )
    .unwrap();
    let broken_link = Workspace::run(&[
        "lesson".into(),
        "list".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--output".into(),
        "json".into(),
    ]);
    assert!(!broken_link.status.success());
    let broken_link: Value = serde_json::from_slice(&broken_link.stdout).unwrap();
    assert_eq!(broken_link["error"]["code"], "CH-INTERNAL-CONTROL-CORRUPT");

    fs::write(&revision_two, original).unwrap();
    fs::remove_file(
        workspace
            .control
            .join("lessons")
            .join(lesson_id)
            .join("r1.json"),
    )
    .unwrap();
    let gap = Workspace::run(&[
        "lesson".into(),
        "list".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--output".into(),
        "json".into(),
    ]);
    assert!(!gap.status.success());
    let gap: Value = serde_json::from_slice(&gap.stdout).unwrap();
    assert_eq!(gap["error"]["code"], "CH-INTERNAL-CONTROL-CORRUPT");
}

#[test]
fn a_later_lesson_activation_does_not_invalidate_a_frozen_handoff_manifest() {
    let workspace = Workspace::initialized();
    install_lesson_authorizer(&workspace, "owner");
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Keep frozen lesson evidence reviewable",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    let worktree = workspace.worktrees.join("F-001");
    fs::create_dir_all(worktree.join("src")).unwrap();
    fs::write(worktree.join("src/a.rs"), "fn historical() {}\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-qm", "freeze empty lesson set"]);
    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);
    let head = support::capture(&worktree, &["rev-parse", "HEAD"]);
    let declaration = workspace.root.join("frozen-declaration.yaml");
    fs::write(
        &declaration,
        format!(
            "delivered_sha: {head}\nbehavior_delivered: freezes a handoff before policy changes\nimplementation_decisions: [minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
        ),
    )
    .unwrap();
    let handoff = workspace.handoff_json(&[
        "create",
        "--card-id",
        "F-001",
        "--declaration",
        &declaration.display().to_string(),
    ]);
    assert!(
        handoff["data"]["handoff"]["lesson_manifest"]["lessons"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let definition = lesson_definition(&workspace);
    let proposed = Workspace::run(&[
        "lesson".into(),
        "propose".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--definition".into(),
        definition,
        "--output".into(),
        "json".into(),
    ]);
    let proposed: Value = serde_json::from_slice(&proposed.stdout).unwrap();
    let lesson_id = proposed["data"]["lesson"]["lesson_id"].as_str().unwrap();
    assert!(
        lesson_command(&workspace, "activate", lesson_id, "owner", false)
            .status
            .success()
    );

    let review = workspace.review_raw(&[
        "begin",
        "--card-id",
        "F-001",
        "--actor",
        "independent-reviewer",
    ]);
    assert!(
        review.status.success(),
        "later policy must not make a frozen handoff stale: {}{}",
        String::from_utf8_lossy(&review.stdout),
        String::from_utf8_lossy(&review.stderr)
    );
}
