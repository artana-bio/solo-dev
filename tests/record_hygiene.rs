//! Record hygiene: a credential must not reach control history.
//!
//! Every record type here is committed to the control repository, and control
//! history is the integrity chain (D-011). There is no later stage that can
//! remove a secret from one without breaking what the record proves, so the
//! refusal at the write is the only place the control can exist. These tests
//! prove it holds at each of those writes, and that the refusal itself does
//! not become the leak by quoting what it refused.

mod support;

use std::fs;

use serde_json::Value;
use support::Workspace;

/// A recognizable credential shape, in a variable so no assertion can pass by
/// accidentally matching a different literal.
const TOKEN: &str = "ghp_0123456789abcdef0123456789abcdef0123";

fn envelope(output: &std::process::Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("an error envelope")
}

fn error_code(output: &std::process::Output) -> String {
    envelope(output)["error"]["code"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Asserts a refusal names the field, and nowhere repeats the value.
fn refused_without_echoing(output: &std::process::Output, field: &str) {
    assert_eq!(output.status.code(), Some(5), "a policy refusal");
    assert_eq!(error_code(output), "CH-POLICY-SENSITIVE-VALUE");

    let whole = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        whole.contains(field),
        "the refusal must name where the value is, so it can be found: {whole}"
    );
    assert!(
        !whole.contains(TOKEN),
        "the refusal must not repeat the value; that is the leak, one layer out: {whole}"
    );
}

/// A cycle open and ready for cards.
fn opened() -> Workspace {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace
}

#[test]
fn a_card_carrying_a_credential_is_refused() {
    let workspace = opened();
    let body = format!(
        "card_id: F-001\ncycle_id: C-001\ntitle: Implement F-001\ngoal: Deliver F-001 using {TOKEN}\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [\"src/**\"]\n  exclude: []\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
        base = workspace.authority_head(),
    );
    let path = workspace.root.join("F-001.yaml");
    fs::write(&path, body).unwrap();

    let output = workspace.card_raw(&["create", "--draft", &path.display().to_string()]);
    refused_without_echoing(&output, "card.goal");
}

#[test]
fn a_credential_in_an_acceptance_behavior_is_refused_by_position() {
    // The list index is part of the field path: a card can have twenty
    // behaviors, and "one of them has a token in it" is not actionable.
    let workspace = opened();
    let body = format!(
        "card_id: F-001\ncycle_id: C-001\ntitle: Implement F-001\ngoal: Deliver F-001\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [\"src/**\"]\n  exclude: []\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works, \"authenticates with {TOKEN}\"]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
        base = workspace.authority_head(),
    );
    let path = workspace.root.join("F-001.yaml");
    fs::write(&path, body).unwrap();

    let output = workspace.card_raw(&["create", "--draft", &path.display().to_string()]);
    refused_without_echoing(&output, "card.acceptance.behaviors[1]");
}

#[test]
fn a_handoff_declaration_carrying_a_credential_is_refused() {
    let workspace = opened();
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);

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
            "delivered_sha: {head}\nbehavior_delivered: adds a.rs\nimplementation_decisions: [minimal]\nassumptions: [\"the deploy key {TOKEN} stays valid\"]\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
        ),
    )
    .unwrap();

    let output = workspace.handoff_raw(&[
        "create",
        "--card-id",
        "F-001",
        "--declaration",
        &declaration.display().to_string(),
    ]);
    refused_without_echoing(&output, "handoff.assumptions[0]");
}

#[test]
fn a_review_finding_carrying_a_credential_is_refused() {
    let workspace = opened();
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);

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
            "delivered_sha: {head}\nbehavior_delivered: adds a.rs\nimplementation_decisions: [minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
        ),
    )
    .unwrap();
    workspace.handoff(&[
        "create",
        "--card-id",
        "F-001",
        "--declaration",
        &declaration.display().to_string(),
    ]);
    workspace.review(&["begin", "--card-id", "F-001"]);

    // A reviewer pasting the credential they found is the likeliest way one
    // arrives here — the finding is *about* the secret.
    let verdict = workspace.root.join("verdict.yaml");
    fs::write(
        &verdict,
        format!(
            "reviewer_actor_id: reviewer-session-a\ndecision: changes_requested\nfindings:\n  - severity: critical\n    location: src/a.rs\n    detail: \"hardcoded credential {TOKEN}\"\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\nresidual_risks: []\n"
        ),
    )
    .unwrap();

    let output = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict.display().to_string(),
    ]);
    refused_without_echoing(&output, "review.findings[0].detail");
}

#[test]
fn a_gate_definition_cannot_set_a_credential_shaped_variable() {
    // `environment.set` is the one field in the schema whose purpose is to
    // carry a literal value a process needs at run time, and the definition is
    // committed to `control/gates`. It is the most natural place in the whole
    // system to put a token.
    let workspace = Workspace::initialized();
    let body = "schema: harness.gate/v1\ngate_id: gate.publish\nrevision: 1\nargv: [\"true\"]\nworking_directory: \".\"\ntimeout_seconds: 60\nenvironment:\n  allow: [PATH]\n  set:\n    DEPLOY_TOKEN: \"placeholder\"\nnetwork_policy: denied\nretry_policy:\n  max_attempts: 1\nartifacts: []\n";
    let path = workspace.gate_definition("publish", body);

    let output = workspace.gate_raw(&["validate", "--definition", &path]);
    assert_eq!(output.status.code(), Some(3), "an invalid gate definition");
    let rendered = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        rendered.contains("DEPLOY_TOKEN") && rendered.contains("allow"),
        "the refusal names the variable and the remedy: {rendered}"
    );
}

