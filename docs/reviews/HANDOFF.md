# Where this stands — handoff

Written at a deliberate stop, not a finish. Everything below is committed; the
working tree is clean and `main` is the only branch that matters.

## State

- 815 tests pass. `cargo fmt --check` and `cargo clippy --all-targets
  --all-features -- -D warnings` are clean.
- All 24 defects from the eight-reviewer independent review are fixed.
- Both release gates (Sections 19.3 and 19.4) remain `BLOCKED`. Nothing here
  changes that, and nothing should be signed.

## Decisions the acceptance owner made (D-067 … D-070)

Threshold C resumes: the remaining repair lands **through the harness**, with
agent reviewers in fresh contexts. An agent may be the declared human reviewer
under Section 15.3 and may give a `critical` card's second approval.
Multi-repository is formally deferred. Section 19.4's ARTANA profile trial is
knowingly outstanding until ARTANA begins.

## Tier 4: seven defects, all confirmed real

Two are fixed and committed: the `status --porcelain` rename parser, and the
generated-artifact scope check.

Five remain. Each has a full investigation in this directory, including a
reproduction, the root cause with file and line, and an adversarial verifier's
objections:

| Defect | State |
| --- | --- |
| `tier4-merge-conflict-token-table.md` | Fix designed and cleared by its verifier. Not implemented. |
| `tier4-dependency-shas-never-bound-to-a-review.md` | Redesigned after rejection; **adjudicated and cleared**. |
| `tier4-merge-honours-gpgsign-and-repository-hooks.md` | Redesigned after rejection; **adjudicated**. |
| `tier4-cycle-status-folds-card-events.md` | Redesigned; **adjudication never ran** — session limit. |
| `tier4-cycle-status-and-card-state-unreachable.md` | Redesigned; **adjudication never ran** — session limit. |

The redesigns are in `tier4-redesign.json`; the original investigations and
verifications are in `tier4-investigation.json`. The two unadjudicated designs
must not be implemented on trust — every one of the four first-round designs was
rejected by its verifier, so an unreviewed design here has a poor base rate.

## The test suite

`over-claiming-tests.md` lists 18 tests that assert less than their names claim,
each with a specific mutation to `src/` that the test does not catch. Six are
severe. None are fixed.

Two land on work done in this session, which is the point rather than an aside:
one test written today is named for a refusal that does not exist on the path it
tests, and `landed_commits_remain_reachable_after_cleanup` — repaired earlier
today — is *still* vacuous for a deeper reason than the one repaired.

## One real defect the audit turned up in passing

The audit trail's timeline carries no timestamps. `src/commands/audit.rs` reads
`event["recorded_at"]`; the field is `occurred_at`, so every `at` is `null`.
This is in the artifact whose only purpose is reconstructing when things
happened, and no test noticed. Not yet in the register as a numbered entry.

## What to do next, in order

1. Implement the conflict-token fix — designed, cleared, self-contained.
2. Implement the two adjudicated redesigns.
3. Re-adjudicate the two designs whose adjudication was cut off.
4. Fix the six severe over-claiming tests, and add a mutation check for every
   mechanism a Section 19.3 criterion cites.
5. Only then re-assess Section 19.3 — through the harness, by an agent that did
   not write the code.

## Housekeeping

Roughly twenty `wf_*` git worktrees under `.claude/worktrees/` are agent scratch
copies from this session's workflows. They hold no work that matters; the
findings are all in this directory. `git worktree remove` or `git worktree
prune` them freely.
