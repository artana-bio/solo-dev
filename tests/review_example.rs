//! `review example` emits a verdict document by constructing a real
//! [`Verdict`] and serializing it, so the emitter and the parser that accepts
//! it (`review record`) are the same `serde` code and cannot disagree.
//!
//! #108: an independent cold-start operator reconstructed the verdict shape
//! by submitting malformed documents and reading the parser's complaints,
//! field by field — the largest single time sink in their run. This file
//! proves the single verifiable result that closes that gap: an operator can
//! obtain a complete, valid example from the tool itself, and feeding it back
//! to `review record` unchanged is accepted.

mod support;

use std::{collections::BTreeSet, fs};

use change_harness::{
    commands::review::Verdict,
    domain::mutation::MutationExemption,
    domain::review::{
        Decision, Disposition, Finding, FindingSeverity, GateAdequacy, HumanAttestation,
        MutationAuthorship, MutationEvidence, ReviewConduct, ReviewerKind, ReviewerProvenance,
    },
};
use support::Workspace;

/// A card handed off and ready for review.
///
/// The handoff actor is left at its default, `operator` — distinct from the
/// emitted example's `reviewer_actor_id` (`reviewer-example`), so recording
/// the example unchanged does not trip the self-review check.
fn handed_off() -> (Workspace, String) {
    let workspace = Workspace::initialized();
    // The emitted example intentionally demonstrates a typed exemption, so
    // this owning fixture installs its closed policy before cycle creation.
    workspace.install_fixture_mutation_exemption_policy();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    let path = workspace.worktrees.join("F-001");
    fs::create_dir_all(path.join("src")).unwrap();
    fs::write(path.join("src/a.rs"), "fn main() {}\n").unwrap();
    support::git(&path, &["add", "-A"]);
    support::git(&path, &["commit", "-q", "-m", "feat: add a.rs"]);
    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);

    let head = support::capture(&path, &["rev-parse", "HEAD"]);
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
    (workspace, head)
}

/// Runs `review example` for real and returns the document it emitted.
///
/// Deliberately not routed through `Workspace::review_raw` / `review_json`:
/// those helpers always append `--control <path>`, and `review example`
/// declares no such flag — passing it would be a usage error, not a
/// no-op, which is itself part of what constraint 1 in #108 means.
fn captured_example() -> String {
    let envelope = Workspace::run_json(&[
        "review".to_owned(),
        "example".to_owned(),
        "--output".to_owned(),
        "json".to_owned(),
    ]);
    assert_eq!(
        envelope["status"], "success",
        "review example must succeed: {envelope}"
    );
    envelope["data"]["example"]
        .as_str()
        .expect("the example document as a string")
        .to_owned()
}

/// An independently constructed, fully valid verdict, used only to establish
/// which fields `Verdict` actually requires. Kept separate from the
/// production `example_verdict` in `src/commands/review.rs` — which is
/// private and not exported anyway — so this test's ground truth cannot be
/// affected by a defect in, or a mutation of, the code under test.
fn reference_verdict() -> Verdict {
    Verdict {
        reviewer_actor_id: "reviewer-reference".to_owned(),
        reviewer_kind: Some(ReviewerKind::Human),
        reviewer_provenance: Some(ReviewerProvenance {
            provider: Some("local".to_owned()),
            model: None,
            session_id: Some("review-session".to_owned()),
            principal_id: Some("human".to_owned()),
        }),
        human_attestation: Some(HumanAttestation {
            evidence_id: "AT-001".to_owned(),
            attestor_actor_id: "attestor".to_owned(),
            attestor_principal_id: None,
            attestor_session_id: None,
            statement: "independent human attestation".to_owned(),
            independently_created: true,
        }),
        mutation_receipt_ids: vec![],
        mutation_exemption: Some(MutationExemption {
            code: "reference_fixture".to_owned(),
            reason: "reference verdict has no executable candidate".to_owned(),
            approved_by: "reference".to_owned(),
        }),
        decision: Decision::Approved,
        reason_category: None,
        findings: vec![Finding {
            severity: FindingSeverity::Low,
            location: "src/reference.rs:1".to_owned(),
            detail: "reference finding, for field-shape probing only".to_owned(),
            disposition: Disposition::Resolved,
        }],
        lesson_checks: vec![],
        gate_adequacy: GateAdequacy {
            gates_observe_acceptance: true,
            unobserved_behaviors: vec![],
            basis: "reference basis".to_owned(),
            mutation_evidence: Some(MutationEvidence::Demonstrated {
                mutation: "reference mutation".to_owned(),
                failing_test: "reference_failing_test".to_owned(),
                oracle: "reference oracle".to_owned(),
                authorship: MutationAuthorship::ReviewerDevised,
            }),
        },
        residual_risks: vec!["reference risk".to_owned()],
        human_reviewer: true,
        review_conduct: Some(ReviewConduct::SeparateProcess),
    }
}

