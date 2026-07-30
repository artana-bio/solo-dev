# Independent review — defect register

Eight independent reviewers, each given the source and the specification and
nothing else: no narrative from the author, no knowledge of who wrote the code
or what had been claimed about it. Each was instructed to find defects rather
than confirm correctness, and to attach a concrete failure scenario to every
finding.

Every reviewer found defects that invalidate a claim recorded in the plan.

Legend: **[V]** reproduced directly during consolidation. **[R]** reproduced by
the reviewer with concrete output.

## Tier 1 — the evidence chain does not hold

**All five are now fixed** (2026-07-29). Each landed with a test that fails
against the unfixed code and a mutation check confirming the enforcement, not
merely the recording, is what the test catches. Until they were fixed, a landed
change carried evidence that could not be trusted, which is the product's entire
proposition.

| # | Defect |
| --- | --- |
| 1 | ✅ **FIXED.** **A gate passes on uncommitted content while the receipt binds the pass to HEAD.** Write a file, do not commit it, the gate passes, delete it. A signed receipt asserts that this commit passed a gate its own tree fails. `handoff` checks cleanliness; `gate run` does not. **[V]** Fixed: the receipt now carries `worktree_clean`, `staleness` reports a dirty or legacy receipt as not applying, and `evidence_is_acceptable` requires `Some(true)`. `gate run` still permits a dirty run — iterating with uncommitted changes is the ordinary loop — it just no longer counts as evidence about the commit. |
| 2 | ✅ **FIXED.** **A re-review approves away a prior critical finding.** Validation inspects only the current review's open findings, never the superseded review's. `WP-320`'s own acceptance line requiring this was never implemented, and the test named for it asserts the defective behaviour is correct. **[V]** Fixed: `check_supersedes` requires every open finding in the superseded review to be named again with a non-blocking disposition. Applies to every re-review, not only approvals — a `changes_requested` that drops a finding leaves the next reviewer a clean predecessor. The test that asserted the defect was correct now dispositions the finding, which is what its name always claimed. |
| 3 | ✅ **FIXED.** **Acceptance's integration digest is never checked, and is computed so it can never match.** Append a member to an integration after acceptance and promotion marks that card landed — never verified, never accepted, not in the landing tree. **[R]** Fixed: `substantive_digest` excludes `status`, which acceptance itself changes and which the old digest covered — that is why the check could never have passed and so was never written. Promotion now compares it and refuses a plan that changed after acceptance. |
| 4 | ✅ **FIXED.** **Two cards can own the same file.** Overlap detection misses patterns carrying a wildcard in the same segment: `src/api_*.rs` against `src/*_handler.rs`. **[V]** Fixed: `segments_intersect` walks both globs together over the string they would have to share, which is exact for `*`-only globs rather than an approximation. Guarded against the opposite failure — `src/a_*.rs` and `src/b_*.rs` still activate together. |
| 5 | ✅ **FIXED.** **A gate's timeout does not bound the gate.** A surviving background process blocks the runner past the deadline; the overrun is recorded as a clean pass. **[R]** Reproduced at 30.1s for a gate declaring 1s. Fixed: the output drain is bounded by the same deadline, kills the process group when a stream is still held past it, and records the overrun as a timeout. |

## Tier 2 — destroys or loses data

