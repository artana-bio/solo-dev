//! #112 acceptance: a landed-but-unverified integration says what to run
//! next.
//!
//! `integration land` builds a real artifact and writes a ref, but it is not
//! a transition — the integration's status stays `prepared` before and
//! after. An operator who does not already know `integration verify` exists
//! had nothing telling them so: three different refusals all said `prepared`
//! and stopped there, and `inspect` reported the same status a never-merged
//! integration would. This file proves the fix from the operator's side: any
//! of the three refusals, or `inspect`, now says what to run next.
//!
//! A new file rather than an extension of an existing one. `per_site_recovery.rs`
//! is the closest candidate by mechanism — two of these three sites are new
//! `HarnessError::ControlWithRecovery` construction sites, exactly its
//! stated subject — but its own doc comment scopes it to
//! "a refusal site can carry its own recovery," and this card's third
//! required test (`inspect_distinguishes_a_landed_integration_from_an_unlanded_one`)
//! is not a refusal at all; it is `inspect`'s success path. Stretching that
//! file's stated boundary to cover a non-refusal test would blur the exact
//! distinction its own comment draws. `landing_commit.rs` is the other
//! candidate — it already drives `inspect` past a landing commit — but its
//! subject is `WP-430` landing commit *construction*; this file's subject is
//! what an operator reads *after* land, with an unrelated fixture shape
//! (stopped short of `verify`, not stopped short of `land`). All three
//! required tests share one fixture (prepared, merged, landed, not verified)
//! and one guarantee (#112 §3), so they belong together in one new file
//! rather than split across those two.
//!
//! Test 2, `a_generic_invalid_transition_does_not_claim_verification_is_needed`,
//! lives in `src/domain/integration.rs`'s own `mod tests` instead of here.
//! `check_transition` is a pure function of two `IntegrationStatus` values,
//! and the property it proves — an unrelated transition's recovery must not
//! mention verification — needs no control repository, no candidate
//! repository, and no CLI process to establish. Driving it through this
//! file's `Workspace` fixture would spend a full CLI round trip on something
//! a bare enum call already proves faster and just as precisely.

mod support;

use support::Workspace;

/// Site 2's own override (`src/commands/acceptance.rs`,
/// `ACCEPTANCE_NEEDS_VERIFICATION_RECOVERY`), copied verbatim so a change to
/// either side makes this test fail rather than silently stop comparing
/// anything — the same convention `per_site_recovery.rs` uses for the
/// original #106 site.
const ACCEPTANCE_RECOVERY: &str = "Run `integration verify` before recording acceptance.";

/// Site 3's own override (`src/commands/integration.rs`, inline in
/// `check_promotion`), copied verbatim for the same reason.
const PROMOTE_RECOVERY: &str = "Run `integration verify` before promotion can be authorized.";

/// `ErrorCode::PolicyInvalidTransition`'s shared table default
/// (`src/error.rs`), copied verbatim — the same convention `per_site_recovery.rs`
/// uses to prove an override is genuinely conditional, not the default
/// wearing the override's clothes.
const CODE_DEFAULT_RECOVERY: &str =
    "Move through the documented states in order, or abandon the subject.";

