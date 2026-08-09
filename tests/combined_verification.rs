//! `WP-440` acceptance: combined verification and integration review.
//!
//! The point of rerunning gates against the landing commit is that a gate which
//! passed on an isolated candidate proves nothing about the combined tree.
//! Everything here checks that the reruns really happened, against really the
//! landing SHA, and that nothing older can be substituted for them.

mod support;

use std::fs;

use support::Workspace;

/// A cycle with `count` approved, merged, landed cards.
fn landed(count: usize) -> (Workspace, String) {
    landed_with_invariants(count, &[])
}

/// The same, with cycle release invariants declared.
fn landed_with_invariants(count: usize, invariants: &[&str]) -> (Workspace, String) {
    let workspace = Workspace::initialized();
    let mut create = vec![
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ];
    for invariant in invariants {
        create.push("--release-invariant");
        create.push(invariant);
    }
    workspace.cycle(&create);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    for index in 1..=count {
        let card = format!("F-{index:03}");
        workspace.activate_card(&card, &[&format!("src/{card}/**")]);
    }
    for index in 1..=count {
        let card = format!("F-{index:03}");
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
    for step in ["merge", "land"] {
        workspace.integration(&[step, "--integration-id", &id, "--actor-id", "coordinator"]);
    }
    (workspace, id)
}

fn error_code(output: &std::process::Output) -> String {
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["code"].as_str().unwrap().to_owned()
}

/// Every receipt stored in the control repository.
fn receipts(workspace: &Workspace) -> Vec<serde_json::Value> {
    let directory = workspace.control.join("receipts");
    let mut names: Vec<_> = fs::read_dir(&directory)
        .expect("a receipts directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    names.sort();
    names
        .iter()
        .map(|path| serde_json::from_str(&fs::read_to_string(path).unwrap()).expect("a receipt"))
        .collect()
}

fn activate_card_with_proof(
    workspace: &Workspace,
    card_id: &str,
    proof_id: &str,
    invariant: &str,
    oracle: &str,
) {
    let baseline = workspace.cycle_json(&["status", "--cycle-id", "C-001"])["data"]["baseline_sha"]
        .as_str()
        .unwrap()
        .to_owned();
    let draft = workspace.root.join(format!("{card_id}-proof.yaml"));
    fs::write(
        &draft,
        format!(
            "card_id: {card_id}\ncycle_id: C-001\ntitle: Proof {card_id}\ngoal: Exercise proof scope\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {baseline}\nwrite_scope:\n  include: [src/{card_id}/**]\n  exclude: []\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\nproof_map:\n  schema: harness.proof-map/v1\n  entries:\n    - id: {proof_id}\n      invariant: {invariant}\n      precondition: fixture exists\n      assertion: gate observes behavior\n      mutation: bypass makes gate fail\n      gate_oracle: {oracle}\n  claim_boundary: only this fixture\n"
        ),
    )
    .unwrap();
    workspace.card(&["create", "--draft", draft.to_str().unwrap()]);
    workspace.card(&["activate", "--card-id", card_id]);
}

#[test]
fn every_final_receipt_evaluates_the_exact_landing_sha() {
    let (workspace, id) = landed(2);
    let landing =
        workspace.integration_json(&["inspect", "--integration-id", &id])["data"]["landing_sha"]
            .as_str()
            .unwrap()
            .to_owned();

    let envelope =
        workspace.integration_json(&["verify", "--integration-id", &id, "--actor-id", "verifier"]);
    assert_eq!(envelope["data"]["landing_sha"], landing);

    let final_receipts: Vec<_> = receipts(&workspace)
        .into_iter()
        .filter(|receipt| receipt["integration_id"] == id.as_str())
        .collect();
    assert!(!final_receipts.is_empty(), "gates must actually have run");
    for receipt in &final_receipts {
        assert_eq!(
            receipt["evaluated_sha"], landing,
            "a final receipt for another commit proves nothing: {receipt}"
        );
        assert!(
            receipt["card_id"].is_null(),
            "a combined run belongs to the integration, not to one card"
        );
        let provenance = &receipt["provenance"];
        assert_eq!(provenance["schema"], "harness.receipt-provenance/v1");
        assert_eq!(provenance["subject"]["kind"], "integration");
        assert_eq!(provenance["subject"]["landing_sha"], landing);
        assert_eq!(provenance["subject"]["integration_id"], id.as_str());
        assert!(
            provenance["subject"]["integration_digest"]
                .as_str()
                .unwrap()
                .starts_with("sha256:")
        );
        assert!(provenance["subject"]["lease_id"].is_null());
    }
}

#[test]
fn stale_feature_evidence_cannot_replace_the_integration_rerun() {
    let (workspace, id) = landed(1);
    // The feature gate already passed on the isolated candidate, and those
    // receipts are still on disk.
    let feature_receipts: Vec<_> = receipts(&workspace)
        .into_iter()
        .filter(|receipt| receipt["card_id"] == "F-001")
        .collect();
    assert!(
        !feature_receipts.is_empty(),
        "the fixture ran feature gates"
    );
    let before = receipts(&workspace).len();

    workspace.integration(&["verify", "--integration-id", &id, "--actor-id", "verifier"]);

    assert!(
        receipts(&workspace).len() > before,
        "verification must produce new receipts, not reuse the feature ones"
    );
    let landing =
        workspace.integration_json(&["inspect", "--integration-id", &id])["data"]["landing_sha"]
            .as_str()
            .unwrap()
            .to_owned();
    for receipt in &feature_receipts {
        assert_ne!(
            receipt["evaluated_sha"], landing,
            "the fixture must actually have stale evidence for this to test anything"
        );
    }
}

#[test]
fn every_gate_any_member_names_is_rerun() {
    let (workspace, id) = landed(2);
    let envelope =
        workspace.integration_json(&["verify", "--integration-id", &id, "--actor-id", "verifier"]);

    // Cards name `gate.unit` as a feature gate and `gate.all` as an
    // integration gate; both must be rerun, and each only once.
    let ids = envelope["data"]["receipt_ids"].as_array().unwrap();
    assert_eq!(ids.len(), 2, "unexpected: {}", envelope["data"]);
    let gates: Vec<String> = receipts(&workspace)
        .into_iter()
        .filter(|receipt| receipt["integration_id"] == id.as_str())
        .map(|receipt| receipt["gate_id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(gates, ["gate.all", "gate.unit"]);
}

#[test]
fn a_combined_gate_failure_blocks_acceptance() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    // Passes for each card alone and fails once both files exist — exactly the
    // failure an isolated feature gate cannot see.
    workspace.register_gate(
        "gate.combined",
        &[
            "sh",
            "-c",
            "! (test -f src/F-001/a.rs && test -f src/F-002/a.rs)",
        ],
    );
    for card in ["F-001", "F-002"] {
        workspace.activate_card_with_gate_sets(
            card,
            &[&format!("src/{card}/**")],
            &["gate.unit"],
            &["gate.combined"],
        );
    }
    for card in ["F-001", "F-002"] {
        workspace.approve_card(card, &format!("src/{card}/a.rs"));
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
    for step in ["merge", "land"] {
        workspace.integration(&[step, "--integration-id", &id, "--actor-id", "coordinator"]);
    }

    let output =
        workspace.integration_raw(&["verify", "--integration-id", &id, "--actor-id", "verifier"]);
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(error_code(&output), "CH-GATE-FAILED");

    // The integration must not have advanced, so acceptance cannot follow.
    assert_eq!(
        workspace.integration_json(&["inspect", "--integration-id", &id])["data"]["status"],
        "prepared"
    );
    let review = workspace.integration_raw(&[
        "review",
        "--integration-id",
        &id,
        "--reviewer-actor-id",
        "reviewer",
    ]);
    assert_eq!(review.status.code(), Some(5));
}

#[test]
fn invalid_integration_junit_is_recorded_as_a_failed_receipt() {
    let workspace = Workspace::initialized();
    let gate_path = workspace.gate_definition(
        "gate.junit.integration",
        "schema: harness.gate/v1\ngate_id: gate.junit.integration\nrevision: 1\nargv: [sh, -c, \"printf '%s' '<testsuite>' > report.xml\"]\nworking_directory: .\ntimeout_seconds: 60\nenvironment:\n  allow: [PATH]\n  set: {}\nnetwork_policy: denied\nretry_policy:\n  max_attempts: 1\nartifacts: []\njunit_reports: [report.xml]\n",
    );
    workspace.gate(&["register", "--definition", &gate_path]);
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Integration JUnit metrics",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gate_sets(
        "F-001",
        &["src/F-001/**"],
        &["gate.unit"],
        &["gate.junit.integration"],
    );
    workspace.approve_card("F-001", "src/F-001/a.rs");
    let integration_id = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
    ])["data"]["integration_id"]
        .as_str()
        .unwrap()
        .to_owned();
    for step in ["merge", "land"] {
        workspace.integration(&[
            step,
            "--integration-id",
            &integration_id,
            "--actor-id",
            "coordinator",
        ]);
    }

    let output = workspace.integration_raw(&[
        "verify",
        "--integration-id",
        &integration_id,
        "--actor-id",
        "verifier",
    ]);
    assert_eq!(output.status.code(), Some(7));
    let receipt = receipts(&workspace)
        .into_iter()
        .find(|receipt| {
            receipt["integration_id"] == integration_id
                && receipt["gate_id"] == "gate.junit.integration"
        })
        .expect("integration invalid JUnit receipt");
    assert_eq!(receipt["passed"], false);
    assert_eq!(receipt["test_results"]["status"], "invalid");
    assert_eq!(receipt["test_results"]["error_code"], "malformed");
}

#[test]
fn a_failed_verification_is_still_recorded() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.register_gate("gate.always-fails", &["false"]);
    workspace.activate_card_with_gate_sets(
        "F-001",
        &["src/F-001/**"],
        &["gate.unit"],
        &["gate.always-fails"],
    );
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
    for step in ["merge", "land"] {
        workspace.integration(&[step, "--integration-id", &id, "--actor-id", "coordinator"]);
    }
    let output =
        workspace.integration_raw(&["verify", "--integration-id", &id, "--actor-id", "verifier"]);
    assert_eq!(output.status.code(), Some(7));

    // A failure that left no trace would let a retry look like the first try.
    let event = workspace
        .events()
        .into_iter()
        .find(|event| event["event_type"] == "integration.verified")
        .expect("a failed verification must be recorded");
    assert_eq!(event["metadata"]["passed"], false);
    assert!(
        workspace
            .control_tracked_files()
            .iter()
            .any(|path| path == &format!("verifications/{id}.json")),
        "the verification record must be committed"
    );
}

