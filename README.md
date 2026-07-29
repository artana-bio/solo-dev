# Change Harness

Change Harness is a project-neutral CLI for coordinating bounded changes made
by humans and coding agents across local Git worktrees.

The repository is intentionally independent from ARTANA. ARTANA will eventually
consume the CLI through project configuration and named gate definitions; it
will not own the workflow engine.

## Status

**Not released. Both release gates are `BLOCKED`, and the evidence this produces
has not been independently re-assessed since it was repaired.**

Every Single-repository MVP work package is implemented, along with all five
hardening packages: failure injection at every journaled boundary, stale-lock
diagnosis with explicit lease reclaim, verified backups, cycle auditing with
evidence cross-checking, and generated-artifact classification. 801 tests pass.

That was previously reported here as "eleven of the twelve Section 19.3 release
criteria are met; the remaining one is the acceptance owner's signature". An
eight-reviewer independent review disproved it. Every one of the eight found
defects that invalidated a recorded claim; twenty-four are catalogued in
[the defect register](./docs/DEFECT-REGISTER.md), five of which meant the
evidence chain did not hold.

**All twenty-four are now fixed**, each with a test that fails against the
unfixed code and a mutation check confirming the test catches the enforcement
rather than the recording. Several more defects were found while fixing them,
including that failure injection could not observe the state it was built to
test, and that five existing fixtures were asserting defects as correct
behaviour.

The gates stay blocked anyway, and the reason is the point. Fixing every
catalogued defect is not the same as the criteria being met: the criteria were
assessed by the author of the code, against tests the same author wrote, and
that assessment is precisely what failed. Section 19.3 and 19.4 need
re-assessing by someone else before either can move.

The harness runs its own development. `SELFHOST-001` took a documentation-only
card through every lifecycle stage against this repository and promoted the
result; every package since has been built the same way. That run is recorded
in [its report](./docs/SELFHOST-001.md), including the two attempts that failed
first and the three defects they exposed — which is the reason Threshold C
exists. **Threshold C is currently suspended** (D-065): certifying the repair of
an evidence chain using that same chain proves nothing about either, so repairs
land as ordinary reviewed commits.

One limitation is worth knowing before relying on the evidence: the reviews in
these runs were recorded by distinct declared actors but not from genuinely
fresh contexts. D-013 makes actor identity declared rather than proven, so the
harness cannot detect this. That gap is exactly the one the eight-reviewer
exercise measured, and it measured badly.

The `SPIKE-001` walking skeleton that preceded implementation is recorded in
[its report](./docs/spikes/SPIKE-001-REPORT.md). No prototype code was merged;
the prototype survives only under `refs/archive/spikes/SPIKE-001`.

## Product boundary

Change Harness automates mechanical controls:

- exact commit and branch checks;
- worktree allocation;
- path and shared-resource ownership;
- named validation gates and structured receipts;
- exact-SHA handoff and independent review binding;
- clean integration and safe promotion;
- archival, recovery, and cleanup.

It does not decide whether a requirement is correct, whether an architecture is
appropriate, whether tests prove the intended behavior, or whether residual risk
is acceptable. Those judgments stay with people, and the harness's job is to
make sure the artifacts they judge are the exact ones that will land.

Local hooks are convenience guardrails, not a security boundary. Strong
authorization requires a separate identity or operating-system boundary.

## Installation

Requires Rust 1.95 or newer and Git 2.50 or newer. The Git floor is not
arbitrary: `git merge-tree --write-tree` is what makes a non-destructive merge
preflight possible, and project validation refuses an older Git rather than
silently falling back to a different algorithm.

```bash
cargo install --path .
```

Check the host before configuring a project:

```bash
change-harness doctor --workspace .
```

Every command accepts `--output text|json`. JSON emits a stable envelope
(`harness.command-result/v1` or `harness.command-error/v1`) with a machine
-readable error code, which is the interface a coding agent should use.

Every command that operates on an existing project takes `--control`. Export it
once instead of repeating it:

```bash
export CHANGE_HARNESS_CONTROL=/path/to/control
```

