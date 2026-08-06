//! #111 frozen proof: `gate abandon`'s guard used to be one `if` testing
//! three disjuncts — wrong actor, already settled, not yet expired — behind
//! one message that named only the first. A holder whose reservation simply
//! had not expired yet read "only the original holder may abandon an
//! expired live reservation" as contradictory, because it grants the one
//! condition they meet (holder) while refusing on conditions the sentence
//! never distinguishes from a description of the reservation's state. This
//! file proves each of the three conditions now produces its own message
//! naming that condition, that the live-and-held-by-you case (the one the
//! motivating operator actually hit) names `gate settle --outcome abandoned`
//! as the working alternative, and that the legitimate case — an expired
//! reservation, abandoned by its own holder — still succeeds.
//!
//! A new file rather than an extension of an existing one, for the same
//! reason #106 and #110 each gave for not folding into a nearby file (see
//! `tests/per_site_recovery.rs` and `tests/interrupted_reservation_recovery.rs`'s
//! own module doc comments): the closest candidate,
//! `tests/validation_reservation_recovery.rs`, already drives `gate abandon`
//! twice, but only to prove #65's expired-reservation generation
//! succession — it checks `!status.success()` on a wrong-actor and a
//! premature abandon and moves on, never inspecting *which* refusal fired.
//! This card's whole guarantee is in the exact text of three distinct
//! messages and one `recovery` field; bolting six message-content
//! assertions onto a 300-line concurrent-recovery-race fixture would bury
//! them the same way #106 and #110 declined to bury theirs.

mod support;

use std::process::{Command, Output};

use support::{Workspace, git};

/// Forces a reservation past its expiry by rewriting `expires_at` directly
/// on disk and committing the edit, mirroring
/// `tests/validation_reservation_recovery.rs`'s private helper of the same
/// name (not reusable from here: it is not exported through `support`).
fn expire(workspace: &Workspace, reservation_id: &str) {
    let path = workspace
        .control
        .join(format!("validation-reservations/{reservation_id}.json"));
    let mut reservation: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    reservation["expires_at"] = serde_json::json!("1970-01-01T00:00:00Z");
    std::fs::write(&path, serde_json::to_vec_pretty(&reservation).unwrap()).unwrap();
    git(&workspace.control, &["add", "-A"]);
    git(
        &workspace.control,
        &["commit", "-q", "-m", "expire reservation fixture"],
    );
}

/// A card with an activated candidate ready for `gate.unit`, the same shape
/// `tests/interrupted_reservation_recovery.rs`'s own `allocated` fixture
/// builds.
fn allocated() -> Workspace {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Gate abandon refusal causes",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);
    workspace
}

/// Reserves `gate.unit` for `F-001` under `actor` and returns the new
/// reservation's id.
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

/// Settles a reservation with a terminal, non-receipt outcome, asserting
/// success. Used to build a settled-but-still-live fixture: unlike `gate
/// abandon`, `gate settle` (`run_settle`, `src/commands/gate.rs`) checks no
/// expiry at all.
fn settle_abandoned(workspace: &Workspace, reservation_id: &str, actor: &str) {
    workspace.gate(&[
        "settle",
        "--reservation-id",
        reservation_id,
        "--outcome",
        "abandoned",
        "--actor",
        actor,
    ]);
}

/// Runs `gate abandon` without asserting the outcome.
fn abandon_raw(workspace: &Workspace, reservation_id: &str, actor: &str) -> Output {
    workspace.gate_raw(&[
        "abandon",
        "--reservation-id",
        reservation_id,
        "--actor",
        actor,
    ])
}

fn envelope_of(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).expect("the JSON envelope")
}