#[test]
fn a_dirty_post_gate_worktree_fails_verification() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    // A gate that writes into the tree it is checking invalidates its own
    // result: the verified tree is no longer the landing tree.
    workspace.register_gate("gate.leaky", &["sh", "-c", "echo leaked > leaked.txt"]);
    workspace.activate_card_with_gate_sets(
        "F-001",
        &["src/F-001/**"],
        &["gate.unit"],
        &["gate.leaky"],
    );
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
    for step in ["merge", "land"] {
        workspace.integration(&[step, "--integration-id", &id, "--actor-id", "coordinator"]);
    }

    let output =
        workspace.integration_raw(&["verify", "--integration-id", &id, "--actor-id", "verifier"]);
    assert_eq!(output.status.code(), Some(7));
    let message = {
        let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        envelope["error"]["message"].as_str().unwrap().to_owned()
    };
    assert!(
        message.contains("dirty"),
        "the diagnosis must name the real problem: {message}"
    );
}

#[test]
fn the_interaction_checklist_names_members_whose_contracts_touch() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);

    for (card, changes, reads) in [
        ("F-001", "api.orders", "api.users"),
        ("F-002", "api.users", "api.orders"),
    ] {
        let body = format!(
            "card_id: {card}\ncycle_id: C-001\ntitle: Implement {card}\ngoal: Deliver {card}\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [\"src/{card}/**\"]\n  exclude: []\ncontract_changes: [{changes}]\ncontract_reads: [{reads}]\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
            base = workspace.authority_head()
        );
        let path = workspace.root.join(format!("{card}.yaml"));
        fs::write(&path, body).unwrap();
        workspace.card(&["create", "--draft", &path.display().to_string()]);
        workspace.card(&["activate", "--card-id", card]);
    }
    for card in ["F-001", "F-002"] {
        workspace.approve_card(card, &format!("src/{card}/a.rs"));
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
    for step in ["merge", "land"] {
        workspace.integration(&[step, "--integration-id", &id, "--actor-id", "coordinator"]);
    }

    let envelope =
        workspace.integration_json(&["verify", "--integration-id", &id, "--actor-id", "verifier"]);
    let interactions = envelope["data"]["interactions"].as_array().unwrap();
    assert_eq!(
        interactions.len(),
        2,
        "each direction is its own thing to check: {}",
        envelope["data"]
    );
    assert_eq!(interactions[0]["changes"], "F-001");
    assert_eq!(interactions[0]["reads"], "F-002");
    assert_eq!(interactions[0]["shared"], serde_json::json!(["api.orders"]));
}