The flag still wins where both are present. `project init` deliberately ignores
the variable: that flag decides where a control repository is *created*, and
defaulting it from something exported for another project is how a project gets
initialized into the wrong place.

The same convenience has a hazard worth knowing: a variable exported for one
project will happily drive a command meant for another, and the command will
succeed — correctly, against the wrong records. `project status` reports when
the worktree you are standing in belongs to a different control repository than
the one in use, which is the case that catches most of it.

## Three repositories

The harness separates three roles, and keeping them apart is what the safety
properties rest on:

| Role | What it holds |
| --- | --- |
| Candidate | Ordinary repository where feature and integration work happens |
| Control | Separate Git repository holding cards, events, reviews, receipts, and integration records |
| Authority | Bare repository owning the protected branch, updated only by promotion |

`project init` creates the control and authority repositories and registers the
authority as a remote of the candidate. It never overwrites an existing remote
and never adopts a directory whose contents nobody checked.

```bash
change-harness project init \
  --project-id example \
  --repository /path/to/repo \
  --control /path/to/control \
  --authority /path/to/authority.git \
  --worktree-root /path/to/worktrees
```

## Operator workflow

The sequence below is the whole lifecycle. `tests/lifecycle.rs` runs exactly
this, twice, against a temporary project.

**1. Register the gates cards may name.** Gates are registered deliberately
rather than declared inline, so a card cannot invent a check that nobody
reviewed.

```bash
change-harness gate register --definition gate.unit.yaml
```

**2. Open a cycle and freeze its baseline.** Activation pins one exact
authority commit; every independent card in the cycle starts from it.

```bash
change-harness cycle create --control $CONTROL --cycle-id C-001 --objective "First slice"
change-harness cycle activate --control $CONTROL --cycle-id C-001
```

**3. Author and activate a card.** A card declares its write scope, the
contracts it reads and changes, the gates it must pass, and what acceptance
looks like. Activation freezes it into an immutable, digested revision.

```bash
change-harness card create --control $CONTROL --draft F-001.yaml
change-harness card activate --control $CONTROL --card-id F-001
```

**4. Do the work in an allocated worktree.** `work start` leases the card to
one actor and allocates a worktree and branch for it. Overlapping write scopes
between active cards are refused here, not discovered at merge time.

```bash
change-harness work start --card-id F-001
change-harness gate run --card-id F-001 --gate-id gate.unit
```

> **Your project must ignore whatever its gates write.** Gates run inside the
> worktree, and an untracked file blocks handoff — deliberately, because an
> untracked file can silently become part of a candidate. A test suite that
> emits coverage data, a compiler that leaves a cache, a formatter that writes
> a backup: any of these will stop a card handing off until the project's
> `.gitignore` covers them. Most projects already do this and never notice;
> the ones that do not will meet it at the first handoff.

**5. Hand off an exact candidate.** The declaration names the SHA the actor
believes they delivered, and the harness refuses if the branch says otherwise.
This is the check `SPIKE-001` found missing: a branch rewritten between
delivery and review produced an internally consistent handoff describing code
nobody wrote.

```bash
change-harness handoff create --control $CONTROL --card-id F-001 --declaration decl.yaml
```

**6. Review independently.** A different actor, a fresh context, and the
review packet only. The verdict records per-finding disposition and whether the
gates could actually observe the acceptance behaviors — both because spike
reviewers needed to say things a binary verdict cannot express.

```bash
change-harness review begin  --control $CONTROL --card-id F-001
change-harness review record --control $CONTROL --card-id F-001 --verdict verdict.yaml
```

**7. See what is waiting.** `integration ready` answers "what is approved and
integrable" from control state, and says why each card that is not ready is
not. An actor arriving with no context does not have to remember.

```bash
change-harness integration ready --control $CONTROL --cycle-id C-001
```

**8. Plan, preflight, and combine.** `prepare` pins each candidate and orders
them topologically. `preflight` simulates the whole merge sequence without
touching a ref, index, or worktree. `merge` combines them in a disposable
worktree that is removed on every path.

