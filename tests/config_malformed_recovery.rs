//! #142: `CH-CONFIG-MALFORMED` used to answer three unrelated failure modes
//! — an unreadable file, a syntax error, and a schema error — with one
//! shared recovery string, "Correct the JSON or YAML syntax of the
//! document.", which was actively wrong for two of the three. B1 (#142 §1)
//! is the reproduction that motivated this card: a second independent
//! cold-start operator hit a missing `authorization_unit` field — valid
//! JSON, invalid schema — and was told to fix syntax that was never broken.
//!
//! Every test here drives the real CLI and asserts on stderr, per #142
//! §13.1: an operator reads stderr, not a `HarnessError` value, so a test on
//! the value proves nothing about what the operator actually sees.
//!
//! # What this file covers, and what it deliberately does not
//!
//! One test per §3 failure mode (read, syntax, schema), including B1
//! verbatim. Two more tests each pin a design decision #142 §10/§12 asks to
//! ship with its own test:
//!
//! - [`gate_definition_recovery_is_identical_for_a_syntax_and_a_schema_failure`]
//!   pins the choice *not* to split a YAML site's recovery by failure mode,
//!   because `serde_yaml_ng` (v0.10.0) exposes no public equivalent of
//!   `serde_json::Error::classify()` — see `GATE_DEFINITION_PARSE_RECOVERY`'s
//!   own doc comment in `src/commands/gate.rs` for where that was
//!   confirmed, in the vendored crate source itself, not assumed.
//! - [`project_validate_missing_config_file_still_gets_the_shared_fallback`]
//!   pins the choice to leave `HarnessError::Config` sites unconverted,
//!   because that variant has no per-site recovery mechanism today — see
//!   `run_validate`'s own doc comment in `src/commands/project.rs`.
//!
//! What this file does not attempt: a test for every one of the 26 sites
//! #142 converted (`tests/config_malformed_coverage.rs` enforces that each
//! carries *a* per-site recovery, structurally, without re-deriving what
//! each one says) or for the two `src/commands/gate.rs` sites left on the
//! shared fallback because they are not a file-read or parse call at all (a
//! `u32` overflow guard needing roughly four billion campaign entries to
//! reach, and a defensive empty-list check already unreachable behind an
//! earlier guard) — neither is practically constructible through the real
//! CLI, and a contrived unit test that never fires for an operator would
//! prove less than the comment already sitting at each site.

mod support;

use std::fs;

use support::Workspace;

/// `ErrorCode::ConfigMalformed`'s shared table entry (`src/error.rs:584`),
/// copied verbatim so a test can assert a site's recovery is *not* this,
/// rather than merely asserting it is non-empty — the exact distinction
/// #142 §13 draws ("`stderr contains \"recovery:\"` passes today and proves
/// nothing").
const SHARED_FALLBACK_RECOVERY: &str = "Correct the JSON or YAML syntax of the document.";

/// Runs `change-harness` with `args` in text mode (no `--output json`) and
/// returns stdout/stderr, exactly as an interactive operator would see them.
fn run_text(args: &[&str]) -> std::process::Output {
    Workspace::run(&args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>())
}

/// The `recovery: ...` line from a failed command's text-mode stderr, or a
/// panic naming the full stderr if the line is missing — matching
/// `tests/per_site_recovery.rs`'s `both_output_modes_agree` extraction.
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

// ---------------------------------------------------------------------
// B1 itself (#142 §1): the exact reproduction, mode 3 (schema failure).
// ---------------------------------------------------------------------

/// B1's exact document: valid JSON, missing `authorization_unit`. Four
/// lines, so the `missing field` error lands "at line 4 column 1" exactly
/// as #142 §1 quotes — confirmed against the built binary while writing
/// this test, not copied on faith.
const B1_DOCUMENT: &str = "{\n  \"version\": \"harness.final-authorization-policy/v1\",\n  \"authorizer_actor_ids\": [\"a\", \"b\"]\n}\n";

