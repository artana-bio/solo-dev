//! Feeds `SKILL.md`'s own embedded example documents through the exact
//! parser and `validate()` the product calls on them, so a document the
//! guide teaches that the product itself refuses cannot go unnoticed again.
//!
//! Contract 145. `SKILL.md:317`'s handoff declaration reads
//! `implementation_decisions: []`; `ActorDeclaration::validate`
//! (`src/domain/handoff.rs:118`) refuses exactly that, naming the field,
//! with `CH-POLICY-INCOMPLETE-HANDOFF`. An operator who copied the template
//! was refused. `tests/skill_guide.rs` and `tests/skill_guide_execution.rs`
//! only ever look at fenced ` ```bash ` blocks and the `change-harness ...`
//! invocations inside them; neither has ever looked inside a fenced
//! ` ```yaml ` block, so nothing caught this.
//!
//! # The extraction rule
//!
//! [`yaml_blocks`] walks `SKILL.md` once, tracking the most recent Markdown
//! heading line (any level, `#` through `######`) as it goes, and records
//! every fenced ` ```yaml ` block together with that heading and the 1-based
//! line number of its opening fence. That heading is the binding key
//! [`KNOWN_DOCUMENTS`] below checks membership against -- "a preceding
//! heading", one of the three binding mechanisms Contract 145 §6 names.
//!
//! # The placeholder rule
//!
//! Both of today's two embedded documents carry one field whose value cannot
//! be taken literally: `base_sha: <exact 40-hex cycle baseline>` and
//! `delivered_sha: <git rev-parse HEAD in the allocated worktree>`. `<...>`
//! is the only such convention read in either document -- confirmed by
//! reading both blocks in full while writing this test; neither contains any
//! other bracket, brace, or mustache marker.
//!
//! [`substitute_and_verify_placeholders`] replaces a **whole-value**
//! `field: <...>` line -- the value is nothing but the placeholder span,
//! start to end -- with a fixed, valid 40-hex constant ([`PLACEHOLDER_SHA`])
//! **only** for field names it is explicitly told are SHA-shaped for that
//! document -- `base_sha` for the card draft, `delivered_sha` for the
//! handoff declaration. Both fields are validated identically downstream, as
//! exactly 40 hex characters (`src/domain/card.rs:621`,
//! `src/domain/handoff.rs:121`), so one fixed constant exercises both real
//! validators without asserting anything a placeholder cannot honestly
//! claim.
//!
//! Every other shape a `<...>` span can take is deliberately left completely
//! unsubstituted *and* is treated as an immediate, named failure of this
//! function -- not passed on for the real parser or `validate()` to maybe
//! catch. There are two such shapes, and an earlier version of this file
//! (`42efd19`) only caught the first:
//!
//! - A whole-value placeholder on a field outside the known SHA list, e.g.
//!   `title: <fill this in>`.
//! - A placeholder **embedded** in a larger value rather than being the
//!   whole of it -- e.g. a card draft's
//!   `acceptance.behaviors: ["<the behavior a test can fail>"]` or
//!   `write_scope.include: ["<paths you will write>"]`. A whole-value-only
//!   check reads that entire bracketed string as ordinary content (it starts
//!   with `[`, not `<`), so the line passes through unsubstituted *and*
//!   unrejected, and the placeholder text ships as a literal, syntactically
//!   valid list entry -- deserializing and validating exactly as if someone
//!   had actually written a real behavior or a real path. A reviewer
//!   mutation against `42efd19` found this by construction, not by reading:
//!   it is the exact silent pass Contract 145 §6 warns against, sitting in
//!   the one place the original rule never looked.
//!
//! Contract 145 §6 asks what should happen when the rule meets a placeholder
//! it does not understand, and rules out both wrong answers: inventing a
//! substitution ("will make a bad document pass") and skipping the document
//! ("turns a new untested block into silence"). Failing here, by name, for
//! both shapes above, is the third option, and it is strictly safer than
//! "leave it and hope validation downstream happens to reject it": a
//! placeholder dropped into a field (or a list entry within one) with no
//! format check of its own would otherwise deserialize and validate as an
//! odd but well-formed string. See
//! `a_placeholder_on_an_unrecognized_field_fails_rather_than_passing_or_being_skipped`
//! and `a_placeholder_embedded_in_a_list_item_fails_rather_than_passing_or_being_skipped`
//! below for this decision's own tests, per Contract 145 §10.
//!
//! # What this rule misses
//!
//! It only recognizes the single `<...>` bracket convention actually present
//! in `SKILL.md` today, in any position within a value. A document that
//! marked a placeholder some other way entirely -- `___`, `TODO`, `{{ }}` --
//! would sail through unrecognized as ordinary text, because
//! [`placeholder_shape`] never flags it as a placeholder in the first place,
//! so it is never substituted *or* rejected. Neither of today's two
//! documents does this (confirmed by reading both in full), so the gap does
//! not affect the current measurement; it is named here rather than
//! silently assumed away, the same discipline `tests/recovery_text.rs`
//! documents for its own backtick convention.
//!
//! # The binding guard
//!
//! `tests/skill_guide.rs:129` refuses to pass on an empty invocation list, so
//! a parser that silently matched nothing cannot pass vacuously.
//! `every_fenced_yaml_block_in_skill_md_is_bound_to_a_known_document` below
//! is that guard's stronger form for this file, per Contract 145 §8: every
//! ` ```yaml ` block found anywhere in `SKILL.md`, not merely the two this
//! file already knows to look for, must resolve to an entry in
//! [`KNOWN_DOCUMENTS`] or the whole suite fails, naming the block's line and
//! heading. A block added under a heading nobody taught this file about is a
//! test failure, not silence.

