# Change Harness Agent Instructions

Read `README.md`, `docs/IMPLEMENTATION_PLAN.md`, and `docs/ARCHITECTURE.md`
before changing implementation. `docs/IMPLEMENTATION_PLAN.md` is authoritative
for work-package scope, dependencies, acceptance gates, and status.

## Engineering rules

- Keep the workflow engine project- and language-independent. Project-specific
  behavior belongs in configuration or adapters.
- Review existing tests before changing code. Add focused unit or
  temporary-repository regression tests for every meaningful behavior change.
- Invoke Git and project commands with explicit argument arrays. Never build
  shell command strings from configuration.
- Treat hooks as advisory. Authoritative checks operate on exact Git objects
  from a trusted control plane.
- Never update a branch ref that is checked out in a worktree without also
  preserving worktree and index consistency.
- Git mutations must be bounded, idempotent, recoverable, and validated before
  execution. Do not hide unsafe behavior behind force flags.
- Do not treat a shared operating-system account as a security boundary.
- Keep modules small and focused. Separate policy evaluation, Git operations,
  state persistence, and command-line presentation.
- Do not add speculative infrastructure. Implement the smallest complete
  end-to-end workflow slice and harden it with evidence.
- Update the implementation plan and status ledger whenever work starts,
  completes, blocks, changes scope, or produces acceptance evidence.
