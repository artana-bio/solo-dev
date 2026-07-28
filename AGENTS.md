# Change Harness Agent Instructions

`docs/IMPLEMENTATION_PLAN.md` is authoritative for work-package scope,
dependencies, acceptance gates, and status, but feature agents MUST NOT load
the entire plan by default.

## Role-specific reading contract

Every implementation agent reads:

1. `README.md`;
2. this file;
3. Sections 1–7 of `docs/IMPLEMENTATION_PLAN.md`;
4. the active work-package or spike section;
5. only the additional headings listed under `Required reading` in the active
   tracker entry;
6. Section 24, Definition of Done.

An implementation item without an explicit `Required reading` entry MUST NOT
start. The coordinator updates the tracker first.

Coordinators, integrators, acceptance owners, and agents changing the plan or
cross-package architecture MUST read the complete implementation plan and
`docs/ARCHITECTURE.md`.

Reviewers follow the independent-review context rule in Section 15.1. They
receive the review packet and relevant repository state, but no inherited
implementation conversation.

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