#[test]
fn a_gate_definition_carrying_a_credential_value_is_refused() {
    let workspace = Workspace::initialized();
    let body = format!(
        "schema: harness.gate/v1\ngate_id: gate.publish\nrevision: 1\nargv: [\"true\"]\nworking_directory: \".\"\ntimeout_seconds: 60\nenvironment:\n  allow: [PATH]\n  set:\n    REGISTRY_URL: \"https://ci:{TOKEN}@registry.example\"\nnetwork_policy: denied\nretry_policy:\n  max_attempts: 1\nartifacts: []\n"
    );
    let path = workspace.gate_definition("publish", &body);

    let output = workspace.gate_raw(&["validate", "--definition", &path]);
    let whole = String::from_utf8_lossy(&output.stdout).to_string();
    assert_ne!(output.status.code(), Some(0), "must not be accepted");
    assert!(
        !whole.contains(TOKEN),
        "and the refusal must not echo it: {whole}"
    );
}

#[test]
fn a_credential_in_gate_argv_is_refused() {
    // `WP-530`'s leak fixture used to be the only thing exercising this path,
    // by registering a gate whose argv held an example AWS key. Fixing that
    // fixture removed the coverage with it, so the case is pinned here on
    // purpose: an argument list is as committed as any other part of the
    // definition, and `--header Authorization: Bearer …` is a realistic way to
    // reach it.
    let workspace = Workspace::initialized();
    let body = format!(
        "schema: harness.gate/v1\ngate_id: gate.fetch\nrevision: 1\nargv: [\"curl\", \"-H\", \"Authorization: Bearer {TOKEN}\"]\nworking_directory: \".\"\ntimeout_seconds: 60\nenvironment:\n  allow: [PATH]\n  set: {{}}\nnetwork_policy: allowed\nretry_policy:\n  max_attempts: 1\nartifacts: []\n"
    );
    let path = workspace.gate_definition("fetch", &body);

    let output = workspace.gate_raw(&["validate", "--definition", &path]);
    assert_ne!(output.status.code(), Some(0), "must not be accepted");
    let whole = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        whole.contains("gate.argv[2]"),
        "the refusal names the argument position: {whole}"
    );
    assert!(!whole.contains(TOKEN), "and does not echo it: {whole}");
}

#[test]
fn a_text_mode_error_redacts_what_the_envelope_would_have() {
    // Text is the default output mode, and a terminal is scrollback. Covered
    // separately because it renders through a different path in `main` than
    // the JSON envelope does.
    let workspace = Workspace::initialized();
    let output = Workspace::run(&[
        "cycle".to_owned(),
        "create".to_owned(),
        "--control".to_owned(),
        workspace.control.display().to_string(),
        "--cycle-id".to_owned(),
        TOKEN.to_owned(),
        "--objective".to_owned(),
        "an identifier that is really a token".to_owned(),
    ]);

    assert_ne!(output.status.code(), Some(0));
    let whole = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!whole.contains(TOKEN), "text mode leaked it: {whole}");
    assert!(
        whole.contains("[redacted:github-token]"),
        "and it says something was removed: {whole}"
    );
}

#[test]
fn structured_error_details_are_redacted_too() {
    // `message` and `details` are built from different sources — one from the
    // error's Display, one from its structured fields — so proving the first
    // is redacted says nothing about the second.
    let workspace = Workspace::initialized();
    let output = workspace.cycle_raw(&[
        "create",
        "--cycle-id",
        TOKEN,
        "--objective",
        "an identifier that is really a token",
    ]);

    let body = envelope(&output);
    let details = serde_json::to_string(&body["error"]["details"]).unwrap();
    assert!(
        !details.contains(TOKEN),
        "the details payload carried it through: {details}"
    );
}

#[test]
fn an_unparsable_command_line_does_not_echo_a_credential() {
    // Regression, RV-000036. A usage error quotes what was typed — that is
    // what makes it useful — so it is the one diagnostic guaranteed to echo an
    // argument back. Both renderers were missed: text mode printed clap's
    // output directly, and the JSON envelope redacted its message and details
    // while rebuilding the top-level `command` field from raw arguments.
    for mode in [Vec::new(), vec!["--output".to_owned(), "json".to_owned()]] {
        let mut argv = vec![TOKEN.to_owned()];
        argv.extend(mode.clone());
        let output = Workspace::run(&argv);

        assert_ne!(output.status.code(), Some(0), "an unknown subcommand");
        let whole = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !whole.contains(TOKEN),
            "a token mistyped as a subcommand reached the terminal ({mode:?}): {whole}"
        );
    }
}

#[test]
fn an_ordinary_card_is_still_accepted() {
    // The control that fires on ordinary evidence gets switched off. This is
    // the half of the behavior that keeps the other half deployable.
    let workspace = opened();
    workspace.activate_card("F-001", &["src/**"]);

    let shown = workspace.card_json(&["status", "--card-id", "F-001"]);
    assert_eq!(shown["status"], "success");
}

#[test]
fn an_error_envelope_redacts_a_credential_it_would_otherwise_echo() {
    // Errors are generated, not authored, so there is nobody to hand them back
    // to for correction — and an error is where an input is most likely to be
    // quoted, because saying what was wrong usually means repeating it.
    let workspace = Workspace::initialized();
    let output = workspace.cycle_raw(&[
        "create",
        "--cycle-id",
        TOKEN,
        "--objective",
        "an identifier that is really a token",
    ]);

    assert_ne!(output.status.code(), Some(0), "an invalid cycle id");
    let whole = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !whole.contains(TOKEN),
        "the envelope must not carry the value through: {whole}"
    );
    assert!(
        whole.contains("[redacted:github-token]"),
        "and it says something was removed rather than silently dropping it: {whole}"
    );
}
