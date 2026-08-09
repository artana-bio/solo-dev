# Change Harness Architecture

This document is the concise architecture summary. The authoritative work
packages, requirements, acceptance gates, and current status are maintained in
[Implementation Plan and Status Ledger](./IMPLEMENTATION_PLAN.md).

## Shape

One thin, complete governance workflow was built before any coordination,
runtime-resource broker, or cross-repository transaction. That whole path now
exists for a single repository:

```text
configure
  → create card
  → allocate worktree
  → verify candidate
  → hand off exact SHA
  → review
  → integrate and test
  → accept and promote
  → archive and close
```

Each arrow is a command that refuses rather than guesses when its preconditions
do not hold, and every refusal carries a stable code and exit category.

The optional coordination architecture described below sits above that path.
The CLI remains standalone, and there is no second workflow database. A person
or script can continue to drive the complete governed lifecycle with the Rust
CLI and control repository alone.

## Boundaries

The implementation has four independent responsibilities:

1. **Policy:** validates cards, ownership, dependencies, review requirements,
   and state transitions without executing Git mutations.
2. **Git:** reads objects and performs bounded worktree, integration, archival,
   and promotion operations.
3. **Control state:** stores authoritative cards, events, reviews, receipts, and
   integration records outside candidate branches.
4. **CLI:** presents commands and machine-readable results to humans and agents.

Project profiles provide repository locations, protected branches, named gates,
generated-artifact policies, and optional runtime adapters. The engine contains
no ARTANA-specific repository names or language commands.

### Read-only observability boundary

Operational visibility is a projection, not another authority path:

```text
captured control HEAD
  → validated durable records
  → typed project snapshot
  → text CLI or JSON command envelope

candidate/authority heads, lock, journal, and worktree checks
  → ephemeral observation overlay
```

The captured `control_head` is authoritative for every durable record in one
snapshot. The overlay reports what the surrounding repositories and local
process state look like while the read occurs; it cannot rewrite, supplement,
or override the captured control records. If the control HEAD moves during
collection, the snapshot refuses rather than combining two durable revisions.

This boundary deliberately introduces no second database and no daemon. The
control repository remains the durable store, the candidate and authority
repositories remain Git sources, and `project snapshot --watch` recollects the
same read-only projection at the terminal boundary.

## Coordinator execution plane

The local coordinator is an optional execution plane, not a second control
plane. It accepts an already authorized Harness card and work packet, asks the
standalone CLI for fresh state, invokes one provider adapter, and returns every
durable transition to the CLI. It never edits the control repository, advances
a card, records a review, accepts a landing, or promotes a ref directly.

```text
authorized card + packet
          ↓
local coordinator ──→ provider adapter ──→ provider CLI process
          │                                      │
          └──── standalone Change Harness CLI ←──┘
                         ↓
             existing control repository
```

Coordinator scheduling state is reconstructable from `project snapshot` and
durable agent-run records in the existing control repository. Process output,
heartbeats, and UI progress are ephemeral events only. A coordinator restart
therefore loses display detail, not workflow authority, and does not require a
coordinator database.

The normalized agent-run transport is versioned JSONL. A durable run binds one
provider and provider session to one Harness role, exact card revision and
digest, allocated worktree and lease, packet digest, and exact candidate SHA.
Before a candidate exists, an incomplete run records `candidate_sha: null` and
cannot satisfy handoff or promotion policy; completion requires one full
40-hex candidate SHA checked against the leased branch. Sequence numbers are
monotonic, unknown fields fail closed, and provider-native output is evidence
input rather than authority.

## Provider adapters

An adapter has one job: translate the provider-neutral launch request and the
provider's structured output into the normalized JSONL run contract. The first
built-in adapters target the installed Codex CLI, Claude Code, and GitHub
Copilot CLI. Each adapter declares its executable/version capabilities,
constructs an explicit argument array, sets the allocated worktree as the
working directory, captures the provider session identifier, supports a
bounded resume when the provider permits it, and maps termination without
inventing success.

Future providers implement the same narrow adapter contract. They do not add
workflow states, write authoritative records, choose Harness roles, or receive
acceptance or promotion credentials. `SKILL.md` remains the single portable,
tool-neutral operating guide supplied in work packets; provider-specific
operating guides are not introduced.

## Trust model

A feature worktree produces an untrusted candidate. Authority belongs to the
control plane and exact Git object IDs.

```text
candidate production
        ↓
independent semantic review
        ↓
clean mechanical integration
        ↓
explicit acceptance and promotion
```

Hooks provide fast feedback but never constitute acceptance evidence. Gate
receipts are bound to the card digest, candidate commit, gate definition, and
harness version. Any relevant change invalidates the receipt and review.

A local process running under one operating-system account cannot prevent that
same account from bypassing hooks or modifying files. The initial threat model
is accidental agent drift, not a malicious local actor.

## Canonical Git authority

