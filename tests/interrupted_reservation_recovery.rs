//! #110 frozen proof: an interrupted `gate run` leaves a reservation whose
//! execution permit was committed but never settled, and that state must be
//! visible from `project status` / `project recover`, and the "already
//! acquired" refusal must name the way out.
//!
//! A new file rather than an extension of an existing one, for the same
//! reason #106 gave for creating `tests/per_site_recovery.rs` instead of
//! folding into a nearby file (see that file's module doc comment): every
//! close candidate here means something different by "recovery".
//! `tests/validation_reservation_recovery.rs` is #65's expired-reservation
//! generation succession — this card's own §3 deliberately excludes expiry
//! ("No timestamps, no heuristics, no thresholds"), so that file's premise
//! (an *expired* reservation) does not build the state this card cares
//! about (a *live, unexpired* one whose permit is stranded).
//! `tests/recovery.rs` and `tests/validation_reservation_settlement.rs` are
//! about the `Journal`'s own interrupted-transaction bookkeeping, which
//! `project recover` already reported before this card touched it; the
//! predicate this file proves (`stranded_execution_permits` in
//! `src/commands/gate.rs`) is a separate, parallel mechanism neither of
//! those exercises. `tests/per_site_recovery.rs` proves
//! `HarnessError::ControlWithRecovery` generically on its one existing
//! site (`require_next_gate`'s final-integration refusal); it does not
//! reach `acquire_governed_gate_execution`, the second site this card
//! migrates. This file is the one place all three meet: it builds a
//! stranded reservation and checks what `project status`, `project
//! recover`, and the "already acquired" refusal each say about it.

mod support;

use std::process::{Command, Output};

use support::Workspace;

/// A card with an activated candidate ready for `gate.unit`, the same shape
/// `tests/gate_runner.rs`'s own `allocated` fixture builds.
fn allocated() -> Workspace {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Interrupted reservation proof",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);
    workspace
}

fn reserve(workspace: &Workspace, actor: &str) -> String {
    workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.unit",
        "--actor",
        actor,
    ])["data"]["reservation"]["reservation_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn describe(output: &Output) -> String {
    format!(
        "{}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Runs `gate run` with `CHANGE_HARNESS_FAIL_AT=governed-execution-after-acquire`
/// — the same injection point `tests/validation_reservation_recovery.rs`
/// uses for its own recovery proof (`command_with_post_acquire_failure`
/// there). It fails *after* `acquire_governed_gate_execution`'s transaction
/// commits the execution permit and *before* the gate subprocess ever runs
/// or settles: exactly the phase split the card's §2 describes for a real
/// `SIGINT`, reached here without sending a signal or killing a process.
fn command_with_post_acquire_failure(workspace: &Workspace, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .env("CHANGE_HARNESS_FAIL_AT", "governed-execution-after-acquire")
        .args(["gate", args[0], "--output", "json", "--control"])
        .arg(&workspace.control)
        .args(&args[1..])
        .output()
        .unwrap()
}

/// Reserves `gate.unit` for `F-001` under `actor` and drives it through the
/// acquire step only, leaving a committed execution permit with no
/// settlement: the predicate `stranded_execution_permits`
/// (`src/commands/gate.rs`) exists to find. Returns the reservation id.
fn stranded_reservation(workspace: &Workspace, actor: &str) -> String {
    let reservation_id = reserve(workspace, actor);
    let interrupted = command_with_post_acquire_failure(
        workspace,
        &[
            "run",
            "--card-id",
            "F-001",
            "--gate-id",
            "gate.unit",
            "--reservation-id",
            &reservation_id,
            "--actor",
            actor,
        ],
    );
    assert!(
        !interrupted.status.success(),
        "the injected post-acquire failure must fail the command: {}",
        describe(&interrupted)
    );
    let envelope: serde_json::Value = serde_json::from_slice(&interrupted.stdout).unwrap();
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("deliberate interruption after governed execution acquire"),
        "wrong failure, this fixture did not actually strand a permit: {envelope}"
    );
    reservation_id
}

fn project_status_json(workspace: &Workspace) -> serde_json::Value {
    Workspace::run_json(&[
        "project".into(),
        "status".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--output".into(),
        "json".into(),
    ])
}

fn project_recover_json(workspace: &Workspace) -> serde_json::Value {
    Workspace::run_json(&[
        "project".into(),
        "recover".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--output".into(),
        "json".into(),
    ])
}

/// Extracts every backtick-delimited span from `text`, in source order.
/// Mirrors `tests/recovery_text.rs` and `tests/gate_runner.rs`'s own copies
/// of this helper.
fn backtick_spans(text: &str) -> Vec<&str> {
    let mut spans = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find('`') else {
            break;
        };
        spans.push(&after_open[..close]);
        rest = &after_open[close + 1..];
    }
    spans
}

