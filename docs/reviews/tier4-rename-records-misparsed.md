# rename-records-misparsed  [medium]  fix_accepted=True

## Summary
`worktree_state` strips three bytes from every NUL-separated field of `git status --porcelain=v1 -z`, but a rename record carries a second field that is the bare source path with no `XY ` prefix, so a staged rename is reported as a dirty path that exists nowhere (`src/alpha.rs` becomes `/alpha.rs`).

## Root cause
src/git/inspect.rs, `worktree_state`, lines 387-402 at HEAD dc9e490. Two coupled mistakes:

Line 390 — the command omits `--no-renames`:
    ["status", "--porcelain=v1", "-z", "--untracked-files=all"]

Line 396 — the parser treats every NUL-separated field as a whole record and blindly removes three bytes:
    .map(|record| record.get(3..).unwrap_or(record).to_owned())

`git status --porcelain=v1 -z` does not emit one field per record. A rename or copy emits TWO fields: `XY <dest>\0<source>\0` (in `-z` the arrow is dropped and the order is reversed relative to the human format, so destination comes first). The parser has no concept of the second field, so it applies the status-prefix strip to a bare path. The `.unwrap_or(record)` fallback is what makes it silent: when `get(3..)` fails on a non-char-boundary it hands back the untouched field instead of complaining, so the failure looks like it sometimes works.

## Files
src/git/inspect.rs, tests/git_inspection.rs

## Proposed fix
Both halves are required; I implemented and mutation-checked both in my worktree, and `cargo fmt --check` + `cargo clippy --all-targets --all-features -- -D warnings` are clean, `cargo test --lib` 383 passed, and git_inspection/handoff/candidate_verification/project_config/worktree_allocation/archive_cleanup/dry_run_parity/lifecycle/combined_verification all pass.

1. Add `--no-renames` to the `git status` invocation in `worktree_state`:
    ["status", "--porcelain=v1", "-z", "--untracked-files=all", "--no-renames"]
   Available since Git 2.18; `MINIMUM_GIT_VERSION` is 2.50.0 (src/git/inspect.rs:22), so it is safe. Verified real output: with the flag the same staged rename becomes `"D  src/alpha.rs\0A  src/beta.rs\0"` — one field per record, and BOTH real paths appear, which is exactly what a cleanliness report wants. It also makes the answer independent of the repo's `status.renames` setting. Do NOT instead teach the parser to consume a second field: keeping rename detection on buys nothing here (`dirty_paths` is a `Vec<String>` used only for reporting and emptiness) and adds a lookahead that can over-consume.

2. Extract a `fn parse_status_porcelain_z(raw: &str) -> Result<Vec<String>, HarnessError>` that refuses a malformed record instead of truncating it: require `record.as_bytes()[2] == b' '`, take `record[3..]`, require it non-empty, else `HarnessError::GitCommand(format!("malformed status record: {record:?}"))`. Byte 3 is always a char boundary because the `XY ` prefix is ASCII, so no UTF-8 hazard remains. Put the reason for `--no-renames` in the doc comment at this function — it is the only place a future reader will look before deleting the flag.

Failing-first tests to write, in tests/git_inspection.rs (real repos, next to the existing `clean_and_dirty_state_counts_untracked_files`):

  a_staged_rename_reports_both_real_paths — commit `src/alpha.rs`, `git mv src/alpha.rs src/beta.rs`, assert dirty_paths contains BOTH `src/alpha.rs` and `src/beta.rs`, then the non-vacuity guard: every reported path must resolve either on disk or via `git cat-file -t HEAD:<path>`. Against unfixed code this fails with `["src/beta.rs", "/alpha.rs"]`.

  a_rename_source_named_like_a_status_prefix_is_not_truncated — commit a file literally named `R  looks-like-status.txt`, rename it, assert the source is reported verbatim. This is the test that kills a heuristic fix.

  the_report_does_not_depend_on_status_renames_config — same rename under `status.renames` = true / false / copies, assert `["alpha.rs", "beta.rs"]` every time.