Promotion must not directly update a branch checked out in a working tree, so a
separate local bare repository owns the protected ref. Candidate and integration
work happens in ordinary clones and linked worktrees; accepted commits reach the
authority through a compare-and-swap against the exact commit the plan was built
against.

The ordering is what matters. Every precondition — the acceptance authorizes
this exact commit, the verification covered it, the landing commit's parents and
tree are right, the local protected worktree is clean and where it should be —
is checked before the authority moves, because that update is the only
irreversible step in the system. If it succeeds and the local fast-forward then
fails, the authority is deliberately *not* rolled back: rewinding a published
branch is worse than leaving a recoverable gap, so the command exits 9 and the
resolution is an operator decision.

## Remote promotion authority

Local/offline use keeps the existing bare authority and its honest same-user
limit: it prevents accidental drift but is not a hard identity boundary. An
optional remote GitHub deployment adds an enforced boundary by protecting the
target branch with repository rules and allowing only a Harness-controlled
GitHub App or service identity to update it. Provider CLIs and the local
coordinator never receive that identity's credential and never own promotion
authority.

Every effect that leaves the machine follows one explicit transaction:

```text
prepare exact intended effect
  → authorize its digest and preconditions
  → execute once through the restricted identity
  → verify the observed remote state and record a receipt
```

For remote promotion, preparation binds the repository and protected ref,
integration and acceptance records, exact accepted landing SHA, expected old
remote SHA, control head, and an idempotency key. Authorization binds that
immutable intent. The acceptance owner authenticates directly to the gateway
through the GitHub App's OAuth boundary; the gateway mints a short-lived signed
capability for that one intent digest, and that capability is never routed
through a provider or the coordinator. The gateway accepts no branch-name-only
candidate and moves only the exact accepted landing SHA with an expected-old
check. Verification reads GitHub back and returns a gateway-signed receipt for
the CLI to commit to the existing control repository. Retry and replay are
resolved by the action ID, capability expiry, and observed ref state, so the
gateway needs no workflow database of its own.

## Dashboard projection

The dashboard is a read-only local projection. Its durable frame is only
`project snapshot --output json`; optional coordinator events add transient
process progress between snapshots. When they disagree, the snapshot wins.

The dashboard has no control-repository writer, no mutating CLI command, no
provider or promotion credential, and no approval control. It may keep an
in-memory display cache, but it cannot persist workflow state or become an
authority source. The CLI and snapshot remain fully usable when the dashboard
and coordinator are absent.

## Delivery sequence

**Spike 0, walking skeleton — done.** One disposable, timeboxed lifecycle run
before any production schema was stabilized: a bounded card in a fresh context,
an exact-baseline worktree, an exact-SHA handoff, a fresh-context review with a
seeded omission, two candidates combined into one landing commit, an
expected-old-SHA promotion, and a deliberate stale-promotion rejection. Only its
findings and plan revisions entered `main`; the prototype survives under
`refs/archive/spikes/SPIKE-001`. Three of its seven findings changed the design
before implementation started, which was the point.

**Slices 1 and 2, single-repository candidate through landing — done.**
Versioned configuration, immutable digested cards, exact-baseline worktrees,
overlap and scope validation, read-only verification, exact-SHA handoff,
independent review, named gates with structured receipts, disposable
integration, exact landing-commit validation, promotion to the bare authority,
and reachability-checked archive and cleanup.

**Slice 3, operational hardening — implemented.** Recovery and failure
injection, concurrency and lease hardening, verified backups, audit reporting,
generated-artifact classification, and the read-only project snapshot are in
the promoted baseline. The separate hardened-release acceptance gate still
records its ARTANA trial limitation in the implementation plan.

**Slice 4, governance extensions — implemented.** Typed mutation governance,
pinned cycle plans, terminal snapshot-noise cleanup, and governed lessons are
present at promoted baseline
`2fa19a4081a8accc19c24593b594835c88dfc07f`.

**Slice 5, provider-neutral coordination — authorized sequence only.** No
production coordinator, provider adapter, remote gateway, or dashboard exists
yet. The executable order is `SPIKE-002` → `WP-900` → `WP-910` → `WP-920` →
`WP-720` → `WP-930` → `WP-940`, and only `SPIKE-002` is `READY`.

## Dogfooding thresholds

- **A — done.** After worktree allocation was accepted, Change Harness creates
  its own implementation worktrees.
- **B — done.** After handoff and review were accepted, Change Harness uses its
  own cards, handoffs, and fresh-context reviews while landing stays manual.
- **C — pending.** After archive and cleanup were accepted, complete
  self-hosting becomes mandatory for subsequent feature work. `SELFHOST-001`
  is the bounded card that proves it, and it has not been run.

## Explicitly deferred

- A public plugin ecosystem
- Heartbeat-driven distributed scheduling
- Cryptographic actor identity
- Automatic semantic conflict resolution
- General container orchestration
- Claims of atomic commits across independent repositories