/// True when a backtick span names a `change-harness` subcommand path
/// rather than a bare flag or a `field: value` mention. Mirrors
/// `tests/recovery_text.rs`'s `names_a_command`.
fn names_a_command(span: &str) -> bool {
    let first_token = span.split_whitespace().next().unwrap_or("");
    !first_token.is_empty() && !first_token.starts_with('-') && !span.contains(':')
}

/// #110 required test 1: a committed acquire with no settlement makes
/// `project status` name that reservation.
///
/// Fails if `project status` stops reading `stranded_execution_permits`, if
/// the predicate stops matching a genuinely stranded permit, or if the
/// reported entry drops the reservation id or holder — mutation 2 in the
/// card's §10 ("report none") is exactly the first of these.
#[test]
fn an_interrupted_run_leaves_a_reservation_project_status_names() {
    let workspace = allocated();
    let reservation_id = stranded_reservation(&workspace, "holder");

    let status = project_status_json(&workspace);
    let stranded = status["data"]["stranded_reservations"]
        .as_array()
        .expect("stranded_reservations must be an array");
    assert_eq!(
        stranded.len(),
        1,
        "exactly one reservation is stranded: {status}"
    );
    assert_eq!(stranded[0]["reservation_id"], reservation_id);
    assert_eq!(stranded[0]["holder_actor_id"], "holder");
}

/// #110 required test 2: the "already acquired" refusal names
/// `gate settle`, in its `recovery` field, as a real, runnable command.
///
/// Fails if the refusal reverts to a plain `HarnessError::Control` (no
/// recovery override — mutation 3 in the card's §10), if the recovery text
/// stops mentioning `gate settle`, or if it names a command shape `clap`
/// itself rejects.
#[test]
fn the_already_acquired_refusal_names_the_way_out() {
    let workspace = allocated();
    let reservation_id = stranded_reservation(&workspace, "holder");

    // Retrying the same run now hits `acquire_governed_gate_execution`'s
    // "already acquired" refusal: the permit committed above is still
    // there, and nothing settled it.
    let retried = workspace.gate_raw(&[
        "run",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.unit",
        "--reservation-id",
        &reservation_id,
        "--actor",
        "holder",
    ]);
    assert!(
        !retried.status.success(),
        "a stranded reservation must refuse a second acquire"
    );
    let envelope: serde_json::Value = serde_json::from_slice(&retried.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "CH-POLICY-INVALID-TRANSITION");
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("already acquired"),
        "wrong refusal, this test is not exercising the migrated site: {envelope}"
    );

    let recovery = envelope["error"]["recovery"]
        .as_str()
        .expect("a recovery string in the JSON envelope");
    assert!(
        recovery.contains("gate settle"),
        "recovery must name the way out: {recovery}"
    );
    assert_ne!(
        recovery, "Move through the documented states in order, or abandon the subject.",
        "an override that happens to equal the code default would not prove anything"
    );

    // Prove the named command is real, the same mechanism
    // `tests/recovery_text.rs` and `tests/gate_runner.rs` use: extract every
    // backtick span, keep the ones shaped like a command, and run each with
    // `--help` appended so `clap` validates the subcommand path and flags.
    let command_spans: Vec<&str> = backtick_spans(recovery)
        .into_iter()
        .filter(|span| names_a_command(span))
        .collect();
    assert!(
        !command_spans.is_empty(),
        "recovery text names no command in backticks: {recovery}"
    );
    for span in command_spans {
        let args: Vec<&str> = span.split_whitespace().collect();
        let help = Command::new(env!("CARGO_BIN_EXE_change-harness"))
            .args(&args)
            .arg("--help")
            .output()
            .expect("the CLI binary should start");
        assert!(
            help.status.success(),
            "recovery names `{span}`, which is not a real command shape (exit {:?}): {recovery}",
            help.status.code()
        );
    }
}