fn describe(output: &Output) -> String {
    format!(
        "{}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Extracts every backtick-delimited span from `text`, in source order.
/// Mirrors `tests/interrupted_reservation_recovery.rs` and
/// `tests/recovery_text.rs`'s own copies of this helper.
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
/// `tests/interrupted_reservation_recovery.rs`'s `names_a_command`.
fn names_a_command(span: &str) -> bool {
    let first_token = span.split_whitespace().next().unwrap_or("");
    !first_token.is_empty() && !first_token.starts_with('-') && !span.contains(':')
}

/// #111 required test 1: a caller who is not the reservation's holder gets a
/// message naming exactly that.
///
/// Fails under §10 mutation 1 (collapse the three messages back into one):
/// the old combined text ("only the original holder may abandon an expired
/// live reservation") also contains "original holder", so the positive
/// assertion alone would not catch a revert — the `expired`/`live` absence
/// assertions are what discriminate, since the old text names both and this
/// condition's own message names neither.
#[test]
fn abandoning_another_actors_reservation_says_so() {
    let workspace = allocated();
    let reservation_id = reserve(&workspace, "holder");

    // Fresh reservation: not settled, not expired. Isolates the wrong-actor
    // condition from the other two.
    let refused = abandon_raw(&workspace, &reservation_id, "other");
    assert!(
        !refused.status.success(),
        "a non-holder must be refused: {}",
        describe(&refused)
    );
    let envelope = envelope_of(&refused);
    assert_eq!(envelope["error"]["code"], "CH-POLICY-INVALID-TRANSITION");
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("original holder"),
        "must name the wrong-actor condition: {message}"
    );
    assert!(
        message.contains(&reservation_id),
        "must name the reservation: {message}"
    );
    assert!(
        !message.contains("expired") && !message.contains("live"),
        "must say nothing about expiry — the old collapsed message did, and \
         a non-holder should learn nothing about a resource they do not \
         hold: {message}"
    );
    assert!(
        !message.contains("settled"),
        "must say nothing about settlement either: {message}"
    );
}

/// #111 required test 2: a reservation that is already settled gets a
/// message naming exactly that, distinct from the wrong-actor message.
///
/// Fails under §10 mutation 1: the old collapsed message never mentions
/// "settled" at all, so this assertion cannot pass against it.
#[test]
fn abandoning_an_already_settled_reservation_says_so() {
    let workspace = allocated();
    let reservation_id = reserve(&workspace, "holder");
    settle_abandoned(&workspace, &reservation_id, "holder");
    expire(&workspace, &reservation_id);

    // Right actor, and now expired too: isolates the already-settled
    // condition from the other two, so only the settled message's own
    // assertions below can be satisfied.
    let refused = abandon_raw(&workspace, &reservation_id, "holder");
    assert!(
        !refused.status.success(),
        "a settled reservation must refuse a second abandon: {}",
        describe(&refused)
    );
    let envelope = envelope_of(&refused);
    assert_eq!(envelope["error"]["code"], "CH-POLICY-INVALID-TRANSITION");
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("already settled"),
        "must name the already-settled condition: {message}"
    );
    assert!(
        message.contains(&reservation_id),
        "must name the reservation: {message}"
    );
    assert!(
        !message.contains("original holder"),
        "must not also claim the actor is wrong — this caller is the real \
         holder: {message}"
    );
    assert!(
        !message.contains("expired") && !message.contains("live"),
        "must not also claim anything about expiry: {message}"
    );
}

/// #111 required test 3: a live (not-yet-expired) reservation, abandoned by
/// its own holder, gets a message naming that condition and a `recovery`
/// naming the command that actually works — `gate settle --outcome
/// abandoned` — proven runnable, not merely present as words. #106's
/// `HarnessError::ControlWithRecovery` puts the command in `recovery`
/// (`&'static str`, cannot interpolate) and leaves the reservation id in
/// `reason` (an owned `String`, already built with `format!`), the pairing
/// #110 established for exactly this kind of split.
///
/// Fails under §10 mutation 1 (collapse): the old message's "not expired" /
/// "still live" wording is absent (it says "an expired live reservation",
/// not "not expired"), and the collapsed error carries no `recovery`
/// override at all, so both the message and the recovery assertions fail.
/// Fails under §10 mutation 2 (invert the expiry check): no refusal is
/// produced at all for a live reservation, so `!refused.status.success()`
/// fails first. Fails under §10 mutation 3 (drop `gate settle` from the
/// live case's recovery): the `recovery.contains("gate settle")` assertion
/// fails.
#[test]
fn abandoning_a_live_reservation_says_so_and_names_gate_settle() {
    let workspace = allocated();
    let reservation_id = reserve(&workspace, "holder");

    // Fresh reservation: right actor, not settled. Isolates the
    // not-yet-expired condition.
    let refused = abandon_raw(&workspace, &reservation_id, "holder");
    assert!(
        !refused.status.success(),
        "an unexpired reservation must refuse gate abandon: {}",
        describe(&refused)
    );
    let envelope = envelope_of(&refused);
    assert_eq!(envelope["error"]["code"], "CH-POLICY-INVALID-TRANSITION");
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("not expired") || message.contains("still live"),
        "must name the not-yet-expired condition: {message}"
    );
    assert!(
        message.contains(&reservation_id),
        "must name the reservation: {message}"
    );
    assert!(
        !message.contains("already settled"),
        "must not also claim the reservation is settled: {message}"
    );
    assert!(
        !message.contains("original holder"),
        "must not also claim the actor is wrong: {message}"
    );

    let recovery = envelope["error"]["recovery"]
        .as_str()
        .expect("a recovery string in the JSON envelope");
    assert!(
        recovery.contains("gate settle"),
        "recovery must name the way out: {recovery}"
    );
    assert!(
        recovery.contains("--outcome abandoned"),
        "recovery must name the exact working invocation, not just the bare \
         command: {recovery}"
    );
    assert!(
        !recovery.contains(&reservation_id),
        "recovery is `&'static str` and cannot interpolate the id — the id \
         belongs in `reason`, which the assertion above already checked: \
         {recovery}"
    );
    assert_ne!(
        recovery, "Move through the documented states in order, or abandon the subject.",
        "an override that happens to equal the code default would not prove anything"
    );

    // Prove the named command is real: extract every backtick span shaped
    // like a command and run each with `--help` appended so `clap` itself
    // validates the subcommand path and flags. Same mechanism
    // `tests/interrupted_reservation_recovery.rs` and
    // `tests/recovery_text.rs` use.
    let command_spans: Vec<&str> = backtick_spans(recovery)
        .into_iter()
        .filter(|span| names_a_command(span))
        .collect();
    assert!(
        !command_spans.is_empty(),
        "recovery names no command in backticks: {recovery}"
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
            "recovery names `{span}`, which is not a real command shape \
             (exit {:?}): {recovery}",
            help.status.code()
        );
    }
}

