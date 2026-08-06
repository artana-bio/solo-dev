//! #106: a refusal site can carry its own recovery.
//!
//! `HarnessError::recovery()` (`src/error.rs`) returns a site's own override
//! when it has one (`HarnessError::ControlWithRecovery`) and
//! `self.code().recovery()` otherwise, and both `render_text_error` and
//! `CommandErrorEnvelope::new` (`src/cli/output.rs`) now ask the *error* for
//! its recovery rather than the *code*. This file is the proof, on the one
//! site #106 migrates: `require_next_gate`'s final-integration refusal in
//! `src/commands/gate.rs`.
//!
//! A new file rather than an extension of an existing one: the closest
//! candidates are `src/cli/output.rs`'s own unit tests, which construct a
//! `HarnessError` directly and would only exercise the general override
//! mechanism a second time — `error.rs`'s own tests already do that job for
//! the type-level plumbing — and `tests/recovery.rs` /
//! `tests/validation_reservation_recovery.rs`, which are about resuming
//! interrupted journaled operations and expired reservations, an unrelated
//! meaning of "recovery". Proving this card's guarantee means triggering
//! the *actual* migrated refusal through the real CLI, on real command
//! fixtures, in both output modes — closer in kind to `tests/cycle_model.rs`
//! and `tests/gate_runner.rs` than to either of those.

mod support;

use support::Workspace;

/// The migrated site's own override text (`src/commands/gate.rs`,
/// `FINAL_INTEGRATION_GATE_RECOVERY`), copied verbatim so a change to either
/// side makes this test fail rather than silently stop comparing anything.
const FINAL_INTEGRATION_RECOVERY: &str = "Run `integration prepare` with `--cycle-id` and `--actor-id` once the card is ready; final-integration checks run automatically during combined verification, not through `gate run`.";

/// `ErrorCode::PolicyInvalidTransition`'s shared table default
/// (`src/error.rs`), copied verbatim for the same reason. Also the exact
/// twelve words #106 quotes as the thing every unmigrated site still says.
const CODE_DEFAULT_RECOVERY: &str =
    "Move through the documented states in order, or abandon the subject.";

/// Drives one card to the exact point where `gate run` on its only
/// final-integration gate (`gate.all`, named by every `activate_card`
/// fixture's default `named_gates.integration`) is refused by
/// `require_next_gate` before it ever reaches the reservation check —
/// the one site #106 migrates onto `HarnessError::ControlWithRecovery`.
fn stage_final_integration_refusal(workspace: &Workspace) {
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/F-001/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);
}

#[test]
fn a_site_with_its_own_recovery_overrides_the_code_default() {
    let workspace = Workspace::initialized();
    stage_final_integration_refusal(&workspace);

    let output = workspace.gate_raw(&["run", "--gate-id", "gate.all", "--card-id", "F-001"]);
    assert!(
        !output.status.success(),
        "gate.all belongs to final integration; running it directly must be refused"
    );
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "CH-POLICY-INVALID-TRANSITION");
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("belongs to final integration"),
        "unexpected refusal, this test is not exercising the migrated site: {envelope}"
    );
    assert_eq!(
        envelope["error"]["recovery"], FINAL_INTEGRATION_RECOVERY,
        "the migrated site's own recovery must reach the JSON envelope"
    );
    assert_ne!(
        envelope["error"]["recovery"], CODE_DEFAULT_RECOVERY,
        "an override that happens to equal the code default would not prove anything"
    );
}

#[test]
fn a_site_without_one_still_gets_the_code_default() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Never activated",
    ]);

    // A draft cycle cannot be sealed: `CycleStatus::check_transition`
    // (`src/domain/cycle.rs`) refuses with `PolicyInvalidTransition` and
    // supplies no override. This is a different construction site from the
    // one #106 migrates, so it must still carry the plain code default,
    // byte for byte.
    let output = workspace.cycle_raw(&["seal", "--cycle-id", "C-001"]);
    assert!(
        !output.status.success(),
        "a draft cycle must refuse sealing"
    );
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "CH-POLICY-INVALID-TRANSITION");
    assert_eq!(envelope["error"]["recovery"], CODE_DEFAULT_RECOVERY);
}

#[test]
fn both_output_modes_agree() {
    let workspace = Workspace::initialized();
    stage_final_integration_refusal(&workspace);

    let json_output = workspace.gate_raw(&["run", "--gate-id", "gate.all", "--card-id", "F-001"]);
    assert!(!json_output.status.success());
    let envelope: serde_json::Value = serde_json::from_slice(&json_output.stdout).unwrap();
    let json_recovery = envelope["error"]["recovery"]
        .as_str()
        .expect("a recovery string in the JSON envelope")
        .to_owned();
    assert_eq!(json_recovery, FINAL_INTEGRATION_RECOVERY);

    // Same refusal, same arguments, no `--output json`: `render_text_error`
    // writes its three lines to stderr (see `main.rs`), prefixing only the
    // first with `error: `, so the `recovery: ` line is found unprefixed.
    let text_output = Workspace::run(&[
        "gate".into(),
        "run".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--gate-id".into(),
        "gate.all".into(),
        "--card-id".into(),
        "F-001".into(),
    ]);
    assert!(!text_output.status.success());
    let stderr = String::from_utf8_lossy(&text_output.stderr).into_owned();
    let text_recovery = stderr
        .lines()
        .find_map(|line| line.strip_prefix("recovery: "))
        .unwrap_or_else(|| panic!("no `recovery: ` line in text-mode stderr:\n{stderr}"))
        .to_owned();

    assert_eq!(
        text_recovery, json_recovery,
        "text and JSON must carry the identical recovery string for the same refusal"
    );
    assert_eq!(text_recovery, FINAL_INTEGRATION_RECOVERY);
}