| # | Defect |
| --- | --- |
| 6 | ✅ **FIXED.** **`project init` adopts an occupied directory and overwrites its `.gitignore`**, reporting success. Section 9.1 requires refusal. The protection exists for the authority path and was never written for control. **[V]** Fixed: `refuse_occupied_control` names what it found and refuses. It tests what is present rather than emptiness, because an interrupted `init` leaves a lock and a journal before Git exists and refusing those would turn a crash into defect 12 permanently. An empty directory that already exists is still accepted. |
| 7 | ✅ **FIXED.** **`archive close --dry-run` performs a real close** — worktrees removed, branches deleted, cards closed, committed. The function never reads the flag. **[V]** Fixed: `preview_close` runs every check the real close makes — archive integrity, transition legality, unarchived commits, worktree cleanliness — and removes nothing. A preview that skips checks is worse than none, so both are tested: it removes nothing, and it refuses for the same reason the real command would. |
| 8 | ✅ **FIXED.** **`recover --resume` marks failed operations complete without recovering them**, destroying the only record that a partial mutation occurred. Found independently by three reviewers. **[V]** Fixed: `--resume` refuses when any unresolved entry is not `integration.promote`, which is the only operation it can actually finish, and names them. Their disposition stays an operator decision, which is what `recover` without `--resume` is for. |
| 9 | ✅ **FIXED.** **`git add -A` sweeps a crashed operation's residue into authoritative history**, including the lock's own scratch file, which the ignore list does not cover. **[R]** Fixed: staging names the paths the harness owns rather than sweeping the directory, so inclusion is deliberate the way everything else here is deny-by-default. The trade runs the other way — a record type omitted from the list would never be committed — so `control_history_holds_everything_a_lifecycle_writes` runs a full lifecycle and refuses to leave anything untracked. |
| 10 | ✅ **FIXED.** **Archive refs are in no backup.** Those refs are the sole justification for `archive close` deleting branches and worktrees; they live in the candidate repository, and backups cover only authority and control. **[R]** Fixed: a third subject bundles `refs/archive/*` from the candidate. The test named `a_backup_contains_every_archive_ref_the_source_holds` checked the *authority* bundle for `refs/heads/main` and never looked at an archive ref — it now checks both card and integration archive refs. |
| 11 | ✅ **FIXED.** **A failed `work start` strands a card permanently and journals that nothing happened.** The branch, worktree, lock and locator are created before the lease is written; the partial/clean decision inspects only the control repository, which is genuinely clean. `recover` then reports nothing is wrong. **[R]** Fixed: `Steps::outside_control` marks the boundaries whose mutation lands outside control — a branch, a worktree, a moved authority — and reaching one makes the operation partial whatever control looks like. The decision is extracted as `terminal_state` and unit-tested, because it could not be reached through failure injection: see the note below. |

**Six of six Tier 2 defects are now fixed** (2026-07-29).

## Tier 3 — wedges, false reports, wrong targets

| # | Defect |
| --- | --- |
| 12 | ✅ **FIXED.** An interrupted `project init` wedges the project: `init` says run recover, recover says run init. **[R]** Fixed: `init` supersedes an earlier unfinished `project.init` and repeats every step, all of which are safe to repeat. Only `project.init` entries — an unresolved entry from any other command still blocks, because nothing here knows how to finish it. |
| 13 | ✅ **FIXED.** A live lock is deleted wherever `ps` is unavailable — any slim container or restricted `PATH`. **[R]** Fixed: liveness is three-valued. `ps` failing to run, or refusing to answer, is `Unknown` and routes to the `Ambiguous` diagnosis that already existed for exactly this doctrine. Four fixtures used a pid so large `ps` rejects it as out of range — they were asserting the bug, and now use an absent pid that `ps` genuinely answers about. |
| 14 | ✅ **FIXED.** Promotion fast-forwards whatever branch HEAD is on, not the protected one, and reports success. **[R]** Fixed: the preconditions now check which branch is checked out before checking where it points, so the refusal lands before the authority moves. Every other local check described the protected branch only if HEAD happened to be it. |
| 15 | ✅ **FIXED.** A post-publish failure is unrecoverable, and recover falsely reports that no promotion reached the authority. **[R]** Fixed: `settle_promotion` journals its boundaries, so an interruption there is attributable and reproducible; and `resume_promotion` accepts a record already marked `promoted` whose cards are short of `landed`. Requiring exactly `Accepted` made that window unrecoverable. |
| 16 | ✅ **FIXED.** Ambient `GIT_DIR`/`GIT_WORK_TREE` override every git call. The gate runner clears its environment; the git layer does not. **[V]** Reproduced: `doctor --workspace <repo>` reported a decoy directory. Fixed: the Git helper removes what redirects — repository, objects, index, config — and also the author/committer identity variables, which outrank repository config and so quietly undid the fixed control identity Section 9.2 requires. |
| 17 | ✅ **FIXED.** "Already up to date." counts as a successful merge, so the harness reports work landed when it published nothing. **[R]** Fixed: the merge helper compares HEAD before and after. `--no-ff` forces a merge commit in every case but one — a commit already contained produces exit 0 and no commit — and exit 0 was read as a completed merge. A candidate already in the tree is now refused as a planning error, which is the coordinator's to resolve. |
| 18 | ✅ **FIXED.** Path validation is bypassed at init — uncanonicalized paths defeat the alias and nesting checks, and a control repository can be created inside the candidate worktree. **[V]** Fixed: a not-yet-existing path is normalized lexically and then resolved through its longest existing prefix, so it is comparable with the candidate's canonical form. Both halves are needed — see the note at `canonicalize_ancestors`. |
| 19 | ✅ **FIXED.** `card revise` performs an unchecked transition; from `landed` it permanently strands the card and its integration. **[R]** Fixed: `is_revisable` refuses from `integrating`, `accepted` and `landed`. The old guard was `is_terminal`, and `landed` is not terminal — it still has to reach `closed`. Revising an *approved* card is still allowed: Section 15.2 makes a revision invalidate the approval, which is the mechanism working. |
| 20 | ✅ **FIXED.** A review can reach a state with no exit; the only escape is defect 19. **[R]** Fixed: `review_pending → active`, and `work resume` covers it. If the branch moves after the handoff, no verdict is recordable — correctly — and the card had no exit but abandonment. This is the revocation `handed_off → active` already allowed, one stage later. |
| 21 | ✅ **FIXED.** A reviewer cannot record `blocked` — the command fails and no review record is written at all. **[R]** Fixed: `review_pending → blocked` added. The verdict is one of three the schema offers; the transition table did not admit it, so recording it aborted the transaction and discarded the reviewer's judgement and their gate-adequacy finding rather than filing them. |
| 22 | ⚠️ **PARTLY FIXED.** Risk policy is dead code; a `critical`-risk card receives no human gate. **[R]** `requires_human_review` was modelled, unit-tested, and called from nowhere. An approval of a `high` or `critical` card now requires `human_reviewer: true` on the verdict and records it. Declared, never proven — D-013 — so it catches the omission, not the liar. **Still unenforced:** Section 15.3's further requirements for `critical`, a rollback exercise and a second human approval. Recorded as unenforced rather than implied. |
| 23 | ✅ **FIXED.** `update-ref` failures are reported as a lost compare-and-swap with the diagnostic discarded, so a stale lock file tells the operator the baseline moved, forever. **[R]** Fixed: when the ref still holds exactly what the caller expected, no race was lost — the write failed for another reason, and the error carries Git's own diagnostic. A genuine compare-and-swap loss is still reported as a rejection. |
| 24 | ✅ **FIXED.** Seven further `--dry-run` paths skip checks the real command enforces. **[R]** A parity suite now builds a state each real command refuses and requires the preview to refuse with the same code. It found three: `gate run` previewed a card with no worktree to run in, `card activate` previewed a card whose scope overlapped an active one, and `work start` refused for the wrong reason. The other five commands tested already had parity. |

