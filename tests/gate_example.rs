//! `gate example` emits a gate definition document by constructing a real
//! [`GateDefinition`] and serializing it, so the emitter and the parser that
//! accepts it (`gate register`, via `parse_definition`) are the same `serde`
//! code and cannot disagree.
//!
//! #108: an independent cold-start operator reconstructed the gate
//! definition schema by submitting malformed documents and reading the
//! parser's complaints, field by field — `argv`,
//! `environment.allow`/`environment.set`, `network_policy`,
//! `retry_policy.max_attempts`, and the rest — the largest single time sink
//! in their run. This file proves the single verifiable result that closes
//! that gap: an operator can obtain a complete, valid gate definition
//! example from the tool itself, and feeding it back to `gate register`
//! unchanged is accepted.
//!
//! Mirrors `tests/review_example.rs`, the sibling proof for the verdict
//! schema, as closely as the two schemas allow. The one structural
//! difference: `GateDefinition` (`src/domain/gate.rs:118`) has no `Option`
//! and no `#[serde(default)]` field, so there is no `optional_fields`
//! advisory to check here — every field is simply required, which
//! `the_emitted_gate_example_is_complete` asserts directly.

mod support;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
};

use change_harness::domain::gate::{
    GATE_SCHEMA, GateDefinition, GateEnvironment, NetworkPolicy, RetryPolicy,
};
use support::Workspace;

/// Runs `gate example` for real and returns the document it emitted.
///
/// Deliberately not routed through `Workspace::gate_raw`/`gate_json`: those
/// helpers always append `--control <path>`, and `gate example` declares no
/// such flag — passing it would be a usage error, not a no-op, which is
/// itself part of what #108 constraint 1 means.
fn captured_example() -> String {
    let envelope = Workspace::run_json(&[
        "gate".to_owned(),
        "example".to_owned(),
        "--output".to_owned(),
        "json".to_owned(),
    ]);
    assert_eq!(
        envelope["status"], "success",
        "gate example must succeed: {envelope}"
    );
    envelope["data"]["example"]
        .as_str()
        .expect("the example document as a string")
        .to_owned()
}

/// The field names present at the top level of a parsed YAML/JSON document.
fn top_level_keys(document: &str) -> BTreeSet<String> {
    let value: serde_json::Value =
        serde_yaml_ng::from_str(document).expect("the document is well-formed YAML");
    value
        .as_object()
        .expect("the document has fields")
        .keys()
        .cloned()
        .collect()
}

#[test]
fn the_emitted_gate_example_is_accepted_by_gate_register() {
    // The test that justifies the card. Capture whatever the tool emits,
    // write it to a file unchanged, and feed it to a real `gate register`
    // invocation — not a parse check, the actual command.
    //
    // Mutations that must make this fail: drop a required field (#108
    // mutation 1), or emit a plausible-but-wrong value, e.g. a
    // `network_policy` variant that does not exist (#108 mutation 2 — the
    // discriminating one, mirroring the cold-start operator's
    // `approve`-vs-`approved` mistake: this proves the round trip exercises
    // real `serde` validation, not merely well-formed YAML). Both exercised
    // by hand against this test in the card's evidence report; neither is
    // reproduced here, because reproducing them means editing the emitter
    // itself, which this file must not do.
    let workspace = Workspace::initialized();
    let example = captured_example();

    let path = workspace.root.join("captured-gate-example.yaml");
    fs::write(&path, &example).unwrap();

    let envelope = workspace.gate_json(&["register", "--definition", &path.display().to_string()]);
    assert_eq!(
        envelope["status"], "success",
        "the tool's own example must be accepted by `gate register` unchanged: {envelope}"
    );
    assert_eq!(
        envelope["data"]["gate_id"], "gate.example",
        "confirms the example's own content was registered, not merely that some command \
         returned success"
    );
    assert_eq!(envelope["data"]["revision"], 1);
}

