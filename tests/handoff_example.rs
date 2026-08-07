//! `handoff example` emits a handoff declaration document by constructing a
//! real [`ActorDeclaration`] and serializing it, so the emitter and the
//! parser that accepts it (`handoff create`) are the same `serde` code and
//! cannot disagree.
//!
//! #180: the same cold-start dead end that motivated `card example`
//! (`tests/card_example.rs`) named `handoff` declaration as the second of
//! two document kinds with no generated example at all. This file proves
//! the same single verifiable result for it: an operator can obtain a
//! complete, valid declaration from the tool itself, and feeding it back to
//! `handoff create` — after replacing the one field no generator could ever
//! know ahead of time, `delivered_sha` — is accepted.

mod support;

use std::{collections::BTreeSet, fs};

use change_harness::{
    domain::handoff::{ActorDeclaration, DeclaredGateFailure},
    policy::convergence::ReasonCategory,
};
use support::Workspace;

/// A card handed off is not this fixture's job — this builds a card ready
/// *to* be handed off, and returns the exact commit `handoff create` must
/// see as its candidate.
///
/// Mirrors `tests/review_example.rs`'s `handed_off()` up to the point a
/// handoff is created, then stops there instead of also creating one: this
/// file's tests are the ones that create the handoff, from the tool's own
/// emitted document.
fn ready_for_handoff() -> (Workspace, String) {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Ready for handoff example",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-100", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-100"]);

    let path = workspace.worktrees.join("F-100");
    fs::create_dir_all(path.join("src")).unwrap();
    fs::write(path.join("src/a.rs"), "fn main() {}\n").unwrap();
    support::git(&path, &["add", "-A"]);
    support::git(&path, &["commit", "-q", "-m", "feat: add a.rs"]);
    workspace.gate(&["run", "--card-id", "F-100", "--gate-id", "gate.unit"]);

    let head = support::capture(&path, &["rev-parse", "HEAD"]);
    (workspace, head)
}

/// Runs `handoff example` for real and returns the document it emitted.
///
/// Deliberately not routed through `Workspace::handoff_raw` / `handoff_json`:
/// those helpers always append `--control <path>`, and `handoff example`
/// declares no such flag — passing it would be a usage error, not a no-op,
/// which is itself part of what constraint 1 in #108 means.
fn captured_example() -> String {
    let envelope = Workspace::run_json(&[
        "handoff".to_owned(),
        "example".to_owned(),
        "--output".to_owned(),
        "json".to_owned(),
    ]);
    assert_eq!(
        envelope["status"], "success",
        "handoff example must succeed: {envelope}"
    );
    envelope["data"]["example"]
        .as_str()
        .expect("the example document as a string")
        .to_owned()
}

/// The hexspeak placeholder `handoff example` uses for `delivered_sha`
/// (`EXAMPLE_DELIVERED_SHA` in `src/commands/handoff.rs`), duplicated here
/// rather than imported: that constant is private, and #180's frozen file
/// scope does not export it, but the deeper reason matches
/// `tests/review_example.rs`'s own `reference_verdict()` precedent — a
/// value this test's ground truth depends on must not be sourced from the
/// code under test.
const PLACEHOLDER_DELIVERED_SHA: &str = "cafebabecafebabecafebabecafebabecafebabe";

/// Replaces the example's placeholder `delivered_sha` with a real commit.
///
/// `handoff create` checks this field against the branch's actual head
/// (`check_delivered_sha`, called from `candidate_of`) before anything
/// else, and refuses any mismatch — unlike `card example`'s `base_sha`,
/// which `card create` never checks against reality, no fixed placeholder
/// could ever be accepted here as emitted, because a generator cannot know
/// a caller's future commit ahead of time. This is exactly the one
/// substitution `handoff example`'s own warning tells an operator to make
/// before installing the document for real; making it here is that same
/// required edit, not a departure from "unchanged" — every other field is
/// fed to `handoff create` exactly as the tool emitted it.
fn with_real_delivered_sha(example: &str, head: &str) -> String {
    let adjusted = example.replace(PLACEHOLDER_DELIVERED_SHA, head);
    assert_ne!(
        adjusted, example,
        "the captured example must contain the placeholder delivered_sha `{PLACEHOLDER_DELIVERED_SHA}` \
         to be substituted; if this fails, `handoff example`'s placeholder value changed and this \
         constant must follow it"
    );
    adjusted
}

