# Change Harness Architecture

This document is the concise architecture summary. The authoritative work
packages, requirements, acceptance gates, and current status are maintained in
[Implementation Plan and Status Ledger](./IMPLEMENTATION_PLAN.md).

## Shape

One thin, complete workflow, built before any distributed coordination,
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

**Slice 3, operational hardening — partial.** Concurrency locks and leases
exist, and every mutating command journals its steps so an interruption is
attributable to a boundary. Systematic failure injection, idempotent automated
recovery, generated-artifact classification, and backup verification are
`WP-500` onward.

**Slice 4, project adapters — not started.** Multiple repository manifests,
cross-repository gates, namespaced runtime resources, and constrained gate
execution.

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
