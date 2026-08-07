//! #179: `ErrorCode::PolicyNotAccepted` used to answer every refusal —
//! "an actor may not authorize a convergence-budget renewal" as readily as
//! "no acceptance exists for this landing commit" — with one shared
//! recovery string, "Record an acceptance for this exact landing commit
//! before promoting.", which was actively wrong for most of them. B2 (#179
//! §1, §8) is the reproduction that motivated this card: a third
//! independent cold-start operator ran `acceptance record` as an actor
//! their project's final-authorization policy does not list, and was told
//! to do the exact thing they had just done.
//!
//! Every test here drives the real CLI and asserts on text-mode stderr,
//! per #142 §13.1 (an operator reads stderr, not a `HarnessError` value)
//! and mirroring `tests/config_malformed_recovery.rs`'s own convention for
//! the identical mechanism on a different code.
//!
//! # What this file covers, and what it deliberately does not
//!
//! One representative test per #179 §7 group, chosen for the shortest real
//! path to each situation, plus B2 verbatim (group 2). What this file does
//! not attempt: a test for every one of the 29 sites — `tests/
//! policy_not_accepted_coverage.rs` enforces that each carries *a*
//! per-site recovery, structurally, without re-deriving what each one
//! says, the same division of labor `config_malformed_recovery.rs` and
//! `config_malformed_coverage.rs` already establish for `ConfigMalformed`.
//! Group 1 and group 2 are also each proven to hold across files, not just
//! within this one — see `tests/convergence.rs`'s
//! `mod policy_not_accepted_disposition_groups`, which pins the identical
//! two constants against a `disposition renew` refusal, a command with
//! nothing to do with acceptance or promotion.
//!
//! Groups 3 and 4 both hinge on a final integration's own sealed-cycle
//! binding going stale relative to its cycle — the same underlying event
//! (the cycle was resealed with different content after
//! `integration prepare --final` captured its digest), reached at two
//! different points in the lifecycle with two different correct fixes.
//! Both tests below reach it the same way `tests/convergence.rs` and
//! `tests/cycle_model.rs` already do for unrelated staleness checks:
//! editing the cycle record's own file directly (here, its `objective`
//! field, chosen because it is content the digest covers but no earlier
//! check in either call path reads), the same "simulate an edit made
//! outside the governed path" technique `support::Workspace::
//! tamper_cycle_status` names for its own, sibling field.

mod support;

use std::fs;

use support::Workspace;

/// `ErrorCode::PolicyNotAccepted`'s shared table entry (`src/error.rs`,
/// `policy_recovery`'s `PolicyNotAccepted` arm), copied verbatim so a test
/// can assert a site's recovery is *not* this, rather than merely
/// asserting it is non-empty.
const SHARED_FALLBACK_RECOVERY: &str =
    "Record an acceptance for this exact landing commit before promoting.";

/// Runs `change-harness` with `args` in text mode (no `--output json`) and
/// returns stdout/stderr, exactly as an interactive operator would see
/// them.
fn run_text(args: &[&str]) -> std::process::Output {
    Workspace::run(&args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>())
}

/// The `recovery: ...` line from a failed command's text-mode stderr, or a
/// panic naming the full stderr if the line is missing.
fn recovery_line(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    stderr
        .lines()
        .find_map(|line| line.strip_prefix("recovery: "))
        .unwrap_or_else(|| panic!("no `recovery: ` line in stderr:\n{stderr}"))
        .to_owned()
}

/// The `code: ...` line from a failed command's text-mode stderr.
fn code_line(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    stderr
        .lines()
        .find_map(|line| line.strip_prefix("code: "))
        .unwrap_or_else(|| panic!("no `code: ` line in stderr:\n{stderr}"))
        .to_owned()
}

