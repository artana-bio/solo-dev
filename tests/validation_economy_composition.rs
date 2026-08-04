//! #22 composition proof: the validation economy's four bounded controls agree
//! in one real control repository without granting authority to a projection.

mod support;

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::Duration,
};

use change_harness::{
    domain::{
        digest::Digest,
        integration::{IntegrationRecord, VerificationRecord},
    },
    runner::receipt::{ProvenanceDimension, Receipt},
};
use support::{Workspace, git};

fn slow_gate(workspace: &Workspace, gate_id: &str, marker: &Path, release: &Path) {
    let marker = serde_json::to_string(&marker.display().to_string()).unwrap();
    let release = serde_json::to_string(&release.display().to_string()).unwrap();
    let definition = workspace.gate_definition(
        gate_id,
        &format!(
            "schema: harness.gate/v1\ngate_id: {gate_id}\nrevision: 1\nargv: [\"sh\", \"-c\", \"printf started > \\\"$MARKER\\\"; while [ ! -e \\\"$RELEASE\\\" ]; do sleep 0.01; done\"]\nworking_directory: \".\"\ntimeout_seconds: 60\nenvironment:\n  allow: [PATH]\n  set:\n    MARKER: {marker}\n    RELEASE: {release}\nnetwork_policy: denied\nretry_policy:\n  max_attempts: 1\nartifacts: []\n"
        ),
    );
    workspace.gate(&["register", "--definition", &definition]);
}

fn cpu_profile(workspace: &Workspace) -> String {
    let path = workspace.root.join("cpu-profile.json");
    fs::write(
        &path,
        r#"{"schema":"harness.cpu-heavy-validation-profile/v1","risk":"high","expected_duration_seconds":60,"resource_cost":{"cpu_cores":1,"memory_mib":1024}}"#,
    )
    .unwrap();
    path.display().to_string()
}

fn await_started(marker: &Path, child: &mut Child) {
    while !marker.exists() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("gate exited before writing its start marker: {status}");
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        child.try_wait().unwrap().is_none(),
        "gate exited after writing its start marker"
    );
}

struct HeldGateRuns {
    release_paths: Vec<PathBuf>,
    children: Vec<Child>,
}

impl HeldGateRuns {
    fn new(release_paths: Vec<PathBuf>, children: Vec<Child>) -> Self {
        Self {
            release_paths,
            children,
        }
    }

    fn child_mut(&mut self, index: usize) -> &mut Child {
        &mut self.children[index]
    }

    fn release(&self) {
        for release in &self.release_paths {
            fs::write(release, "release\n").unwrap();
        }
    }

    fn release_and_reap(mut self) -> Vec<Output> {
        self.release();
        self.children
            .drain(..)
            .map(|child| child.wait_with_output().unwrap())
            .collect()
    }
}

impl Drop for HeldGateRuns {
    fn drop(&mut self) {
        self.release();
        for child in &mut self.children {
            let _ = child.wait();
        }
    }
}

fn schedule_raw(
    workspace: &Workspace,
    reservation_id: &str,
    state_revision: Option<&str>,
) -> std::process::Output {
    let mut args = vec![
        "gate".to_owned(),
        "schedule".to_owned(),
        "--output".to_owned(),
        "json".to_owned(),
        "--control".to_owned(),
        workspace.control.display().to_string(),
        "--reservation-id".to_owned(),
        reservation_id.to_owned(),
    ];
    if let Some(state_revision) = state_revision {
        args.extend(["--state-revision".to_owned(), state_revision.to_owned()]);
    }
    Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .args(args)
        .output()
        .unwrap()
}

fn complete_fixture_provenance(receipt: &mut Receipt) {
    let provenance = receipt.provenance.as_mut().unwrap();
    for (dimension, value) in [
        (
            ProvenanceDimension::Toolchain,
            b"composition-toolchain".as_slice(),
        ),
        (
            ProvenanceDimension::Inputs,
            b"composition-inputs".as_slice(),
        ),
        (
            ProvenanceDimension::Fixtures,
            b"composition-fixtures".as_slice(),
        ),
        (ProvenanceDimension::Cache, b"composition-cache".as_slice()),
        (
            ProvenanceDimension::TrustMode,
            b"composition-trust".as_slice(),
        ),
    ] {
        provenance
            .dimensions
            .insert(dimension, Digest::of_bytes(value));
    }
}

