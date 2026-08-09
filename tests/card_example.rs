//! `card example` emits a card draft document by constructing a real
//! [`CardDraft`] and serializing it, so the emitter and the parser that
//! accepts it (`card create`) are the same `serde` code and cannot disagree.
//!
//! #180: a third independent cold-start operator, authoring their first card
//! from `README.md`, dead-ended on `acceptance: invalid type: sequence,
//! expected struct Acceptance` with a recovery that said outright no example
//! existed. This file proves the single verifiable result that closes that
//! gap: an operator can obtain a complete, valid card draft from the tool
//! itself, and feeding it back to `card create` unchanged is accepted.

mod support;

use std::{collections::BTreeSet, fs};

use change_harness::domain::card::{
    Acceptance, CardDraft, NamedGates, PROOF_MAP_SCHEMA, ProofMap, ProofMapEntry, Risk, WriteScope,
};
use support::Workspace;

/// A project with an active cycle named to match the emitted example's own
/// `cycle_id` (`C-100`) — `card create` requires the named cycle to exist
/// and accept cards (`cycle_accepting_cards`), so this is the one piece of
/// state the example's own document cannot supply for itself.
fn cycle_ready_for_card_create() -> Workspace {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-100",
        "--objective",
        "Example cycle for card example acceptance",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-100"]);
    workspace
}

/// Runs `card example` for real and returns the document it emitted.
///
/// Deliberately not routed through `Workspace::card_raw` / `card_json`:
/// those helpers always append `--control <path>`, and `card example`
/// declares no such flag — passing it would be a usage error, not a no-op,
/// which is itself part of what constraint 1 in #108 means.
fn captured_example() -> String {
    let envelope = Workspace::run_json(&[
        "card".to_owned(),
        "example".to_owned(),
        "--output".to_owned(),
        "json".to_owned(),
    ]);
    assert_eq!(
        envelope["status"], "success",
        "card example must succeed: {envelope}"
    );
    envelope["data"]["example"]
        .as_str()
        .expect("the example document as a string")
        .to_owned()
}

/// An independently constructed, fully valid card draft, used only to
/// establish which fields `CardDraft` actually requires. Kept separate from
/// the production `example_card_draft` in `src/commands/card.rs` — which is
/// private and not exported anyway — so this test's ground truth cannot be
/// affected by a defect in, or a mutation of, the code under test.
///
/// `proof_map` is populated rather than left `None`: `CardDraft` carries
/// `#[serde(default, skip_serializing_if = "Option::is_none")]` on that
/// field, so a `None` reference would silently drop the key from its own
/// serialization, making `reference_keys` (below) disagree with the real
/// example's `example_keys` over a field the production emitter genuinely
/// shows. A reference oracle that cannot see its own field is not an oracle.
fn reference_card_draft() -> CardDraft {
    CardDraft {
        card_id: "F-900".parse().expect("`F-900` is a well-formed card id"),
        cycle_id: "C-900".parse().expect("`C-900` is a well-formed cycle id"),
        title: "Reference title".to_owned(),
        goal: "Reference goal".to_owned(),
        non_goals: vec!["Reference non-goal".to_owned()],
        risk: Risk::Low,
        change_kind: "feature".to_owned(),
        base_sha: "b".repeat(40),
        write_scope: WriteScope {
            include: vec!["src/reference.rs".to_owned()],
            exclude: vec![],
        },
        contract_reads: vec!["reference.domain".to_owned()],
        contract_changes: vec!["reference.domain".to_owned()],
        depends_on: vec![],
        exclusive_resources: vec!["reference-resource".to_owned()],
        named_gates: NamedGates {
            feature: vec!["gate.reference".to_owned()],
            review: vec![],
            integration: vec!["gate.reference".to_owned()],
        },
        acceptance: Acceptance {
            behaviors: vec!["Reference behavior".to_owned()],
            regressions: vec![],
        },
        generated_artifacts: vec![],
        review_policy: "independent".to_owned(),
        rollback_strategy: "revert".to_owned(),
        proof_map: Some(ProofMap {
            schema: PROOF_MAP_SCHEMA.to_owned(),
            entries: vec![ProofMapEntry {
                id: Some("proof-1".to_owned()),
                invariant: "reference invariant".to_owned(),
                precondition: "reference precondition".to_owned(),
                assertion: "reference assertion".to_owned(),
                mutation: "reference mutation".to_owned(),
                gate_oracle: Some("gate.reference".to_owned()),
            }],
            claim_boundary: "reference boundary".to_owned(),
        }),
    }
}

