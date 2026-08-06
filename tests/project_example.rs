//! `project example` emits a convergence-policy document by constructing a
//! real [`ConvergencePolicy`] and serializing it, so the emitter and the
//! parser that accepts it (`project set-convergence-policy`) are the same
//! `serde` code and cannot disagree.
//!
//! #108: an independent cold-start operator reconstructed the convergence
//! policy schema by trial and error — that `version` must be the exact
//! string `harness.convergence-policy/v1`, that all four risk tiers
//! including `critical` must be present, that dimension names are
//! `snake_case`. This file proves the single verifiable result that closes
//! that gap: an operator can obtain a complete, valid convergence policy
//! example from the tool itself, and feeding it back to `project
//! set-convergence-policy` unchanged is accepted.
//!
//! The trap that keeps this document JSON rather than YAML, confirmed
//! against this codebase before any of this was written:
//! `read_convergence_policy` (`src/commands/project.rs`) parses only JSON
//! (`serde_json::from_str`) — unlike `review record`'s `read_verdict`, which
//! accepts YAML (`serde_yaml_ng::from_str`). Every capture in this file
//! therefore reads either `--output json`'s own JSON envelope or plain-text
//! stdout that is itself JSON, never YAML.

mod support;

use std::{collections::BTreeSet, fs};

use change_harness::config::{CardConvergenceLimits, ConvergencePolicy};
use support::Workspace;

/// A project with no cycles and no recorded convergence facts.
///
/// The exact shape
/// `set_convergence_policy_installs_on_a_project_with_no_open_cycles`
/// (`tests/convergence.rs`) uses to install a first policy without either of
/// #107's extra refusals — `refuse_blocking_cycles` (an open cycle) or
/// `refuse_orphaning_facts` (a recorded convergence fact) — having anything
/// to refuse. Built exactly this way so the install can never fail for a
/// reason unrelated to the document itself, or a passing test here would
/// prove nothing about the emitted example.
fn installable_project() -> Workspace {
    Workspace::initialized()
}

/// Runs `project example` for real and returns the document it emitted, read
/// from the JSON envelope's `data.example`.
///
/// Deliberately not routed through any `Workspace` control-repository
/// helper: `project example` declares no `--control` flag, and passing one
/// would be a usage error, not a no-op — itself part of what #108 constraint
/// 1 means.
fn captured_example() -> String {
    let envelope = Workspace::run_json(&[
        "project".to_owned(),
        "example".to_owned(),
        "--output".to_owned(),
        "json".to_owned(),
    ]);
    assert_eq!(
        envelope["status"], "success",
        "project example must succeed: {envelope}"
    );
    envelope["data"]["example"]
        .as_str()
        .expect("the example document as a string")
        .to_owned()
}

/// Writes `document` to a file under `workspace.root` and feeds it to a real
/// `project set-convergence-policy` invocation, returning its parsed JSON
/// envelope. Never asserts success itself — every call site names its own
/// expectation.
fn install(workspace: &Workspace, document: &str, file_name: &str) -> serde_json::Value {
    let path = workspace.root.join(file_name);
    fs::write(&path, document).unwrap();
    Workspace::run_json(&[
        "project".to_owned(),
        "set-convergence-policy".to_owned(),
        "--control".to_owned(),
        workspace.control.display().to_string(),
        "--policy".to_owned(),
        path.display().to_string(),
        "--output".to_owned(),
        "json".to_owned(),
    ])
}

/// The field names present at the top level of a parsed JSON document.
fn top_level_keys(document: &str) -> BTreeSet<String> {
    let value: serde_json::Value =
        serde_json::from_str(document).expect("the document is well-formed JSON");
    value
        .as_object()
        .expect("the document has fields")
        .keys()
        .cloned()
        .collect()
}

#[test]
fn the_emitted_policy_example_is_accepted_by_set_convergence_policy() {
    // The test that justifies the card. Capture whatever the tool emits,
    // write it to a file unchanged, and feed it to a real `project
    // set-convergence-policy` invocation — not a parse check, the actual
    // command. `installable_project` has no cycles and no recorded
    // convergence facts, so #107's two extra refusals never fire; a failure
    // here can only be about the document itself.
    //
    // Mutations that must make this fail: drop the `critical` risk tier
    // (#108 mutation 1, the exact omission the cold-start operator made),
    // and emit YAML instead of JSON (#108 mutation 2 — the real reader
    // parses only JSON, so YAML must be rejected). Both are exercised by
    // hand against this test in the card's evidence report, not reproduced
    // here, because reproducing them means editing the emitter itself, which
    // this file must not do.
    let workspace = installable_project();
    let example = captured_example();

    let envelope = install(&workspace, &example, "captured-example.json");
    assert_eq!(
        envelope["status"], "success",
        "the tool's own example must be accepted by `project set-convergence-policy` unchanged: \
         {envelope}"
    );
    assert_eq!(
        envelope["data"]["changed"], true,
        "the install must actually change the project, confirming the document was really \
         installed and not merely accepted as a parse-only no-op: {envelope}"
    );
}