/// Drives a sealed cycle's single card through prepare(--final)/merge/
/// land/verify/review to one reviewed final integration — the shared
/// shape every test below needs before `validate_final_authorization` or
/// `exception_bindings` will even run. Mirrors `tests/exceptions.rs`'s own
/// `reviewed_final`/`drive_to_reviewed_final` fixture exactly, minus its
/// policy installation, which every test below controls for itself.
fn reviewed_final(workspace: &Workspace) -> String {
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "policy recovery",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/**"]);
    workspace.approve_card("F-001", "src/a.rs");
    workspace.cycle(&["seal", "--cycle-id", "C-001"]);
    let id = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
        "--final",
    ])["data"]["integration_id"]
        .as_str()
        .unwrap()
        .to_owned();
    for step in ["merge", "land"] {
        workspace.integration(&[step, "--integration-id", &id, "--actor-id", "coordinator"]);
    }
    workspace.integration(&["verify", "--integration-id", &id, "--actor-id", "verifier"]);
    workspace.integration(&[
        "review",
        "--integration-id",
        &id,
        "--reviewer-actor-id",
        "reviewer",
    ]);
    id
}

/// Installs a final-authorization policy through the governed
/// `project set-final-authorization-policy`, deliberately never
/// hand-writing `project.json` directly — see `tests/exceptions.rs`'s own
/// comment on why every one of its later tests does the same.
fn install_policy(
    workspace: &Workspace,
    authorizer_actor_ids: &[&str],
    exception_triggers: &[&str],
) {
    let policy = serde_json::json!({
        "version": "harness.final-authorization-policy/v1",
        "authorization_unit": "sealed_cycle",
        "authorizer_actor_ids": authorizer_actor_ids,
        "exception_triggers": exception_triggers,
    });
    let path = workspace.root.join("final-authorization-policy.json");
    fs::write(&path, serde_json::to_string_pretty(&policy).unwrap()).unwrap();
    let output = Workspace::run(&[
        "project".into(),
        "set-final-authorization-policy".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--policy".into(),
        path.display().to_string(),
        "--actor".into(),
        "operator".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert!(
        output.status.success(),
        "installing the final-authorization policy failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Rewrites cycle `cycle_id`'s own `objective` field directly on disk,
/// without going through any governed command or commit — the same
/// "edit made outside the governed path" technique
/// `support::Workspace::tamper_cycle_status` uses for its sibling field,
/// applied here to change what the cycle's own canonical digest covers
/// without touching `status` (so every check that reads `status` alone,
/// rather than the full digest, stays unaffected — the isolation this
/// file's tests need to reach the seal-mismatch check specifically,
/// rather than some earlier, differently-coded refusal).
fn tamper_cycle_objective(workspace: &Workspace, cycle_id: &str, objective: &str) {
    let path = workspace.control.join(format!("cycles/{cycle_id}.json"));
    let raw = fs::read_to_string(&path).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    value["objective"] = serde_json::json!(objective);
    fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&value).unwrap()),
    )
    .unwrap();
}

// ---------------------------------------------------------------------
// B2 itself (#179 §1, §8): group 2, the exact reproduction.
// ---------------------------------------------------------------------

#[test]
fn b2_actor_not_authorized_names_the_authorizer_list_and_example_command() {
    let workspace = Workspace::initialized();
    // Installed before the cycle exists: `project set-final-authorization-
    // policy` refuses to change the project's digest while any
    // non-terminal cycle could still be compared against
    // `project_revision` (`CH-POLICY-INVALID-CYCLE`), so the policy must
    // be in place before `reviewed_final` creates one — the same ordering
    // `tests/convergence.rs`'s own `opened_with_disposition_policies`
    // documents for the identical reason.
    install_policy(&workspace, &["owner"], &[]);
    let id = reviewed_final(&workspace);

    let output = run_text(&[
        "acceptance",
        "record",
        "--control",
        workspace.control.to_str().unwrap(),
        "--integration-id",
        &id,
        "--authorizer-actor-id",
        "reviewer",
    ]);

    assert!(
        !output.status.success(),
        "an actor absent from authorizer_actor_ids must refuse"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        stderr.contains("actor reviewer is not configured to authorize final sealed cycles"),
        "not exercising B2's reproduction (#179 §1); got:\n{stderr}"
    );
    assert_eq!(code_line(&output), "CH-POLICY-NOT-ACCEPTED");

    let recovery = recovery_line(&output);
    assert_ne!(
        recovery, SHARED_FALLBACK_RECOVERY,
        "B2's whole complaint was this fallback text describing the exact thing the operator \
         had just attempted; got:\n{stderr}"
    );
    assert!(
        recovery.contains("final_authorization_policy.authorizer_actor_ids"),
        "#179 §8 requires B2's recovery to name the field an operator must check; got: \
         {recovery:?}"
    );
    assert!(
        recovery.contains("project example-final-authorization"),
        "#179 §8 requires B2's recovery to name the command that prints a document containing \
         valid authorizer ids; got: {recovery:?}"
    );
}

// ---------------------------------------------------------------------
// Group 1: no policy configured at all.
// ---------------------------------------------------------------------

#[test]
fn policy_not_configured_names_the_example_and_install_commands() {
    let workspace = Workspace::initialized();
    let id = reviewed_final(&workspace);
    // Deliberately never calls `install_policy`.

    let output = run_text(&[
        "acceptance",
        "record",
        "--control",
        workspace.control.to_str().unwrap(),
        "--integration-id",
        &id,
        "--authorizer-actor-id",
        "anyone",
    ]);

    assert!(
        !output.status.success(),
        "a final-cycle acceptance with no final_authorization_policy configured must refuse"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        stderr.contains("final authorization is not configured for this project"),
        "not exercising the policy-missing site; got:\n{stderr}"
    );
    assert_eq!(code_line(&output), "CH-POLICY-NOT-ACCEPTED");

    let recovery = recovery_line(&output);
    assert_ne!(recovery, SHARED_FALLBACK_RECOVERY, "got:\n{stderr}");
    assert!(
        recovery.contains("final_authorization_policy"),
        "got: {recovery:?}"
    );
    assert!(
        recovery.contains("project example-final-authorization"),
        "got: {recovery:?}"
    );
    assert!(
        recovery.contains("project set-final-authorization-policy"),
        "got: {recovery:?}"
    );
    // Group 2's field-level text must not leak into group 1's refusal:
    // there is no policy to check a list *inside*, so naming
    // `authorizer_actor_ids` here would send the operator hunting through
    // a document that does not exist yet.
    assert!(
        !recovery.contains("authorizer_actor_ids"),
        "a missing-policy refusal must not send the operator to a field inside a policy that \
         does not exist yet; got: {recovery:?}"
    );
}

// ---------------------------------------------------------------------
// Group 3: this final integration's own binding is stale, before any
// decision is recorded — recording is exactly what is being refused, so
// the fix is to re-prepare, not to "record a fresh acceptance".
// ---------------------------------------------------------------------

#[test]
fn integration_seal_stale_before_recording_names_prepare_final() {
    let workspace = Workspace::initialized();
    // See `b2_actor_not_authorized_names_the_authorizer_list_and_example_command`
    // for why the policy must be installed before the cycle exists.
    install_policy(&workspace, &["owner"], &[]);
    let id = reviewed_final(&workspace);
    tamper_cycle_objective(&workspace, "C-001", "resealed with different content");

    let output = run_text(&[
        "acceptance",
        "record",
        "--control",
        workspace.control.to_str().unwrap(),
        "--integration-id",
        &id,
        "--authorizer-actor-id",
        "owner",
    ]);

    assert!(
        !output.status.success(),
        "recording an acceptance against a final integration whose sealed-cycle binding no \
         longer matches its cycle must refuse"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        stderr.contains("no longer binds its sealed cycle"),
        "not exercising the pre-record seal-staleness site; got:\n{stderr}"
    );
    assert_eq!(code_line(&output), "CH-POLICY-NOT-ACCEPTED");

    let recovery = recovery_line(&output);
    assert_ne!(recovery, SHARED_FALLBACK_RECOVERY, "got:\n{stderr}");
    assert!(
        recovery.contains("integration prepare --final"),
        "the fix here is to re-prepare, not to retry recording (recording is exactly what is \
         refused); got: {recovery:?}"
    );
}

// ---------------------------------------------------------------------
// Group 4: an existing decision no longer covers the current state — the
// identical trigger as group 3 (the cycle was resealed), reached instead
// at promotion-recheck time, once an acceptance already exists. The fix
// is a fresh decision, not a re-preparation, and the recovery text must
// say so.
// ---------------------------------------------------------------------

#[test]
fn final_authorization_stale_after_recording_names_acceptance_record() {
    let workspace = Workspace::initialized();
    // See `b2_actor_not_authorized_names_the_authorizer_list_and_example_command`
    // for why the policy must be installed before the cycle exists.
    install_policy(&workspace, &["owner"], &[]);
    let id = reviewed_final(&workspace);

    let record = workspace.acceptance_raw(&[
        "record",
        "--integration-id",
        &id,
        "--authorizer-actor-id",
        "owner",
    ]);
    assert!(
        record.status.success(),
        "recording the acceptance against the untampered cycle must succeed: {}{}",
        String::from_utf8_lossy(&record.stdout),
        String::from_utf8_lossy(&record.stderr)
    );

    // Only now does the cycle go stale — after a valid acceptance already
    // exists, isolating the promotion-recheck site from the pre-record one
    // `integration_seal_stale_before_recording_names_prepare_final` above
    // exercises.
    tamper_cycle_objective(&workspace, "C-001", "resealed again after acceptance");

    let output = run_text(&[
        "integration",
        "promote",
        "--control",
        workspace.control.to_str().unwrap(),
        "--integration-id",
        &id,
        "--actor-id",
        "owner",
    ]);

    assert!(
        !output.status.success(),
        "promoting once the accepted cycle has been resealed must refuse"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        stderr.contains("no longer matches its sealed cycle"),
        "not exercising the promotion-recheck seal-staleness site; got:\n{stderr}"
    );
    assert_eq!(code_line(&output), "CH-POLICY-NOT-ACCEPTED");

    let recovery = recovery_line(&output);
    assert_ne!(recovery, SHARED_FALLBACK_RECOVERY, "got:\n{stderr}");
    assert!(
        recovery.contains("acceptance record"),
        "the fix here is a fresh decision, not a re-preparation (an acceptance already exists); \
         got: {recovery:?}"
    );
    assert!(
        recovery.contains("integration exception raise"),
        "got: {recovery:?}"
    );
    // Group 3's re-prepare advice must not leak into group 4's refusal:
    // re-preparing does nothing for a *promotion* recheck against an
    // acceptance that already exists.
    assert!(
        !recovery.contains("integration prepare"),
        "a stale-existing-decision refusal must not tell the operator to re-prepare; got: \
         {recovery:?}"
    );
}

// ---------------------------------------------------------------------
// Group 5: a policy exists, but this specific exception trigger is not
// among `exception_triggers` — a different field than group 2's, needing
// its own text.
// ---------------------------------------------------------------------

#[test]
fn exception_trigger_not_enabled_names_exception_triggers_field() {
    let workspace = Workspace::initialized();
    // See `b2_actor_not_authorized_names_the_authorizer_list_and_example_command`
    // for why the policy must be installed before the cycle exists.
    install_policy(&workspace, &["owner"], &["policy_change"]);
    let id = reviewed_final(&workspace);

    let output = run_text(&[
        "integration",
        "exception",
        "raise",
        "--control",
        workspace.control.to_str().unwrap(),
        "--integration-id",
        &id,
        "--actor-id",
        "owner",
        "--trigger",
        "critical_residual_risk",
        "--evidence-ref",
        "receipt:R-001",
    ]);

    assert!(
        !output.status.success(),
        "raising a trigger the policy does not declare must refuse"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        stderr.contains("is not enabled by final authorization policy"),
        "not exercising the trigger-not-enabled site; got:\n{stderr}"
    );
    assert_eq!(code_line(&output), "CH-POLICY-NOT-ACCEPTED");

    let recovery = recovery_line(&output);
    assert_ne!(recovery, SHARED_FALLBACK_RECOVERY, "got:\n{stderr}");
    assert!(
        recovery.contains("final_authorization_policy.exception_triggers"),
        "got: {recovery:?}"
    );
    assert!(
        recovery.contains("project set-final-authorization-policy"),
        "got: {recovery:?}"
    );
    // Group 2's field must not leak into group 5's refusal: the actor
    // (`owner`) is fine here, the trigger is the problem.
    assert!(
        !recovery.contains("authorizer_actor_ids"),
        "a trigger-not-enabled refusal must not send the operator to check the actor list; got: \
         {recovery:?}"
    );
}