/// Which top-level fields of `reference` the real `CardDraft` deserializer
/// refuses to do without.
///
/// Computed, not declared: for each field, this removes it from the
/// serialized reference and asks `serde_json` whether `CardDraft` still
/// parses. A hardcoded list of "the required fields" is exactly the kind of
/// claim that can silently drift from what the parser actually accepts —
/// the failure mode #108 exists to close for the example document itself —
/// so the test that guards the example's completeness must not reintroduce
/// it for the oracle that checks that example.
fn required_fields(reference: &CardDraft) -> BTreeSet<String> {
    let value = serde_json::to_value(reference).expect("CardDraft serializes");
    let object = value
        .as_object()
        .expect("CardDraft serializes to a document with fields")
        .clone();
    object
        .keys()
        .filter(|key| {
            let mut reduced = object.clone();
            reduced.remove(key.as_str());
            serde_json::from_value::<CardDraft>(serde_json::Value::Object(reduced)).is_err()
        })
        .cloned()
        .collect()
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
fn the_emitted_card_draft_example_is_accepted_by_card_create() {
    // The test that justifies the card. Capture whatever the tool emits,
    // write it to a file unchanged, and feed it to a real `card create`
    // invocation — not a parse check, the actual command.
    //
    // Unlike `handoff example`'s `delivered_sha`, nothing here needs
    // editing before `card create` accepts it: `card create` never checks
    // `base_sha` against a real baseline (`require_cycle_baseline` runs only
    // at `card activate`), so the placeholder hex value round-trips as-is.
    // The only fixture state this needs is a cycle named `C-100`, matching
    // the example's own `cycle_id`, active and accepting cards.
    //
    // Mutation that must make this fail: drop a required field from the
    // emitted example, or emit `acceptance` as a sequence instead of the
    // `Acceptance` struct the operator's dead end names — see #180 §11
    // mutations 1 and 2. Neither is reproduced here; both are exercised by
    // hand against this test in the card's evidence report, because
    // reproducing them means editing the emitter itself, which this file
    // must not do.
    let workspace = cycle_ready_for_card_create();
    let example = captured_example();

    let path = workspace.root.join("captured-example.yaml");
    fs::write(&path, &example).unwrap();

    let envelope = workspace.card_json(&["create", "--draft", &path.display().to_string()]);
    assert_eq!(
        envelope["status"], "success",
        "the tool's own example must be accepted by `card create` unchanged: {envelope}"
    );
    assert_eq!(
        envelope["data"]["card_id"], "F-100",
        "confirms the example's own content was recorded, not merely that some command \
         returned success"
    );
    assert_eq!(envelope["data"]["cycle_id"], "C-100");
    assert_eq!(envelope["data"]["state"], "draft");
}

#[test]
fn the_emitted_card_draft_example_text_mode_stdout_is_accepted_by_card_create() {
    // The channel an operator actually redirects: `change-harness card
    // example > draft.yaml` writes text-mode stdout, not the JSON envelope
    // `captured_example` reads `data.example` from.
    //
    // Mutation that must make this fail: decorate the emitted text-mode
    // stdout (for instance, prepend a line ahead of the document) — see
    // #180 §11 mutation 4. Exercised by hand against this test in the
    // card's evidence report, not reproduced here, since reproducing it
    // means editing the emitter itself, which this file must not do.
    //
    // Deliberately not routed through `Workspace::run_json`, and no
    // `--output` flag at all: this must be exactly the plain-text default a
    // caller gets from `card example > draft.yaml`.
    let workspace = cycle_ready_for_card_create();

    let output = Workspace::run(&["card".to_owned(), "example".to_owned()]);
    assert!(
        output.status.success(),
        "card example must succeed (exit {:?})",
        output.status.code()
    );

    // Assert only on stdout: the entire point is that stdout, exactly as a
    // shell redirect would capture it, round-trips through `card create`
    // unchanged. Whatever landed on stderr (the warning) is not this test's
    // concern — proving that stays true is what this test does.
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();

    let path = workspace.root.join("captured-stdout-example.yaml");
    fs::write(&path, &stdout).unwrap();

    let envelope = workspace.card_json(&["create", "--draft", &path.display().to_string()]);
    assert_eq!(
        envelope["status"], "success",
        "text-mode stdout, written to a file unchanged, must be accepted by `card create` — \
         this is the exact `card example > draft.yaml` workflow an operator would actually \
         run: {envelope}"
    );
    assert_eq!(envelope["data"]["card_id"], "F-100");
}

#[test]
fn the_emitted_card_draft_example_is_complete() {
    // Every field a caller must supply has to appear in the emitted
    // example, or a caller copying it verbatim would still hit a `missing
    // field` error the example was supposed to prevent.
    //
    // Mutation that must make this fail: drop a required field from the
    // emitted example (#180 §11 mutation 1).
    let required = required_fields(&reference_card_draft());
    // Sanity check, not the mechanism: `required` above is derived by
    // asking the real deserializer, field by field, not read off this
    // list. Kept so a reader sees at a glance what "required" currently
    // resolves to, without re-running the removal loop in their head.
    assert_eq!(
        required,
        BTreeSet::from([
            "card_id".to_owned(),
            "cycle_id".to_owned(),
            "title".to_owned(),
            "goal".to_owned(),
            "risk".to_owned(),
            "change_kind".to_owned(),
            "base_sha".to_owned(),
            "write_scope".to_owned(),
            "named_gates".to_owned(),
            "acceptance".to_owned(),
            "review_policy".to_owned(),
            "rollback_strategy".to_owned(),
        ])
    );

    let example = captured_example();
    let example_keys = top_level_keys(&example);
    for field in &required {
        assert!(
            example_keys.contains(field),
            "the emitted example is missing required field `{field}`:\n{example}"
        );
    }

    let reference_keys = top_level_keys(
        &serde_json::to_string(&reference_card_draft()).expect("reference serializes"),
    );
    assert_eq!(
        example_keys, reference_keys,
        "the example should name every field CardDraft has, required or optional, so a reader \
         can see the complete shape — including which fields exist to omit — in one place"
    );
}

#[test]
fn the_emitted_card_draft_example_passes_validate() {
    // #145: deserialization alone does not catch a field that is present
    // but semantically empty (the `implementation_decisions: []` class of
    // defect that card established for `ActorDeclaration`; `CardDraft`'s
    // own equivalent is an empty `acceptance.behaviors` or
    // `named_gates.feature`). This pins that `CardDraft::validate` — not
    // merely `CardDraft::parse` — accepts what the tool emits, as its own,
    // narrower check.
    //
    // Mutation that must make this fail: emit an empty
    // `implementation_decisions` in the *handoff* example — #180 §11
    // mutation 3 names that file specifically, because `handoff example`'s
    // `validate()` is where #145's exact defect class is reproduced for
    // this card. This card's own draft has no field #145 flags the same
    // way, so this test's purpose here is the general guarantee: `validate`
    // is checked at all, independent of whatever `card create`'s own
    // pipeline happens to also check.
    let example = captured_example();
    let draft = CardDraft::parse(&example).expect("the example parses");
    draft
        .validate()
        .expect("the tool's own example must satisfy validate()");
}

#[test]
fn the_emitted_card_draft_example_names_every_optional_field() {
    // #180 §4: `CardDraft` is the first example document with genuinely
    // optional fields, and the decision to reuse #108's remove-and-reparse
    // machinery (`optional_fields` in `src/commands/card.rs`) for it needs
    // a test of its own, or the decision is unverifiable and a reader could
    // wonder whether the reported set is really what the deserializer
    // accepts, or a hand-typed guess.
    let envelope = Workspace::run_json(&[
        "card".to_owned(),
        "example".to_owned(),
        "--output".to_owned(),
        "json".to_owned(),
    ]);
    let optional: BTreeSet<String> = envelope["data"]["optional_fields"]
        .as_array()
        .expect("optional_fields is a JSON array")
        .iter()
        .map(|value| {
            value
                .as_str()
                .expect("each optional field name is a string")
                .to_owned()
        })
        .collect();
    assert_eq!(
        optional,
        BTreeSet::from([
            "non_goals".to_owned(),
            "contract_reads".to_owned(),
            "contract_changes".to_owned(),
            "depends_on".to_owned(),
            "exclusive_resources".to_owned(),
            "generated_artifacts".to_owned(),
            "proof_map".to_owned(),
        ]),
        "every field CardDraft's own #[serde(default)] makes optional must be reported, and \
         nothing else"
    );
}

#[test]
fn the_emitted_card_draft_example_warns_that_base_sha_must_be_replaced() {
    // Review of #180: `base_sha`'s placeholder round-trips through `card
    // create` unchecked (`the_emitted_card_draft_example_is_accepted_by_card_create`
    // above) but is refused one stage later, at `card activate`
    // (`require_cycle_baseline`, `src/commands/card.rs`) — see this file's
    // module doc for the exact refusal shape. The warning on stderr is the
    // only thing that tells an operator to replace it before reaching that
    // later refusal, and nothing checked its content until this test.
    //
    // Asserted on stderr specifically, not through `--output json`: stderr
    // is the channel an operator actually sees running `card example`
    // directly (`main.rs` writes every warning there, in both output
    // modes), the same discipline
    // `the_emitted_card_draft_example_text_mode_stdout_is_accepted_by_card_create`
    // applies to the document itself. Deliberately not asserted verbatim —
    // only the load-bearing claim, that `base_sha` is named and replacement
    // is required — because the exact prose will be reworded over time and
    // a verbatim test would then be deleted rather than fixed.
    //
    // Mutation that must make this fail: reword the warning to drop the
    // `base_sha` clause (verified by hand against this exact test in the
    // card's evidence report).
    let output = Workspace::run(&["card".to_owned(), "example".to_owned()]);
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("base_sha") && stderr.contains("replaced"),
        "the warning on stderr must name `base_sha` and say it must be replaced before real \
         use: {stderr}"
    );
}

#[test]
fn card_example_is_discoverable_from_help() {
    // #108 §6 constraint 2: a surface nobody finds is the problem restated.
    let output = Workspace::run(&["card".to_owned(), "--help".to_owned()]);
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(
        help.lines()
            .any(|line| line.trim_start().starts_with("example")),
        "`card --help` must list `example` as a subcommand: {help}"
    );
}

#[test]
fn card_example_needs_no_control_repository_or_cycle() {
    // #108 §6 constraint 1: reachable before an operator has anything set
    // up, not only after a failed attempt has taught them the shape by
    // accident. No `--control`, no project, no cycle anywhere in this test.
    let output = Workspace::run(&[
        "card".to_owned(),
        "example".to_owned(),
        "--output".to_owned(),
        "json".to_owned(),
    ]);
    assert!(
        output.status.success(),
        "card example must succeed with no control repository and no cycle: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}