Unit tests in the `src/git/inspect.rs` test module, because the strict-parse half is NOT observable through real Git once the flag is present (see overcorrection_risk):

  a_status_record_without_the_two_letter_code_is_refused_not_truncated — `parse_status_porcelain_z` must return Err for `"src/alpha.rs\0"`, `"ab\0"`, `"?? \0"`, `"a\0"`.
  status_records_keep_the_whole_path_after_the_two_letter_code — `"?? a\0 M na me\nwith\nnewlines.txt\0D  一二三.md\0"` parses to the three exact paths.

Mutations that must fail these tests (all four run and confirmed):
  M1 revert both halves -> a_staged_rename_reports_both_real_paths fails at the source assertion with `["src/beta.rs", "/alpha.rs"]`; the other two real-repo tests fail with `ha.rs` in place of `alpha.rs`.
  M2 keep `--no-renames`, restore the lax `get(3..).unwrap_or(record)` -> ALL FIVE real-repo tests still pass (this is why the unit test is mandatory); a_status_record_without_the_two_letter_code_is_refused_not_truncated fails.
  M3 keep the strict parser, drop `--no-renames` -> worktree_state hard-errors `GitCommand("malformed status record: \"src/alpha.rs\"")`, and a_rename_source_named_like_a_status_prefix_is_not_truncated fails by *assertion* rather than error, proving the shape check alone is not a rename detector.
  M4 delete `--untracked-files=all` -> existing `clean_and_dirty_state_counts_untracked_files` still holds it.

## Over-correction risk
Three distinct opposite failures, each with a guard.

1. A parser that refuses too much. `parse_status_porcelain_z` returning Err turns any unexpected Git output into a hard failure in `handoff`, `work resume`, `work verify`, `archive close` and `project init` validation — a cleanliness check that cannot answer blocks the whole lifecycle. M3 shows this is not hypothetical: the strict parser WITHOUT `--no-renames` errors on every staged rename. The two halves must land together, and the guard that holds the parser open is status_records_keep_the_whole_path_after_the_two_letter_code, which requires `?? `, ` M`, `D  `, embedded newlines and multi-byte paths to parse rather than be refused, plus the existing tests/git_inspection.rs:173 paths_containing_spaces_and_unicode_are_inspected_correctly and tests/git_inspection.rs:245 clean_and_dirty_state_counts_untracked_files.

2. A fix that drops the rename source instead of un-mangling it — e.g. reporting only the `XY `-prefixed field and discarding unprefixed ones, or filtering out "paths that do not exist". That silently under-reports: a staged rename moving a file OUT of a card's write scope would leave the source invisible in `dirty_paths`, which is the same class of hole `check_change` exists to close on the committed side (src/policy/verification.rs:220). Guard: a_staged_rename_reports_both_real_paths asserts BOTH paths, not just that no invented path appears.

3. A heuristic fix — "strip three bytes only when the field looks like it has an `XY ` prefix". Guard: a_rename_source_named_like_a_status_prefix_is_not_truncated. A rename source literally named `R  looks-like-status.txt` satisfies every such heuristic and is silently truncated to `looks-like-status.txt`; I reproduced exactly this (`R  weird.txt` -> `weird.txt`, exists? false). A relative of this is a lookahead fix that consumes a second field on `R`/`C` and swallows the following record; guard a_rename_does_not_swallow_the_next_dirty_path asserts the exact set `["README.md","alpha.rs","untracked.txt","zeta.rs"]` from a rename plus an unrelated modification plus an untracked file.

Note on existing coverage, since it is the reason this survived: NO existing test drives `worktree_state` against a rename. tests/git_inspection.rs:245 clean_and_dirty_state_counts_untracked_files claims "clean and dirty state" but only writes one untracked file and asserts with `.contains("untracked.txt")`; tests/git_inspection.rs:173 only asserts `clean` on a clean tree; tests/git_inspection.rs:434 inspection_performs_no_repository_mutation merely calls it and discards the result; src/policy/verification.rs:650 builds `dirty_paths` by hand and never touches the parser. The one real rename test in the area, tests/git_inspection.rs:320 diff_detects_renames_across_a_real_commit_pair, is sound but exercises the OTHER parser — its presence is probably what made this area look covered.

## VERIFIER objections
I tried to refute this and could not. The defect is real and the fix is correct. Reproduced independently, then mutation-checked every claim. What follows is what survived scrutiny and what did not.

WHAT I CONFIRMED