use change_harness::domain::{card::CardDraft, handoff::ActorDeclaration};

/// Reads `SKILL.md` from the repository root.
///
/// Deliberately re-implemented here rather than imported: `tests/skill_guide.rs`
/// and `tests/skill_guide_execution.rs` each already define an identical
/// two-line helper of this name, but Contract 145 §5 forbids editing either
/// file (to make room for a `pub` export) and an integration test binary
/// cannot import an item from another one directly.
fn skill_md() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/SKILL.md");
    std::fs::read_to_string(path).expect("SKILL.md should be readable at the repository root")
}

/// One fenced ` ```yaml ` block, together with the nearest preceding
/// Markdown heading line and the 1-based line number of its opening fence.
#[derive(Debug)]
struct YamlBlock {
    heading: String,
    line: usize,
    body: String,
}

/// Extracts every fenced ` ```yaml ` block in `source`, in document order,
/// each bound to the most recent Markdown heading line (any level) that
/// precedes it. See the module doc comment, "The extraction rule".
fn yaml_blocks(source: &str) -> Vec<YamlBlock> {
    let mut blocks = Vec::new();
    let mut current_heading = String::new();
    let mut in_block = false;
    let mut block_start_line = 0;
    let mut body_lines: Vec<&str> = Vec::new();

    for (index, line) in source.lines().enumerate() {
        let line_number = index + 1;
        let trimmed = line.trim();
        if in_block {
            if trimmed == "```" {
                blocks.push(YamlBlock {
                    heading: current_heading.clone(),
                    line: block_start_line,
                    body: body_lines.join("\n"),
                });
                in_block = false;
                body_lines.clear();
            } else {
                body_lines.push(line);
            }
            continue;
        }
        if trimmed.starts_with('#') {
            trimmed.clone_into(&mut current_heading);
        } else if trimmed == "```yaml" {
            in_block = true;
            block_start_line = line_number;
        }
    }
    assert!(
        !in_block,
        "SKILL.md has an unterminated ```yaml fence starting at line {block_start_line}"
    );
    blocks
}

/// The exact heading text immediately preceding each of `SKILL.md`'s two
/// embedded documents today. The explicit table Contract 145 §6 asks for:
/// membership here is what "bound to a known document" means below.
const CARD_DRAFT_HEADING: &str = "### 3. Write a bounded card";
const HANDOFF_DECLARATION_HEADING: &str = "### 6. Verify and hand off";
const KNOWN_DOCUMENTS: &[&str] = &[CARD_DRAFT_HEADING, HANDOFF_DECLARATION_HEADING];