**All ten Tier 3 defects are addressed** (2026-07-29); defect 22 partly, with what
remains unenforced named at the check.

## Tier 4 — contract and correctness

✅ **FIXED:** the error envelope reported a different `command` granularity than
the success envelope — the group `review` where a success carried
`review.record` — so the field could not be used for the routing it exists to
support. Each subcommand enum now carries its own dotted path. Found while
fixing it: `acceptance inspect` succeeded while reporting
`command: "acceptance.record"`, labelling a read as the write whose reporting
function it reused. And `cli_validate_reports_the_exact_invalid_field_in_json`
asserted the group, so it was holding the defect in place.

✅ **FIXED:** argument-parsing failures escaped the JSON contract entirely.
`Cli::parse` exits inside clap, so a caller that asked for `--output json` got
usage text on stderr and nothing on stdout — and a malformed invocation is the
failure an agent is most likely to hit. Parsing failures now render through the
envelope when JSON was requested, and keep clap's own help when it was not.

✅ **FIXED:** every I/O failure was classified as a harness defect — exit 10,
the category reserved for "the harness is broken, file a bug". A read-only
filesystem, a permissions mistake, a full disk are all the operator's to fix.
Those kinds are now `CH-PRECONDITION-CONTROL-ACCESS`, exit 4. `NotFound` stays
internal on purpose: a control file missing when the state says it exists is
corruption, and nothing here can tell that from someone deleting the directory. ✅ **FIXED:** the conflict-token table did not match Git's real output. Two of
its six tokens were strings no supported Git emits — `CONFLICT
(directory/file)` and `CONFLICT (distinct types)`, the latter being message prose
where the real token is `CONFLICT (distinct modes)` — so
`ConflictKind::DistinctTypes` was unreachable and every type change was reported
as `other`. `CONFLICT (binary)` was absent, so Git's binary triple produced
**two** conflict rows for one path, one of them labelled *textual*: an actor was
told to edit conflict markers Git had not written, because it wrote "our" side
verbatim instead. Fixed, with the duplicate `contents` record dropped and
`Binary` classified structural.
`an_unknown_conflict_token_is_kept_and_treated_as_structural` used `CONFLICT
(submodule)` — a *real* Git 2.50 token — so it pinned the gap open rather than
guarding the default. Nine further real tokens still land in `other`, which is
the right class; naming them is cosmetic and left undone. ✅ **FIXED:** rename records were mis-parsed into paths that do not exist —
specifically in the `status --porcelain=v1 -z` parser, **not** the `--raw -z`
diff parser, which is correct and consumes its second field properly. A rename
emits two NUL-separated fields and the second is a bare path with no `XY `
prefix, so stripping three bytes turned `src/alpha.rs` into `/alpha.rs`.
`--no-renames` plus a parser that refuses a malformed record rather than
truncating it. Recording the disambiguation so the next reader does not
re-audit `diff.rs`. ✅ **FIXED:** dependency SHAs were never bound to a review. Neither
`HandoffRecord` nor `ReviewRecord` carried one, so Section 10.7's required
field did not exist and invariant 7.3.6's invalidation trigger was
unimplemented: an approval survived its dependency being re-reviewed at a
different commit.