#[test]
fn the_emitted_gate_example_text_mode_stdout_is_accepted_by_gate_register() {
    // The channel an operator actually redirects: `change-harness gate
    // example > gate.yaml` writes text-mode stdout, not the JSON envelope
    // `captured_example` reads `data.example` from. #108 §9.3: the merged
    // first half of this issue shipped its verdict round-trip test reading
    // only `data.example` from `--output json`, so this exact channel went
    // untested there and needed a repair. This file does not repeat that.
    //
    // Mutation that must make this fail: prepend a line ahead of the
    // document (e.g. `gate example:\n`), exercised by hand against this
    // test in the card's evidence report, not reproduced here, since
    // reproducing it means editing the emitter itself, which this file must
    // not do.
    //
    // Deliberately not routed through `Workspace::run_json`, and no
    // `--output` flag at all: this must be exactly the plain-text default a
    // caller gets from `gate example > gate.yaml`, not the JSON envelope
    // `captured_example` already covers.
    let workspace = Workspace::initialized();

    let output = Workspace::run(&["gate".to_owned(), "example".to_owned()]);
    assert!(
        output.status.success(),
        "gate example must succeed (exit {:?})",
        output.status.code()
    );

    // Assert only on stdout: the entire point is that stdout, exactly as a
    // shell redirect would capture it, round-trips through `gate register`
    // unchanged. Whatever landed on stderr is not this test's concern —
    // proving that stays true is what this test does, not what it checks.
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();

    let path = workspace.root.join("captured-stdout-gate-example.yaml");
    fs::write(&path, &stdout).unwrap();

    let envelope = workspace.gate_json(&["register", "--definition", &path.display().to_string()]);
    assert_eq!(
        envelope["status"], "success",
        "text-mode stdout, written to a file unchanged, must be accepted by `gate register` \
         — this is the exact `gate example > gate.yaml` workflow an operator would actually \
         run: {envelope}"
    );
    assert_eq!(envelope["data"]["gate_id"], "gate.example");
}

#[test]
fn the_emitted_gate_example_is_complete() {
    // Every field a caller must supply has to appear in the emitted
    // example, or a caller copying it verbatim would still hit a `missing
    // field` error the example was supposed to prevent.
    //
    // "Derive from the struct if you can" (#108 §9.2): `GateDefinition` has
    // no `Option` and no `#[serde(default)]` field, so — unlike
    // `tests/review_example.rs`'s `required_fields`, which has to ask the
    // real deserializer field-by-field to separate required fields from
    // optional ones — every field here is required by construction: Rust's
    // struct-literal syntax will not compile `reference` below unless every
    // field of `GateDefinition` is supplied. The struct itself is already
    // the proof of completeness; `reference_keys` just reads its field names
    // back out through serialization instead of retyping them by hand,
    // which is exactly the kind of hand-maintained claim #108 exists to stop
    // from silently drifting from what the struct actually has.
    //
    // Mutation that must make this fail: emit an example missing any field
    // (#108 mutation 1; also the direct generalization of mutation 3, since
    // every field of this struct is required — there is no optional one to
    // single out).
    let reference = GateDefinition {
        schema: GATE_SCHEMA.to_owned(),
        gate_id: "reference".to_owned(),
        purpose: Some("reference gate".to_owned()),
        semantics: Some("true exits successfully".to_owned()),
        revision: 1,
        argv: vec!["true".to_owned()],
        working_directory: ".".to_owned(),
        timeout_seconds: 1,
        environment: GateEnvironment::default(),
        network_policy: NetworkPolicy::Denied,
        retry_policy: RetryPolicy::default(),
        artifacts: vec![],
    };
    let reference_keys = top_level_keys(
        &serde_json::to_string(&reference).expect("reference gate definition serializes"),
    );

    let example = captured_example();
    let example_keys = top_level_keys(&example);
    for field in &reference_keys {
        assert!(
            example_keys.contains(field),
            "the emitted example is missing field `{field}`:\n{example}"
        );
    }
    assert_eq!(
        example_keys, reference_keys,
        "the example should name exactly the fields `GateDefinition` has, so a reader can see \
         the complete shape in one place"
    );
}

#[test]
fn the_emitted_gate_example_is_rendered_as_yaml() {
    // #108 §3 establishes YAML as the right format here: `parse_definition`
    // reads with `serde_yaml_ng`, the same parser `review example`'s
    // document is read back with. Left unpinned, this would be a choice a
    // future change could silently reverse without any test above noticing
    // — `serde_yaml_ng` accepts JSON as the YAML it syntactically is, so
    // every assertion above would keep passing even if this switched to
    // `serde_json::to_string_pretty`. This is the test that would actually
    // catch that.
    let envelope = Workspace::run_json(&[
        "gate".to_owned(),
        "example".to_owned(),
        "--output".to_owned(),
        "json".to_owned(),
    ]);
    assert_eq!(envelope["data"]["format"], "yaml");

    let example = captured_example();
    assert!(
        !example.trim_start().starts_with('{'),
        "the example must be rendered as YAML, not a JSON object: {example}"
    );
}