/// #110 required test 3, the no-false-positive test: a normally settled
/// reservation is never named as stranded.
///
/// Fails if `project status`/`project recover` stop checking settlement at
/// all — mutation 1 in the card's §10 ("report every reservation, settled
/// or not") — which would make a routine, healthy `gate run` look exactly
/// like an interrupted one.
#[test]
fn a_settled_reservation_is_never_reported() {
    let workspace = allocated();
    // A normal, healthy run: reserve, then run to completion. `gate.unit`
    // is registered as `["true"]`, so it always succeeds, and the governed
    // path settles the reservation as part of the same command — no
    // interruption anywhere in this path.
    let reservation_id = reserve(&workspace, "holder");
    let run = workspace.gate_raw(&[
        "run",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.unit",
        "--reservation-id",
        &reservation_id,
        "--actor",
        "holder",
    ]);
    assert!(
        run.status.success(),
        "a healthy run must succeed: {}",
        describe(&run)
    );

    let status = project_status_json(&workspace);
    let stranded = status["data"]["stranded_reservations"]
        .as_array()
        .expect("stranded_reservations must be an array");
    assert!(
        stranded.is_empty(),
        "a settled reservation must never be named as stranded: {status}"
    );

    let recover = project_recover_json(&workspace);
    assert_eq!(recover["data"]["recovery_required"], false);
    assert_eq!(
        recover["data"]["stranded_reservations"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

/// #110 §8: pins the §7.2 decision that `project recover` only *reports* a
/// stranded reservation and never clears it — clearing would mean
/// asserting the process that acquired the permit is dead, which nothing
/// on disk tells the Harness (§6: a `SIGKILL`, a power cut, and a closed
/// laptop all leave the identical state). This is the one choice in this
/// card genuinely left open by "establish and report", so it gets its own
/// test rather than riding along with the three required ones above: none
/// of those call `project recover` and then check whether the fact
/// survived.
///
/// Also pins the one guarantee this card makes that no other test
/// checks: `project recover`'s text names *this exact reservation's*
/// settle command with its real id filled in, not merely the words
/// `gate settle` in the abstract. An earlier version of this test
/// asserted only structural properties (head unchanged, still stranded,
/// retry still refused) and stayed green even after the guidance text
/// was stripped down to "release reservation {id} yourself" — silent
/// about whether the operator was actually told anything useful.
///
/// Fails if a future change makes `recover` delete the permit file,
/// synthesize a settlement for it, or otherwise write to control on the
/// plain (non-`--resume`) reporting path (first half); or if it stops
/// naming this reservation's real id in a runnable `gate settle` line,
/// whether by dropping the command entirely or by leaving the id generic
/// the way the refusal's `&'static str` recovery has to (second half).
#[test]
fn recover_reports_a_stranded_reservation_without_clearing_it() {
    let workspace = allocated();
    let reservation_id = stranded_reservation(&workspace, "holder");
    let before_head = workspace.control_head();

    let recover = project_recover_json(&workspace);
    assert_eq!(recover["data"]["recovery_required"], true);
    let stranded = recover["data"]["stranded_reservations"]
        .as_array()
        .expect("stranded_reservations must be an array");
    assert_eq!(stranded.len(), 1);
    assert_eq!(stranded[0]["reservation_id"], reservation_id);

    // `recover` must not have written anything to control while reporting.
    assert_eq!(
        workspace.control_head(),
        before_head,
        "project recover must not write to control while merely reporting"
    );
    // ...and the permit itself must still be exactly as stranded as before.
    let status_after = project_status_json(&workspace);
    assert_eq!(
        status_after["data"]["stranded_reservations"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "recover must not have cleared the stranded permit: {status_after}"
    );

    // The operator still cannot re-acquire it: recovery reported the fact,
    // it did not fix it.
    let retried = workspace.gate_raw(&[
        "run",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.unit",
        "--reservation-id",
        &reservation_id,
        "--actor",
        "holder",
    ]);
    assert!(
        !retried.status.success(),
        "recover must not have silently cleared the reservation for reuse"
    );

    // The claim this card actually makes for `project recover`: unlike the
    // refusal's `recovery` field, which is `&'static str` and can only ever
    // name the command shape (see `the_already_acquired_refusal_names_the_way_out`
    // above), `project recover`'s own text is an ordinary `String` and must
    // hand the operator this exact reservation's fully worked settle line,
    // id filled in — not just the words `gate settle` somewhere in the
    // output. `CommandOutcome` does not derive `Serialize`
    // (`src/cli/output.rs`) and `render(OutputFormat::Json)` builds a
    // separate `CommandResult` envelope with no `text` field at all, so
    // this can only be observed in text mode; the JSON assertions above
    // never touch it.
    let text_output = Workspace::run(&[
        "project".into(),
        "recover".into(),
        "--control".into(),
        workspace.control.display().to_string(),
    ]);
    assert!(
        text_output.status.success(),
        "project recover (text mode) must succeed: {}",
        describe(&text_output)
    );
    let text = String::from_utf8_lossy(&text_output.stdout).into_owned();
    let expected_command =
        format!("gate settle --reservation-id {reservation_id} --outcome abandoned");
    assert!(
        text.contains(&expected_command),
        "project recover must hand the operator this reservation's exact, runnable settle \
         command (`{expected_command}`), not merely mention `gate settle` in the abstract: {text}"
    );
}
