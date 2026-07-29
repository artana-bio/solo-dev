# merge-conflict-token-table  [medium]  fix_accepted=True

## Summary
Two of the six tokens in `ConflictKind::parse` are strings Git never emits (`CONFLICT (directory/file)`, `CONFLICT (distinct types)`), so every type-change conflict is reported as `other`, and `CONFLICT (binary)` is missing entirely, so Git's binary triple (`CONFLICT (binary)` + `Auto-merging` + `CONFLICT (contents)`) yields two Conflict rows for one path — one of them labelled textual, telling an actor to edit conflict markers Git did not write.

## Root cause
All at pristine HEAD (dc9e490) line numbers.

1. src/git/merge.rs:71 — `"CONFLICT (directory/file)" | "CONFLICT (file/directory)" => Self::DirectoryFile,`. The first alternative is a string no supported Git emits. Dead, harmless, but it is the table claiming knowledge it does not have.

2. src/git/merge.rs:72 — `"CONFLICT (distinct types)" => Self::DistinctTypes,`. Git's type token is `CONFLICT (distinct modes)`; "distinct types" is message prose. `ConflictKind::DistinctTypes` is therefore unreachable — a dead enum variant with a dead `name()` arm at src/git/merge.rs:102 — and every file-vs-symlink / file-vs-submodule type change is reported with kind `other`.

3. src/git/merge.rs:66-75 — no arm for `CONFLICT (binary)`, so it falls to `Self::Other`. Combined with src/git/merge.rs:216-236, which pushes every record whose token `starts_with("CONFLICT")` with no per-path reconciliation, one binary file produces two `Conflict` values: `Other` (Structural) and `Content` (Textual).

Consumers of the wrong data:
- src/commands/integration.rs:1085-1090 counts them into the JSON `textual_conflicts` / `structural_conflicts` fields.
- src/commands/integration.rs:1113-1123 prints one `[{kind.name()}] {paths}: {detail}` line per record, so a single binary file yields:
    [other] b.bin: warning: Cannot merge binary files: b.bin (a vs. b)
    [content] b.bin: CONFLICT (content): Merge conflict in b.bin
- `Conflict` is never persisted to disk (no hits in src/domain or src/control), so adding an enum variant needs no migration.

What the existing tests actually assert:

- MUTATION PROOF that the token table is untested: replacing both strings at src/git/merge.rs:71-72 with `MUTANT (...)` and running `cargo test --no-fail-fast` gives 30 suites, all `test result: ok`, 0 failed (381 lib + the acceptance suites). Nothing in the repository exercises `ConflictKind::DistinctTypes` or `ConflictKind::DirectoryFile`.