fn complete_one_card_integration(workspace: &Workspace) -> String {
    workspace.activate_card("F-001", &["src/integration/**"]);
    workspace.approve_card("F-001", "src/integration/a.rs");
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
    workspace.integration(&[
        "verify",
        "--integration-id",
        &integration_id,
        "--actor-id",
        "verifier",
    ]);
    workspace.integration(&[
        "review",
        "--integration-id",
        &integration_id,
        "--reviewer-actor-id",
        "reviewer",
    ]);
    workspace.acceptance(&[
        "record",
        "--integration-id",
        &integration_id,
        "--acceptance-owner",
        "owner",
    ]);
    workspace.integration(&[
        "promote",
        "--integration-id",
        &integration_id,
        "--actor-id",
        "promoter",
    ]);
    integration_id
}

fn exact_integration_request(workspace: &Workspace, integration_id: &str) -> serde_json::Value {
    let integration: IntegrationRecord = serde_json::from_str(
        &fs::read_to_string(
            workspace
                .control
                .join(format!("integrations/{integration_id}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    let verification: VerificationRecord = serde_json::from_str(
        &fs::read_to_string(
            workspace
                .control
                .join(format!("verifications/{integration_id}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    let receipt_id = verification.receipt_ids.first().unwrap();
    let receipt_path = workspace
        .control
        .join(format!("receipts/{receipt_id}.json"));
    let mut receipt: Receipt =
        serde_json::from_str(&fs::read_to_string(&receipt_path).unwrap()).unwrap();
    complete_fixture_provenance(&mut receipt);
    fs::write(
        &receipt_path,
        format!("{}\n", serde_json::to_string_pretty(&receipt).unwrap()),
    )
    .unwrap();
    git(&workspace.control, &["add", "receipts"]);
    git(
        &workspace.control,
        &["commit", "-qm", "complete composition provenance"],
    );

    let expected = receipt.provenance.unwrap();
    serde_json::json!({
        "schema": "harness.receipt-compatibility/v1",
        "context": {
            "integration_id": integration_id,
            "cycle_id": integration.cycle_id,
            "landing_sha": integration.landing_sha,
            "baseline_sha": integration.baseline_sha,
            "integration_digest": integration.substantive_digest().unwrap(),
            "verification_digest": verification.digest().unwrap(),
            "policy_digest": expected.policy_digest,
        },
        "stage": "final_integration",
        "check": {
            "gate_id": receipt.gate_id,
            "gate_digest": receipt.gate_digest,
            "receipt_schema": receipt.schema,
            "max_attempts": 1,
        },
        "expected": expected,
    })
}

#[test]
#[allow(clippy::too_many_lines)]
fn validation_economy_composition_is_fail_fast_exact_and_never_authorizes_from_a_dashboard() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "one bounded validation-economy composition",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    let markers = [
        workspace.root.join("lane-one"),
        workspace.root.join("lane-two"),
    ];
    let releases = [
        workspace.root.join("release-lane-one"),
        workspace.root.join("release-lane-two"),
    ];
    workspace.register_gate("gate.narrow", &["true"]);
    workspace.register_gate("gate.mutation", &["false"]);
    slow_gate(&workspace, "gate.cpu.one", &markers[0], &releases[0]);
    slow_gate(&workspace, "gate.cpu.two", &markers[1], &releases[1]);
    slow_gate(
        &workspace,
        "gate.cpu.three",
        &workspace.root.join("lane-three"),
        &workspace.root.join("release-lane-three"),
    );
    workspace.register_gate("gate.independent", &["true"]);
    for (card, scope, gate) in [
        ("F-002", "src/narrow/**", "gate.narrow"),
        ("F-003", "README.md", "gate.mutation"),
        ("F-004", "src/cpu-one/**", "gate.cpu.one"),
        ("F-005", "src/cpu-two/**", "gate.cpu.two"),
        ("F-006", "src/cpu-three/**", "gate.cpu.three"),
        ("F-007", "src/independent/**", "gate.independent"),
    ] {
        workspace.activate_card_with_gates(card, &[scope], &[gate]);
    }

    // #31: exact, complete integration evidence can be reused; a changed
    // fixture identity is stale evidence and deterministically requires rerun.
    let integration_id = complete_one_card_integration(&workspace);
    let request = exact_integration_request(&workspace, &integration_id);
    let request_path = workspace.root.join("exact-reuse.json");
    fs::write(&request_path, serde_json::to_vec_pretty(&request).unwrap()).unwrap();
    let exact = workspace.gate_json(&[
        "status",
        "--integration-id",
        &integration_id,
        "--compatibility-request",
        request_path.to_str().unwrap(),
    ]);
    assert_eq!(
        exact["data"]["receipt_compatibility"]["disposition"]["kind"],
        "compatible_reuse"
    );
    let mut stale_request = request;
    stale_request["expected"]["dimensions"]["fixtures"] =
        serde_json::json!(Digest::of_bytes(b"different-fixture"));
    let stale_request_path = workspace.root.join("stale-reuse.json");
    fs::write(
        &stale_request_path,
        serde_json::to_vec_pretty(&stale_request).unwrap(),
    )
    .unwrap();
    let stale = workspace.gate_json(&[
        "status",
        "--integration-id",
        &integration_id,
        "--compatibility-request",
        stale_request_path.to_str().unwrap(),
    ]);
    assert_eq!(
        stale["data"]["receipt_compatibility"]["disposition"]["kind"],
        "rerun_required"
    );
    assert_eq!(
        stale["data"]["receipt_compatibility"]["disposition"]["reasons"],
        serde_json::json!(["fixtures"])
    );

    // #30: a read-only dashboard selects the narrow gate but cannot authorize
    // it. Exact duplicate reservations have one durable winner.
    workspace.work(&["start", "--card-id", "F-002"]);
    let head_before_dashboard = workspace.control_head();
    let preflight = workspace.gate_json(&["preflight", "--card-id", "F-002"]);
    assert_eq!(preflight["data"]["next_permitted_stage"], "narrow");
    assert_eq!(preflight["data"]["next_permitted_gate"], "gate.narrow");
    assert_eq!(
        workspace.control_head(),
        head_before_dashboard,
        "dashboard is read-only"
    );
    assert!(
        workspace.gate_json(&["status", "--card-id", "F-002"])["data"]["receipts"]
            .as_array()
            .unwrap()
            .is_empty(),
        "a dashboard must never create a gate receipt or advance the ladder"
    );
    let winner = workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-002",
        "--gate-id",
        "gate.narrow",
        "--actor",
        "holder",
    ]);
    let reservation_id = winner["data"]["reservation"]["reservation_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let duplicate = workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-002",
        "--gate-id",
        "gate.narrow",
        "--actor",
        "other",
    ]);
    assert_eq!(
        duplicate["data"]["disposition"]["kind"],
        "wait_for_reserved_run"
    );
    assert_eq!(
        duplicate["data"]["reservation"]["reservation_id"],
        reservation_id
    );
    workspace.gate(&[
        "run",
        "--card-id",
        "F-002",
        "--gate-id",
        "gate.narrow",
        "--reservation-id",
        &reservation_id,
        "--actor",
        "holder",
    ]);
    let narrow_worktree: std::path::PathBuf = workspace.work_json(&[
        "status",
        "--card-id",
        "F-002",
    ])["data"]["held_lease"]["worktree_path"]
        .as_str()
        .unwrap()
        .into();
    fs::write(narrow_worktree.join("changed.txt"), "changed\n").unwrap();
    git(&narrow_worktree, &["add", "changed.txt"]);
    git(
        &narrow_worktree,
        &["commit", "-qm", "stale narrow evidence"],
    );
    let stale_preflight = workspace.gate_json(&["preflight", "--card-id", "F-002"]);
    assert_eq!(stale_preflight["data"]["next_permitted_stage"], "narrow");
    assert_eq!(
        stale_preflight["data"]["next_permitted_gate"],
        "gate.narrow"
    );

    // #32: declared mutations are reserved once, run in an isolated source,
    // restored after each witness, and never leak into the candidate worktree.
    workspace.work(&["start", "--card-id", "F-003"]);
    let campaign = workspace.root.join("campaign.json");
    fs::write(&campaign, r#"{"schema":"harness.declared-mutation-campaign/v1","mutations":[{"id":"M-001","path":"README.md","expected_utf8":"hello\n","replacement_utf8":"mutant-one\n"},{"id":"M-002","path":"README.md","expected_utf8":"hello\n","replacement_utf8":"mutant-two\n"}]}"#).unwrap();
    let mutation_winner = workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-003",
        "--gate-id",
        "gate.mutation",
        "--execution-mode",
        "declared-mutations",
        "--campaign",
        campaign.to_str().unwrap(),
        "--actor",
        "holder",
    ]);
    let mutation_reservation = mutation_winner["data"]["reservation"]["reservation_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mutation_duplicate = workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-003",
        "--gate-id",
        "gate.mutation",
        "--execution-mode",
        "declared-mutations",
        "--campaign",
        campaign.to_str().unwrap(),
        "--actor",
        "other",
    ]);
    assert_eq!(
        mutation_duplicate["data"]["reservation"]["reservation_id"],
        mutation_reservation
    );
    let mutation_worktree: std::path::PathBuf = workspace.work_json(&[
        "status",
        "--card-id",
        "F-003",
    ])["data"]["held_lease"]["worktree_path"]
        .as_str()
        .unwrap()
        .into();
    let baseline = fs::read(mutation_worktree.join("README.md")).unwrap();
    let mutation = workspace.gate_json(&[
        "mutate",
        "--reservation-id",
        &mutation_reservation,
        "--campaign",
        campaign.to_str().unwrap(),
        "--actor",
        "holder",
    ]);
    assert_eq!(
        mutation["data"]["mutation_witnesses"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        fs::read(mutation_worktree.join("README.md")).unwrap(),
        baseline
    );
    let status = Command::new("git")
        .arg("-C")
        .arg(&mutation_worktree)
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8(status.stdout).unwrap().is_empty(),
        "mutation must not leak into candidate workspace"
    );

    // These completed composition lanes must not accidentally become the
    // alternative work that #33 is evaluating below.
    for card in ["F-002", "F-003"] {
        workspace.work(&[
            "block",
            "--card-id",
            card,
            "--reason",
            "completed composition lane",
        ]);
    }

    // #33: with two CPU-heavy lanes occupied, the third reservation is deferred
    // while independent work is eligible; it becomes a true wait when that
    // alternative is blocked, and starts only after a terminal lane settlement.
    for card in ["F-004", "F-005", "F-006", "F-007"] {
        workspace.work(&["start", "--card-id", card]);
    }
    let profile = cpu_profile(&workspace);
    let reserve_cpu = |card: &str, gate: &str| {
        workspace.gate_json(&[
            "reserve",
            "--card-id",
            card,
            "--gate-id",
            gate,
            "--execution-mode",
            "cpu-heavy",
            "--cpu-profile",
            &profile,
            "--actor",
            "holder",
        ])["data"]["reservation"]["reservation_id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let first = reserve_cpu("F-004", "gate.cpu.one");
    let second = reserve_cpu("F-005", "gate.cpu.two");
    let third = reserve_cpu("F-006", "gate.cpu.three");
    let run_cpu = |card: &'static str, gate: &'static str, reservation: String| {
        let control = workspace.control.clone();
        Command::new(env!("CARGO_BIN_EXE_change-harness"))
            .args([
                "gate",
                "run",
                "--output",
                "json",
                "--control",
                control.to_str().unwrap(),
                "--card-id",
                card,
                "--gate-id",
                gate,
                "--reservation-id",
                &reservation,
                "--actor",
                "holder",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let mut held_runs = HeldGateRuns::new(
        releases.to_vec(),
        vec![
            run_cpu("F-004", "gate.cpu.one", first),
            run_cpu("F-005", "gate.cpu.two", second),
        ],
    );
    await_started(&markers[0], held_runs.child_mut(0));
    await_started(&markers[1], held_runs.child_mut(1));
    let defer: serde_json::Value =
        serde_json::from_slice(&schedule_raw(&workspace, &third, None).stdout).unwrap();
    assert_eq!(
        defer["data"]["recommendation"]["kind"],
        "start_independent_work"
    );
    let prior_revision = defer["data"]["state_revision"].as_str().unwrap().to_owned();
    workspace.work(&[
        "block",
        "--card-id",
        "F-007",
        "--reason",
        "independent work is unavailable",
    ]);
    let wait: serde_json::Value =
        serde_json::from_slice(&schedule_raw(&workspace, &third, None).stdout).unwrap();
    assert_eq!(wait["data"]["recommendation"]["kind"], "wait");
    assert!(
        !schedule_raw(&workspace, &third, Some(&prior_revision))
            .status
            .success(),
        "stale dashboard state must not authorize action"
    );
    for output in held_runs.release_and_reap() {
        assert!(
            output.status.success(),
            "held gate must settle successfully"
        );
    }
    let start: serde_json::Value =
        serde_json::from_slice(&schedule_raw(&workspace, &third, None).stdout).unwrap();
    assert_eq!(start["data"]["recommendation"]["kind"], "start");
}
