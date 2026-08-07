//! `project example-final-authorization` emits a final-authorization-policy
//! document by constructing a real [`FinalAuthorizationPolicy`] and
//! serializing it, so the emitter and the parser that accepts it (`project
//! set-final-authorization-policy`) are the same `serde` code and cannot
//! disagree.
//!
//! #143: a second independent cold-start operator hit the one unnavigable
//! refusal in 139 commands — `project set-final-authorization-policy`
//! rejecting a document whose required `authorization_unit` field appeared
//! in no example, no help text, and no section of `SKILL.md`. Resolved only
//! by running `strings` on the binary. This file proves the single
//! verifiable result that closes that gap: an operator can obtain a
//! complete, valid final-authorization policy example from the tool itself,
//! and feeding it back to `project set-final-authorization-policy` unchanged
//! is accepted.
//!
//! Kept separate from `tests/project_example.rs` rather than appended to it:
//! that file is #108's, scoped to the convergence-policy example, and
//! frozen — this document is a different struct behind a different
//! subcommand, so it gets its own file, the same way `tests/exceptions.rs`
//! and `tests/convergence.rs` already separate by document rather than
//! sharing one grab-bag file.
//!
//! Confirmed against this codebase before any of this was written, mirroring
//! the same trap `tests/project_example.rs` documents for its own command:
//! `read_final_authorization_policy` (`src/commands/project.rs`) parses only
//! JSON (`serde_json::from_str`), so every capture in this file reads either
//! `--output json`'s own JSON envelope or plain-text stdout that is itself
//! JSON, never YAML.

mod support;

use std::{collections::BTreeSet, fs};

use change_harness::config::{ConvergencePolicy, FinalAuthorizationPolicy};
use support::Workspace;