/// Which top-level fields of `reference` the real `Verdict` deserializer
/// refuses to do without.
///
/// Computed, not declared: for each field, this removes it from the
/// serialized reference and asks `serde_json` whether `Verdict` still parses.
/// A hardcoded list of "the required fields" is exactly the kind of claim
/// that can silently drift from what the parser actually accepts — the
/// failure mode #108 exists to close for the example document itself — so
/// the test that guards the example's completeness must not reintroduce it
/// for the oracle that checks that example.
fn required_fields(reference: &Verdict) -> BTreeSet<String> {
    let value = serde_json::to_value(reference).expect("Verdict serializes");
    let object = value
        .as_object()
        .expect("Verdict serializes to a document with fields")
        .clone();
    object
        .keys()
        .filter(|key| {
            let mut reduced = object.clone();
            reduced.remove(key.as_str());
            serde_json::from_value::<Verdict>(serde_json::Value::Object(reduced)).is_err()
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
fn the_emitted_example_is_accepted_by_review_record() {
    // The test that justifies the card. Capture whatever the tool emits,
    // write it to a file unchanged, and feed it to a real `review record`
    // invocation — not a parse check, the actual command.
    //
    // Mutation that must make this fail: emit an example missing a required
    // field (drop `gate_adequacy`), or with a plausible-but-wrong value
    // (`decision: approve` rather than `approved`) — see #108 mutations 1
    // and 2. Both are exercised by hand against this test in the card's
    // evidence report; neither is reproduced here; because reproducing them
    // means editing the emitter itself, which this file must not do.
    let (workspace, reviewed) = handed_off();
    let example = captured_example();

    let path = workspace.root.join("captured-example.yaml");
    fs::write(&path, &example).unwrap();

    let envelope = workspace.review_json(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path.display().to_string(),
        // #120: `review record` now refuses when `--actor` disagrees with
        // the verdict's declared `reviewer_actor_id`. The captured example
        // (unchanged, per this file's whole point) always declares
        // `reviewer-example` (`example_verdict` in `src/commands/review.rs`),
        // so this must name the same reviewer or the fixture would be
        // refused for a reason unrelated to what this file tests.
        "--actor",
        "reviewer-example",
    ]);
    assert_eq!(
        envelope["status"], "success",
        "the tool's own example must be accepted by `review record` unchanged: {envelope}"
    );
    assert_eq!(envelope["data"]["review"]["decision"], "approved");
    assert_eq!(
        envelope["data"]["review"]["reviewer_actor_id"], "reviewer-example",
        "confirms the example's own content was recorded, not merely that some command \
         returned success"
    );
    assert_eq!(
        envelope["data"]["review"]["candidate_sha"], reviewed,
        "the review binds to the exact candidate the fixture handed off"
    );
}

#[test]
fn the_emitted_example_text_mode_stdout_is_accepted_by_review_record() {
    // The channel an operator actually redirects: `change-harness review
    // example > verdict.yaml` writes text-mode stdout, not the JSON
    // envelope `captured_example` reads `data.example` from. That channel
    // was previously unpinned by any test in this file — the warning
    // belongs on stderr, but nothing asserted it stayed there, so a future
    // change routing it onto stdout instead would corrupt every such
    // redirect silently, and this file would not notice.
    //
    // Mutations that must make this fail: prepend text ahead of the
    // document (`example_verdict:\n`), and fold the warning into stdout
    // instead of stderr — both exercised by hand against this test in the
    // card's evidence report, not reproduced here, since reproducing them
    // means editing the emitter itself, which this file must not do.
    //
    // Deliberately not routed through `Workspace::run_json`, and no
    // `--output` flag at all: this must be exactly the plain-text default a
    // caller gets from `review example > verdict.yaml`, not the JSON
    // envelope `captured_example` already covers.
    let (workspace, reviewed) = handed_off();

    let output = Workspace::run(&["review".to_owned(), "example".to_owned()]);
    assert!(
        output.status.success(),
        "review example must succeed (exit {:?})",
        output.status.code()
    );

    // Assert only on stdout: the entire point is that stdout, exactly as a
    // shell redirect would capture it, round-trips through `review record`
    // unchanged. Whatever landed on stderr is not this test's concern —
    // proving that stays true is what this test does, not what it checks.
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();

    let path = workspace.root.join("captured-stdout-example.yaml");
    fs::write(&path, &stdout).unwrap();

    let envelope = workspace.review_json(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path.display().to_string(),
        // #120: `review record` now refuses when `--actor` disagrees with
        // the verdict's declared `reviewer_actor_id`. The captured example
        // (unchanged, per this file's whole point) always declares
        // `reviewer-example` (`example_verdict` in `src/commands/review.rs`),
        // so this must name the same reviewer or the fixture would be
        // refused for a reason unrelated to what this file tests.
        "--actor",
        "reviewer-example",
    ]);
    assert_eq!(
        envelope["status"], "success",
        "text-mode stdout, written to a file unchanged, must be accepted by `review record` \
         — this is the exact `review example > verdict.yaml` workflow an operator would \
         actually run: {envelope}"
    );
    assert_eq!(envelope["data"]["review"]["decision"], "approved");
    assert_eq!(
        envelope["data"]["review"]["candidate_sha"], reviewed,
        "the review binds to the exact candidate the fixture handed off"
    );
}

// #95 gap 1, §10 test 2. Distinct from the test above: that one proves
// stdout round-trips through `review record` unchanged, full stop, and would
// stay green even if `mutation_evidence` were dropped from the emitter,
// because nothing about that fixture card's write scope needed it. This one
// asserts the new shape is actually present on stdout — not `--output json`
// — before ever handing it to `review record`.
//
// Mutation (§11.1): drop `mutation_evidence` from `example_verdict` (or blank
// it via `#[serde(skip_serializing_if)]` firing on real content, which cannot
// happen for a required, non-empty `Demonstrated` value, but a regression
// that made it happen would be exactly the failure mode this guards). This
// test must fail on the shape assertion below before it ever reaches
// `review record`.
#[test]
fn the_emitted_example_carries_mutation_evidence_on_stdout_and_is_accepted() {
    let (workspace, reviewed) = handed_off();

    let output = Workspace::run(&["review".to_owned(), "example".to_owned()]);
    assert!(
        output.status.success(),
        "review example must succeed (exit {:?})",
        output.status.code()
    );
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();

    // The new shape, on stdout, before this ever touches `review record`.
    // `status: demonstrated` is `MutationEvidence`'s serde tag (internally
    // tagged, `tag = "status"`); the other three are `Demonstrated`'s fields.
    for needle in [
        "mutation_evidence",
        "status: demonstrated",
        "mutation:",
        "failing_test:",
        "oracle:",
    ] {
        assert!(
            stdout.contains(needle),
            "stdout must carry `{needle}`, the new mutation-evidence shape: {stdout}"
        );
    }

    let path = workspace
        .root
        .join("captured-stdout-mutation-evidence.yaml");
    fs::write(&path, &stdout).unwrap();

    let envelope = workspace.review_json(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path.display().to_string(),
        // #120: `review record` now refuses when `--actor` disagrees with
        // the verdict's declared `reviewer_actor_id`. The captured example
        // (unchanged, per this file's whole point) always declares
        // `reviewer-example` (`example_verdict` in `src/commands/review.rs`),
        // so this must name the same reviewer or the fixture would be
        // refused for a reason unrelated to what this file tests.
        "--actor",
        "reviewer-example",
    ]);
    assert_eq!(
        envelope["status"], "success",
        "the example carrying mutation_evidence must still be accepted by `review record`: {envelope}"
    );
    assert_eq!(
        envelope["data"]["review"]["gate_adequacy"]["mutation_evidence"]["status"], "demonstrated",
        "the recorded review must carry what stdout showed, not a stripped-down version of it"
    );
    assert_eq!(
        envelope["data"]["review"]["candidate_sha"], reviewed,
        "the review binds to the exact candidate the fixture handed off"
    );
}