- src/git/merge.rs:421 `an_unknown_conflict_token_is_kept_and_treated_as_structural` — a fixture asserting the defect as correct behaviour. It parses `CONFLICT (submodule)`, which is a REAL Git 2.50 type token (it is in the binary's short-description table above), and asserts it maps to `Other`/Structural. It does not test an unknown token at all; it pins the gap open.

- src/git/merge.rs:353 `two_sides_adding_the_same_path_conflict` — named for add/add, asserts only `conflicts.len() == 1` and `paths == ["new.txt"]`, never the kind. Git reports add/add via the `CONFLICT (contents)` token (message says `CONFLICT (add/add)`), so nothing in the suite pins add/add's classification. Add/add of text really is textual — Git writes markers — so the add/add half of the claim is FALSE as a behaviour defect; the test is simply not asserting what its name says.

- No test anywhere in src/ or tests/ constructs a binary conflict.

- tests/merge_preflight.rs:139 `a_candidate_conflicting_with_the_moved_branch_is_reported_as_textual` uses a plain text file (shared.txt) and asserts textual_conflicts==1 / structural_conflicts==0 / kind=="content". It is correct and unaffected — it is the pre-existing guard against over-correction.

## Files
src/git/merge.rs, docs/DEFECT-REGISTER.md, docs/IMPLEMENTATION_PLAN.md

## Proposed fix
Implemented and validated in the worktree: `cargo fmt --check` clean, `cargo clippy --all-targets --all-features -- -D warnings` clean, lib 385 passed / 0 failed, `cargo test --test merge_preflight` 16 passed / 0 failed. All changes are confined to src/git/merge.rs.

STEP 1 — write the failing-first tests (all four fail against unfixed code; see the mutation table below):

  /// Adds a committed binary file to `main` before branching.
  fn with_binary(fixture: &Fixture) {
      fs::write(fixture.path.join("b.bin"), b"\x00\x01\x02base\x00").unwrap();
      git(&fixture.path, &["add", "-A"]);
      git(&fixture.path, &["commit", "-q", "-m", "bin base"]);
  }

  #[test] fn a_binary_conflict_is_reported_once_and_is_not_textual() {
      // main gets b.bin; branch "ba" writes AAAA into it, branch "bb" writes BBBB.
      let preview = merge_tree(&fixture.path, "ba", "bb").expect("a preview");
      assert_eq!(preview.conflicts.len(), 1, "unexpected: {preview:?}");
      assert_eq!(preview.conflicts[0].kind, ConflictKind::Binary);
      assert_eq!(preview.of_class(ConflictClass::Textual).count(), 0);
      assert_eq!(preview.of_class(ConflictClass::Structural).count(), 1);
  }

  #[test] fn guard_a_text_conflict_beside_a_binary_one_survives() {
      // branches "ma"/"mb" each change BOTH b.bin and f.txt incompatibly.
      let preview = merge_tree(&fixture.path, "ma", "mb").expect("a preview");
      assert_eq!(preview.conflicts.len(), 2, "unexpected: {preview:?}");
      assert_eq!(preview.of_class(ConflictClass::Textual).count(), 1);
      assert_eq!(preview.of_class(ConflictClass::Textual).next().unwrap().paths, ["f.txt"]);
  }

  #[test] fn a_type_change_is_named_not_lumped_into_other() {
      // branch "ta" writes a regular file `t`; branch "tb" symlinks `t` -> f.txt.
      assert_eq!(preview.conflicts.len(), 1);
      assert_eq!(preview.conflicts[0].kind, ConflictKind::DistinctTypes);
      assert_eq!(preview.conflicts[0].kind.class(), ConflictClass::Structural);
  }

  #[test] fn a_file_against_a_directory_is_named() {
      // branch "fa" writes file `thing`; branch "fb" creates dir `thing/inner.txt`.
      assert_eq!(preview.conflicts.len(), 1);
      assert_eq!(preview.conflicts[0].kind, ConflictKind::DirectoryFile);
  }

STEP 2 — src/git/merge.rs, add a `Binary` variant to `ConflictKind` (after `DistinctTypes`), doc'd "Both sides changed a file Git cannot merge textually."; add `Self::Binary` to the Structural arm of `class()`; add `Self::Binary => "binary"` to `name()`.

STEP 3 — correct the table in `parse` (replacing lines 71-72):

  "CONFLICT (file/directory)" => Self::DirectoryFile,
  "CONFLICT (distinct modes)" => Self::DistinctTypes,
  "CONFLICT (binary)" => Self::Binary,

Delete `"CONFLICT (directory/file)"` and `"CONFLICT (distinct types)"` outright — accepting a token Git cannot emit is exactly how a future test comes to assert a fiction and pass. Add a comment recording that the table was taken from `type_short_descriptions` in Git 2.50's merge-ort and that a token not listed there is correctly handled by the `_ => Self::Other` arm.

STEP 4 — collapse the binary double-report at the end of `parse_preview`, immediately before `Ok(MergePreview { tree, conflicts })` (change `let mut conflicts` is already `mut`):

  // Git reports a binary conflict twice for the same path: `CONFLICT
  // (binary)` and then `CONFLICT (contents)`. The second is a lie about what
  // is in the tree — Git wrote one side verbatim, with no conflict markers —
  // so keeping it would both double the count and tell an actor to edit
  // markers that do not exist.
  let binary: std::collections::HashSet<Vec<String>> = conflicts
      .iter()
      .filter(|conflict| conflict.kind == ConflictKind::Binary)
      .map(|conflict| conflict.paths.clone())
      .collect();
  conflicts.retain(|conflict| {
      conflict.kind != ConflictKind::Content || !binary.contains(&conflict.paths)
  });

The key is the EXACT path vector, not path membership. Both records carry `["b.bin"]` in every binary form observed (real binary content, binary add/add, and a text file marked `binary` in .gitattributes), so equality is sufficient; membership/subset matching would be looser than the evidence supports. The `CONFLICT (binary)` record is the one kept because its detail names both sides ("Cannot merge binary files: b.bin (a vs. b)"), which is what an actor choosing a side needs.

STEP 5 — repair the fixture at src/git/merge.rs:421. `an_unknown_conflict_token_is_kept_and_treated_as_structural` must use a token Git cannot emit, e.g. `ConflictKind::parse("CONFLICT (wormhole)")`, not `CONFLICT (submodule)` (a real token). Otherwise the test silently becomes a submodule-classification test the moment anyone extends the table.