/// A fixed, syntactically valid 40-hex object id substituted for any
/// recognized SHA placeholder. Not a real commit: `CardDraft::validate` and
/// `ActorDeclaration::validate` check only shape (`len() == 40`, all hex),
/// never that the object exists -- the same fixture value `src/domain/card.rs`
/// and `src/domain/handoff.rs`'s own unit tests use (`"a".repeat(40)`).
const PLACEHOLDER_SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// The field name and trimmed value of a `field: value` line. Leading
/// indentation is ignored, so a nested field is recognized the same as a
/// top-level one. `None` for a line with no top-level `:` at all -- plain
/// prose, or a mapping header such as `write_scope:` whose value is empty.
fn field_and_value(line: &str) -> Option<(&str, &str)> {
    let (field, rest) = line.trim_start().split_once(':')?;
    Some((field.trim(), rest.trim()))
}

/// How, if at all, a line's value carries a `<...>` placeholder span.
///
/// The distinction this makes -- whole value versus embedded -- is the
/// entire fix for the gap a reviewer mutation found in `42efd19`: a
/// placeholder is not only the shape `field: <...>`, it is also
/// `field: ["<...>"]` and any other value that merely *contains* a `<...>`
/// span without being nothing else. See the module doc comment, "The
/// placeholder rule".
enum PlaceholderShape {
    /// No `<` ever followed by a later `>` anywhere in the value.
    None,
    /// The value is *exactly* one `<...>` span and nothing else. The only
    /// shape a whole-value SHA substitution is allowed to replace.
    WholeValue,
    /// A `<...>` span exists somewhere in the value, but the value is not
    /// exactly that span -- e.g. embedded in a list item, or alongside other
    /// text. Always a failure in [`substitute_and_verify_placeholders`],
    /// regardless of field name: this is the shape the original
    /// whole-value-only check could not see at all.
    Embedded,
}

/// Classifies `value` per [`PlaceholderShape`]. Finds the first `<`, then
/// checks whether a `>` follows it anywhere later in the value; whether that
/// span is the *entire* value decides `WholeValue` versus `Embedded`.
fn placeholder_shape(value: &str) -> PlaceholderShape {
    let Some(open) = value.find('<') else {
        return PlaceholderShape::None;
    };
    if !value[open + 1..].contains('>') {
        return PlaceholderShape::None;
    }
    if value.starts_with('<') && value.ends_with('>') && value.len() > 1 {
        PlaceholderShape::WholeValue
    } else {
        PlaceholderShape::Embedded
    }
}

/// Substitutes a whole-value `field: <...>` line whose field is named in
/// `sha_fields` with [`PLACEHOLDER_SHA`], preserving indentation. Any other
/// `<...>` span -- an unrecognized field, or a recognized field whose
/// placeholder is not the whole value -- fails immediately, naming the field
/// and the value, rather than being silently substituted or silently passed
/// through. See the module doc comment, "The placeholder rule".
fn substitute_and_verify_placeholders(body: &str, sha_fields: &[&str]) -> Result<String, String> {
    let mut out = Vec::with_capacity(body.lines().count());
    for line in body.lines() {
        let indent = &line[..line.len() - line.trim_start().len()];
        let Some((field, value)) = field_and_value(line) else {
            out.push(line.to_owned());
            continue;
        };
        match placeholder_shape(value) {
            PlaceholderShape::None => out.push(line.to_owned()),
            PlaceholderShape::WholeValue if sha_fields.contains(&field) => {
                out.push(format!("{indent}{field}: {PLACEHOLDER_SHA}"));
            }
            PlaceholderShape::WholeValue | PlaceholderShape::Embedded => {
                return Err(format!(
                    "`{field}: {value}` carries a `<...>` placeholder this extraction rule has \
                     no substitution for (known whole-value SHA fields for this document: \
                     {sha_fields:?}); failing rather than silently substituting a guess or \
                     skipping the document"
                ));
            }
        }
    }
    Ok(out.join("\n"))
}

/// Deserializes `body` as a [`CardDraft`] with the same parser
/// `commands::card::read_draft` uses (`CardDraft::parse`, which calls
/// `serde_yaml_ng` internally), then calls the same [`CardDraft::validate`]
/// the product calls before a card may activate.
fn validate_card_draft(body: &str) -> Result<(), String> {
    let substituted = substitute_and_verify_placeholders(body, &["base_sha"])?;
    let draft = CardDraft::parse(&substituted).map_err(|error| format!("deserialize: {error}"))?;
    draft
        .validate()
        .map_err(|error| format!("validate: {error}"))
}