#[test]
fn cycle_invariants_are_carried_into_verification_unanswered() {
    let (workspace, id) = landed_with_invariants(1, &["no public API breakage"]);
    let envelope =
        workspace.integration_json(&["verify", "--integration-id", &id, "--actor-id", "verifier"]);

    let invariants = envelope["data"]["invariants"].as_array().unwrap();
    assert_eq!(invariants.len(), 1);
    assert_eq!(invariants[0]["invariant"], "no public API breakage");
    assert_eq!(
        invariants[0]["machine_checked"], false,
        "a free-text invariant is a reviewer's judgment, and must not claim otherwise"
    );
}

#[test]
fn a_non_member_card_cannot_lend_its_proof_to_an_integration() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Proof scope",
        "--release-invariant",
        "borrowed-invariant",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    activate_card_with_proof(
        &workspace,
        "F-001",
        "member-proof",
        "member-invariant",
        "gate.unit",
    );
    activate_card_with_proof(
        &workspace,
        "F-002",
        "borrowed-proof",
        "borrowed-invariant",
        "gate.unit",
    );
    workspace.approve_card("F-001", "src/F-001/a.rs");
    let id = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--card-id",
        "F-001",
        "--actor-id",
        "coordinator",
    ])["data"]["integration_id"]
        .as_str()
        .unwrap()
        .to_owned();
    for step in ["merge", "land"] {
        workspace.integration(&[step, "--integration-id", &id, "--actor-id", "coordinator"]);
    }

    let verified =
        workspace.integration_json(&["verify", "--integration-id", &id, "--actor-id", "verifier"]);
    assert_eq!(verified["data"]["invariants"][0]["machine_checked"], false);
    assert!(verified["data"]["invariants"][0]["proof_entry_id"].is_null());
}