STEP 6 (recommended, not required for correctness) — the remaining nine real tokens currently land in `Other`/Structural, which is the right CLASS but an unhelpful name. Optionally add `Submodule` covering the five `CONFLICT (submodule*)` tokens and `DirectoryRename` covering `CONFLICT (directory rename suggested)` and `CONFLICT (file in way of directory rename)`, and map `CONFLICT (rename involved in collision)` to `RenameRename`. All are Structural. Doing this REQUIRES Step 5 first.

MUTATIONS THAT MUST FAIL (all three verified in the worktree):
  M1 delete the `conflicts.retain(...)` block
     -> a_binary_conflict_is_reported_once_and_is_not_textual fails at `conflicts.len()`: left 2, right 1
     -> guard_a_text_conflict_beside_a_binary_one_survives fails at `conflicts.len()`: left 3, right 2
  M2 `Self::Content | Self::Binary => ConflictClass::Textual`
     -> a_binary_conflict... fails at `of_class(Textual).count()`: left 1, right 0
     -> guard... fails at `of_class(Textual).count()`: left 2, right 1
  M3 replace the two corrected token strings with `MUTANT (...)`
     -> a_type_change_is_named_not_lumped_into_other and a_file_against_a_directory_is_named fail
     (against the unfixed table this mutation changes nothing: all 801 tests still pass, which is the proof the area was untested)

STEP 7 — docs. Update docs/DEFECT-REGISTER.md:86-87, which currently states this claim as open. Add a line to the WP-420 "Delivered notes" in docs/IMPLEMENTATION_PLAN.md (near line 2248) recording that the token table is transcribed from Git's own short-description table, that a binary conflict is reported once and structurally, and that `Other` remains the safe default for a token this Git does not have.

## Over-correction risk
Three distinct ways a too-aggressive fix breaks this, and what holds each open.

1. DE-DUPING TOO BROADLY. The tempting shortcut is "if any binary conflict exists, drop the content conflicts", or "drop any content conflict whose path appears in any binary record". Either silently swallows a genuine text conflict that happens to share a merge with a binary one — and because the preflight's whole job is to refuse, a swallowed textual conflict is the worst possible failure here. Verified: mutating the retain to `conflict.kind != ConflictKind::Content || binary.is_empty()` leaves `a_binary_conflict_is_reported_once_and_is_not_textual` GREEN and fails ONLY `guard_a_text_conflict_beside_a_binary_one_survives`. The binary test alone does not hold this open; the mixed-merge guard is load-bearing and must be written.

2. DROPPING THE WRONG RECORD OF THE PAIR. Keeping the `CONFLICT (contents)` row and discarding the `CONFLICT (binary)` row also fixes the count, and would pass a naive `conflicts.len() == 1` test — while restoring the exact wrong instruction (edit markers that Git did not write). The guard is the `assert_eq!(preview.conflicts[0].kind, ConflictKind::Binary)` plus `of_class(Textual).count() == 0` pair in `a_binary_conflict_is_reported_once_and_is_not_textual`; verified by mutation M2, which fails at the Textual-count assertion, not at the length assertion.

3. RECLASSIFYING TOO MUCH AS STRUCTURAL. "Binary is not textual" invites "content conflicts are risky too" — folding `Content` into Structural, or promoting add/add out of Textual, would make `ConflictClass::Textual` unreachable and the whole textual/structural split meaningless. Evidence says add/add and ordinary content conflicts DO carry `<<<<<<<` markers in the written tree and are genuinely textual. The pre-existing guard is tests/merge_preflight.rs:139 `a_candidate_conflicting_with_the_moved_branch_is_reported_as_textual`, which pins `textual_conflicts == 1`, `structural_conflicts == 0`, `kind == "content"` for a pure text conflict on shared.txt; it must keep passing untouched. Confirmed: 16/16 in that suite pass with the fix applied.

A fourth, quieter one: extending the token table (Step 6) without first repairing src/git/merge.rs:421 would flip `an_unknown_conflict_token_is_kept_and_treated_as_structural` from a fixture asserting a real token maps to `Other` into a compile-green test asserting nothing, or into a spurious failure. Repair that fixture to a fabricated token (`CONFLICT (wormhole)`) in the same change.

## VERIFIER objections
I tried to refute this and could not. The defect reproduces exactly as described, and the fix survives every mutation I could aim at it. The objections below are to the finding's *claims about its own evidence*, not to the code change.