1. The reproduction is exact. Real repo, git 2.50.1 (Apple Git-155), `git mv src/alpha.rs src/beta.rs` staged: `git status --porcelain=v1 -z --untracked-files=all` emits `R  src/beta.rs\0src/alpha.rs\0`. Driving the harness's own `worktree_state` through a real-repo test at unfixed HEAD produced verbatim `["src/beta.rs", "/alpha.rs"]` — the exact string the finding predicts. Also reproduced the data-dependent corruption: a source literally named `R  looks-like-status.txt` came back as `looks-like-status.txt`, i.e. wrong but plausible.

2. The relocation is correct and is the most valuable part of the finding. `src/git/diff.rs::parse_raw_z` is genuinely fine — I read it and it consumes a second field on Renamed/Copied in the right order (source first, dest second), matching real `--raw -z` output. `tests/git_inspection.rs::diff_detects_renames_across_a_real_commit_pair` passes at every mutation I applied. The defect really is only in `src/git/inspect.rs::worktree_state`.

3. Not deliberate. No D-nnn decision in docs/IMPLEMENTATION_PLAN.md touches status parsing or rename detection; Section 13.1 only says "clean/dirty state including untracked files" and "parsing MUST use stable porcelain … with NUL delimiters". No code comment defends `get(3..).unwrap_or(record)`.

4. Blast radius is as stated, and understated in the harness's favour. `dirty_paths` is only ever `.join(", ")`ed into message text (src/policy/verification.rs:161-170, src/commands/handoff.rs:342-344, src/config/validate.rs:452-463) or serialized (src/commands/work.rs:895, 938). It is never matched against a card's write scope and never joined to a filesystem path, so the fix cannot cause a new refusal anywhere. `clean` is genuinely never wrong.

5. Mutations M1, M2, M3 all behave exactly as claimed, including the subtle part. M1 (both halves reverted): four real-repo tests fail at the assertion that matters. M2 (`--no-renames` kept, lax parser restored): all real-repo tests still pass and only the unit test `a_status_record_without_the_two_letter_code_is_refused_not_truncated` fails — the finding was right that the unit test is mandatory, not decorative. M3 (strict parser, no flag): three tests fail with `GitCommand("malformed status record: \"src/alpha.rs\"")` while `a_rename_source_named_like_a_status_prefix_is_not_truncated` fails by *assertion* with `["plain.txt", "looks-like-status.txt"]`, which is precisely the claim that a shape check is not a rename detector. That distinction is real.

6. Full gate green with the fix: `cargo fmt --check` clean, `cargo clippy --all-targets --all-features -- -D warnings` clean, `cargo test` exit 0, 808 passed / 0 failed (801 baseline + 7 new).

OBJECTIONS THAT STAND

A. Mutation M4 is FALSE as claimed. The finding asserts "M4 delete `--untracked-files=all` -> existing `clean_and_dirty_state_counts_untracked_files` still holds it." It does not. I deleted the flag and ran the ENTIRE suite: `cargo test` exit 0, 808 passed, 0 failed. Nothing in the repo pins that flag. The reason is that every existing fixture writes its untracked file at the repository root, where Git's default `-unormal` reports it anyway; one directory down the default collapses to `?? nested/` and the file is never named. This matters because the fix edits exactly that argv array. I added `an_untracked_file_inside_an_untracked_directory_is_named` to /Users/alvaro/Documents/Code/change-harness/.claude/worktrees/wf_81894ca4-835-12/tests/git_inspection.rs, mutation-checked: it fails with `left: ["nested/"]`.

B. The proposed fix as literally specified panics. The text says "require `record.as_bytes()[2] == b' '`". That is an index, and it panics on any record shorter than 3 bytes — including `"a\0"`, which is in the finding's own list of inputs the unit test must see refused. A `bytes.len() < 4` guard has to come first. The finding claims it implemented and mutation-checked this, so the implementation presumably differs from the prose; but the prose is what a repairer would copy. I implemented it with the length guard.

C. Two callers are missing from the blast-radius list, and they matter for the over-correction half, not the reporting half. `src/commands/gate.rs:582` and `src/commands/work.rs:351` both call `worktree_state` and read `.clean` only, so the corrupt path never surfaces there — but the strict parser's hard-error path now also reaches `gate run` and `work start` step 12 (Section 13.2's post-creation cleanliness validation). The finding's over-correction section enumerates handoff / work resume / work verify / archive close / project init and stops. A freshly created worktree cannot contain a rename, so this is theoretical, but the analysis should have named them.