/// A project with no cycles, whose declared final-authorizer actors at
/// `project init` are exactly `actor_ids`.
///
/// Built directly through raw `project init` args rather than
/// `Workspace::initialized()`, which passes no
/// `--final-authorizer-actor-id` at all: #143 §8's warning exists because
/// `authorizer_actor_ids` must name actors declared in the target project,
/// and this fixture makes that coupling concrete rather than assumed, by
/// declaring at `project init` the exact actor ids
/// `example_final_authorization_policy` (`src/commands/project.rs`) names.
///
/// Verified separately, and worth recording here: nothing in
/// `project set-final-authorization-policy` actually cross-checks its
/// installed `authorizer_actor_ids` against what a project declared at
/// `init` — `run_set_final_authorization_policy`'s own doc comment says so
/// explicitly ("No `authorizer_actor_ids` gate"), and its only checks are
/// the new policy's own `validate()` and `refuse_blocking_cycles`. So this
/// arrangement is not load-bearing for the install below to succeed; it is
/// done anyway because the card's evidence requirement asks for a project
/// whose declared actors match the example's, and it is the more honest
/// fixture regardless — the one an operator installing this document for
/// real would actually need for the policy to mean anything.
fn installable_project_with_authorizers(actor_ids: &[&str]) -> Workspace {
    let workspace = Workspace::new();
    let mut args = vec![
        "project".to_owned(),
        "init".to_owned(),
        "--project-id".to_owned(),
        "example".to_owned(),
        "--repository".to_owned(),
        workspace.repository.display().to_string(),
        "--control".to_owned(),
        workspace.control.display().to_string(),
        "--authority".to_owned(),
        workspace.authority.display().to_string(),
        "--worktree-root".to_owned(),
        workspace.worktrees.display().to_string(),
    ];
    for actor_id in actor_ids {
        args.push("--final-authorizer-actor-id".to_owned());
        args.push((*actor_id).to_owned());
    }
    let output = Workspace::run(&args);
    assert!(
        output.status.success(),
        "project init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    workspace
}

/// A project whose declared final-authorizer actors are exactly the two
/// `example_final_authorization_policy` names: `primary-authorizer` and
/// `secondary-authorizer`.
fn installable_project() -> Workspace {
    installable_project_with_authorizers(&["primary-authorizer", "secondary-authorizer"])
}

/// Runs `project example-final-authorization` for real and returns the
/// document it emitted, read from the JSON envelope's `data.example`.
///
/// Deliberately not routed through any `Workspace` control-repository
/// helper: `project example-final-authorization` declares no `--control`
/// flag, and passing one would be a usage error, not a no-op — itself part
/// of what #108 constraint 1, carried into #143, means.
fn captured_example() -> String {
    let envelope = Workspace::run_json(&[
        "project".to_owned(),
        "example-final-authorization".to_owned(),
        "--output".to_owned(),
        "json".to_owned(),
    ]);
    assert_eq!(
        envelope["status"], "success",
        "project example-final-authorization must succeed: {envelope}"
    );
    envelope["data"]["example"]
        .as_str()
        .expect("the example document as a string")
        .to_owned()
}

/// Writes `document` to a file under `workspace.root` and feeds it to a real
/// `project set-final-authorization-policy` invocation, returning its parsed
/// JSON envelope. Never asserts success itself — every call site names its
/// own expectation.
fn install(workspace: &Workspace, document: &str, file_name: &str) -> serde_json::Value {
    let path = workspace.root.join(file_name);
    fs::write(&path, document).unwrap();
    Workspace::run_json(&[
        "project".to_owned(),
        "set-final-authorization-policy".to_owned(),
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
fn the_emitted_final_authorization_example_is_accepted_by_set_final_authorization_policy() {
    // The test that justifies the card. Capture whatever the tool emits,
    // write it to a file unchanged, and feed it to a real `project
    // set-final-authorization-policy` invocation — not a parse check, the
    // actual command. `installable_project`'s declared final-authorizer
    // actors are exactly the example's `authorizer_actor_ids`, so a failure
    // here can only be about the document itself, never about the fixture's
    // own actors disagreeing with it.
    //
    // Mutations that must make this fail: drop a required field (#143
    // mutation 1), and emit `authorization_unit: "sealed-cycle"` (#143
    // mutation 2 — the exact class of mistake the operator made, and the
    // discriminating mutation: it proves this exercises
    // `FinalAuthorizationPolicy::validate`, not merely well-formed JSON).
    // Both are exercised by hand against this test in the card's evidence
    // report, not reproduced here, because reproducing them means editing
    // the emitter itself, which this file must not do.
    let workspace = installable_project();
    let example = captured_example();

    let envelope = install(
        &workspace,
        &example,
        "captured-final-authorization-example.json",
    );
    assert_eq!(
        envelope["status"], "success",
        "the tool's own final-authorization example must be accepted by `project \
         set-final-authorization-policy` unchanged: {envelope}"
    );
    assert_eq!(
        envelope["data"]["changed"], true,
        "the install must actually change the project, confirming the document was really \
         installed and not merely accepted as a parse-only no-op: {envelope}"
    );
}

#[test]
fn the_emitted_final_authorization_example_text_mode_stdout_is_accepted_by_set_final_authorization_policy()
 {
    // The channel an operator actually redirects: `change-harness project
    // example-final-authorization > policy.json` writes text-mode stdout,
    // not the JSON envelope `captured_example` reads `data.example` from.
    // #108's first half shipped `review example` without the equivalent test
    // and needed a repair, and #108's second half
    // (`tests/project_example.rs`) wrote it down so the gap would not repeat
    // — this file does not repeat it a third time.
    //
    // Mutation that must make this fail: prepend a line ahead of the
    // document (#143 mutation 3). Exercised by hand against this test in the
    // card's evidence report, not reproduced here, since reproducing it
    // means editing the emitter itself, which this file must not do.
    //
    // Deliberately not routed through `Workspace::run_json`, and no
    // `--output` flag at all: this must be exactly the plain-text default a
    // caller gets from `project example-final-authorization > policy.json`.
    let workspace = installable_project();

    let output = Workspace::run(&[
        "project".to_owned(),
        "example-final-authorization".to_owned(),
    ]);
    assert!(
        output.status.success(),
        "project example-final-authorization must succeed (exit {:?})",
        output.status.code()
    );

    // Assert only on stdout: the entire point is that stdout, exactly as a
    // shell redirect would capture it, round-trips through `project
    // set-final-authorization-policy` unchanged. Whatever landed on stderr
    // (the placeholder-actor warning) is not this test's concern.
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();

    let envelope = install(
        &workspace,
        &stdout,
        "captured-stdout-final-authorization-example.json",
    );
    assert_eq!(
        envelope["status"], "success",
        "text-mode stdout, written to a file unchanged, must be accepted by `project \
         set-final-authorization-policy` — this is the exact `project \
         example-final-authorization > policy.json` workflow an operator would actually run: \
         {envelope}"
    );
    assert_eq!(envelope["data"]["changed"], true);
}

#[test]
fn the_emitted_final_authorization_example_is_complete() {
    // Every field a caller must supply has to appear in the emitted example,
    // or a caller copying it verbatim would still hit a missing-field error
    // the example was supposed to prevent.
    let example = captured_example();

    // Parses with the real, frozen `FinalAuthorizationPolicy` deserializer,
    // which is `deny_unknown_fields` — so a parse failure here would itself
    // prove incompleteness (a missing required field) or excess (an unknown
    // one) just as loudly as the assertions below would.
    let policy: FinalAuthorizationPolicy =
        serde_json::from_str(&example).expect("the example parses as a FinalAuthorizationPolicy");

    // Comparing the emitted document's key set against the same value
    // re-serialized catches a field silently dropped from the *emitted* JSON
    // even though the struct itself still declares it — the same
    // completeness property `tests/project_example.rs`'s equivalent test
    // establishes for `ConvergencePolicy`.
    let emitted_keys = top_level_keys(&example);
    let reference_keys =
        top_level_keys(&serde_json::to_string(&policy).expect("policy serializes"));
    assert_eq!(
        emitted_keys, reference_keys,
        "the example should name every top-level field the emitted policy carries: {example}"
    );

    // Which of those keys `Deserialize` actually requires, derived from the
    // struct rather than read off a hand-maintained list: for each key
    // present, remove it and ask whether `FinalAuthorizationPolicy` still
    // parses without it. The same technique `review::optional_fields`
    // (`src/commands/review.rs`) uses for `Verdict`, applied here because
    // — unlike `ConvergencePolicy`, which has no optional field at all —
    // `FinalAuthorizationPolicy` has exactly one
    // (`exception_triggers`), so completeness has to distinguish "present"
    // from "required" rather than treat them as the same question.
    let value: serde_json::Value =
        serde_json::from_str(&example).expect("the example is well-formed JSON");
    let object = value.as_object().expect("the document has fields");
    let required: BTreeSet<String> = object
        .keys()
        .filter(|key| {
            let mut reduced = object.clone();
            reduced.remove(key.as_str());
            serde_json::from_value::<FinalAuthorizationPolicy>(serde_json::Value::Object(reduced))
                .is_err()
        })
        .cloned()
        .collect();
    let expected_required: BTreeSet<String> =
        ["version", "authorization_unit", "authorizer_actor_ids"]
            .into_iter()
            .map(String::from)
            .collect();
    assert_eq!(
        required, expected_required,
        "these are the only fields a caller must supply; a caller copying the example and \
         dropping any one of them must fail to install it, and nothing outside this set may \
         be required either: {example}"
    );

    // `exception_triggers` is the one field `required` above correctly
    // excludes. Confirmed here that it is shown anyway, non-empty, rather
    // than silently reverting to the default empty list, which
    // `skip_serializing_if` would then omit from the document entirely —
    // teaching a reader nothing about the five names `ExceptionTrigger`
    // accepts.
    assert!(
        !policy.exception_triggers.is_empty(),
        "exception_triggers is optional but its shape should still be shown in the example: \
         {example}"
    );
}

#[test]
fn the_example_carries_a_warning_that_authorizer_actor_ids_are_placeholders() {
    // #143 §8: this document needs its own warning, for a different reason
    // than `project example`'s illustrative-budget one — `authorizer_actor_ids`
    // must name actors declared in the target project, so the emitted
    // document is a valid *shape* that will not authorize anyone anywhere
    // unedited. Not one of #143's four required mutations, so not exercised
    // by hand in the evidence report; this protects a choice the card made
    // beyond what §11 lists, per §9's standing requirement that any such
    // choice ships with its own test.
    //
    // Mutation that would make this fail: delete the `.with_warning(..)`
    // call in `run_example_final_authorization`, or reword it to drop what
    // it says about `authorizer_actor_ids` needing to be declared in the
    // target project.
    let envelope = Workspace::run_json(&[
        "project".to_owned(),
        "example-final-authorization".to_owned(),
        "--output".to_owned(),
        "json".to_owned(),
    ]);
    assert_eq!(envelope["status"], "success");
    let warnings = envelope["warnings"]
        .as_array()
        .expect("warnings is an array");
    assert!(
        warnings
            .iter()
            .any(|warning| warning.as_str().is_some_and(|text| {
                text.contains("authorizer_actor_ids")
                    && text.contains("real")
                    && (text.contains("declared") || text.contains("actually"))
            })),
        "the envelope must warn plainly that authorizer_actor_ids must name actors declared in \
         the target project, or the emitted document will not authorize anyone unedited: \
         {envelope}"
    );
}

#[test]
fn project_example_final_authorization_is_discoverable_from_help() {
    // #143 §7: discoverability is the entire point of this card, and the
    // chosen surface — a sibling subcommand rather than a selector on the
    // existing `example` — only actually delivers that if it appears in
    // `project --help`'s own command list. This is the test for that
    // "establish and justify" choice, per §9's standing requirement.
    let output = Workspace::run(&["project".to_owned(), "--help".to_owned()]);
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(
        help.lines()
            .any(|line| line.trim_start().starts_with("example-final-authorization")),
        "`project --help` must list `example-final-authorization` as a subcommand: {help}"
    );
}

#[test]
fn project_example_final_authorization_needs_no_control_repository_or_project() {
    // #143 §7, carrying #108 constraint 1 forward: whichever surface is
    // chosen must be reachable before an operator has anything set up, not
    // only after a failed attempt has taught them the shape by accident. No
    // `--control`, no project anywhere in this test.
    let output = Workspace::run(&[
        "project".to_owned(),
        "example-final-authorization".to_owned(),
        "--output".to_owned(),
        "json".to_owned(),
    ]);
    assert!(
        output.status.success(),
        "project example-final-authorization must succeed with no control repository and no \
         project: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn bare_project_example_still_emits_the_convergence_policy() {
    // #143 §7's frozen constraint: adding `project example-final-authorization`
    // must not repoint bare `project example`, which someone has already
    // redirected to a file. Asserted by parsing stdout as the real,
    // `deny_unknown_fields` `ConvergencePolicy` — a document shaped like
    // `FinalAuthorizationPolicy` instead (different required fields, and
    // each type rejects the other's) could not parse as one.
    //
    // Mutation that must make this fail: point bare `project example` at the
    // final-authorization document (#143 mutation 4). Exercised by hand
    // against this test in the card's evidence report, not reproduced here,
    // since reproducing it means editing the emitter itself, which this file
    // must not do.
    let output = Workspace::run(&["project".to_owned(), "example".to_owned()]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let _: ConvergencePolicy = serde_json::from_str(&stdout).expect(
        "bare `project example` must still emit a document that parses as ConvergencePolicy",
    );
}