/// An independently constructed, fully valid declaration, used only to
/// establish which fields `ActorDeclaration` actually requires. Kept
/// separate from the production `example_declaration` in
/// `src/commands/handoff.rs` — which is private and not exported anyway —
/// so this test's ground truth cannot be affected by a defect in, or a
/// mutation of, the code under test.
///
/// `gate_failures` is populated rather than left `vec![]`: `ActorDeclaration`
/// carries `#[serde(default, skip_serializing_if = "Vec::is_empty")]` on
/// that field, so an empty reference would silently drop the key from its
/// own serialization, making `reference_keys` (below) disagree with the
/// real example's `example_keys` over a field the production emitter
/// genuinely shows.
fn reference_declaration() -> ActorDeclaration {
    ActorDeclaration {
        delivered_sha: "b".repeat(40),
        behavior_delivered: "Reference behavior".to_owned(),
        implementation_decisions: vec!["Reference decision".to_owned()],
        assumptions: vec!["Reference assumption".to_owned()],
        known_limitations: vec!["Reference limitation".to_owned()],
        residual_risks: vec!["Reference risk".to_owned()],
        rollback_notes: "Reference rollback".to_owned(),
        gate_failures: vec![DeclaredGateFailure {
            gate_id: "gate.reference".to_owned(),
            reason_category: ReasonCategory::Regression,
        }],
    }
}