INDEPENDENTLY REPRODUCED (Git 2.50.1, MINIMUM_GIT_VERSION 2.50.0 at src/git/inspect.rs:22):
- `strings` on the git binary yields exactly the 16 standalone `CONFLICT (...)` tokens listed. `CONFLICT (directory/file)` and `CONFLICT (distinct types)` are absent; `CONFLICT (binary)` is present.
- Live `merge-tree --write-tree --name-only -z`: binary-vs-binary emits the triple `CONFLICT (binary)` / `Auto-merging` / `CONFLICT (contents)`, all with paths `["b.bin"]`. File-vs-symlink emits `CONFLICT (distinct modes)` (the string "distinct types" is message prose only). File-vs-directory emits `CONFLICT (file/directory)` in BOTH argument orders.
- Through the crate's own parser at pristine HEAD: `MergePreview { conflicts: [Conflict{paths:["b.bin"],kind:Other,...}, Conflict{paths:["b.bin"],kind:Content,...}] }` — two rows, one falsely Textual. Mixed text+binary gives 3 rows / textual=2.
- Claim C verified: `git cat-file -p <tree>:b.bin | od -c` -> `\0 001 A A A A \0`, zero conflict markers. Git wrote "our" side verbatim. `ConflictClass::Textual` ("resolved by editing the file's contents") is a lie there.
- Not a recorded decision. No D-0xx covers it; docs/DEFECT-REGISTER.md:86-87 already lists it as an open claim.

MUTATIONS I RAN MYSELF (fix applied, tests as proposed) — all fail at the assertion that matters, not an earlier one:
- M1 delete the `retain` block -> binary test fails at `conflicts.len()` (left 2, right 1); guard fails at `len()` (left 3, right 2).
- M2 `Self::Content | Self::Binary => Textual` -> binary test fails at src/git/merge.rs:464 `of_class(Textual).count() == 0`; guard at :483. NOT at the length assertion.
- M4 over-broad dedup (`|| binary.is_empty()`) -> binary test stays GREEN; only the guard fails. The finding's claim that `guard_a_text_conflict_beside_a_binary_one_survives` is load-bearing is CONFIRMED, and it is the only thing standing between this fix and swallowing a real textual conflict.
- M5 keep the Content row, drop the Binary row -> binary test fails at `kind == ConflictKind::Binary` (left: Content). Overcorrection #2 is caught.
Gate with the fix: `cargo fmt --check` clean, `cargo clippy --all-targets --all-features -- -D warnings` clean, `cargo test --no-fail-fast` 805 passed / 0 failed across 30 suites, including tests/merge_preflight.rs 16/16 (the pre-existing textual guard, untouched).

OBJECTION 1 — the "failing-first" claim is FALSE for one of the four tests. Step 1 states "all four fail against unfixed code". I reverted the production code to pristine HEAD while keeping all four new tests: result 8 passed / 3 FAILED. `a_file_against_a_directory_is_named` PASSES at pristine HEAD, because the live alternative `"CONFLICT (file/directory)" => Self::DirectoryFile` on src/git/merge.rs:71 already handled it. It is a characterization test, not a regression test. This is precisely the trap the brief warns about, and the finding walked into it. (Its M3 mutation does catch the test, but M3 is applied to the *fixed* code — that is not the same as failing first.)

OBJECTION 2 — the summary conflates the two dead strings. Only `"CONFLICT (distinct types)"` produces the stated harm. `"CONFLICT (directory/file)"` is inert: it sits in an alternation with the real token, so removing it changes no observable behaviour. The summary sentence ("Two of the six tokens ... so every type-change conflict is reported as `other`") credits both with an effect only one has. The finding concedes this in root_cause ("Dead, harmless") but not in the summary.

OBJECTION 3 — a material fact the finding never states, which bounds the severity AND supplies the missing safety proof. Nothing in the harness branches on `ConflictClass`. `MergePreview::is_clean()` is `self.conflicts.is_empty()` (src/git/merge.rs:137); the refusal at src/commands/integration.rs:1330 uses only that; `ConflictClass` appears exactly twice outside merge.rs, both as JSON counters (src/commands/integration.rs:1086,1089). So neither the defect nor the fix can change what the harness accepts or refuses — a binary conflict always refused and still does. This is a reporting defect, which argues "medium" is generous. It is also the proof that the dedup cannot manufacture a false clean: every dropped `Content` row has a retained `Binary` row with the identical path vector, so a non-empty `conflicts` can never become empty. The finding never makes this argument, and NO test guards it.