D. The two halves are not independent, and the finding presents them as if they were. With the strict parser in place, `--no-renames` stops being an accuracy improvement and becomes load-bearing for the command not to hard-fail — M3 demonstrates `worktree_state` erroring on every staged rename. `parse_status_porcelain_z` is a private free function whose correctness is now a property of one caller's argv. The doc comment mitigates this and the repo's own doctrine (D-058: surface what you cannot resolve rather than silently omitting it) favours the loud version, so I would keep it — but it is a tripwire bolted to the real fix, not a second fix.

E. Severity "medium" is generous. No gate is bypassed, no data is lost, no refusal is skipped, and `clean` is provably never wrong. The damage is confined to one JSON array and some message text. Low-to-medium is the honest range. The finding's own strongest argument for medium — that `dirty_paths` is machine-readable and an agent could act on `/alpha.rs` — is fair, but nothing in this repo consumes it programmatically today.

F. Cosmetic: `a_rename_source_named_like_a_status_prefix_is_not_truncated` creates a file named `R  looks-like-status.txt`. Fine on macOS and Linux; it would not survive a Windows checkout. The repo appears unix-only, so this is a note rather than an objection.

## VERIFIER missed
1. `--untracked-files=all` is completely unguarded — see objection A. Verified by running the full 808-test suite with the flag deleted: everything passes. The investigator asserted the opposite without running it, which is the same class of error the review's central lesson warns about: trusting that a test named for a behaviour checks that behaviour. `clean_and_dirty_state_counts_untracked_files` does not hold that flag; it only holds that a root-level untracked file is reported. I added a guard at /Users/alvaro/Documents/Code/change-harness/.claude/worktrees/wf_81894ca4-835-12/tests/git_inspection.rs (`an_untracked_file_inside_an_untracked_directory_is_named`), mutation-confirmed to fail with `["nested/"]`.

2. No over-refusal guard for a conflicted worktree. The over-correction section correctly identifies "a parser that refuses too much" as risk 1, but every guard it proposes is a synthetic string in a unit test. The real shape most likely to blindside a strict status parser in this harness is a mid-merge worktree — `handoff` and `work verify` run in exactly that state, and `UU`/`AA`/`DU` records are what Git emits there. I added `a_conflicted_worktree_is_still_reportable` in the same file, which builds a real merge conflict and asserts `worktree_state` still answers. It passes, so this is a guard rather than a defect, but it was the missing one.

3. `src/commands/gate.rs:582` and `src/commands/work.rs:351` — two `worktree_state` callers absent from the blast-radius enumeration (objection C).

4. The defect is already on the books. docs/DEFECT-REGISTER.md, Tier 4, in the run-on paragraph at lines 86-92: "Rename records are mis-parsed into paths that do not exist." It is listed among the not-yet-fixed Tier 4 items. The finding never mentions this, and does not propose a register update. It also does not note that the register's wording is ambiguous between the two parsers — which is precisely the trap the investigator fell into and then correctly climbed out of. That disambiguation ("the `--raw -z` diff parser is correct; the `status --porcelain=v1 -z` parser is not") is the single most useful sentence produced here and belongs in the register entry when it is marked fixed, or the next reader will re-audit diff.rs.

5. Nothing asserts the refusal's error classification. `HarnessError::GitCommand` maps to `ErrorCode::ExternalGitCommand` / exit category ExternalTool — i.e. "the external tool did something unexpected", which is the right bucket for malformed Git output. But no proposed test pins it, so a future refactor could reclassify a malformed status record as a harness defect (exit 10) unnoticed. Minor, and consistent with how other Git-parse errors in this module are handled.

Files touched during verification (all under the isolated worktree):
/Users/alvaro/Documents/Code/change-harness/.claude/worktrees/wf_81894ca4-835-12/src/git/inspect.rs — `--no-renames` added to `worktree_state`; `parse_status_porcelain_z` extracted with a `bytes.len() < 4 || bytes[2] != b' '` guard; two unit tests.
/Users/alvaro/Documents/Code/change-harness/.claude/worktrees/wf_81894ca4-835-12/tests/git_inspection.rs — the four proposed real-repo tests plus the two guards I found missing.