/// Deserializes `body` as an [`ActorDeclaration`] with the same parser
/// `commands::handoff::read_declaration` uses (`serde_yaml_ng` directly --
/// `ActorDeclaration` has no `parse` method of its own), then calls the same
/// [`ActorDeclaration::validate`] the product calls before a handoff may be
/// created.
fn validate_handoff_declaration(body: &str) -> Result<(), String> {
    let substituted = substitute_and_verify_placeholders(body, &["delivered_sha"])?;
    let declaration: ActorDeclaration =
        serde_yaml_ng::from_str(&substituted).map_err(|error| format!("deserialize: {error}"))?;
    declaration
        .validate()
        .map_err(|error| format!("validate: {error}"))
}

/// Finds the one block bound to `heading`, failing loudly rather than
/// silently returning `None` to a caller that might ignore it.
fn block_under<'a>(blocks: &'a [YamlBlock], heading: &str) -> &'a YamlBlock {
    blocks
        .iter()
        .find(|block| block.heading == heading)
        .unwrap_or_else(|| {
            panic!("SKILL.md carries no fenced ```yaml block under heading {heading:?}")
        })
}

/// The card draft under "### 3. Write a bounded card" is the positive
/// control: Contract 145 §2 measured it valid once `base_sha` is
/// substituted, and it must keep passing in the same run the handoff
/// declaration below fails in -- a test that failed both would prove nothing
/// about its own discrimination.
#[test]
fn card_draft_in_skill_md_validates() {
    let skill_md = skill_md();
    let blocks = yaml_blocks(&skill_md);
    let block = block_under(&blocks, CARD_DRAFT_HEADING);
    validate_card_draft(&block.body).unwrap_or_else(|error| {
        panic!(
            "SKILL.md:{} card draft (under {:?}) failed to validate as \
             change_harness::domain::card::CardDraft: {error}",
            block.line, block.heading
        );
    });
}

/// The handoff declaration under "### 6. Verify and hand off" is the subject
/// of Contract 145: at this contract's base commit it reads
/// `implementation_decisions: []`, which
/// `change_harness::domain::handoff::ActorDeclaration::validate` refuses.
/// This test must fail, naming the handoff declaration and the
/// `implementation_decisions` field, until `SKILL.md` is corrected.
#[test]
fn handoff_declaration_in_skill_md_validates() {
    let skill_md = skill_md();
    let blocks = yaml_blocks(&skill_md);
    let block = block_under(&blocks, HANDOFF_DECLARATION_HEADING);
    validate_handoff_declaration(&block.body).unwrap_or_else(|error| {
        panic!(
            "SKILL.md:{} handoff declaration (under {:?}) failed to validate as \
             change_harness::domain::handoff::ActorDeclaration: {error}",
            block.line, block.heading
        );
    });
}

/// Contract 145 §8's guard: every fenced ` ```yaml ` block anywhere in
/// `SKILL.md`, not merely the two named above, must be bound to a known
/// document heading, or the suite fails naming the block rather than
/// silently skipping it.
#[test]
fn every_fenced_yaml_block_in_skill_md_is_bound_to_a_known_document() {
    let skill_md = skill_md();
    let blocks = yaml_blocks(&skill_md);

    // Companion to `tests/skill_guide.rs:129`: a scan that silently matched
    // nothing would let every assertion below pass vacuously no matter what
    // SKILL.md said.
    assert!(
        !blocks.is_empty(),
        "found zero fenced ```yaml blocks in SKILL.md; either both embedded documents were \
         removed or the ```yaml fence convention this file scans for was not followed"
    );

    for block in &blocks {
        assert!(
            KNOWN_DOCUMENTS.contains(&block.heading.as_str()),
            "SKILL.md:{} carries a fenced ```yaml block under heading {:?} that no binding in \
             tests/guide_documents.rs's KNOWN_DOCUMENTS recognizes; Contract 145 §8 requires \
             this to fail the suite rather than pass unvalidated -- add a binding (and a test \
             that calls validate() on it) instead of widening this list to silence the failure",
            block.line,
            block.heading,
        );
    }

    // The reverse direction: an entry in `KNOWN_DOCUMENTS` with no matching
    // block would mean a document this suite claims to validate was quietly
    // removed from the guide. `block_under` would already panic on that in
    // the two tests above; asserting the exact count here as well keeps the
    // same failure visible from this structural test too, and pins the
    // re-measured block count from Contract 145 §2 (two, today) as a live
    // check rather than only a claim in a report.
    assert_eq!(
        blocks.len(),
        KNOWN_DOCUMENTS.len(),
        "expected exactly {} known embedded document(s) in SKILL.md, found {}: {:?}",
        KNOWN_DOCUMENTS.len(),
        blocks.len(),
        blocks
            .iter()
            .map(|block| (block.line, block.heading.as_str()))
            .collect::<Vec<_>>(),
    );
}