OBJECTION 4 — Step 5's premise is overstated. `an_unknown_conflict_token_is_kept_and_treated_as_structural` is called "a fixture asserting the defect as correct behaviour" that "pins the gap open". It does neither. `CONFLICT (submodule)` genuinely maps to `Other`/`Structural`, which is correct before and after the fix; the test blocks nothing and holds nothing open. It is merely mis-named. The repair to `CONFLICT (wormhole)` is worthwhile hygiene and it becomes necessary only if Step 6 is ever done — but do not bank it as a defect found.

OBJECTION 5 — the fix is incomplete against its own stated rationale (see `missed`, symlinks).

OBJECTION 6 — doc drift the fix introduces and Step 7 does not cover. `MergePreview.conflicts` is documented at src/git/merge.rs:130-131 as "Every conflict Git reported, in the order it reported them." After the `retain` that is false: one record is deliberately dropped. Step 7 updates the plan and register but not this doc comment, in a codebase whose whole review lesson is that prose which no longer matches the code is how the next reviewer gets misled.

Step 6 (adding `Submodule`/`DirectoryRename` variants) is unverified scope creep and should not ride along.

## VERIFIER missed
1. SYMLINKS — the same harm the finding calls the core of the defect, left unfixed and unmentioned. Two branches each adding a symlink `link` with different targets produce `CONFLICT (contents)` -> `ConflictKind::Content` -> `ConflictClass::Textual`. I inspected the tree Git wrote: `git ls-tree` shows `120000 blob ... link` and `git cat-file -p <tree>:link` is `m.txt` — one side's target verbatim, zero conflict markers. That is byte-for-byte the same "telling an actor to edit conflict markers Git did not write" that section C uses to justify the whole fix, and the proposed change does nothing about it. The investigator probed binary exhaustively (real binary, add/add, .gitattributes-binary, binary modify/delete) and never probed a symlink content conflict. Unlike the binary case Git gives no separate token here, so it needs a different mechanism (e.g. reading the mode) — which is a reason to scope it out explicitly, not to omit it.

2. BINARY + RENAME/RENAME — the "one path, one row" property does not hold. Base `b.bin`, side A renames to `one.bin` and edits, side B renames to `two.bin` and edits. Git emits `CONFLICT (binary)` with paths `["b.bin"]` and `CONFLICT (rename/rename)` with paths `["b.bin","one.bin","two.bin"]`. The `retain` only drops `Content`, so the fix still yields two rows for one file. Both are `Structural`, so no textual lie and no over-count of the class that matters — acceptable, but the finding asserts a property it did not test. Worse, the `CONFLICT (binary)` row names `b.bin`, a path present in neither side's tree. That is the ADJACENT OPEN CLAIM sitting in the same register paragraph (docs/DEFECT-REGISTER.md:87, "Rename records are mis-parsed into paths that do not exist") and it lives in the same `parse_preview` function. These two findings will collide; whoever lands second must re-verify that the exact-path-vector dedup still matches, because a rename fix that rewrites `paths` is exactly what would silently break it.

3. WHY `distinct modes` IS SAFE TO MAP TO `DistinctTypes` — asserted from message prose, never tested. I tested it: 644-vs-755 with no content change merges cleanly and emits no record at all, and two regular files cannot take a third mode, so a mode-only disagreement cannot produce this token. The mapping is right, but the finding did not establish that `CONFLICT (distinct modes)` is exclusively the type-change token rather than also an executable-bit conflict — which would have mislabelled a permissions dispute as a type change.

4. THE GOOD NEWS THE FINDING DID NOT ESTABLISH — binary rename+modify vs modify (`b.bin` -> `renamed.bin` plus edits on one side, in-place edit on the other) emits `CONFLICT (binary)` and `CONFLICT (contents)` BOTH with paths `["renamed.bin"]` (post-rename name), in both argument orders. This is the one case where the binary row and the content row could plausibly have disagreed on the path and defeated exact-vector matching. They do not. The finding's justification for exact-vector equality rested on three same-path cases and never probed the rename variant that would have falsified it.

5. NO GUARD ON THE ONE DANGEROUS PROPERTY — no test asserts that the `retain` can never empty `conflicts` and turn a refusal into a clean preflight. The reasoning is airtight by construction (the `Binary` row is always retained), but the codebase's stated lesson is that untested invariants are how defects survive, and this is the only invariant here whose violation would be a safety failure rather than a labelling one. A two-line test asserting `!preview.is_clean()` for the binary fixture would close it.
