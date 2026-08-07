# Change Harness

Change Harness is a project- and language-neutral CLI that lets one developer
delegate real work to autonomous coding agents — Codex, Claude Code, or any
other — and know what actually landed without reading every diff.

## Why this exists

The mechanics of running agents in parallel are becoming commodity. Worktree
isolation, fan-out, dependency ordering, retries: those ship inside the agent
tools themselves and improve with every release. Writing another orchestrator
means racing the vendors on their own ground.

What parallelism scales instead is the volume of claims a person must
validate. An agent that says "tests pass" is making a claim. An agent
supervising other agents is making a claim about claims, and a verifier adds
trust only when it holds independent evidence and a separate authority
boundary — not merely because a second model read the first one's summary.
That ceiling is what actually bounds delegation: hand over more work and your
own review capacity becomes the bottleneck you were trying to remove.

Change Harness supplies that boundary by changing the unit of authorization:
not "an agent reviewed this diff," but "this exact artifact satisfied a
previously declared argument for promotion." Three questions get hard to
answer the moment an agent can move code, and it makes all three mechanically
verifiable rather than a matter of trust:

- did the exact code that was reviewed reach the protected branch, unchanged?
- did the tests that ran belong to that same commit, or to something adjacent?
- if two changes land together, was the *combination* ever checked — or only
  each piece in isolation?

Every review, every test receipt, and every promotion binds to an exact Git
commit SHA — not a branch name, not whatever is checked out right now. A card
cannot be approved and then quietly changed before it lands. A gate cannot pass
on uncommitted content and be presented as evidence about a commit. Two changes
cannot land together without the combined result being verified as its own
thing, separately from either change alone. The gate is non-conversational: a
refusal is an exit code, not an opinion, and nothing in this loop can be talked
into "close enough." Absence of evidence is a first-class failure state —
unproven refuses exactly as failed does, where a language model would collapse
uncertainty into plausible approval.

It is a mechanical control, not a judgment call. The harness does not decide
whether code is *good*. It proves that the required argument for the change is
complete under the declared contract and policy, and that it is about the
artifact that actually lands — nothing more, and nothing less is mechanical.

## What it is for

One developer, many agents, any project. The design centre is a solo operator
running several agents at once on their own machine, and three properties
follow from it.

**Durable.** The control repository is an ordinary Git repository on disk
holding cards, receipts, reviews, locks, and lineage. It outlives the session
that created it, the agent that crashed, and the laptop that rebooted.
Orchestration state living inside one agent's context does not.

**Cross-tool.** Codex can author a change and Claude Code can review it, bound
to the exact SHA that was handed off — which neither of them can then move
without the approval ceasing to apply. No vendor has an incentive to build that
seam. This project develops itself through it: the defect fixes described below
were each authored by a different tool than the one that wrote the original
code (D-076).

**Project-neutral.** Nothing in the engine knows Rust, cargo, or this
repository. [`tests/project_neutrality.rs`](./tests/project_neutrality.rs)
drives a complete lifecycle against a Python project whose gates are ordinary
shell commands — written precisely because every other fixture *is* Rust, so a
cargo assumption could have been baked in with every suite still passing.

What it is not is a security boundary. Every actor identity is a string the
caller typed (D-013), so the harness catches the same actor blessing its own
work, not an operator determined to defeat their own controls. For one
developer coordinating cooperating agents that is the correct scope rather than
a shortfall: there is no adversary here, only unreliable narrators. The
`Residual` section of [`SKILL.md`](./SKILL.md) states exactly where the line
falls.

ARTANA is the first project this was built to govern, and consumes the CLI
through project configuration and named gate definitions. It does not own the
workflow engine, and nothing here depends on it.

## For coding agents

The operating guide is [`SKILL.md`](./SKILL.md):
the lifecycle in order, the exact YAML each command accepts, the actor-flag
split, and every common refusal with its remedy. It is written to be followed
without reading anything else. An independent reviewer drove a complete
lifecycle from `project init` through promotion and archive using the file
alone — and then failed it, on three sentences that were word-for-word wrong
about the CLI. Those were corrected and re-verified against the binary before
a second reviewer accepted it.

It is a portable, tool-neutral guide. Copy `SKILL.md` into any repository this
harness governs, or provide it to any person or agent using the CLI. It does
not contain live assignments or authority: those always come from fresh
Harness status queries. That split is deliberate, and it is what an agent that
has lost the thread recovers from — `card status` and `cycle status` answer
what this actor currently owns and what is blocking it, so the next step comes
from read state rather than from whatever survived in a context window.

Agents contributing to Change Harness itself want [`AGENTS.md`](./AGENTS.md)
instead, which carries the reading contract and engineering rules for building
the tool rather than driving it.

## Status

**The Single-repository MVP release is accepted and signed (2026-07-30).**
Section 19.4 (hardening) remains `BLOCKED` — it needs an ARTANA checkout that
does not exist yet, tracked by D-070.

Every Single-repository MVP work package is implemented, along with all five
hardening packages: failure injection at every journaled boundary, stale-lock
diagnosis with explicit lease reclaim, verified backups, cycle auditing with
evidence cross-checking, and generated-artifact classification. 858 tests pass.

That was previously reported here as "eleven of the twelve Section 19.3 release
criteria are met; the remaining one is the acceptance owner's signature". An
eight-reviewer independent review disproved it. Every one of the eight found
defects that invalidated a recorded claim; twenty-four are catalogued in
[the defect register](./docs/DEFECT-REGISTER.md), five of which meant the
evidence chain did not hold — for example, a gate could pass on uncommitted
content while the receipt bound that pass to a commit, or a re-review could
approve away a prior critical finding.

