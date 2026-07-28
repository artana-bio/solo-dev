# Change Harness

Change Harness is a project-neutral CLI for coordinating bounded changes made
by humans and coding agents across local Git worktrees.

The repository is intentionally independent from ARTANA. ARTANA will eventually
consume the CLI through project configuration and named gate definitions; it
will not own the workflow engine.

## Status

Foundation complete; workflow implementation has not started. The next
milestone is the timeboxed `SPIKE-001` walking skeleton, which will test the
complete card-to-authority workflow on a disposable toy repository before the
production schemas are stabilized.

The current CLI provides a read-only `doctor` command that validates the host
Git installation and reports whether a path belongs to a Git repository. It
does not yet create worktrees, integrate branches, or update protected refs.

## Product boundary

Change Harness will automate mechanical controls:

- exact commit and branch checks;
- worktree allocation;
- path and shared-resource ownership;
- named validation gates and structured receipts;
- exact-SHA handoff and independent review binding;
- clean integration and safe promotion;
- archival, recovery, and cleanup.

It will not decide whether a requirement is correct, whether an architecture is
appropriate, whether tests prove the intended behavior, or whether residual
risk is acceptable.

Local hooks are convenience guardrails, not a security boundary. Strong
authorization requires a separate identity or operating-system boundary.

## Development

The repository pins the same Rust toolchain family currently used by ARTANA,
but the resulting binary is independent of the target repository's programming
language.

```bash
cargo run -- doctor --workspace .
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

See:

- [Implementation Plan and Status Ledger](./docs/IMPLEMENTATION_PLAN.md) for
  authoritative requirements, work packages, acceptance gates, and current
  status.
- [Architecture](./docs/ARCHITECTURE.md) for the shorter design summary.