The binding is constrained by what the candidate incorporates, rather than what
the dependency's current approval names. A handoff records, per declared
dependency, the newest commit that dependency ever handed off which is an
ancestor of this candidate.
The binding goes stale only when the dependency's standing approval no longer
*contains* that commit — containment, not equality, because a dependency that
gained review-requested fixes on top has not moved out from under anyone, while
a rewrite that discards the bound commit has.

That resolution rule is what avoids re-serializing the cycle. An earlier design
compared against the dependency's current approval, which would have invalidated
a dependent every time its dependency was re-reviewed and forced dependencies to
be approved in order. The proof it is gone: `tests/integration_plan.rs` still
approves `F-002` before `F-001` and needed no repair.

Deliberately **not** fixed, and named at the check:

- The four other Section 15.2 invalidation triggers this does not reach.
- **A candidate can incorporate dependency work through a commit the dependency
  never handed off without the binding showing that gap.** The resolution asks
  which of the dependency's *handed-off* commits is an ancestor. When unhanded
  work sits on an earlier handed-off ancestor, it records that older commit;
  a later approval that still contains the older commit passes containment with
  no warning. It records `null` only when no handed-off ancestor exists. Closing
  the gap means binding against the dependency's branch rather than its
  handoffs, which is a wider change than this card owns.
- **Section 10.2's dependency-base precondition is unenforced here.** A card's
  `base_sha` is checked only as 40 hexadecimal characters; nothing verifies that
  it is the dependency's exact accepted SHA.
- **Cycle status folds card events.** This remains an open Tier 4 defect.
- **Five cycle statuses and one card state are unreachable.** This remains an
  open Tier 4 defect.
✅ **FIXED:** authoritative integration merges no longer inherit
`commit.gpgsign` or commit-stage hooks. The merge stops before committing; the
harness writes and verifies the exact two-parent object, then runs only the
project's best-effort `post-merge` checkout hook. Control commits are likewise
neutralised both in their local configuration and per invocation, including
already-existing control repositories whose local settings were removed.

**Still unenforced:** a refusing `reference-transaction` hook can prevent
integration-worktree creation, because that operation must retain the
project's `post-checkout` hook that materialises the tree smoke gates judge.
`post-merge` itself may also hang; its detached children do not delay the
integration, but the hook process retains Git's normal unbounded behaviour.

✅ **FIXED:** generated-artifact scope checks compared globs with `==`. Both
directions were wrong and in opposite ways: a shared artifact covered by a glob
include was **accepted** — one path with two owners, the exact thing the class
exists to prevent — while a per-card source the card plainly owns was
**refused**. Excludes were ignored entirely. Sources now require containment
(`Scope::allows`) and shared paths are refused on intersection; the asymmetry is
deliberate and stated at the function.

Three adjacent holes are **left open** and named at the check rather than
implied closed: nothing compares one card's declared shared artifact against
another *active* card's write scope; the per-card arm never checks the
artifact's own `path`, only its `sources`; and `Transient`/`Serialized` are not
checked at all. The first matters more after this fix than before, because
carving the path out of the scope is now the normal way to declare a shared
artifact.

Two fixtures had to change. Neither asserted the broken comparison — they
constructed cards the hole permitted, and needed a legal card to reach the
verify-side rule. A third,
`a_per_card_artifact_generated_from_owned_sources_is_accepted`, declared
`sources: ["src/**"]` against `include: ["src/**"]`: it passed because the two
strings were identical, so it certified `==` rather than ownership and could not
tell the implementations apart.

## Found later, by the test audit