**All twenty-four are fixed**, plus nine more found while fixing them or while
auditing the test suite for tests that pass without actually checking what
their name claims — including that failure injection could not observe the
state it was built to test, that six severe tests could not fail under the
exact conditions they existed to catch, and that an integration merge could
silently inherit the operator's own Git configuration (a signing setting, a
repository hook) and let it alter or block an authoritative commit. Each fix
carries a test that fails against the unfixed code and a mutation check
confirming the test catches the real enforcement, not just its own recording of
what happened.

Fixing every catalogued defect was never going to be the same thing as the
release criteria being *proven* met: the original criteria were assessed by
the author of the code, against tests the same author wrote, and that
self-assessment is precisely what failed the first time. The release rule that
governed everything since (D-075) settles each criterion by experiment rather
than by written opinion: break the mechanism a criterion depends on, and
confirm the test that is supposed to catch it actually fails. A reviewer who
did not write the code ran that experiment for all twelve Section 19.3
criteria.

**Eleven of twelve came back `MET`** this way — nine by mutation, two (work
packages done, README accurate) by direct record inspection where mutation
doesn't apply. **The twelfth is not `MET`, and the record says so rather than
rounding up.** Only 2 of 18 known-weak tests touch the mandatory-scenario
trace; the other ~48 have never been mutation-tested by anyone. The acceptance
owner reviewed that exact bound and chose to accept it as disclosed residual
risk (D-079) rather than commission an audit of unknown size — the pattern all
week was that looking harder kept finding more, while what it found kept
getting less severe. Signing (D-080) certifies that distinction, not that it
disappeared: eleven criteria demonstrated, one knowingly carried in the open.
[The plan's Section 19.3](./docs/IMPLEMENTATION_PLAN.md) has the full
per-criterion evidence.

The harness runs its own development. `SELFHOST-001` took a documentation-only
card through every lifecycle stage against this repository and promoted the
result; every package since has been built the same way. That run is recorded
in [its report](./docs/SELFHOST-001.md), including the two attempts that failed
first and the three defects they exposed — which is the reason Threshold C
exists. Threshold C was briefly suspended while the evidence chain itself was
under repair (D-065), then resumed once that repair was independently verified
(D-067): the remaining defect fixes landed through the harness's own lifecycle,
each authored by a different tool than the one that wrote the original code
(D-076), gated, handed off at an exact SHA, and independently reviewed before
promotion.

One limitation is worth knowing before relying on any of this evidence: D-013
makes actor identity a declared claim rather than a proven one, so the harness
itself cannot tell a genuinely independent reviewer from the same author typing
a different name. The reviews recorded during the original build-out used
distinct declared actors but not genuinely fresh contexts — that gap is exactly
what the eight-reviewer exercise measured, and it measured badly. The repair
that followed changed the practice, not the mechanism: each fix was reviewed by
an agent given only the code and the specification, with no memory of writing
it, and often by an implementing tool (Codex) entirely separate from the
reviewing one (Claude). That discipline is why the repair caught real defects
on nearly every round — including the repair *of* the repair, twice — but it is
still a practice the team follows, not a guarantee the tool enforces.

The `SPIKE-001` walking skeleton that preceded implementation is recorded in
[its report](./docs/spikes/SPIKE-001-REPORT.md). No prototype code was merged;
the prototype survives only under `refs/archive/spikes/SPIKE-001`.

## Product boundary

Change Harness automates mechanical controls — the things that can be settled
by inspecting a Git object rather than by forming an opinion:

- exact commit and branch checks;
- worktree allocation;
- path and shared-resource ownership;
- named validation gates and structured receipts;
- exact-SHA handoff and independent review binding;
- clean integration and safe promotion;
- archival, recovery, and cleanup.

It does not decide whether a requirement is correct, whether an architecture is
appropriate, whether tests prove the intended behavior, or whether residual risk
is acceptable. Those judgments stay with you — the point is that the artifacts
you judge are the exact ones that will land, and that an agent cannot revise
them afterwards without the evidence ceasing to apply.

That division is what makes the delegation safe to widen. An agent can be wrong
about anything in the second list and the first list still holds; the failure
shows up as a refusal you can read, not as a change that quietly landed.

Local hooks are convenience guardrails, not a security boundary. Strong
authorization requires a separate identity or operating-system boundary.

## Installation

> **New to Change Harness?** [`QUICKSTART.md`](./QUICKSTART.md) is a
> hands-on walkthrough — install, adopt a project, register two gates, hand
> off, review, and land a change on the protected branch — every command run
> for real before it was written down. Budget 30–40 minutes the first time;
> ten after that.

### Quick install

```sh
curl -fsSL https://raw.githubusercontent.com/artana-bio/solo-dev/main/install.sh | sh
```

This downloads a pre-built binary for macOS (arm64 or x86_64) or Linux (x86_64
or aarch64), verifies its checksum, and installs it without requiring a Rust
toolchain. Change Harness is distributed under the
[ARTANA proprietary, all-rights-reserved license](./LICENSE).

### From source

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

> **Driving this CLI as an agent?** The operating guide — lifecycle in order,
> the exact YAML shapes the deserializers accept, the actor-flag split, and
> the common refusal codes with their remedies — is maintained at
> [`SKILL.md`](./SKILL.md). Copy that one tool-neutral file into a governed
> project so any agent or person can discover it.

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
change-harness gate reserve --card-id F-001 --gate-id gate.unit
change-harness gate run --card-id F-001 --gate-id gate.unit --reservation-id VR-000001
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
change-harness review record --control $CONTROL --card-id F-001 --verdict verdict.yaml --actor reviewer-example
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
- [Thesis](./docs/THESIS.md) for the product argument as it survived external
  adversarial review, the three kinds of truth the harness does and does not
  provide, and the one experiment — with kill thresholds fixed in advance —
  that remains capable of falsifying the idea.
