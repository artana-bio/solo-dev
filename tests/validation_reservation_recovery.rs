//! #65 frozen proof: expired reservation recovery creates one linked successor.

mod support;

use std::{
    fs,
    path::Path,
    process::{Command, Output},
    sync::{Arc, Barrier},
    thread,
    time::{Duration, Instant},
};

use support::Workspace;

fn marker_gate(workspace: &Workspace, gate_id: &str, marker: &std::path::Path, verdict: &str) {
    let command = format!("printf invoked > \"$MARKER_PATH\"; {verdict}");
    let argv = serde_json::to_string(&["sh", "-c", &command]).unwrap();
    let marker = serde_json::to_string(&marker.display().to_string()).unwrap();
    let body = format!(
        "schema: harness.gate/v1\ngate_id: {gate_id}\nrevision: 1\nargv: {argv}\nworking_directory: \".\"\ntimeout_seconds: 60\nenvironment:\n  allow: [PATH]\n  set:\n    MARKER_PATH: {marker}\nnetwork_policy: denied\nretry_policy:\n  max_attempts: 1\nartifacts: []\n",
    );
    let definition = workspace.gate_definition(gate_id, &body);
    workspace.gate(&["register", "--definition", &definition]);
}

fn expire(workspace: &Workspace, reservation_id: &str) {
    let path = workspace
        .control
        .join(format!("validation-reservations/{reservation_id}.json"));
    let mut reservation: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    reservation["expires_at"] = serde_json::json!("1970-01-01T00:00:00Z");
    fs::write(&path, serde_json::to_vec_pretty(&reservation).unwrap()).unwrap();
    assert!(
        Command::new("git")
            .args(["-C", workspace.control.to_str().unwrap(), "add", "-A"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args([
                "-C",
                workspace.control.to_str().unwrap(),
                "commit",
                "-m",
                "expire reservation fixture",
            ])
            .status()
            .unwrap()
            .success()
    );
}

fn reserve_process(control: &std::path::Path, actor: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .args([
            "gate",
            "reserve",
            "--output",
            "json",
            "--control",
            control.to_str().unwrap(),
            "--card-id",
            "F-001",
            "--gate-id",
            "gate.named",
            "--actor",
            actor,
        ])
        .output()
        .unwrap()
}

/// Only the bounded, administrative project-lock refusal is a tolerable
/// outcome for the losing side of the recovery race. A policy or validation
/// failure must stay visible to the test immediately.
fn is_project_lock_refusal(stdout: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(stdout)
        .ok()
        .and_then(|envelope| envelope["error"]["code"].as_str().map(str::to_owned))
        .as_deref()
        == Some("CH-POLICY-LOCK-HELD")
}

fn describe(output: &Output) -> String {
    format!(
        "{}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Re-runs one contender's reserve after its administrative lock refusal,
/// retrying only that refusal within a bound. Any other outcome — success,
/// a different error, or exhaustion — returns immediately.
fn reserve_after_lock_refusal(control: &Path, actor: &str) -> Output {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let output = reserve_process(control, actor);
        if output.status.success()
            || !is_project_lock_refusal(&output.stdout)
            || Instant::now() >= deadline
        {
            return output;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn command_with_post_acquire_failure(workspace: &Workspace, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .env("CHANGE_HARNESS_FAIL_AT", "governed-execution-after-acquire")
        .args(["gate", args[0], "--output", "json", "--control"])
        .arg(&workspace.control)
        .args(&args[1..])
        .output()
        .unwrap()
}

#[test]
fn holder_abandons_only_expired_reservations_and_one_linked_generation_recovers() {
    let workspace = Workspace::initialized();
    let named_marker = workspace.root.join("named-ran");
    let mutation_marker = workspace.root.join("mutation-ran");
    marker_gate(&workspace, "gate.named", &named_marker, "true");
    marker_gate(&workspace, "gate.mutation", &mutation_marker, "false");
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Recover one expired reservation",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gates("F-001", &["src/named/**"], &["gate.named"]);
    workspace.activate_card_with_gates("F-002", &["src/mutation/**"], &["gate.mutation"]);
    workspace.work(&["start", "--card-id", "F-001"]);
    workspace.work(&["start", "--card-id", "F-002"]);

    let campaign = workspace.root.join("campaign.json");
    fs::write(
        &campaign,
        r#"{"schema":"harness.declared-mutation-campaign/v1","mutations":[{"id":"M-001","path":"README.md","expected_utf8":"hello\n","replacement_utf8":"mutant\n"}]}"#,
    )
    .unwrap();
    let named = workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.named",
        "--actor",
        "holder",
    ]);
    let mutation = workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-002",
        "--gate-id",
        "gate.mutation",
        "--execution-mode",
        "declared-mutations",
        "--campaign",
        campaign.to_str().unwrap(),
        "--actor",
        "holder",
    ]);
    let named_id = named["data"]["reservation"]["reservation_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mutation_id = mutation["data"]["reservation"]["reservation_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let premature = workspace.gate_raw(&[
        "abandon",
        "--reservation-id",
        &named_id,
        "--actor",
        "holder",
    ]);
    assert!(
        !premature.status.success(),
        "a live unexpired permit is not abandonable"
    );
    expire(&workspace, &named_id);
    expire(&workspace, &mutation_id);

    let wrong_holder =
        workspace.gate_raw(&["abandon", "--reservation-id", &named_id, "--actor", "other"]);
    assert!(
        !wrong_holder.status.success(),
        "only the original holder may abandon"
    );

    for reservation_id in [&named_id, &mutation_id] {
        let abandoned = workspace.gate_json(&[
            "abandon",
            "--reservation-id",
            reservation_id,
            "--actor",
            "holder",
        ]);
        assert_eq!(
            abandoned["data"]["settlement"]["outcome"]["kind"],
            "abandoned"
        );
    }

    let old_run = command_with_post_acquire_failure(
        &workspace,
        &[
            "run",
            "--card-id",
            "F-001",
            "--gate-id",
            "gate.named",
            "--reservation-id",
            &named_id,
            "--actor",
            "holder",
        ],
    );
    let old_mutate = command_with_post_acquire_failure(
        &workspace,
        &[
            "mutate",
            "--reservation-id",
            &mutation_id,
            "--campaign",
            campaign.to_str().unwrap(),
            "--actor",
            "holder",
        ],
    );
    assert!(!old_run.status.success());
    assert!(!old_mutate.status.success());
    assert!(!named_marker.exists());
    assert!(!mutation_marker.exists());

    let barrier = Arc::new(Barrier::new(3));
    let run = |actor: &'static str| {
        let barrier = Arc::clone(&barrier);
        let control = workspace.control.clone();
        thread::spawn(move || {
            barrier.wait();
            reserve_process(&control, actor)
        })
    };
    let first = run("recover-a");
    let second = run("recover-b");
    barrier.wait();
    let first = first.join().unwrap();
    let second = second.join().unwrap();
    // The project lock is fail-fast: under load a contender can exhaust the
    // binary's bounded internal retry and exit with the administrative
    // CH-POLICY-LOCK-HELD refusal instead of a decision. That contender is
    // the waiter. Every other failure is a real one.
    for (actor, output) in [("recover-a", &first), ("recover-b", &second)] {
        assert!(
            output.status.success() || is_project_lock_refusal(&output.stdout),
            "reserve ({actor}) failed beyond the administrative lock refusal: {}",
            describe(output)
        );
    }
    assert!(
        first.status.success() || second.status.success(),
        "the lock admits one contender, so at most one refusal\nfirst: {}\nsecond: {}",
        describe(&first),
        describe(&second)
    );
    let observe = |actor: &'static str, output: Output| -> serde_json::Value {
        let output = if output.status.success() {
            output
        } else {
            // Classified above as the lock refusal: the race is settled, so
            // the refused contender must now observe the durable winner.
            let observed = reserve_after_lock_refusal(&workspace.control, actor);
            assert!(
                observed.status.success(),
                "the refused contender ({actor}) must observe the winner: {}",
                describe(&observed)
            );
            observed
        };
        serde_json::from_slice(&output.stdout).expect("the JSON envelope")
    };
    let first = observe("recover-a", first);
    let second = observe("recover-b", second);
    let (winner, waiter) = if first["data"]["disposition"]["kind"] == "reserved" {
        (first, second)
    } else {
        (second, first)
    };
    assert_eq!(winner["data"]["disposition"]["kind"], "reserved");
    assert_eq!(
        waiter["data"]["disposition"]["kind"],
        "wait_for_reserved_run"
    );
    assert_eq!(
        winner["data"]["reservation"]["reservation_id"],
        waiter["data"]["reservation"]["reservation_id"],
    );
    assert_eq!(winner["data"]["reservation"]["generation"], 2);
    assert_eq!(
        winner["data"]["reservation"]["predecessor_reservation_id"],
        named_id,
    );
    assert_eq!(
        fs::read_dir(workspace.control.join("validation-reservations"))
            .unwrap()
            .count(),
        3,
        "concurrent recovery must create exactly one next generation",
    );
    let predecessor: serde_json::Value = serde_json::from_slice(
        &fs::read(
            workspace
                .control
                .join(format!("validation-reservations/{named_id}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(predecessor["generation"], 1);
    assert!(predecessor["predecessor_reservation_id"].is_null());
}

#[test]
fn only_the_project_lock_refusal_is_a_tolerable_concurrent_outcome() {
    assert!(is_project_lock_refusal(
        br#"{"error":{"code":"CH-POLICY-LOCK-HELD"}}"#
    ));
    assert!(!is_project_lock_refusal(
        br#"{"error":{"code":"CH-POLICY-INVALID-TRANSITION"}}"#
    ));
    assert!(!is_project_lock_refusal(b"not a command envelope"));
}