// #28 §12 mutation 3. Mutation: drop `authorship` from `example_verdict`'s
// `MutationEvidence::Demonstrated` (in practice, since `authorship` is a
// required, non-`Option` field and the struct literal would then fail to
// compile, the equivalent mutation exercised for the evidence report is
// `#[serde(skip_serializing)]` on the field — it still omits the key from
// what `review example` emits, and the round trip below fails exactly as a
// dropped field would: `authorship` is required at deserialize time with no
// default, so re-parsing an example missing it is refused).
//
// This test must fail on the shape assertion below before it ever reaches
// `review record` — mirroring how the test above pins `mutation_evidence`
// itself, this pins the field #28 adds inside it.
#[test]
fn the_emitted_example_carries_mutation_authorship_on_stdout_and_is_accepted() {
    let (workspace, reviewed) = handed_off();

    let output = Workspace::run(&["review".to_owned(), "example".to_owned()]);
    assert!(
        output.status.success(),
        "review example must succeed (exit {:?})",
        output.status.code()
    );
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();

    for needle in [
        "authorship: reviewer_devised",
        "review_conduct: separate_process",
    ] {
        assert!(
            stdout.contains(needle),
            "stdout must carry `{needle}`, the new fields #28 adds: {stdout}"
        );
    }

    let path = workspace
        .root
        .join("captured-stdout-mutation-authorship.yaml");
    fs::write(&path, &stdout).unwrap();

    let envelope = workspace.review_json(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path.display().to_string(),
        // #120, as above: the captured example always declares
        // `reviewer-example`.
        "--actor",
        "reviewer-example",
    ]);
    assert_eq!(
        envelope["status"], "success",
        "the example carrying authorship and review_conduct must still be accepted by `review \
         record`: {envelope}"
    );
    assert_eq!(
        envelope["data"]["review"]["gate_adequacy"]["mutation_evidence"]["authorship"],
        "reviewer_devised",
        "the recorded review must carry what stdout showed, not a stripped-down version of it"
    );
    assert_eq!(
        envelope["data"]["review"]["review_conduct"], "separate_process",
        "the recorded review must carry the declared conduct too"
    );
    assert_eq!(
        envelope["data"]["review"]["candidate_sha"], reviewed,
        "the review binds to the exact candidate the fixture handed off"
    );
}