#[test]
fn the_emitted_policy_example_text_mode_stdout_is_accepted_by_set_convergence_policy() {
    // The channel an operator actually redirects: `change-harness project
    // example > policy.json` writes text-mode stdout, not the JSON envelope
    // `captured_example` reads `data.example` from. This test exists because
    // the merged first half of #108 shipped `review example` without the
    // equivalent test and needed a repair — its round-trip read
    // `data.example` from `--output json`, leaving this channel untested.
    // This file does not repeat that gap.
    //
    // Mutation that must make this fail: prepend a line ahead of the
    // document (#108 mutation 3). Exercised by hand against this test in the
    // card's evidence report, not reproduced here, since reproducing it
    // means editing the emitter itself, which this file must not do.
    //
    // Deliberately not routed through `Workspace::run_json`, and no
    // `--output` flag at all: this must be exactly the plain-text default a
    // caller gets from `project example > policy.json`.
    let workspace = installable_project();

    let output = Workspace::run(&["project".to_owned(), "example".to_owned()]);
    assert!(
        output.status.success(),
        "project example must succeed (exit {:?})",
        output.status.code()
    );

    // Assert only on stdout: the entire point is that stdout, exactly as a
    // shell redirect would capture it, round-trips through `project
    // set-convergence-policy` unchanged. Whatever landed on stderr (the
    // illustrative-budget warning) is not this test's concern — proving
    // that stays true is what this test does, not what it checks.
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();

    let envelope = install(&workspace, &stdout, "captured-stdout-example.json");
    assert_eq!(
        envelope["status"], "success",
        "text-mode stdout, written to a file unchanged, must be accepted by `project \
         set-convergence-policy` — this is the exact `project example > policy.json` workflow \
         an operator would actually run: {envelope}"
    );
    assert_eq!(envelope["data"]["changed"], true);
}

#[test]
fn the_emitted_policy_example_is_complete() {
    // Every field a caller must supply has to appear in the emitted example,
    // or a caller copying it verbatim would still hit a missing-field error
    // the example was supposed to prevent.
    //
    // Derived from the struct, not read off a hand-maintained list:
    // `ConvergencePolicy`, and every type nested inside it, has no `Option`
    // and no `#[serde(default)]` field, so completeness reduces to one
    // question a real parse round-trip already answers — every field the
    // struct declares must be present in the emitted document, because the
    // real `Deserialize` implementation refuses to construct the value
    // otherwise.
    //
    // Mutation that must make this fail: drop the `critical` risk tier
    // (#108 mutation 1).
    let example = captured_example();

    // Parses with the real, frozen `ConvergencePolicy` deserializer, which
    // is `deny_unknown_fields` — so a parse failure here would itself prove
    // incompleteness (a missing required field) just as loudly as a
    // completeness assertion below would.
    let policy: ConvergencePolicy =
        serde_json::from_str(&example).expect("the example parses as a ConvergencePolicy");

    // Comparing the emitted document's key set against the same value
    // re-serialized catches a field silently dropped from the *emitted*
    // JSON even though the struct itself still declares it — the same
    // completeness property `review example`'s test establishes by diffing
    // against an independently constructed reference. `deny_unknown_fields`
    // on every nested type also means this comparison catches the reverse
    // mistake: anything present that `ConvergencePolicy` does not declare.
    let emitted_keys = top_level_keys(&example);
    let reference_keys =
        top_level_keys(&serde_json::to_string(&policy).expect("policy serializes"));
    assert_eq!(
        emitted_keys, reference_keys,
        "the example should name every top-level field ConvergencePolicy has, so a reader can \
         see the complete shape in one place: {example}"
    );

    // The struct-level check above cannot see past one layer of nesting: a
    // `ConvergencePolicy` that deserializes successfully already proves
    // every nested field it declares is present with *some* value, but not
    // that the shape a caller sees matches what `CardConvergenceLimits`
    // actually names. Walked explicitly so a mutation renaming or dropping
    // one nested dimension at one risk tier is reported by name rather than
    // only by a top-level key-set mismatch it would not even cause.
    for (tier, limits) in [
        ("low", &policy.card_limits.low),
        ("medium", &policy.card_limits.medium),
        ("high", &policy.card_limits.high),
        ("critical", &policy.card_limits.critical),
    ] {
        let rendered = serde_json::to_string(limits).expect("limits serialize");
        for field in [
            "review_returns",
            "repair_attempts",
            "gate_failures",
            "material_scope_revisions",
        ] {
            assert!(
                top_level_keys(&rendered).contains(field),
                "card_limits.{tier} is missing `{field}`: {rendered}"
            );
        }
    }
    assert!(
        top_level_keys(&serde_json::to_string(&policy.cycle_limits).unwrap())
            .contains("integration_failures"),
        "cycle_limits is missing `integration_failures`"
    );
}

