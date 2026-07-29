# SELFHOST-001 — complete self-hosting run

Threshold C requires one bounded documentation-only card to travel every
lifecycle stage using harness commands alone, with no manual Git mutation
substituting for one. This file is that card's deliverable, and it was produced
by the run it describes.

## What the card was

| Field | Value |
| --- | --- |
| Card | `F-001`, revision 1 |
| Cycle | `C-001`, "Prove complete self-hosting with a documentation-only card" |
| Write scope | `docs/SELFHOST-001.md` only |
| Feature gate | `gate.fmt` — `cargo fmt --check` |
| Integration gate | `gate.test` — `cargo test --quiet` |

The gates are the project's real quality gates, not stand-ins. A self-hosting
run whose gates were `true` would demonstrate the plumbing and nothing else.

## Two attempts failed first, which was the point

The run took three attempts. The first two reached combined verification and
stopped there, because `cargo test` — run against the landing commit in the
harness's own disposable integration worktree — failed. Three real defects came
out of them, none reachable by any test against a temporary project:

- **Two tests asserted properties of their environment.** One asserted the
  source checkout was not on a detached HEAD; the other asserted the `doctor`
  workspace role was "main worktree" or "linked worktree". The integration
  worktree is detached *by design*, so the harness's own suite failed on the
  harness's own correct behaviour. Fixing the first was not enough — the second
  attempt failed on the second one. Both now say the same thing: any non-bare
  role is admissible, because this crate is developed from linked worktrees and
  gated from a detached one.
- **`abandoned` was a state nothing could reach.** Section 11.3 permits it from
  every pre-promoted state, and a non-terminal integration holds its cycle's
  integration lease — so the first failed plan held cycle `C-001` with no way
  to release it. `integration abandon` was added, and was then used twice to
  clear exactly that situation.

The failures are recorded here rather than quietly retried. "The lifecycle
completed" is a much weaker claim than "the lifecycle completed, and getting
there found three defects that only self-hosting could find" — and the second
claim is the one that justifies Threshold C existing.

## Stages executed

1. `project init` — created the control repository and the bare authority, and
   registered the authority as a remote of this repository. The authority was
   seeded from `main`.
2. `gate register` — registered `gate.fmt` and `gate.test`. Cards may only name
   gates that already exist, so this precedes card activation.
3. `cycle create` and `cycle activate` — froze one exact authority baseline.
4. `card create` and `card activate` — froze the card into an immutable,
   digested revision.
5. `work start` — leased the card and allocated a worktree and branch from the
   exact baseline.
6. `gate run` — produced a receipt bound to the exact candidate commit.
7. `handoff create` — bound the delivered SHA the actor declared to what the
   branch actually held.
8. `review begin` and `review record` — a review by a different declared actor.
9. `integration prepare`, `preflight`, `merge`, `land` — planned, simulated,
   combined, and built the landing commit without moving any branch.
10. `integration verify` and `integration review` — reran both gates against
    the landing commit and recorded the integration review.
11. `acceptance record` — the decision that authorizes promotion.
12. `integration promote` — moved the protected branch with a compare-and-swap
    and fast-forwarded the local worktree.
13. `archive create`, `verify`, `close` — preserved reachability, then removed
    the branch and worktree.

## Recorded limitations

Two, both stated rather than left for a reader to infer:

- The review was performed by a distinct declared actor but not from a
  genuinely fresh context: the session that authored the card also recorded the
  review. D-013 makes actor identity declared rather than proven, so the
  harness cannot detect this. It is recorded as an accepted-risk finding on the
  review record itself. This run therefore demonstrates the mechanical controls
  fully and the procedural independence only partly.
- The three defects above were fixed by ordinary commits to `main`, not through
  the harness. That is legitimate — Threshold C makes self-hosting mandatory
  *after* `SELFHOST-001` passes, not during it — but it means those particular
  changes did not themselves travel the lifecycle.

## What this establishes

The three repository roles stayed separate throughout: work happened in a
candidate worktree, every authoritative record went to the control repository,
and only promotion touched the authority. No lifecycle step required a Git
command issued by hand.

From here, Change Harness feature work uses this lifecycle.