#[test]
fn b1_final_authorization_schema_failure_names_the_example_command() {
    let workspace = Workspace::new();
    let policy_path = workspace.root.join("final-auth.json");
    fs::write(&policy_path, B1_DOCUMENT).unwrap();
    // `read_final_authorization_policy` is called before the control
    // repository is ever opened (`run_set_final_authorization_policy`,
    // `src/commands/project.rs`), so a nonexistent `--control` still
    // reaches the exact refusal B1 hit — the same shortcut #142's own
    // evidence-gathering used to reproduce it against the shipped binary.
    let missing_control = workspace.root.join("no-such-control");

    let output = run_text(&[
        "project",
        "set-final-authorization-policy",
        "--control",
        missing_control.to_str().unwrap(),
        "--policy",
        policy_path.to_str().unwrap(),
        "--actor",
        "coordinator",
    ]);

    assert!(
        !output.status.success(),
        "a final-authorization policy missing authorization_unit must refuse"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        stderr.contains("missing field `authorization_unit`"),
        "not exercising B1's reproduction; got:\n{stderr}"
    );
    assert_eq!(code_line(&output), "CH-CONFIG-MALFORMED");

    let recovery = recovery_line(&output);
    assert_ne!(
        recovery, SHARED_FALLBACK_RECOVERY,
        "B1's whole complaint was this generic syntax text naming the one thing that was not \
         wrong; got:\n{stderr}"
    );
    assert!(
        recovery.contains("project example-final-authorization"),
        "B1's fix is #143's sibling command, which the recovery must name so an operator can \
         actually compare their document against a working one; got: {recovery:?}"
    );
    assert!(
        !recovery.to_lowercase().contains("syntax"),
        "this is a schema failure, not a syntax failure — the recovery must not send the \
         operator looking for a syntax error that does not exist; got: {recovery:?}"
    );
}

// ---------------------------------------------------------------------
// Mode 2: the syntax-failure sibling of B1's exact site, proving
// `serde_json::Error::is_data()` actually splits the two rather than
// collapsing them back into one generic message.
// ---------------------------------------------------------------------

#[test]
fn final_authorization_syntax_failure_is_told_apart_from_the_schema_failure() {
    let workspace = Workspace::new();
    let policy_path = workspace.root.join("final-auth.json");
    // Genuinely invalid JSON — an unterminated object — not merely a
    // document that fails its schema. `serde_json::Error::classify()`
    // reports this as `Category::Syntax`, not `Category::Data`.
    fs::write(&policy_path, "{ not json").unwrap();
    let missing_control = workspace.root.join("no-such-control");

    let output = run_text(&[
        "project",
        "set-final-authorization-policy",
        "--control",
        missing_control.to_str().unwrap(),
        "--policy",
        policy_path.to_str().unwrap(),
        "--actor",
        "coordinator",
    ]);

    assert!(!output.status.success(), "invalid JSON must refuse");
    assert_eq!(code_line(&output), "CH-CONFIG-MALFORMED");

    let recovery = recovery_line(&output);
    assert_ne!(recovery, SHARED_FALLBACK_RECOVERY);
    assert!(
        recovery.contains("not valid JSON"),
        "a true syntax failure should say so plainly; got: {recovery:?}"
    );
    assert!(
        !recovery.contains("project example-final-authorization"),
        "a syntax failure needs no reference document — the operator already has an exact \
         line and column — so this recovery should not name the example command the schema \
         branch does; got: {recovery:?}"
    );
    assert!(
        !recovery.to_lowercase().contains("schema"),
        "this is a syntax failure, not a schema failure; got: {recovery:?}"
    );
}

// ---------------------------------------------------------------------
// Mode 1: B1's second reproduction (#142 §1) — the same wrong recovery
// reached the operator twice more for files that did not exist at all.
// ---------------------------------------------------------------------

#[test]
fn card_draft_read_failure_names_the_read_problem_not_a_syntax_problem() {
    let workspace = Workspace::initialized();
    let missing_draft = workspace.root.join("F-004-fast.yaml");
    assert!(!missing_draft.exists());

    let output = run_text(&[
        "card",
        "create",
        "--control",
        workspace.control.to_str().unwrap(),
        "--draft",
        missing_draft.to_str().unwrap(),
        "--actor",
        "coordinator",
    ]);

    assert!(
        !output.status.success(),
        "a draft path that does not exist must refuse"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        stderr.contains("cannot read draft"),
        "not exercising the read-failure site; got:\n{stderr}"
    );
    assert_eq!(code_line(&output), "CH-CONFIG-MALFORMED");

    let recovery = recovery_line(&output);
    assert_ne!(
        recovery, SHARED_FALLBACK_RECOVERY,
        "B1's second reproduction: a file that does not exist has no syntax to correct; \
         got:\n{stderr}"
    );
    assert!(recovery.contains("read failure"), "got: {recovery:?}");
    assert!(
        !recovery.to_lowercase().contains("schema"),
        "a read failure is not a schema problem; got: {recovery:?}"
    );
    assert!(
        !recovery.starts_with("Correct the"),
        "must not reintroduce an instruction to fix syntax or content for a file that was \
         never read; got: {recovery:?}"
    );
}