**25. ✅ FIXED. The audit trail's timeline carried no timestamps.**
`src/commands/audit.rs` built each timeline entry's `at` from
`event["recorded_at"]`, and the event schema's field is `occurred_at`, so every
`at` was `null` — in both the timeline and the protected-branch transition
record. The audit report's whole purpose is reconstructing when things happened.
Fixed: both report projections now use `occurred_at`; the timeline test requires
each entry's timestamp to equal its source event's `occurred_at`, and the
protected-branch test makes the same exact comparison for the promotion event.
Deleting either projection's `at` field or substituting a constant now fails its
named test.

Not looked for. It surfaced because
`the_timeline_reconstructs_the_cycle_in_order` asserts only the relative order
of three `type` strings and never reads a timestamp, so deleting the line
entirely leaves all twelve audit tests green. The defect and the test that
should have caught it are the same finding twice.

Recorded after the original tiers were drawn; it remains in this section so the
time it was found is not obscured.

## Why failure injection did not catch defect 11

`CHANGE_HARNESS_FAIL_AT` raises `RecoveryIncomplete`, whose category is
`RecoveryRequired`, and the clean/partial decision takes that arm before
consulting anything else. So **every failure-injection test in the suite records
`FailedPartial` regardless**, and none of them can observe whether the rest of
the decision is correct. `WP-500`'s "failure injection at every journaled
boundary" is real coverage of the journal and no coverage at all of the state it
writes.

Found while confirming defect 11: a test asserting `recovery_required == true`
at the exact boundary in question passed against the unfixed code. The decision
is now a separate function with its own tests, one of which states this
limitation so the next person reaching for injection sees why it will not work.

## The test suite

Roughly forty-five tests assert less than their names claim. Four mutations were
confirmed to survive the entire suite of 732 tests:

- deleting the Git version-compliance check — ⚠️ **partly addressed.** The
  probe test asserted `meets_minimum_version == (parsed_version >= MINIMUM)`,
  restating the implementation, so hardcoding `true` left it passing. It now
  asserts the value, and a separate test exercises the ordering with versions
  this host cannot supply. A hardcoded `true` would still slip past without an
  old Git to run against — but that field only feeds `doctor`'s report. The
  check with teeth is `check_git_version`, and
  `an_unsatisfiable_minimum_git_version_fails_explicitly` already exercises it
- deleting the worktree-support probe — **still open.** Report-only: nothing
  acts on `supports_worktrees`, and it cannot be forced false on a host whose
  Git supports worktrees. Recorded rather than papered over
- adding an illegal `Draft → Promoted` transition to the authoritative table
- replacing the system clock with a fixed constant, so every timestamp in the
  audit trail becomes the same fabricated instant — ✅ **FIXED**, one test now
  requires a recorded timestamp to fall between two readings of the host clock
  taken around the command. Every other test uses `FixedClock` for determinism,
  which is correct and is exactly why nothing was left checking the real one

The sharpest single instance: `git rev-parse --verify` echoes any 40-character
hex string and exits 0 whether or not the object exists. Both tests asserting
that commits survive garbage collection used it, so deleting the ref-retention
mechanism they exist to defend was undetected.

✅ **FIXED.** Both now use `cat-file -e`, via a `object_exists` helper that says
at its definition why `rev-parse` cannot be used for this.

Repairing it exposed a second layer worth recording. Swapping the check was not
enough: with `landing::retain` deleted, the landing commit still survived
`gc --prune=now` in that fixture, so the test still could not see the mechanism
go. What detects its removal is asserting that
`refs/harness/landing/<id>` actually resolves — the test had only asserted that
the *envelope reported* that ref name, which is the envelope agreeing with
itself. Confirmed by mutation: deleting `retain` now fails the test named for
it.

## Cause

Not carelessness on any individual test. The implementation and the tests that
certified it were written by the same author, so each test could only check a
case that author had already considered. The defects cluster precisely in the
cases they had not — and several tests do not merely miss a defect, they assert
the defective behaviour is correct.

The mechanism that reliably separates the sound tests from the over-claiming
ones is a guard proving the fixture is not vacuous: "the fixture must actually
have written the secret", "gates must actually have run", removing the source
repositories before a restore drill. Where that guard is present the test holds.

## Consequence for self-hosting

Threshold C made the harness's own lifecycle mandatory for further work. That is
suspended until Tier 1 is repaired: certifying the repair of an evidence chain
using the same evidence chain is circular. Repairs land as ordinary reviewed
commits, each with an independent reviewer that did not write it.