#[test]
fn the_example_policy_narrows_every_dimension_as_risk_climbs() {
    // The budget values in `example_convergence_policy` are a deliberate
    // choice, not an arbitrary one, so the choice ships with its own test
    // here rather than staying an unverified claim in a comment.
    //
    // The chosen shape (see `example_convergence_policy` in
    // `src/commands/project.rs`) demonstrates the one relationship a schema
    // this shape cannot express on its own but every real policy should
    // have: the budget on each of the four card dimensions narrows as
    // declared risk rises, so a `critical` card is walked back to an
    // authorized decision after fewer unresolved attempts than a `low` one
    // gets. This is checked against the real emitted document, by
    // comparison rather than against hardcoded numbers, so it fails however
    // the ordering is broken — reversed, flattened, or partially
    // scrambled — without this test needing to know or restate the exact
    // limits chosen.
    //
    // Mutation that must make this fail: swap the `low` and `critical`
    // dimensions (or otherwise give a higher-risk tier a larger budget than
    // a lower-risk one on any dimension).
    let example = captured_example();
    let policy: ConvergencePolicy =
        serde_json::from_str(&example).expect("the example parses as a ConvergencePolicy");

    let as_tuple = |limits: &CardConvergenceLimits| {
        (
            limits.review_returns,
            limits.repair_attempts,
            limits.gate_failures,
            limits.material_scope_revisions,
        )
    };
    let low = as_tuple(&policy.card_limits.low);
    let medium = as_tuple(&policy.card_limits.medium);
    let high = as_tuple(&policy.card_limits.high);
    let critical = as_tuple(&policy.card_limits.critical);

    let dimensions: [(&str, u32, u32, u32, u32); 4] = [
        ("review_returns", low.0, medium.0, high.0, critical.0),
        ("repair_attempts", low.1, medium.1, high.1, critical.1),
        ("gate_failures", low.2, medium.2, high.2, critical.2),
        (
            "material_scope_revisions",
            low.3,
            medium.3,
            high.3,
            critical.3,
        ),
    ];
    for (name, low, medium, high, critical) in dimensions {
        assert!(
            low >= medium && medium >= high && high >= critical,
            "{name}: expected low ({low}) >= medium ({medium}) >= high ({high}) >= critical \
             ({critical}), so the budget narrows as risk climbs rather than widens or stays flat"
        );
        assert!(
            low > critical,
            "{name}: low ({low}) and critical ({critical}) must actually differ, or the \
             ordering above is trivially satisfied by four identical numbers and teaches nothing \
             about risk-scaling"
        );
    }
}

#[test]
fn the_example_carries_a_warning_that_the_budget_is_illustrative_not_a_recommendation() {
    // The other half of the same budget-values decision: `ConvergencePolicy`
    // is `deny_unknown_fields`, so the JSON document itself has no field
    // left to carry a caution in without breaking the round-trip tests
    // above — the caution has to live in the command envelope's `warnings`
    // instead, never in `data.example` or stdout.
    //
    // Mutation that must make this fail: delete the `.with_warning(..)` call
    // in `run_example`, or reword it to drop the "not a recommendation"
    // phrase.
    let envelope = Workspace::run_json(&[
        "project".to_owned(),
        "example".to_owned(),
        "--output".to_owned(),
        "json".to_owned(),
    ]);
    assert_eq!(envelope["status"], "success");
    let warnings = envelope["warnings"]
        .as_array()
        .expect("warnings is an array");
    assert!(
        warnings.iter().any(|warning| warning
            .as_str()
            .is_some_and(|text| text.contains("not a recommendation"))),
        "the envelope must warn that the budget is illustrative, not a recommendation, since the \
         JSON document itself cannot carry that caution: {envelope}"
    );
}

#[test]
fn the_example_is_pretty_printed_for_a_human_redirecting_it_to_a_file() {
    // A formatting choice with no explicit requirement behind it, tested
    // rather than left silent: `project example > policy.json` is meant to
    // be read and edited by a person, the same way `ProjectConfig::to_json`
    // (`src/config/mod.rs`) always renders the control repository's own
    // project document pretty rather than minified. A single-line compact
    // document would still parse and would still pass every test above, so
    // only this test would catch that regression.
    let example = captured_example();
    assert!(
        example.contains('\n'),
        "the emitted example should be pretty-printed, not a single compact line: {example}"
    );
}

#[test]
fn project_example_is_discoverable_from_help() {
    // #108 constraint 2: a surface nobody finds is the problem restated.
    let output = Workspace::run(&["project".to_owned(), "--help".to_owned()]);
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(
        help.lines()
            .any(|line| line.trim_start().starts_with("example")),
        "`project --help` must list `example` as a subcommand: {help}"
    );
}

#[test]
fn project_example_needs_no_control_repository_or_project() {
    // #108 constraint 1: reachable before an operator has anything set up,
    // not only after a failed attempt has taught them the shape by accident.
    // No `--control`, no project anywhere in this test.
    let output = Workspace::run(&[
        "project".to_owned(),
        "example".to_owned(),
        "--output".to_owned(),
        "json".to_owned(),
    ]);
    assert!(
        output.status.success(),
        "project example must succeed with no control repository and no project: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}