/// Decoupled from `SKILL.md`'s real content, so this cannot pass merely
/// because today's guide happens to be shaped right: confirms
/// [`yaml_blocks`] itself -- heading tracking and line numbering -- against
/// a synthetic document, the same "no false positive" discipline
/// `tests/skill_guide_execution.rs`'s
/// `a_correctly_matched_reserve_and_run_pair_is_accepted` uses for its own
/// extraction helpers.
#[test]
fn yaml_block_extraction_binds_heading_and_line_from_synthetic_markdown() {
    let synthetic = "# Title\n\n\
        ## Section one\n\n\
        prose\n\n\
        ```yaml\n\
        a: 1\n\
        b: 2\n\
        ```\n\n\
        ## Section two\n\n\
        ```yaml\n\
        c: 3\n\
        ```\n";

    let blocks = yaml_blocks(synthetic);
    assert_eq!(blocks.len(), 2, "{blocks:?}");
    assert_eq!(blocks[0].heading, "## Section one");
    assert_eq!(blocks[0].body, "a: 1\nb: 2");
    assert_eq!(blocks[0].line, 7);
    assert_eq!(blocks[1].heading, "## Section two");
    assert_eq!(blocks[1].body, "c: 3");
    assert_eq!(blocks[1].line, 14);
}

/// This decision's own test, per Contract 145 §10: `SKILL.md`'s two
/// documents do not carry an unrecognized placeholder today, so this
/// constructs one synthetically rather than waiting for the guide to grow
/// one before the rule's behavior on it can be checked.
#[test]
fn a_placeholder_on_an_unrecognized_field_fails_rather_than_passing_or_being_skipped() {
    let body = "base_sha: <exact 40-hex cycle baseline>\ntitle: <fill this in>\n";
    let error = substitute_and_verify_placeholders(body, &["base_sha"])
        .expect_err("a placeholder on a field outside the known SHA list must fail");
    assert!(
        error.contains("title"),
        "the failure should name the unrecognized field: {error}"
    );
}

/// The other half of the same decision: a placeholder the rule *does*
/// recognize is substituted, not also failed.
#[test]
fn a_recognized_sha_placeholder_is_substituted_with_a_valid_40_hex_constant() {
    let body = "base_sha: <exact 40-hex cycle baseline>\n";
    let substituted = substitute_and_verify_placeholders(body, &["base_sha"])
        .expect("a recognized placeholder field must substitute cleanly");
    assert_eq!(substituted, format!("base_sha: {PLACEHOLDER_SHA}"));
}

/// This shape's own test. A reviewer mutation against `42efd19` found that
/// `substitute_and_verify_placeholders`' original whole-value-only check
/// read a placeholder embedded in a list item -- exactly the shape a card
/// draft's `acceptance.behaviors` or `write_scope.include` entry would take
/// if someone wrote a placeholder there -- as ordinary content: the value
/// `["<paths you will write>"]` does not itself start with `<`, so it was
/// neither substituted nor rejected, and shipped through as a literal,
/// well-formed, wrong string. This must fail exactly like a placeholder on
/// an unrecognized field, naming the field, not pass through silently.
#[test]
fn a_placeholder_embedded_in_a_list_item_fails_rather_than_passing_or_being_skipped() {
    let body = "base_sha: <exact 40-hex cycle baseline>\ninclude: [\"<paths you will write>\"]\n";
    let error = substitute_and_verify_placeholders(body, &["base_sha"]).expect_err(
        "a placeholder embedded in a list item must fail, not pass through as literal text",
    );
    assert!(
        error.contains("include"),
        "the failure should name the field carrying the embedded placeholder: {error}"
    );
}