#[test]
fn the_emitted_example_is_complete() {
    // Every field a caller must supply has to appear in the emitted example,
    // or a caller copying it verbatim would still hit a `missing field`
    // error the example was supposed to prevent.
    //
    // Mutation that must make this fail: emit an example missing a required
    // field (drop `gate_adequacy` — #108 mutation 1). Also fails on emitting
    // an example missing *any* field, required or optional (#108 mutation
    // 3), which is a stricter property than the card names but a direct
    // consequence of #108 §6's decision to show every field so its shape is
    // visible — see this test's own final assertion.
    let required = required_fields(&reference_verdict());
    // Sanity check, not the mechanism: `required` above is derived by asking
    // the real deserializer, field by field, not read off this list. Kept so
    // a reader sees at a glance what "required" currently resolves to,
    // without re-running the removal loop in their head. Deleting this
    // assertion would not weaken this test's ability to catch a mutation —
    // the loop below, over the independently computed `required` set, does
    // that on its own.
    assert_eq!(
        required,
        BTreeSet::from([
            "reviewer_actor_id".to_owned(),
            "decision".to_owned(),
            "gate_adequacy".to_owned(),
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

    let reference_keys =
        top_level_keys(&serde_json::to_string(&reference_verdict()).expect("reference serializes"));
    assert_eq!(
        example_keys, reference_keys,
        "the example should name every field Verdict has, required or optional, so a reader \
         can see the complete shape — including which fields exist to omit — in one place"
    );
}

#[test]
fn review_example_is_discoverable_from_help() {
    // #108 §6 constraint 2: a surface nobody finds is the problem restated.
    let output = Workspace::run(&["review".to_owned(), "--help".to_owned()]);
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(
        help.lines()
            .any(|line| line.trim_start().starts_with("example")),
        "`review --help` must list `example` as a subcommand: {help}"
    );
}

#[test]
fn review_example_needs_no_control_repository_or_card() {
    // #108 §6 constraint 1: reachable before an operator has anything set
    // up, not only after a failed attempt has taught them the shape by
    // accident. No `--control`, no project, no card anywhere in this test.
    let output = Workspace::run(&[
        "review".to_owned(),
        "example".to_owned(),
        "--output".to_owned(),
        "json".to_owned(),
    ]);
    assert!(
        output.status.success(),
        "review example must succeed with no control repository and no card: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}