// ---------------------------------------------------------------------
// Establish-and-justify choice #1 (#142 §10, §12): `serde_yaml_ng` offers
// no syntax/schema split, so a YAML site's recovery is one message, honest
// for both. Pinned on `gate example`'s document kind, which does have an
// example command, so the "honest for both" text still has to work without
// ever claiming to know which of the two actually happened.
// ---------------------------------------------------------------------

#[test]
fn gate_definition_recovery_is_identical_for_a_syntax_and_a_schema_failure() {
    let syntax_broken = "schema: harness.gate/v1\ngate_id: \"unterminated\n";
    let schema_broken = "schema: harness.gate/v1\nrevision: 1\nargv: [\"true\"]\nworking_directory: \".\"\ntimeout_seconds: 60\nenvironment:\n  allow: [PATH]\n  set: {}\nnetwork_policy: denied\nretry_policy:\n  max_attempts: 1\nartifacts: []\n";

    let workspace = Workspace::new();
    let syntax_path = workspace.root.join("syntax-broken.yaml");
    let schema_path = workspace.root.join("schema-broken.yaml");
    fs::write(&syntax_path, syntax_broken).unwrap();
    fs::write(&schema_path, schema_broken).unwrap();

    // `gate validate` needs no `--control`, reachable before an operator
    // has a project at all — see `DefinitionArgs`'s own doc comment.
    let syntax_output = run_text(&[
        "gate",
        "validate",
        "--definition",
        syntax_path.to_str().unwrap(),
    ]);
    let schema_output = run_text(&[
        "gate",
        "validate",
        "--definition",
        schema_path.to_str().unwrap(),
    ]);

    assert!(!syntax_output.status.success());
    assert!(!schema_output.status.success());
    assert_eq!(code_line(&syntax_output), "CH-CONFIG-MALFORMED");
    assert_eq!(code_line(&schema_output), "CH-CONFIG-MALFORMED");

    let syntax_stderr = String::from_utf8_lossy(&syntax_output.stderr).into_owned();
    let schema_stderr = String::from_utf8_lossy(&schema_output.stderr).into_owned();
    assert!(
        syntax_stderr.contains("while scanning a quoted scalar")
            || syntax_stderr.contains("unexpected end of stream"),
        "not exercising a genuine YAML tokenizer error; got:\n{syntax_stderr}"
    );
    assert!(
        schema_stderr.contains("missing field `gate_id`"),
        "not exercising a genuine schema error; got:\n{schema_stderr}"
    );

    let syntax_recovery = recovery_line(&syntax_output);
    let schema_recovery = recovery_line(&schema_output);
    assert_ne!(syntax_recovery, SHARED_FALLBACK_RECOVERY);
    assert_eq!(
        syntax_recovery, schema_recovery,
        "gate.rs's GATE_DEFINITION_PARSE_RECOVERY is a single honest-for-both message by \
         design (serde_yaml_ng exposes no classify()); a genuine YAML syntax error and a \
         genuine schema error must produce byte-identical recovery text"
    );
    assert!(
        syntax_recovery.contains("gate example"),
        "got: {syntax_recovery:?}"
    );
}

// ---------------------------------------------------------------------
// Establish-and-justify choice #2 (#142 §10, §12): `HarnessError::Config`
// has no per-site recovery mechanism, so `project validate`'s file-read
// failure stays on the shared fallback — deliberately, not by oversight.
// See `run_validate`'s own doc comment (`src/commands/project.rs`) for the
// full reasoning this test pins.
// ---------------------------------------------------------------------

#[test]
fn project_validate_missing_config_file_still_gets_the_shared_fallback() {
    let workspace = Workspace::new();
    let missing_config = workspace.root.join("no-such-config.json");
    assert!(!missing_config.exists());

    let output = run_text(&[
        "project",
        "validate",
        "--config",
        missing_config.to_str().unwrap(),
    ]);

    assert!(
        !output.status.success(),
        "a missing config file must refuse"
    );
    assert_eq!(code_line(&output), "CH-CONFIG-MALFORMED");
    assert_eq!(
        recovery_line(&output),
        SHARED_FALLBACK_RECOVERY,
        "this is HarnessError::Config, which has no per-site recovery mechanism today (#142's \
         evidence report names this as a follow-up); if this now differs from the shared \
         fallback, either the mechanism was extended (update this test to match, deliberately) \
         or this assertion caught an accidental regression"
    );
}
