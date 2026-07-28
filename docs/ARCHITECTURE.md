# Change Harness Architecture

## Recommendation

Build one thin, complete workflow before adding distributed coordination,
runtime-resource brokers, or cross-repository transactions.

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

Promotion must not directly update a branch checked out in a working tree.
The intended design uses a separate local bare repository as the canonical ref
authority. Candidate and integration worktrees operate in ordinary clones or
linked worktrees; accepted commits are promoted to the bare authority with an
expected-old-SHA check.

This boundary is not implemented in the foundation release.

## Initial delivery sequence

### Slice 1: single-repository candidate

- Versioned project configuration
- Immutable card schema and digest
- Worktree creation from an exact baseline
- Overlap and scope validation
- Read-only verification and exact-SHA handoff

### Slice 2: review and landing

- Independent review record
- Named gates and structured receipts
- Disposable integration worktree
- Exact landing-commit validation
- Accepted promotion to a local bare authority
- Reachability-checked archive and cleanup

### Slice 3: operational hardening

- Idempotent recovery after interrupted commands
- Concurrency locks and leases
- Generated-artifact classifications
- Backup verification

### Slice 4: project adapters

- Multiple repository manifests
- Cross-repository gates
- Namespaced runtime resources
- Optional constrained gate execution

## Explicitly deferred

- A public plugin ecosystem
- Heartbeat-driven distributed scheduling
- Cryptographic actor identity
- Automatic semantic conflict resolution
- General container orchestration
- Claims of atomic commits across independent repositories