/// #111 required test 4, the no-false-positive case: the legitimate use —
/// an expired reservation, abandoned by its own holder — must keep working.
/// Without this test, a change that refuses everything (or that never falls
/// through any of the three guards) would still pass tests 1 through 3.
///
/// Fails under §10 mutation 2 (invert the expiry check): the legitimate,
/// now-expired case is refused instead of succeeding.
#[test]
fn abandoning_an_expired_reservation_you_hold_still_succeeds() {
    let workspace = allocated();
    let reservation_id = reserve(&workspace, "holder");
    expire(&workspace, &reservation_id);

    let settled = workspace.gate_json(&[
        "abandon",
        "--reservation-id",
        &reservation_id,
        "--actor",
        "holder",
    ]);
    assert_eq!(
        settled["data"]["settlement"]["outcome"]["kind"],
        "abandoned"
    );
    assert_eq!(
        settled["data"]["settlement"]["reservation_id"],
        reservation_id
    );
}

/// #111 §6/§8: pins the ordering decision — actor checked first — against a
/// case the four required tests above cannot distinguish, because each of
/// them isolates its one condition. Here two of the three are true at once:
/// wrong actor *and* already settled. If the code refused on settlement
/// first, this test would see the already-settled message instead, and a
/// non-holder would have learned a fact about a reservation they do not
/// hold. This is the choice §6 asks to be established and justified, and
/// the standing requirement in §8: since this decision is testable, it gets
/// its own test rather than riding along with the required four, none of
/// which construct a state where two conditions are simultaneously true.
///
/// Fails under a mutation that checks settlement before the actor: the
/// message would name "already settled" instead of "original holder", and
/// the second assertion (no settlement leak) would also fail since the
/// message would contain "already settled".
#[test]
fn wrong_actor_refusal_wins_even_when_the_reservation_is_also_settled() {
    let workspace = allocated();
    let reservation_id = reserve(&workspace, "holder");
    settle_abandoned(&workspace, &reservation_id, "holder");

    let refused = abandon_raw(&workspace, &reservation_id, "other");
    assert!(
        !refused.status.success(),
        "a non-holder must be refused even on a settled reservation: {}",
        describe(&refused)
    );
    let envelope = envelope_of(&refused);
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("original holder"),
        "the wrong-actor condition must be checked (and reported) before \
         settlement, so a non-holder learns nothing else about a \
         reservation they do not hold: {message}"
    );
    assert!(
        !message.contains("already settled"),
        "must not leak the settled fact to a non-holder: {message}"
    );
    let recovery = envelope["error"]["recovery"].as_str().unwrap_or("");
    assert!(
        !recovery.contains("gate settle"),
        "a non-holder gets no gate-settle guidance either — that guidance \
         exists only for the live-and-held-by-you case: {recovery}"
    );
}

/// #111 §6/§8: pins the second half of the ordering decision — settlement
/// checked before expiry — against a case the four required tests cannot
/// distinguish, for the same reason as the test above. Here the right actor
/// holds a reservation that is both already settled and still live: settled
/// immediately, with no call to `expire`, which only works at all because
/// `gate settle` (`run_settle`, `src/commands/gate.rs`) has no expiry check
/// of its own — unlike required test 2 above, which settles a reservation
/// this file has *already* expired, so it never lands in this state. If
/// `run_abandon` checked expiry first, the holder here would be wrongly
/// told the reservation is merely "not yet expired," implying that waiting
/// would help, when the real answer is that it is already over.
///
/// Fails under a mutation that checks expiry before settlement: the message
/// would name "not expired"/"still live" instead of "already settled", and
/// would carry the live case's `gate settle` recovery instead of the
/// settled case's plain code default.
#[test]
fn already_settled_refusal_wins_even_when_the_reservation_is_also_live() {
    let workspace = allocated();
    let reservation_id = reserve(&workspace, "holder");
    settle_abandoned(&workspace, &reservation_id, "holder");

    let refused = abandon_raw(&workspace, &reservation_id, "holder");
    assert!(
        !refused.status.success(),
        "an already-settled reservation must refuse even though it has not \
         expired yet: {}",
        describe(&refused)
    );
    let envelope = envelope_of(&refused);
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("already settled"),
        "settlement is the more definitive fact and must be reported before \
         expiry: {message}"
    );
    assert!(
        !message.contains("not expired") && !message.contains("still live"),
        "must not also claim the reservation is merely live, which would \
         wrongly imply waiting helps: {message}"
    );
    let recovery = envelope["error"]["recovery"].as_str().unwrap_or("");
    assert!(
        !recovery.contains("gate settle"),
        "the already-settled refusal carries no override — it is a plain \
         `HarnessError::Control` — so it must not carry the live case's \
         gate-settle recovery: {recovery}"
    );
}