#[test]
fn the_emitted_gate_example_argv_actually_runs() {
    // #108 §7: "a gate definition names a command that actually runs; an
    // example naming something absurd teaches badly." That is a claim about
    // the chosen `argv` (`true`), and the standing rule for a claim made
    // under an "establish and justify" instruction is that it ships with its
    // own test.
    //
    // #108b review repair: the first version of this test ran argv in the
    // *test process's own, uncleared* environment, so it could not tell
    // "the example's `environment.allow` names `PATH`" apart from "the
    // example's `environment.allow` is empty" — exactly the class of drift
    // #108 exists to prevent, and exactly what the review caught by
    // emptying the example's allowlist and watching this test not notice.
    // This version instead computes the effective environment the runner
    // would actually construct (`env_clear()`, then only `environment.allow`
    // names present in the ambient environment, then every fixed
    // `environment.set` entry — mirroring `run_attempt_with_validation_cache`,
    // src/runner/mod.rs:159-167) and spawns under exactly that.
    //
    // A plain "did it still spawn" check under that mirrored environment is
    // not sufficient by itself — checked against the real
    // `runner::run_attempt` before writing this, not assumed: every POSIX
    // system this was checked on falls back to a default search path
    // (`/bin`, `/usr/bin`, …) when `PATH` is entirely *absent* from a
    // process's environment, which is exactly what an emptied
    // `environment.allow` produces. `true` lives on that fallback path, so
    // it keeps spawning successfully with no declared `PATH` at all — a
    // dropped-`PATH` bug would still pass a spawn-only check. So the
    // coherence property is asserted directly below: an argv element with
    // no `/` resolves through `PATH`, so `environment.allow`/
    // `environment.set` must actually supply one. The spawn further down
    // still runs for real, under that same mirrored environment, which is
    // what catches an argv that is simply wrong — not found anywhere, not
    // even via that fallback.
    //
    // Mutations that must make this fail: empty the example's
    // `environment.allow` (the coherence assertion below trips, independent
    // of any OS fallback), or replace `true` with a command that does not
    // exist anywhere (the spawn itself fails).
    let example = captured_example();
    let definition: GateDefinition =
        serde_yaml_ng::from_str(&example).expect("the example is well-formed");

    let mut effective_environment: BTreeMap<String, String> = BTreeMap::new();
    for name in &definition.environment.allow {
        if let Ok(value) = std::env::var(name) {
            effective_environment.insert(name.clone(), value);
        }
    }
    for (name, value) in &definition.environment.set {
        effective_environment.insert(name.clone(), value.clone());
    }

    let program = &definition.argv[0];
    if !program.contains('/') {
        assert!(
            effective_environment.contains_key("PATH"),
            "argv[0] `{program}` has no `/`, so resolving it depends on `PATH` — the example's \
             `environment.allow` must actually supply one, or the gate this document describes \
             cannot be relied on to run; an OS-level fallback search path can mask this in a \
             plain spawn check, which is why it is asserted directly instead"
        );
    }

    let mut command = std::process::Command::new(program);
    command.args(&definition.argv[1..]);
    command.env_clear();
    for (name, value) in &effective_environment {
        command.env(name, value);
    }
    let status = command
        .status()
        .unwrap_or_else(|error| panic!("the example argv must be able to start: {error}"));
    assert!(
        status.success(),
        "the example argv {:?} must actually run and succeed under its own declared \
         environment {effective_environment:?}, not merely look plausible",
        definition.argv,
    );
}

#[test]
fn gate_example_is_discoverable_from_help() {
    // #108 §7: a surface nobody finds is the problem restated.
    let output = Workspace::run(&["gate".to_owned(), "--help".to_owned()]);
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(
        help.lines()
            .any(|line| line.trim_start().starts_with("example")),
        "`gate --help` must list `example` as a subcommand: {help}"
    );
}

#[test]
fn gate_example_needs_no_control_repository() {
    // #108 §7: reachable before an operator has anything set up, not only
    // after a failed attempt has taught them the shape by accident. No
    // `--control` anywhere in this test.
    let output = Workspace::run(&[
        "gate".to_owned(),
        "example".to_owned(),
        "--output".to_owned(),
        "json".to_owned(),
    ]);
    assert!(
        output.status.success(),
        "gate example must succeed with no control repository: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}