/// Which top-level fields of `reference` the real `ActorDeclaration`
/// deserializer refuses to do without.
///
/// Computed, not declared: for each field, this removes it from the
/// serialized reference and asks `serde_json` whether `ActorDeclaration`
/// still parses. A hardcoded list of "the required fields" is exactly the
/// kind of claim that can silently drift from what the parser actually
/// accepts — the failure mode #108 exists to close for the example document
/// itself — so the test that guards the example's completeness must not
/// reintroduce it for the oracle that checks that example.
fn required_fields(reference: &ActorDeclaration) -> BTreeSet<String> {
    let value = serde_json::to_value(reference).expect("ActorDeclaration serializes");
    let object = value
        .as_object()
        .expect("ActorDeclaration serializes to a document with fields")
        .clone();
    object
        .keys()
        .filter(|key| {
            let mut reduced = object.clone();
            reduced.remove(key.as_str());
            serde_json::from_value::<ActorDeclaration>(serde_json::Value::Object(reduced)).is_err()
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
fn the_emitted_handoff_declaration_example_is_accepted_by_handoff_create() {
    // The test that justifies the card. Capture whatever the tool emits,
    // replace only the one field that can never be known ahead of time
    // (`delivered_sha`), write the result to a file, and feed it to a real
    // `handoff create` invocation — not a parse check, the actual command.
    //
    // Mutation that must make this fail: drop a required field from the
    // emitted example, or emit an empty `implementation_decisions` — see
    // #180 §11 mutations 1 and 3. Neither is reproduced here; both are
    // exercised by hand against this test in the card's evidence report,
    // because reproducing them means editing the emitter itself, which this
    // file must not do.
    let (workspace, head) = ready_for_handoff();
    let example = captured_example();
    let adjusted = with_real_delivered_sha(&example, &head);

    let path = workspace.root.join("captured-example.yaml");
    fs::write(&path, &adjusted).unwrap();

    let envelope = workspace.handoff_json(&[
        "create",
        "--card-id",
        "F-100",
        "--declaration",
        &path.display().to_string(),
    ]);
    assert_eq!(
        envelope["status"], "success",
        "the tool's own example, with only its placeholder `delivered_sha` replaced by the \
         real commit, must be accepted by `handoff create`: {envelope}"
    );
    assert_eq!(
        envelope["data"]["handoff"]["declaration"]["behavior_delivered"],
        "What the candidate actually does.",
        "confirms the example's own content was recorded, not merely that some command \
         returned success"
    );
    assert_eq!(
        envelope["data"]["handoff"]["declaration"]["gate_failures"][0]["gate_id"], "gate.unit",
        "the example's own gate_failures entry was recorded unchanged"
    );
    assert_eq!(envelope["data"]["handoff"]["candidate_sha"], head);
}

#[test]
fn the_emitted_handoff_declaration_example_text_mode_stdout_is_accepted_by_handoff_create() {
    // The channel an operator actually redirects: `change-harness handoff
    // example > decl.yaml` writes text-mode stdout, not the JSON envelope
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
    // caller gets from `handoff example > decl.yaml`.
    let (workspace, head) = ready_for_handoff();

    let output = Workspace::run(&["handoff".to_owned(), "example".to_owned()]);
    assert!(
        output.status.success(),
        "handoff example must succeed (exit {:?})",
        output.status.code()
    );

    // Assert only on stdout: the entire point is that stdout, exactly as a
    // shell redirect would capture it, round-trips through `handoff create`
    // once the one un-knowable field is replaced. Whatever landed on
    // stderr (the warning) is not this test's concern.
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let adjusted = with_real_delivered_sha(&stdout, &head);

    let path = workspace.root.join("captured-stdout-example.yaml");
    fs::write(&path, &adjusted).unwrap();

    let envelope = workspace.handoff_json(&[
        "create",
        "--card-id",
        "F-100",
        "--declaration",
        &path.display().to_string(),
    ]);
    assert_eq!(
        envelope["status"], "success",
        "text-mode stdout, with only its placeholder `delivered_sha` replaced, must be \
         accepted by `handoff create` — this is the exact `handoff example > decl.yaml` \
         workflow an operator would actually run: {envelope}"
    );
    assert_eq!(envelope["data"]["handoff"]["candidate_sha"], head);
}

#[test]
fn the_emitted_handoff_declaration_example_is_complete() {
    // Every field a caller must supply has to appear in the emitted
    // example, or a caller copying it verbatim would still hit a `missing
    // field` error the example was supposed to prevent.
    //
    // Mutation that must make this fail: drop a required field from the
    // emitted example (#180 §11 mutation 1). Also fails on emitting an
    // example missing *any* field, required or optional — a direct
    // consequence of #180's decision to show every field so its shape is
    // visible, mirroring #108 §6's decision for `review example` — see
    // this test's own final assertion.
    let required = required_fields(&reference_declaration());
    assert_eq!(
        required,
        BTreeSet::from([
            "delivered_sha".to_owned(),
            "behavior_delivered".to_owned(),
            "implementation_decisions".to_owned(),
            "assumptions".to_owned(),
            "known_limitations".to_owned(),
            "residual_risks".to_owned(),
            "rollback_notes".to_owned(),
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
        &serde_json::to_string(&reference_declaration()).expect("reference serializes"),
    );
    assert_eq!(
        example_keys, reference_keys,
        "the example should name every field ActorDeclaration has, required or optional, so a \
         reader can see the complete shape — including which fields exist to omit — in one \
         place"
    );
}

#[test]
fn the_emitted_handoff_declaration_example_passes_validate() {
    // #145: deserialization alone does not catch a field that is present
    // but semantically empty. An `implementation_decisions: []` document
    // deserializes into a well-formed `ActorDeclaration` — the key is
    // present, `serde` asks for nothing more — and only `validate()`
    // refuses it (`domain::handoff::tests::an_absent_key_fails_deserialization_and_never_reaches_validate`
    // pins the distinction on the production type). This test pins that
    // `ActorDeclaration::validate` — not merely successful parsing —
    // accepts what the tool emits.
    let example = captured_example();
    let declaration: ActorDeclaration =
        serde_yaml_ng::from_str(&example).expect("the example parses");
    declaration
        .validate()
        .expect("the tool's own example must satisfy validate()");
}

#[test]
fn the_emitted_handoff_declaration_example_names_every_optional_field() {
    // #180 §4: `ActorDeclaration` has exactly one `#[serde(default)]`
    // field, `gate_failures`. The decision to compute that set with the
    // same remove-and-reparse machinery `card example` uses, rather than
    // asserting it by hand, needs a test of its own — see
    // `tests/card_example.rs`'s equivalent for the fuller case.
    let envelope = Workspace::run_json(&[
        "handoff".to_owned(),
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
        BTreeSet::from(["gate_failures".to_owned()]),
        "gate_failures is the only field ActorDeclaration's own #[serde(default)] makes \
         optional"
    );
}

#[test]
fn the_emitted_handoff_declaration_example_warns_that_delivered_sha_must_be_replaced() {
    // Review of #180: unlike `card example`'s `base_sha`, `delivered_sha`'s
    // placeholder cannot round-trip through `handoff create` at all — see
    // `with_real_delivered_sha`'s own doc comment above — so this
    // particular warning is not a proxy for a *later*-stage refusal the way
    // the card one is. It still went unchecked, and a warning nothing reads
    // is not verified to say what it needs to.
    //
    // Asserted on stderr specifically, not through `--output json`: stderr
    // is the channel an operator actually sees running `handoff example`
    // directly (`main.rs` writes every warning there, in both output
    // modes), the same discipline
    // `the_emitted_handoff_declaration_example_text_mode_stdout_is_accepted_by_handoff_create`
    // applies to the document itself. Deliberately not asserted verbatim —
    // only the load-bearing claim, that `delivered_sha` is named and
    // replacement is required — because the exact prose will be reworded
    // over time and a verbatim test would then be deleted rather than
    // fixed.
    //
    // Mutation that must make this fail: reword the warning to drop the
    // `delivered_sha` clause.
    let output = Workspace::run(&["handoff".to_owned(), "example".to_owned()]);
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("delivered_sha") && stderr.contains("replaced"),
        "the warning on stderr must name `delivered_sha` and say it must be replaced before \
         real use: {stderr}"
    );
}

#[test]
fn handoff_example_is_discoverable_from_help() {
    // #108 §6 constraint 2: a surface nobody finds is the problem restated.
    let output = Workspace::run(&["handoff".to_owned(), "--help".to_owned()]);
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(
        help.lines()
            .any(|line| line.trim_start().starts_with("example")),
        "`handoff --help` must list `example` as a subcommand: {help}"
    );
}

#[test]
fn handoff_example_needs_no_control_repository_or_card() {
    // #108 §6 constraint 1: reachable before an operator has anything set
    // up, not only after a failed attempt has taught them the shape by
    // accident. No `--control`, no project, no card anywhere in this test.
    let output = Workspace::run(&[
        "handoff".to_owned(),
        "example".to_owned(),
        "--output".to_owned(),
        "json".to_owned(),
    ]);
    assert!(
        output.status.success(),
        "handoff example must succeed with no control repository and no card: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}