#[test]
fn duplicate_proof_ids_across_integration_members_refuse_before_evidence_mutation() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Proof uniqueness",
        "--release-invariant",
        "shared-proof",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    activate_card_with_proof(
        &workspace,
        "F-001",
        "shared-proof",
        "first-invariant",
        "gate.unit",
    );
    activate_card_with_proof(
        &workspace,
        "F-002",
        "shared-proof",
        "conflicting-invariant",
        "gate.all",
    );
    for card in ["F-001", "F-002"] {
        workspace.approve_card(card, &format!("src/{card}/a.rs"));
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
    for step in ["merge", "land"] {
        workspace.integration(&[step, "--integration-id", &id, "--actor-id", "coordinator"]);
    }
    let head_before = workspace.control_head();
    let receipts_before = receipts(&workspace).len();

    let refusal =
        workspace.integration_raw(&["verify", "--integration-id", &id, "--actor-id", "verifier"]);
    assert_eq!(refusal.status.code(), Some(5));
    assert_eq!(error_code(&refusal), "CH-POLICY-INVALID-CARD");
    assert_eq!(workspace.control_head(), head_before);
    assert_eq!(receipts(&workspace).len(), receipts_before);
}

#[test]
fn a_review_cannot_leave_a_declared_invariant_unaddressed() {
    let (workspace, id) = landed_with_invariants(1, &["no public API breakage"]);
    workspace.integration(&["verify", "--integration-id", &id, "--actor-id", "verifier"]);

    let output = workspace.integration_raw(&[
        "review",
        "--integration-id",
        &id,
        "--reviewer-actor-id",
        "reviewer",
    ]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-INVARIANT-UNADDRESSED");

    // Confirming it explicitly is what lets the review through.
    workspace.integration(&[
        "review",
        "--integration-id",
        &id,
        "--reviewer-actor-id",
        "reviewer",
        "--invariant-holds",
        "no public API breakage",
    ]);
}

#[test]
fn the_integration_review_records_residual_risk() {
    let (workspace, id) = landed(2);
    workspace.integration(&["verify", "--integration-id", &id, "--actor-id", "verifier"]);

    let envelope = workspace.integration_json(&[
        "review",
        "--integration-id",
        &id,
        "--reviewer-actor-id",
        "reviewer",
        "--residual-risk",
        "the migration is untested against production volumes",
    ]);
    assert_eq!(envelope["data"]["status"], "reviewed");
    assert_eq!(
        envelope["data"]["residual_risks"],
        serde_json::json!(["the migration is untested against production volumes"])
    );

    let event = workspace
        .events()
        .into_iter()
        .find(|event| event["event_type"] == "integration.reviewed")
        .expect("the review must be recorded");
    assert_eq!(
        event["metadata"]["residual_risks"][0],
        "the migration is untested against production volumes"
    );
}

#[test]
fn whoever_verified_an_integration_cannot_also_review_it() {
    let (workspace, id) = landed(1);
    workspace.integration(&["verify", "--integration-id", &id, "--actor-id", "verifier"]);

    let output = workspace.integration_raw(&[
        "review",
        "--integration-id",
        &id,
        "--reviewer-actor-id",
        "verifier",
    ]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-SAME-ACTOR");
}

#[test]
fn an_unverified_integration_cannot_be_reviewed() {
    let (workspace, id) = landed(1);
    let output = workspace.integration_raw(&[
        "review",
        "--integration-id",
        &id,
        "--reviewer-actor-id",
        "reviewer",
    ]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-INVALID-TRANSITION");
}

#[test]
fn verification_leaves_no_worktree_behind() {
    let (workspace, id) = landed(2);
    let worktrees_before = workspace.candidate_worktrees();

    workspace.integration(&["verify", "--integration-id", &id, "--actor-id", "verifier"]);

    assert!(!workspace.worktrees.join(".integration").join(&id).exists());
    assert_eq!(workspace.candidate_worktrees(), worktrees_before);
}