/// One approved, merged card, ready for `integration prepare`.
fn prepare_one_approved_card(workspace: &Workspace) {
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/F-001/**"]);
    workspace.approve_card("F-001", "src/F-001/a.rs");
}

/// A workspace with one integration that is `prepared`, merged, and landed,
/// but not verified — exactly what `integration land` leaves behind, and the
/// state this card exists to give guidance about.
fn landed_but_unverified() -> (Workspace, String) {
    let workspace = Workspace::initialized();
    prepare_one_approved_card(&workspace);
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
    workspace.integration(&[
        "merge",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
    ]);
    workspace.integration(&["land", "--integration-id", &id, "--actor-id", "coordinator"]);
    (workspace, id)
}

fn error_envelope(output: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).expect("an error envelope")
}

/// Test 1 (§9): all three refusals from #112 §2's transcript guide the
/// operator toward `integration verify`, asserted separately so one
/// regressing cannot hide behind another passing (§10 mutation 1 reverts
/// exactly one refusal and checks that only its own assertion fails).
#[test]
fn each_unverified_refusal_names_what_to_run_next() {
    let (workspace, id) = landed_but_unverified();

    // The fixture must actually reproduce #112 §2's transcript before
    // trusting anything downstream of it: still `prepared`, with a landing
    // commit already built.
    let inspected = workspace.integration_json(&["inspect", "--integration-id", &id]);
    assert_eq!(inspected["data"]["status"], "prepared");
    assert!(
        inspected["data"]["landing_sha"].is_string(),
        "fixture must have a landing commit before any of the three refusals are meaningful: {inspected}"
    );

    // --- Site 1: `integration review` (domain/integration.rs, check_transition) ---
    let review = workspace.integration_raw(&[
        "review",
        "--integration-id",
        &id,
        "--reviewer-actor-id",
        "reviewer",
    ]);
    assert!(
        !review.status.success(),
        "a prepared integration cannot move to reviewed"
    );
    let envelope = error_envelope(&review);
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("cannot move from prepared to reviewed"),
        "unexpected refusal, this is not site 1: {envelope}"
    );
    let site1_recovery = envelope["error"]["recovery"]
        .as_str()
        .expect("a recovery string")
        .to_lowercase();
    // Deliberately a loose "verif" stem, not the exact site 1 wording: site
    // 1 is a domain-layer guard that names permitted successor *states*
    // (see #112 §6's layering decision), so its text says `verified`, the
    // state — not `integration verify`, the command, which sites 2 and 3
    // say directly. A stem match is also what keeps this assertion passing
    // under §10 mutation 2 (site 1 made to unconditionally say `integration
    // verify`), which is supposed to fail test 2, not this one.
    assert!(
        site1_recovery.contains("verif"),
        "site 1 (integration review): recovery must point toward verification, got {site1_recovery:?}"
    );

    // --- Site 2: `acceptance record` (commands/acceptance.rs, require_reviewed) ---
    let acceptance = workspace.acceptance_raw(&[
        "record",
        "--integration-id",
        &id,
        "--authorizer-actor-id",
        "owner",
        "--rollback-reference",
        "revert",
    ]);
    assert!(
        !acceptance.status.success(),
        "a prepared integration cannot be accepted"
    );
    let envelope = error_envelope(&acceptance);
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("acceptance follows an integration review"),
        "unexpected refusal, this is not site 2: {envelope}"
    );
    assert_eq!(
        envelope["error"]["recovery"], ACCEPTANCE_RECOVERY,
        "site 2 (acceptance record): recovery must be its own override, verbatim"
    );

    // --- Site 3: `integration promote` (commands/integration.rs, check_promotion) ---
    let promote = workspace.integration_raw(&[
        "promote",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
    ]);
    assert!(
        !promote.status.success(),
        "a prepared integration cannot be promoted"
    );
    let envelope = error_envelope(&promote);
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("only an accepted integration may be promoted"),
        "unexpected refusal, this is not site 3: {envelope}"
    );
    assert_eq!(
        envelope["error"]["recovery"], PROMOTE_RECOVERY,
        "site 3 (integration promote): recovery must be its own override, verbatim"
    );
}

/// §8: sites 2 and 3 gate their override on `record.status ==
/// IntegrationStatus::Prepared && record.landing_sha.is_some()` — not on
/// status alone. That second condition is a discretionary choice the
/// contract left open (§6 calls sites 2 and 3 "straightforward" but does
/// not specify the exact gate), and nothing in
/// `each_unverified_refusal_names_what_to_run_next` can catch it being
/// dropped: that test's fixture always has a landing commit by the time it
/// checks any refusal, so `status == Prepared` alone would pass it too.
/// This test is the one that would fail if `status_gate_refusal`'s
/// `landing_sha.is_some()` clause were removed: a merely-prepared
/// integration (no merge, no land) must still get the plain code default,
/// not the verify-specific override, because `integration verify` would
/// itself immediately refuse with "no landing commit" — different, unhelpful
/// advice for an operator who has not even merged yet.
#[test]
fn an_unlanded_prepared_integration_keeps_the_code_default_recovery() {
    let workspace = Workspace::initialized();
    prepare_one_approved_card(&workspace);
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

    // Deliberately no `integration merge` and no `integration land`:
    // `landing_sha` stays `None` while `status` is already `Prepared`.
    let inspected = workspace.integration_json(&["inspect", "--integration-id", &id]);
    assert_eq!(inspected["data"]["status"], "prepared");
    assert!(
        inspected["data"]["landing_sha"].is_null(),
        "fixture must not have a landing commit: {inspected}"
    );

    let acceptance = workspace.acceptance_raw(&[
        "record",
        "--integration-id",
        &id,
        "--authorizer-actor-id",
        "owner",
        "--rollback-reference",
        "revert",
    ]);
    assert!(!acceptance.status.success());
    let envelope = error_envelope(&acceptance);
    assert_eq!(
        envelope["error"]["recovery"], CODE_DEFAULT_RECOVERY,
        "site 2 must not apply its override to an unlanded prepared integration: {envelope}"
    );

    let promote = workspace.integration_raw(&[
        "promote",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
    ]);
    assert!(!promote.status.success());
    let envelope = error_envelope(&promote);
    assert_eq!(
        envelope["error"]["recovery"], CODE_DEFAULT_RECOVERY,
        "site 3 must not apply its override to an unlanded prepared integration: {envelope}"
    );
}

/// Test 3 (§9): `inspect` reports something different for a landed
/// integration than for an unlanded one — the same status (`prepared`)
/// either way, since `land` is not a transition, so the distinction has to
/// come from somewhere other than `status`.
#[test]
fn inspect_distinguishes_a_landed_integration_from_an_unlanded_one() {
    let workspace = Workspace::initialized();
    prepare_one_approved_card(&workspace);
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
    workspace.integration(&[
        "merge",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
    ]);

    // Unlanded: merged, but `integration land` has not run yet.
    let unlanded = workspace.integration_json(&["inspect", "--integration-id", &id]);
    assert_eq!(unlanded["data"]["status"], "prepared");
    assert!(
        unlanded["data"]["landing_sha"].is_null(),
        "fixture must not have a landing commit yet: {unlanded}"
    );
    assert!(
        unlanded["data"]["next_permitted_action"].is_null(),
        "an unlanded integration must not claim any next action: {unlanded}"
    );

    workspace.integration(&["land", "--integration-id", &id, "--actor-id", "coordinator"]);

    // Landed: same status as above — `land` is not a transition — but a
    // different report.
    let landed = workspace.integration_json(&["inspect", "--integration-id", &id]);
    assert_eq!(
        landed["data"]["status"], "prepared",
        "land must not be a transition"
    );
    assert!(
        landed["data"]["landing_sha"].is_string(),
        "fixture must have a landing commit now: {landed}"
    );
    assert_eq!(
        landed["data"]["next_permitted_action"], "integration.verify",
        "a landed integration must name integration.verify as the next action: {landed}"
    );

    // The property under test, stated directly: landed and unlanded must not
    // read the same. §10 mutation 3 (report the same thing for both) fails
    // exactly this assertion.
    assert_ne!(
        unlanded["data"]["next_permitted_action"], landed["data"]["next_permitted_action"],
        "inspect reported the same next_permitted_action for a landed and an unlanded integration"
    );

    // Also visible in the text rendering an operator actually reads, not
    // only in the JSON field a script would check.
    let landed_text = Workspace::run(&[
        "integration".into(),
        "inspect".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--integration-id".into(),
        id.clone(),
    ]);
    assert!(landed_text.status.success());
    let stdout = String::from_utf8_lossy(&landed_text.stdout).into_owned();
    assert!(
        stdout.contains("next action: integration.verify"),
        "text-mode inspect must also name the next action:\n{stdout}"
    );
}