```bash
change-harness integration prepare   --control $CONTROL --cycle-id C-001 --actor-id coordinator
change-harness integration preflight --control $CONTROL --integration-id INT-001
change-harness integration merge     --control $CONTROL --integration-id INT-001 --actor-id coordinator
```

**9. Build the landing commit, then verify it.** The landing commit has the
authority baseline as first parent and the integration head as second, carries
the exact verified tree, and moves no branch. Verification then reruns every
gate any member named *against the landing commit* — a gate that passed on an
isolated candidate proves nothing about the combined tree.

```bash
change-harness integration land   --control $CONTROL --integration-id INT-001 --actor-id coordinator
change-harness integration verify --control $CONTROL --integration-id INT-001 --actor-id verifier
change-harness integration review --control $CONTROL --integration-id INT-001 \
  --reviewer-actor-id integration-reviewer
```

**10. Accept, then promote.** Acceptance is the only thing that authorizes
moving the protected branch, and it binds one exact landing commit. Promotion
checks every precondition first, moves the authority with a compare-and-swap,
and fast-forwards the local protected worktree.

```bash
change-harness acceptance record   --control $CONTROL --integration-id INT-001 --acceptance-owner owner
change-harness integration promote --control $CONTROL --integration-id INT-001 --actor-id promoter
```

**11. Archive, then clean up.** Archive refs keep the landing commit and every
candidate reachable after the branches are gone. `close` refuses to remove
anything holding commits reachable from nowhere else, so cleanup cannot become
data loss.

```bash
change-harness archive create --control $CONTROL --integration-id INT-001
change-harness archive verify --control $CONTROL --integration-id INT-001
change-harness archive close  --control $CONTROL --integration-id INT-001
```

Every mutating command accepts `--dry-run`, which validates against real state
and reports what would change without changing it.

## Backups and the restore drill

Two repositories hold what cannot be reconstructed from the code: the authority
owns the protected ref, and the control repository owns every card, review,
receipt, and decision — the record of *why* the code looks the way it does.

```bash
change-harness backup create --control $CONTROL --destination /Volumes/backup/harness
change-harness backup verify --control $CONTROL --destination /Volumes/backup/harness
```

A destination on the same device as its source is **refused**. A copy beside
the original survives an accidental deletion and nothing else — not a failed
disk, not a corrupted filesystem, not a lost laptop — and calling it a backup
is what stops someone making a real one. `--allow-same-device` overrides the
refusal for a single-disk machine, and the result still says the guarantee is
weak.

Verification restores the bundle into a throwaway repository and runs `fsck`
over the result, because `git bundle verify` accepts a bundle truncated
mid-pack. "Verified" therefore means the thing worth meaning: it restores.

To restore for real, clone the bundle:

```bash
git clone --mirror /Volumes/backup/harness/authority.bundle restored-authority.git
```

## Recovering from an interruption

Mutating commands journal each step before performing it, so an interrupted
operation is attributable to a boundary rather than guessed at.

```bash
change-harness project status  --control $CONTROL
change-harness project recover --control $CONTROL
```

One case is deliberate: if promotion moves the authority and the local
fast-forward then fails, the command exits 9 and records
`authority_promoted_local_sync_pending`. The authority is **not** rolled back.
Rewinding a published branch is worse than leaving a recoverable gap, so
resolving it is an operator decision.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success |
| 2 | Usage error |
| 3 | Configuration error |
| 4 | Precondition not met |
| 5 | Policy refusal |
| 6 | Conflict with concurrent state |
| 7 | A gate failed |
| 8 | External tool failure |
| 9 | Recovery required |
| 10 | Internal error |

Exit 1 is deliberately unassigned, so an uncategorized process failure such as a
panic stays distinguishable from every classified outcome.

## Development

```bash
cargo test
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
```

All three must pass. The crate sets `unsafe_code = "forbid"`.

See:

- [Implementation Plan and Status Ledger](./docs/IMPLEMENTATION_PLAN.md) for
  authoritative requirements, work packages, acceptance gates, decision and
  risk registers, and current status.
- [Architecture](./docs/ARCHITECTURE.md) for the shorter design summary.
