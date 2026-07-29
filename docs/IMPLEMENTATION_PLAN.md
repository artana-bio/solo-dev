# Change Harness Implementation Plan and Status Ledger

## 1. Document control

| Field | Value |
| --- | --- |
| Document status | Authoritative implementation plan |
| Plan revision | 40 |
| Plan date | 2026-07-28 |
| Implementation baseline | `4729d18` (`chore: scaffold generic change harness`) |
| Previous plan commit | `c51f2dc` (`Land INT-001 (1 card, individual)`), the SELFHOST-001 landing commit |
| Repository | `/Users/alvaro/Documents/Code/change-harness` |
| Active branch | `claude/project-status-review-543d65` |
| Current release stage | Single-repository MVP |
| Current implementation status | Every Single-repository MVP package (`WP-000`, `SPIKE-001`, `WP-100`–`WP-130`, `WP-200`–`WP-250`, `WP-300`–`WP-320`, `WP-400`–`WP-460`) plus hardening `WP-500`, `WP-510`, and `WP-520`; 726 tests passing. `SELFHOST-001` completed, and every package since has been built through the harness itself. Eleven of the twelve Section 19.3 criteria are met; only the acceptance owner's signature remains. |
| Next executable work package | Acceptance owner signs the Section 19.3 release record; then `WP-530` and `WP-540` |
| Final acceptance owner | Alvaro Alvarez |

This file is the authoritative delivery plan and current status ledger for
Change Harness. `docs/ARCHITECTURE.md` summarizes the design but does not
supersede requirements, acceptance gates, dependencies, or status recorded
here.

Every implementation change MUST update this file when it:

- starts or completes a work package;
- changes a dependency, requirement, CLI contract, schema, or invariant;
- discovers or resolves a risk;
- records acceptance evidence;
- changes a release gate;
- defers, splits, replaces, or abandons planned work.

Status changes MUST be supported by repository evidence. An agent statement,
chat message, green hook, or uncommitted local result is not sufficient.

## 2. Status vocabulary

Only these status values may be used:

| Status | Exact meaning |
| --- | --- |
| `DONE` | All acceptance criteria passed, evidence is recorded, and the implementation is committed. |
| `IN_PROGRESS` | Work has started on one identified branch or worktree. Only one owner is active. |
| `READY` | Requirements and dependencies are complete enough to begin without another product decision. |
| `BLOCKED` | Work cannot proceed until the recorded blocking condition is resolved. |
| `NOT_STARTED` | Work is planned but a dependency or sequencing decision prevents it from starting. |
| `DEFERRED` | Work is intentionally outside the current release target. |
| `ABANDONED` | Work was started or planned and explicitly rejected; retained only for auditability. |

`DONE` is binary. Partial implementation remains `IN_PROGRESS`. Passing a
subset of tests does not make a work package `DONE`.

A spike uses the same status vocabulary but has its own timebox and evidence
gate. A successful spike validates or revises the plan; it does not count as
production implementation.

## 3. Executive objective

Change Harness will be a project-neutral command-line tool that coordinates
bounded changes made by humans and coding agents in local Git repositories. It
will replace the mechanical functions of a hosted pull-request platform for
local development:

- work allocation;
- exact baseline selection;
- worktree creation;
- ownership enforcement;
- candidate handoff;
- independent review binding;
- reproducible gates and receipts;
- deterministic integration;
- explicit acceptance;
- safe promotion;
- recovery, archival, and cleanup.

Change Harness will not replace semantic engineering judgment. Humans or
independent reviewers remain responsible for requirement correctness,
architecture, test adequacy, residual risk, and final acceptance.

## 4. Release definitions

### 4.1 Foundation

The repository exists, builds, and exposes a read-only diagnostic command.
Foundation does not manage development work.

### 4.2 Walking-skeleton validation

Before production workflow code starts, one disposable lifecycle MUST test the
highest-risk assumptions:

```text
bounded card
  → fresh implementation context
  → exact-baseline worktree
  → exact-SHA handoff
  → fresh-context review
  → candidate combination
  → exact landing commit
  → expected-old-SHA bare-authority promotion
  → deliberate stale-promotion rejection
```

The walking skeleton is evidence gathering, not an alternate implementation.
Only its report and resulting plan changes enter `main`.

### 4.3 Single-repository MVP

The MVP is complete only when one repository can execute this full lifecycle
without manual Git mutation:

```text
initialize project
  → create and activate cycle
  → create immutable card
  → allocate isolated worktree
  → implement and checkpoint
  → verify candidate scope
  → run named gates
  → create exact-SHA handoff
  → record independent review
  → prepare clean integration
  → test exact landing commit
  → record acceptance
  → promote to canonical bare authority
  → synchronize clean main worktree
  → archive evidence
  → close and safely remove worktree
```

### 4.4 Hardened single-repository release

The hardened release adds interruption recovery, concurrency locks, backup
verification, generated-artifact governance, failure injection, and a complete
audit report.

### 4.5 Multi-repository release

The multi-repository release coordinates exact candidate SHAs across
independent repositories with a workspace manifest. It does not claim that Git
can perform one atomic commit across repositories.

### 4.6 Optional isolation release

Runtime-resource brokers, sandboxed gate execution, stronger actor identity,
and plugin distribution are separate extensions. They are not prerequisites
for the single-repository MVP.

## 5. Scope and non-goals

### 5.1 In scope

- Local Git repositories and linked worktrees
- A separate local bare Git repository as canonical ref authority
- A separate versioned control repository
- Deterministic, non-interactive CLI commands
- Human-readable and JSON output
- Project-neutral configuration
- Immutable work cards after activation
- Exact commit IDs and content digests
- Named gates defined by trusted project policy
- Independent semantic review records
- Acceptance records
- Single-repository integration and promotion
- Cross-repository manifests after the single-repository workflow is proven
- macOS arm64 as the first validated host

### 5.2 Explicit non-goals for MVP

- Replacing human product judgment
- Preventing malicious behavior by the same operating-system account
- Hosted collaboration or a web interface
- General job scheduling
- General container orchestration
- Arbitrary shell commands embedded in cards
- Automatic semantic merge-conflict resolution
- Automatic production deployment
- GitHub, GitLab, Jira, or Linear integration
- Windows support
- Atomic commits across independent repositories
- A public plugin ecosystem

## 6. Definitions

| Term | Definition |
| --- | --- |
| Authority repository | Bare Git repository whose protected branch ref is the canonical accepted state. |
| Candidate repository | Ordinary Git clone or repository in which feature and integration objects are created. |
| Control repository | Separate Git repository containing authoritative configuration, cards, events, reviews, receipts, and integration records. |
| Project | One configured candidate repository and its authority/control boundaries. |
| Cycle | A bounded integration period with one frozen baseline and declared cards. |
| Card | Immutable activated definition of one independently reviewable outcome. |
| Card revision | Replacement immutable card created when an activated requirement changes. |
| Lease | Exclusive record assigning one card to one actor and worktree. |
| Candidate | Exact feature commit produced for one card. |
| Handoff | Generated record binding a card revision, baseline, candidate SHA, diff facts, and evidence. |
| Reviewer | Actor other than the feature actor who evaluates semantic correctness. |
| Receipt | Structured result of one named gate run against one exact commit. |
| Integration | Deterministic combination of approved candidates from one cycle. |
| Landing commit | Exact commit tested and accepted for promotion to the protected branch. |
| Acceptance | Explicit decision approving one landing commit and its declared residual risk. |
| Promotion | Expected-old-SHA update of the authority repository's protected branch. |
| Archive ref | Durable ref preserving candidate or integration reachability after ordinary branches are removed. |
| Project profile | Project-specific repository paths, named gates, generated-artifact rules, and optional adapters. |
| Spike | Timeboxed experiment that tests named hypotheses without producing production implementation. |

### 6.1 Specification maturity

The following are committed constraints and are not provisional:

- scope and non-goals in Section 5;
- definitions in Section 6;
- invariants in Section 7;
- the separation of candidate, control, review, integration, and acceptance
  authority;
- the requirement for exact SHAs, expected-old promotion, and a bare
  authority.

Detailed schemas, command payloads, and workflow ergonomics in Sections 9–15
are provisional until `SPIKE-001` is `DONE`. Production implementation MUST NOT
begin at `WP-100` until the spike report either confirms those contracts or
this plan is revised. A spike-driven schema correction is expected learning,
not unauthorized scope expansion.

## 7. Non-negotiable invariants

The implementation MUST preserve all invariants below.

### 7.1 Authority invariants

1. Candidate worktrees never contain authoritative cards or acceptance state.
2. A generated `.agent/` file is a cache and never overrides the control
   repository.
3. Acceptance always names an exact landing commit SHA.
4. Reviews always name an exact card digest, baseline SHA, and candidate SHA.
5. Gate receipts always name an exact gate-definition digest and evaluated SHA.
6. Hooks are advisory and never count as integration evidence.
7. No branch name is accepted as proof of code identity.

### 7.2 Git safety invariants

1. The CLI never constructs shell command strings from project configuration.
2. Git is invoked with explicit executable and argument arrays.
3. The CLI never performs `git reset --hard`, force checkout, force worktree
   removal, or unconditional force push as a normal workflow action.
4. Promotion never directly changes a branch ref that is checked out in a
   working tree.
5. Promotion is rejected unless the authority branch equals the recorded
   expected baseline SHA.
6. Worktree removal is rejected until candidate commits are landed, archived,
   or explicitly abandoned.
7. Dirty main worktrees block synchronization and promotion preparation.
8. Every mutating operation records enough state to resume or safely diagnose
   an interruption.

### 7.3 Workflow invariants

1. One card has at most one active implementation lease.
2. Active cards may not overlap write scopes or exclusive resources.
3. Cards are immutable after activation.
4. A card change creates a new revision and invalidates previous handoffs,
   reviews, and receipts.
5. A candidate SHA change invalidates its handoff and review.
6. A relevant dependency SHA change invalidates dependent evidence.
7. A feature actor cannot approve its own candidate under the declared actor
   identity model.
8. The integrator reruns required gates from a clean disposable worktree.
9. Semantic conflict resolution requires a new reviewed candidate or explicit
   integration-fix card.
10. Final acceptance cannot be inferred from green tests.

### 7.4 Evidence invariants

1. Passing and failed gate attempts are recorded.
2. Logs may live outside Git, but their location and SHA-256 digest are
   recorded.
3. A receipt for an earlier SHA is stale and cannot be reused.
4. Gate success requires exit code zero and completion before the configured
   timeout.
5. A retry policy is declared by the gate; undeclared reruns do not convert a
   flaky result into deterministic evidence.
6. The integration worktree MUST be clean before and after final verification.

## 8. Target repository architecture

The intended source layout is:

```text
change-harness/
  Cargo.toml
  rust-toolchain.toml
  AGENTS.md
  README.md
  docs/
    ARCHITECTURE.md
    IMPLEMENTATION_PLAN.md
  schemas/
    project.schema.json
    cycle.schema.json
    card.schema.json
    event.schema.json
    gate.schema.json
    receipt.schema.json
    handoff.schema.json
    review.schema.json
    acceptance.schema.json
    integration.schema.json
    workspace-manifest.schema.json
  src/
    cli/
      mod.rs
      output.rs
      exit.rs
    commands/
      doctor.rs
      project.rs
      cycle.rs
      card.rs
      work.rs
      gate.rs
      handoff.rs
      review.rs
      integration.rs
      acceptance.rs
      archive.rs
    config/
      mod.rs
      load.rs
      validate.rs
    control/
      mod.rs
      repository.rs
      transaction.rs
      event_store.rs
      lock.rs
    domain/
      ids.rs
      digest.rs
      project.rs
      cycle.rs
      card.rs
      event.rs
      gate.rs
      review.rs
      integration.rs
    git/
      mod.rs
      command.rs
      inspect.rs
      diff.rs
      worktree.rs
      merge.rs
      authority.rs
      archive.rs
    policy/
      mod.rs
      ownership.rs
      resources.rs
      transitions.rs
      evidence.rs
    runner/
      mod.rs
      gate.rs
      receipt.rs
      logs.rs
    lib.rs
    main.rs
  tests/
    cli/
    fixtures/
    support/
    candidate_lifecycle.rs
    review_lifecycle.rs
    integration_lifecycle.rs
    recovery.rs
    safety.rs
```

This is a target decomposition, not permission to create empty modules. A module
is added only when its owning work package implements behavior and tests.

## 9. Configuration contract

### 9.1 Bootstrap inputs

Project initialization MUST require explicit paths. No destructive or
authority-related path may be inferred from the current directory.

```bash
change-harness project init \
  --project-id example \
  --repository /absolute/path/to/repository \
  --control /absolute/path/to/example-control \
  --authority /absolute/path/to/example-authority.git \
  --protected-branch main
```

Initialization MUST fail when:

- `project-id` is invalid or already bound to different paths;
- the candidate repository is not a Git repository;
- the protected branch does not exist;
- the authority path exists and is not an empty directory or compatible bare
  repository;
- the control path exists and is not an empty directory or compatible control
  repository;
- any resolved paths alias each other;
- the authority or control path is nested inside a candidate worktree;
- the candidate repository has unresolved Git operations;
- the protected branch cannot be resolved to one exact commit.

`project init` MUST support `--dry-run`. The dry run performs all read-only
validation and prints the planned filesystem and Git mutations.

### 9.2 Authoritative project file

The control repository stores `project/project.json`:

```json
{
  "schema": "harness.project/v1",
  "project_id": "example",
  "repository": "/absolute/path/to/repository",
  "control_repository": "/absolute/path/to/example-control",
  "authority_repository": "/absolute/path/to/example-authority.git",
  "authority_remote": "harness-authority",
  "protected_branch": "main",
  "worktree_root": "/absolute/path/to/example-worktrees",
  "default_output": "text",
  "host_policy": {
    "supported_os": ["macos"],
    "minimum_git_version": "2.50.0"
  }
}
```

Rules:

- Authoritative state uses JSON.
- Deserialization rejects unknown fields.
- All stored paths are absolute, normalized, and validated before use.
- Symlink resolution is recorded. A later change in resolved target blocks
  mutations until the project is revalidated.
- The control repository commits every authoritative transition.
- Control commits use the fixed repository-local identity
  `Change Harness <change-harness@local.invalid>`. Workflow actor identity is
  recorded in the authoritative event and is not inferred from Git author
  configuration.
- Project configuration changes create a revision event and invalidate active
  operations affected by the change.

### 9.3 Worktree link

Each allocated worktree receives an ignored `.agent/project.json`:

```json
{
  "schema": "harness.worktree-link/v1",
  "project_id": "example",
  "card_id": "F-123",
  "card_revision": 1,
  "control_repository": "/absolute/path/to/example-control",
  "lease_id": "L-000123"
}
```

This file is a locator only. The CLI MUST compare it with the authoritative
control record before acting. Allocation adds `.agent/` to the candidate
repository's common `.git/info/exclude` when no equivalent rule exists. The CLI
does not modify the candidate's committed `.gitignore` for this purpose.

## 10. Authoritative data contracts

### 10.1 Canonical serialization and digests

- Human-authored draft cards MAY use YAML.
- Activated cards and all authoritative records MUST be stored as JSON.
- Unknown fields MUST be rejected.
- Activated records MUST be canonicalized using one documented canonical JSON
  algorithm before hashing.
- Digests use SHA-256 and the prefix `sha256:`.
- The original draft MAY be retained, but only the canonical activated JSON
  and its digest are authoritative.
- Digest test vectors MUST be committed before activation is implemented.

### 10.2 Cycle record

Required fields:

```text
schema
cycle_id
objective
status
baseline_sha
harness_version
project_revision
release_invariants[]
card_ids[]
atomic_groups[]
created_by
created_at
activated_at
```

Rules:

- A cycle baseline is frozen on activation.
- Independent cards use the cycle baseline.
- Dependent cards use exact accepted dependency SHAs declared in the card.
- A protected-branch change does not silently change an active cycle.
- The coordinator must explicitly continue, revise, or abandon an affected
  cycle.

### 10.3 Card record

Required fields:

```text
schema
card_id
revision
cycle_id
title
goal
non_goals[]
risk
change_kind
base_sha
write_scope.include[]
write_scope.exclude[]
contract_reads[]
contract_changes[]
depends_on[]
exclusive_resources[]
named_gates.feature[]
named_gates.review[]
named_gates.integration[]
acceptance.behaviors[]
acceptance.regressions[]
generated_artifacts[]
review_policy
rollback_strategy
created_by
created_at
```

Rules:

- Card IDs match `F-[0-9]{3,}`.
- Revisions begin at `1` and increase by exactly one.
- Include and exclude patterns use repository-relative `/` separators.
- Absolute paths, parent traversal, empty patterns, and `.git/**` are rejected.
- Write scope is deny-by-default.
- Contract domains and exclusive resources are checked independently from path
  overlap.
- Cards reference named gates; they never define executable commands.
- Activation requires an existing cycle, exact base SHA, satisfiable
  dependencies, non-overlapping ownership, and a non-empty acceptance section.

### 10.4 Event record

Required fields:

```text
schema
event_id
project_id
cycle_id
card_id
card_revision
card_digest
event_type
actor_id
occurred_at
previous_state
next_state
head_sha
metadata
```

The control repository's Git history is the primary integrity chain. A
secondary `previous_event_digest` field will not be introduced until a
non-Git event transport exists.

### 10.5 Gate definition

Required fields:

```text
schema
gate_id
revision
argv[]
working_directory
timeout_seconds
environment.allow[]
environment.set{}
network_policy
retry_policy
artifacts[]
```

Rules:

- `argv` is a non-empty string array.
- No command is parsed by a shell.
- Working directories must resolve inside the disposable evaluation worktree.
- Secret environment variables are deny-by-default.
- MVP network policy is declarative and reported; hard enforcement is deferred
  until a sandbox executor exists.
- A gate revision changes its digest and invalidates older receipts.

### 10.6 Receipt

Required fields:

```text
schema
receipt_id
project_id
cycle_id
card_id            (optional; set for a card's gate run)
card_digest        (optional; set with card_id)
integration_id     (optional; set for a combined verification run)
evaluated_sha
gate_id
gate_digest
harness_version
environment_fingerprint
started_at
finished_at
duration_ms
exit_code
termination
stdout_digest
stderr_digest
artifact_digests{}
log_location
attempt
```

`termination` is one of `completed`, `timeout`, `signal`, or `runner_error`.

Exactly one subject is always present: `card_id` with `card_digest` for a
gate run against one card's candidate, or `integration_id` for a combined
verification run against a landing commit. See D-046.

### 10.7 Handoff

Machine-computed fields:

- card identity and digest;
- cycle and baseline;
- candidate branch and SHA;
- dependency SHAs;
- commit list;
- changed paths including renames, deletions, type changes, and modes;
- diff statistics;
- current receipts;
- clean-worktree result.

Actor-authored fields:

- behavior delivered;
- implementation decisions;
- assumptions;
- known limitations;
- residual risks;
- rollback notes.

Handoff creation fails if the worktree is dirty, the candidate is outside
scope, required gates are stale or missing, or the branch no longer matches its
lease.

### 10.8 Review

Required fields:

```text
schema
review_id
card_id
card_revision
card_digest
baseline_sha
candidate_sha
reviewer_actor_id
feature_actor_id
decision
findings[]
review_receipts[]
residual_risks[]
reviewed_at
```

Decision is one of `approved`, `changes_requested`, or `blocked`.

### 10.9 Acceptance

Required fields:

```text
schema
acceptance_id
integration_id
landing_sha
integration_record_digest
receipt_ids[]
acceptance_owner
decision
residual_risks[]
rollback_reference
accepted_at
```

Only `decision: accepted` authorizes promotion. The MVP compares declared actor
IDs for separation but does not claim cryptographic identity proof.

## 11. State machines

### 11.1 Cycle states

```text
draft → active → integrating → accepted → landed → closed
  └──────────────→ abandoned
active → blocked → active
integrating → blocked → integrating
```

No other transition is valid.

### 11.2 Card states

```text
draft
  → ready
  → leased
  → active
  → handed_off
  → review_pending
  → approved
  → integrating
  → accepted
  → landed
  → closed
```

Alternative transitions:

```text
active → blocked → active
review_pending → changes_requested → active
handed_off → active
approved → active
any non-landed state → abandoned
landed → closed
```

Rules:

- `handed_off → active` invalidates the handoff and review.
- `approved → active` requires a new handoff and review.
- `landed` cannot transition to `abandoned`.
- `closed` is terminal.

### 11.3 Integration states

```text
draft → prepared → verified → reviewed → accepted → promoted → archived
draft|prepared|verified|reviewed → blocked
blocked → prepared
any pre-promoted state → abandoned
```

### 11.4 Command authorization by state

| Command | Required state | Resulting state |
| --- | --- | --- |
| `card activate` | Card `draft`; cycle `active` | Card `ready` |
| `work start` | Card `ready` | Card `active` through `leased` |
| `work checkpoint` | Card `active` or `blocked` | State unchanged |
| `handoff create` | Card `active` | Card `handed_off` |
| `review begin` | Card `handed_off` | Card `review_pending` |
| `review record --approve` | Card `review_pending` | Card `approved` |
| `review record --changes-requested` | Card `review_pending` | Card `changes_requested` |
| `integration prepare` | All selected cards `approved` | Cards `integrating`; integration `prepared` |
| `integration verify` | Integration `prepared` | Integration `verified` |
| `integration review` | Integration `verified` | Integration `reviewed` |
| `acceptance record` | Integration `reviewed` | Integration and cards `accepted` |
| `integration promote` | Integration `accepted` | Integration `promoted`; cards `landed` |
| `archive close` | Integration `promoted` | Integration `archived`; cards `closed` |

## 12. CLI contract

### 12.1 Global behavior

Every command MUST:

- be non-interactive by default;
- support `--output text|json`;
- print results to stdout;
- print diagnostics to stderr;
- return a documented exit code;
- include a stable machine-readable error code in JSON mode;
- resolve project state before mutation;
- acquire the required lock before mutation;
- reject unknown configuration fields;
- avoid printing secrets;
- preserve incomplete state for recovery after failure.

`--output` is the canonical global output option. The foundation command's
existing `doctor --format text|json` option remains accepted through the
Single-repository MVP and emits a deprecation warning once `--output` is
available. It may be removed only in a documented breaking release.

`--format json` keeps the pre-envelope payload rather than emitting the
Section 12.4 envelope. `WP-100` acceptance requires existing `doctor` behavior
to remain compatible, and moving the payload under `data` would break every
existing caller. The option is therefore a compatibility shim, not a strict
alias. Supplying both `--output` and `--format` is a usage error rather than a
silent precedence rule. See D-027.

Mutating commands MUST support `--dry-run` unless the command only appends an
actor-authored review or acceptance record. Dry runs perform no filesystem,
Git-ref, control-state, or process mutations.

### 12.2 Exit codes

| Exit code | Category | Meaning |
| --- | --- | --- |
| `0` | Success | Command completed and postconditions passed. |
| `2` | Usage | Invalid CLI arguments or unsupported option combination. |
| `3` | Configuration | Missing, invalid, or incompatible project/control configuration. |
| `4` | Precondition | Repository, worktree, branch, state, or cleanliness precondition failed. |
| `5` | Policy | Ownership, dependency, identity-separation, or state-transition violation. |
| `6` | Conflict | Textual/semantic merge conflict or stale expected SHA. |
| `7` | Gate | Named gate failed, timed out, or produced invalid evidence. |
| `8` | External tool | Git or another required executable failed unexpectedly. |
| `9` | Recovery required | A mutation partially completed and requires `recover`. |
| `10` | Internal | Harness invariant violation or unclassified defect. |

### 12.3 Required command surface

```text
change-harness doctor

change-harness project init
change-harness project validate
change-harness project status
change-harness project recover

change-harness cycle create
change-harness cycle activate
change-harness cycle status
change-harness cycle abandon

change-harness card create
change-harness card validate
change-harness card activate
change-harness card revise
change-harness card status

change-harness work start
change-harness work status
change-harness work checkpoint
change-harness work resume
change-harness work block

change-harness gate run
change-harness gate status

change-harness handoff create
change-harness handoff inspect
change-harness handoff revoke

change-harness review begin
change-harness review record
change-harness review inspect

change-harness integration prepare
change-harness integration verify
change-harness integration inspect
change-harness integration review
change-harness integration promote

change-harness acceptance record
change-harness acceptance inspect

change-harness archive create
change-harness archive verify
change-harness archive close
```

Commands are introduced only by their owning work package. Help output MUST
mark unimplemented commands absent rather than presenting placeholders.

### 12.4 Stable command output envelope

JSON output uses:

```json
{
  "schema": "harness.command-result/v1",
  "command": "work.start",
  "status": "success",
  "project_id": "example",
  "operation_id": "OP-000123",
  "data": {},
  "warnings": []
}
```

JSON errors use:

```json
{
  "schema": "harness.command-error/v1",
  "command": "work.start",
  "status": "error",
  "error": {
    "code": "CH-POLICY-OWNERSHIP-OVERLAP",
    "message": "Card F-124 overlaps active card F-123",
    "details": {
      "paths": ["src/shared/**"]
    },
    "recovery": "Revise card ownership or serialize the cards."
  }
}
```

## 13. Git operation specifications

### 13.1 Read-only inspection

The Git module MUST provide typed operations for:

- Git version;
- repository root and common directory;
- bare/non-bare state;
- worktree inventory in porcelain format;
- exact ref resolution;
- object existence and type;
- ancestry;
- merge base;
- changed paths and modes;
- clean/dirty state including untracked files;
- in-progress merge, rebase, cherry-pick, revert, or bisect state;
- remote URL and ref state;
- commit/tree construction inputs.

Parsing MUST use stable porcelain or machine-readable formats with NUL
delimiters where Git supports them.

### 13.2 Worktree creation

`work start` MUST:

1. acquire the project mutation lock;
2. load the authoritative card and cycle;
3. validate the card state and lease availability;
4. verify the exact base object exists and is a commit;
5. re-run path, contract, dependency, and exclusive-resource overlap checks;
6. confirm the target branch and worktree path do not exist;
7. record an `allocating` operation journal;
8. create the branch from the exact base SHA;
9. create the linked worktree;
10. lock the worktree against pruning;
11. write the ignored `.agent/project.json`;
12. validate branch, `HEAD`, worktree registration, and cleanliness;
13. append lease and activation events;
14. commit control-state changes;
15. mark the operation complete.

On failure, recovery MUST distinguish:

- nothing created;
- branch created, no worktree;
- worktree registered, directory incomplete;
- worktree complete, control event missing;
- control event committed, response interrupted.

### 13.3 Candidate verification

Verification MUST compare Git objects, not the feature worktree's cached card.
It MUST detect:

- added, modified, deleted, and renamed paths;
- file-mode changes;
- symlink additions or target changes;
- submodule entries;
- `.gitmodules` changes;
- case-only path changes;
- paths outside include scope;
- paths matching exclude scope;
- protected harness or control paths;
- undeclared dependency manifests;
- undeclared generated artifacts;
- commit-message policy violations;
- base ancestry mismatch;
- candidate SHA mismatch;
- dirty or untracked worktree state.

### 13.4 Merge preflight

The implementation MAY use `git merge-tree --write-tree` only after the
minimum supported Git version and output/exit behavior are covered by tests.
Fallback behavior MUST be explicit. Unsupported Git versions fail project
validation; the CLI does not silently switch algorithms.

### 13.5 Landing commit

The landing commit MUST:

- have the expected authority baseline as first parent;
- have the verified integration head as second parent;
- contain the exact verified integration tree;
- use a deterministic subject containing the integration ID;
- include cycle, card, integration-record, and receipt identifiers in trailers;
- be created before final verification;
- remain unreachable from the protected authority branch until accepted.

### 13.6 Promotion

Promotion MUST:

1. acquire the project and integration locks;
2. reload the acceptance, integration, receipt, and landing objects;
3. verify every digest and SHA;
4. verify the authority protected branch equals `expected_main_sha`;
5. verify the landing commit first parent equals `expected_main_sha`;
6. verify the landing tree equals the verified integration tree;
7. verify required archive refs can be created;
8. verify the registered local main worktree is clean and at
   `expected_main_sha`;
9. push the landing SHA to the authority branch with an exact expected-old
   condition;
10. confirm the authority now resolves to the landing SHA;
11. fast-forward the clean local main worktree so its index and files remain
    consistent;
12. record promotion evidence in the control repository;
13. create archive refs;
14. mark the integration and cards landed.

If authority promotion succeeds but local synchronization fails, the operation
returns exit code `9`, records `authority_promoted_local_sync_pending`, and
requires `project recover`. It MUST NOT roll the authority backward
automatically.

## 14. Named-gate execution

### 14.1 Runner rules

- The runner invokes the configured executable directly.
- Environment variables are allowlisted.
- The candidate process does not inherit production credentials.
- stdout and stderr are streamed to bounded log files.
- The runner computes hashes after closing logs.
- Timeouts terminate the process group, not only the immediate child.
- The receipt records whether termination was clean, timed out, signaled, or
  failed in the runner.
- The integration runner uses a clean disposable worktree.
- Gate execution can run untrusted candidate build scripts; therefore the
  integration environment MUST contain no secrets even before sandboxing is
  implemented.

### 14.2 Retry rules

- Default maximum attempts: `1`.
- A retry requires a gate-defined policy.
- Every attempt receives a separate receipt.
- The final result includes the complete attempt list.
- A gate that passes only after an undeclared retry remains failed for
  acceptance.

### 14.3 Log retention

MVP defaults:

- receipt metadata: retained indefinitely in the control repository;
- passing logs: retained for 30 days or until the containing integration is
  archived and backed up, whichever is later;
- failing logs: retained for 90 days;
- final landing-gate logs: retained for one year;
- credentials and known secrets: never intentionally logged.

Retention settings are configurable, but shortening them requires an explicit
project-policy revision.

## 15. Review and acceptance specifications

### 15.1 Independent review

For the initial one-human, multiple-agent-session operating model,
`independent` has this exact procedural meaning:

1. the reviewer runs in a fresh agent session or context;
2. the reviewer session is not forked from and does not inherit the feature
   agent's conversation, hidden reasoning, or working summary;
3. the reviewer receives only the authoritative review packet, the exact
   repository objects it references, and repository guidance required to
   inspect those objects;
4. the reviewer actor/session ID differs from the feature actor/session ID;
5. the reviewer does not edit the candidate branch;
6. requested changes return to the feature actor through a structured review
   record;
7. high- and critical-risk human-review requirements still apply.

Fresh context is procedural independence, not a security boundary or
cryptographic identity proof.

The reviewer MUST receive:

- authoritative cycle and card;
- exact baseline and candidate SHAs;
- complete diff;
- contract-domain changes;
- feature receipts;
- implementation decisions;
- assumptions and limitations.

The reviewer MUST evaluate:

1. requirement fidelity;
2. architecture and responsibility boundaries;
3. public API/schema/persistence compatibility;
4. error, timeout, concurrency, and partial-state paths;
5. negative and boundary cases;
6. whether tests could pass while behavior remains wrong;
7. unnecessary dependencies or complexity;
8. security, privacy, logging, and audit implications;
9. deterministic generated changes;
10. maintainability by another human or agent.

### 15.2 Review invalidation

Approval becomes invalid when any of these changes:

- candidate SHA;
- card revision or digest;
- cycle invariant;
- required dependency SHA;
- required gate definition;
- reviewer-required receipt;
- declared contract change.

### 15.3 Risk policy

| Risk | Minimum review |
| --- | --- |
| Low | One independent agent or human reviewer |
| Medium | One independent reviewer plus acceptance owner |
| High | Human reviewer plus independent technical review |
| Critical | Explicit human acceptance, rollback exercise, and second human approval before public or destructive use |

Until multiple human approvers are available, critical changes remain blocked
from public or destructive use. Local prototypes may proceed only when they
cannot affect production data or authority.

## 16. Test strategy

### 16.1 Test layers

| Layer | Purpose |
| --- | --- |
| Unit | Pure parsing, validation, state transitions, path matching, canonicalization, and policy decisions |
| Component | Git command construction, output parsing, control transactions, and receipt generation |
| Integration | Complete commands against temporary real Git repositories |
| Lifecycle | Full card-to-promotion flows against temporary authority/control/candidate repositories |
| Failure injection | Interrupted or partially completed mutations and recovery |
| Compatibility | Supported Git and macOS versions |
| Security regression | Traversal, symlink, argument-injection, secret-output, and candidate-code execution boundaries |

### 16.2 Mandatory temporary-repository scenarios

Before Single-repository MVP acceptance, tests MUST cover:

1. new project initialization;
2. incompatible existing authority path;
3. incompatible existing control path;
4. dirty candidate main;
5. active Git merge/rebase state;
6. exact baseline worktree creation;
7. duplicate branch;
8. duplicate worktree path;
9. overlapping write scope;
10. overlapping contract domain;
11. duplicate exclusive resource;
12. invalid dependency DAG;
13. out-of-scope modification;
14. excluded-path modification;
15. rename across ownership boundary;
16. deletion outside scope;
17. executable-bit change;
18. symlink addition;
19. `.gitmodules` change;
20. stale handoff;
21. stale review;
22. self-review declaration;
23. feature gate failure;
24. feature gate timeout;
25. retry-policy violation;
26. clean disposable integration;
27. textual merge conflict;
28. semantic-fix card requirement;
29. combined gate failure;
30. landing-tree mismatch;
31. authority `main` moved before promotion;
32. dirty local main before promotion;
33. promotion succeeds and local synchronization succeeds;
34. promotion succeeds and local synchronization is interrupted;
35. recovery after each mutation boundary journaled by an MVP work package,
    meaning worktree allocation and promotion;
36. archive reachability;
37. cleanup rejection for unarchived commits;
38. successful cleanup after archival;
39. JSON success envelope;
40. JSON error envelope and stable exit code.


Coverage trace. Every scenario is mapped to the test that exercises it, so a
claim of coverage can be checked rather than taken on faith.

| # | Scenario | Test |
| --- | --- | --- |
| 1 | new project initialization | `init_creates_a_committed_control_repository`, `init_creates_a_bare_authority` |
| 2 | incompatible existing authority path | `init_refuses_a_path_holding_unrelated_content`, `init_refuses_an_authority_with_a_working_tree` |
| 3 | incompatible existing control path | `incompatible_reinitialization_fails_without_altering_anything` |
| 4 | dirty candidate main | `scenario_4_a_dirty_candidate_is_refused` |
| 5 | active Git merge/rebase state | `scenario_5_an_unresolved_git_operation_is_refused` |
| 6 | exact baseline worktree creation | `allocation_creates_a_branch_at_the_exact_card_base` |
| 7 | duplicate branch | `an_existing_branch_blocks_allocation` |
| 8 | duplicate worktree path | `an_existing_worktree_path_blocks_allocation` |
| 9 | overlapping write scope | `overlapping_cards_cannot_both_activate` |
| 10 | overlapping contract domain | `contract_overlap_is_refused_even_when_paths_are_disjoint` |
| 11 | duplicate exclusive resource | `an_exclusive_resource_cannot_be_double_booked` |
| 12 | invalid dependency DAG | `a_dependency_cycle_is_refused_with_an_explanatory_path` |
| 13 | out-of-scope modification | `scenario_13_an_out_of_scope_modification_blocks`, `an_out_of_scope_modification_refuses_handoff` |
| 14 | excluded-path modification | `scenario_14_an_excluded_path_blocks_and_is_named_distinctly` |
| 15 | rename across ownership boundary | `scenario_15_a_rename_across_an_ownership_boundary_blocks` |
| 16 | deletion outside scope | `scenario_16_a_deletion_outside_scope_blocks` |
| 17 | executable-bit change | `scenario_17_an_executable_bit_change_is_reported` |
| 18 | symlink addition | `scenario_18_a_symlink_addition_blocks` |
| 19 | `.gitmodules` change | `scenario_19_a_gitmodules_change_blocks` |
| 20 | stale handoff | `reviewing_a_superseded_handoff_is_refused`, `a_head_change_invalidates_an_existing_handoff` |
| 21 | stale review | `a_candidate_change_invalidates_an_approval`, `a_card_revision_invalidates_an_approval` |
| 22 | self-review declaration | `self_review_is_refused` |
| 23 | feature gate failure | `a_failing_gate_refuses_but_still_records_its_receipt` |
| 24 | feature gate timeout | `a_timeout_is_recorded_distinctly_from_a_failure` |
| 25 | retry-policy violation | `a_pass_beyond_the_declared_attempts_is_not_acceptable_evidence` |
| 26 | clean disposable integration | `a_clean_merge_records_the_integration_head_and_tree`, `the_disposable_worktree_is_removed_after_a_successful_merge` |
| 27 | textual merge conflict | `a_candidate_conflicting_with_the_moved_branch_is_reported_as_textual`, `a_textual_conflict_blocks_the_merge_without_landing_anything` |
| 28 | semantic-fix card requirement | `scenario_28_a_semantic_conflict_cannot_be_resolved_in_place` |
| 29 | combined gate failure | `a_combined_gate_failure_blocks_acceptance` |
| 30 | landing-tree mismatch | `scenario_30_a_landing_tree_that_no_longer_matches_the_merge_is_refused` |
| 31 | authority `main` moved before promotion | `a_moved_authority_branch_fails_before_the_update` |
| 32 | dirty local main before promotion | `a_dirty_local_main_fails_before_the_authority_update` |
| 33 | promotion and local sync both succeed | `an_exact_accepted_landing_promotes` |
| 34 | promotion succeeds, local sync interrupted | `a_local_sync_failure_after_promotion_requires_recovery_and_does_not_rewind` |
| 35 | recovery at each MVP mutation boundary | `scenario_35_both_mvp_mutation_boundaries_are_recoverable`, `scenario_35_an_allocation_boundary_is_recoverable`, `an_interruption_at_any_journal_boundary_is_diagnosable` |
| 36 | archive reachability | `landed_commits_remain_reachable_after_cleanup` |
| 37 | cleanup rejection for unarchived commits | `unarchived_unique_commits_block_cleanup` |
| 38 | successful cleanup after archival | `closing_removes_the_worktrees_and_branches` |
| 39 | JSON success envelope | `output_option_emits_the_stable_result_envelope` |
| 40 | JSON error envelope and stable exit code | `json_mode_renders_failures_as_the_error_envelope` |

Tracing the list found four genuine gaps rather than confirming what was
already there: scenarios 4 and 5 had no enforcement at all — Section 9.1
requires initialization to fail on unresolved Git operations, and nothing
checked it — and scenarios 28 and 30 had only their positive cases tested.

Scenario ownership for the two recovery scenarios is explicit because both were
previously claimed by a post-MVP work package:

| Scenario | MVP owner | Hardened extension |
| --- | --- | --- |
| 34 | `WP-450` partial-success journal and recovery | `WP-500` injected interruption at the promotion boundary |
| 35 | `WP-230` allocation recovery and `WP-450` promotion recovery | `WP-500` systematic failure injection at every journaled boundary |

`WP-500` broadens coverage to boundaries introduced after the MVP and to
injected rather than naturally occurring interruptions. It is not a
prerequisite for the Single-repository MVP gate.

### 16.3 Quality gate

Every work package that changes Rust code MUST pass:

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

The package owner adds focused commands when appropriate. No ignored, skipped,
or quarantined test counts as acceptance unless the plan explicitly records
why it is outside the package gate.

## 17. Work package plan

### WP-000 — Repository foundation

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Commit | `4729d18` |
| Dependencies | None |
| Owner | Codex |

Delivered:

- independent Git repository on `main`;
- pinned Rust `1.95.0`;
- modular CLI foundation;
- read-only `doctor`;
- README, architecture, and agent instructions;
- three CLI integration tests;
- strict formatting and lint gate.

Evidence:

- `cargo fmt --check` passed;
- `cargo test` passed with three integration tests;
- `cargo clippy --all-targets --all-features -- -D warnings` passed;
- working tree was clean after the foundation commit.

### SPIKE-001 — Disposable end-to-end walking skeleton

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Owner | Claude, coordinator role |
| Started | 2026-07-28 |
| Executed | 2026-07-28, 29 minutes elapsed against a 16-hour budget |
| Accepted | 2026-07-28 by Alvaro Alvarez, acceptance owner |
| Hypothesis results | `H-01` through `H-07` all `PASS`; see `docs/spikes/SPIKE-001-REPORT.md` |
| Prototype head | `1bb3fc81195c43c27fc01488e3efa7a2bc3ae377`, archived at `refs/archive/spikes/SPIKE-001` |
| Prototype size | 219 non-generated executable lines against a 300-line budget |
| Dependencies | `WP-000` |
| Target release | Walking-skeleton validation |
| Branch | `spike/SPIKE-001-walking-skeleton` |
| Branch base | Plan revision 3, which carries the `AGENTS.md` spike exception the spike depends on |
| Maximum duration | 16 active engineering hours or two consecutive working days, whichever occurs first |
| Maximum prototype size | 300 non-generated executable lines; fixtures, logs, and the report are excluded |
| Coordinator required reading | Complete implementation plan and `docs/ARCHITECTURE.md` |
| Implementer required reading | Spike implementation packet only |
| Reviewer required reading | Spike review packet only |

Purpose:

Validate the agent-facing ceremony, evidence invalidation, integration shape,
and bare-authority promotion before production schemas and command contracts are
stabilized.

Required setup:

1. create the spike branch from plan revision 3, which is the first commit
   carrying the `AGENTS.md` spike exception the packets depend on;
2. create a disposable toy candidate repository outside this repository;
3. create a disposable bare authority repository;
4. seed two independent toy changes;
5. seed one deliberate but non-obvious acceptance omission in the first
   candidate;
6. prepare one bounded implementation packet without including the full plan;
7. prepare one independent review packet;
8. record every manual or prototype command used.

Required packet contents:

`H-01` and `H-02` test packet sufficiency, so the packets are the experiment
and not merely its paperwork. Both are stored under `docs/spikes/` and quoted
verbatim in the report. Packet fields are derived from the provisional handoff
contract in Section 10.7 and the reviewer input list in Section 15.1; the spike
tests whether those lists are sufficient, excessive, or wrong.

The implementation packet MUST contain:

- the toy repository path, exact base SHA, and target branch name;
- card identity, title, goal, and non-goals;
- write scope includes and excludes;
- acceptance behaviors and regressions;
- named gates and the exact argv used to run them in the toy repository;
- commit and reporting instructions;
- the exact fields the implementer must return, meaning candidate SHA,
  implementation decisions, assumptions, known limitations, and residual risks;
- an explicit statement that the packet is the complete assigned context and
  that missing information must be reported rather than assumed.

The implementation packet MUST NOT contain the implementation plan, the
hypothesis table, review criteria, or any indication that an omission was
seeded.

The review packet MUST contain:

- the same toy card and cycle identity given to the implementer;
- exact baseline and candidate SHAs;
- the complete diff;
- contract-domain changes;
- feature gate receipts;
- the implementer's decisions, assumptions, and limitations;
- the ten evaluation criteria in Section 15.1;
- the decision vocabulary `approved`, `changes_requested`, or `blocked`, and
  the requirement that findings be specific and located.

The review packet MUST NOT contain the implementer's conversation, the
hypothesis table, or the location, nature, or existence of the seeded omission.

Required hypotheses:

| ID | Hypothesis | Pass condition |
| --- | --- | --- |
| `H-01` | A bounded packet is sufficient for implementation | A fresh implementation context completes the assigned candidate with zero requirement-clarification messages. Tool/environment failures may be reported but do not count as requirement clarification. |
| `H-02` | Fresh-context review adds semantic value | A fresh reviewer with no inherited implementation conversation identifies the seeded omission before being told its location or nature and records `changes_requested`. |
| `H-03` | Evidence is bound to exact code | After the initial review, changing the candidate SHA causes the previous handoff and review to be rejected as stale. |
| `H-04` | Corrected work can be independently approved | The corrected exact candidate receives a new handoff and an approval from a different session ID. |
| `H-05` | Candidate combination is understandable | Two candidate changes are combined into one landing commit whose parents and tree can be independently explained and verified. |
| `H-06` | Bare-authority promotion is safe | Promotion succeeds when the authority ref matches the expected baseline, then a deliberately stale expected baseline is rejected without changing the authority ref. |
| `H-07` | Context recovery is practical | After a simulated context reset, both implementer and reviewer can identify the exact current state and next action from their packets and Git objects without consulting earlier chat. |

Required report:

`docs/spikes/SPIKE-001-REPORT.md` MUST contain:

- start and finish timestamps and active hours;
- prototype branch and head SHA;
- toy baseline, candidate, landing, and authority SHAs;
- exact actor/session identifiers;
- exact packets given to implementer and reviewer;
- commands executed and exit results;
- each hypothesis marked `PASS` or `FAIL` with evidence;
- every clarification or missing-field request;
- observed command/artifact count;
- confusing, redundant, missing, or unsafe ceremony;
- schema and CLI fields that changed during the exercise;
- recommendation to preserve, simplify, reorder, or reject the planned design;
- exact plan sections and work packages requiring revision.

Disposition rules:

- Prototype code MUST NOT merge into `main`.
- Only the report and approved documentation changes may merge into `main`.
- The prototype head is retained as `refs/archive/spikes/SPIKE-001`.
- `WP-100` remains blocked until the report is accepted.
- If any of `H-02`, `H-03`, `H-05`, or `H-06` fails, the spike is `BLOCKED`
  or `DONE` with a failed outcome, the plan is revised, and production
  implementation does not begin.
- Exceeding the time or line budget stops the spike. The response is
  simplification, not extending the budget.

Acceptance:

- all seven hypotheses pass;
- the report contains every required field;
- no prototype implementation is present on `main`;
- the archive ref resolves to the recorded prototype head;
- Sections 9–15 and the work-package sequence are updated from observed
  evidence;
- the acceptance owner approves the report;
- `WP-100` is changed to `READY`.

### WP-100 — Core contracts and stable command output

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Owner | Claude |
| Started | 2026-07-28 |
| Completed | 2026-07-28 |
| Branch | `claude/project-status-review-543d65` |
| Dependencies | `SPIKE-001`, `DONE` and accepted 2026-07-28 |
| Target release | Single-repository MVP |

Evidence:

- `cargo fmt --check`, `cargo test` with 47 unit and 12 integration tests, and
  strict Clippy all passed from a clean worktree;
- committed digest vectors cover the SHA-256 empty-string and `abc` cases, and
  prove canonical JSON makes field order immaterial while a material change
  moves the digest;
- every exit-code category is asserted against its documented number, and every
  registered error code is asserted to carry its category prefix, a unique
  rendering, and recovery guidance;
- both envelopes round-trip through committed fixtures;
- `doctor --format json` still emits its original top-level payload, so the
  three foundation tests pass unchanged.

Deliverables:

- domain ID types for project, cycle, card, lease, receipt, review, integration,
  acceptance, event, and operation;
- validated ID parsers and display formats;
- UTC clock abstraction for deterministic tests;
- SHA-256 digest type;
- stable JSON success/error envelopes;
- exit-code mapping;
- global `--output text|json`;
- machine-readable error-code registry.

Acceptance:

- invalid IDs are rejected with stable error codes;
- JSON output round-trips through committed test fixtures;
- no command prints JSON diagnostics to stdout outside the result envelope;
- existing `doctor` behavior remains compatible;
- unit tests cover every exit-code category.

### WP-110 — Project configuration and validation

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Owner | Claude |
| Completed | 2026-07-28 |
| Dependencies | `WP-100` |
| Target release | Single-repository MVP |

Evidence:

- 18 temporary-repository tests plus 10 unit tests cover valid and invalid
  configurations;
- unknown fields fail at both top level and nested, and each names itself;
- relative, missing, aliased, and nested paths each fail with a distinct code
  naming the exact field, and a test asserts validation creates nothing;
- symlink aliasing is detected, which a string comparison would miss, and a
  legitimate symlinked path is accepted with its resolution recorded;
- an unsatisfiable minimum Git version fails as `CH-CONFIG-GIT-VERSION` while a
  malformed one fails as `CH-CONFIG-INVALID-VALUE`, so an operator can tell a
  policy failure from a typo;
- the CLI returns exit 3 and a JSON envelope whose `details.field` names the
  offending field.

Implementation note: `worktree_root` is exempt from the existence check because
it is created on demand at allocation. Every other configured role must already
exist to be validated.

Deliverables:

- `harness.project/v1` schema;
- strict JSON loader;
- absolute path normalization;
- symlink-target recording;
- candidate/control/authority alias detection;
- protected-branch validation;
- minimum Git-version validation;
- `project validate` and extended `doctor`.

Acceptance:

- unknown configuration fields fail;
- missing or aliased paths fail without mutation;
- unsupported Git versions fail explicitly;
- text and JSON diagnostics identify the exact invalid field;
- temporary-repository tests cover valid and invalid configurations.

### WP-120 — Control repository and transaction journal

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Owner | Claude |
| Completed | 2026-07-28 |
| Dependencies | `WP-110` |
| Target release | Single-repository MVP |

Evidence:

- 15 temporary-repository tests plus 15 unit tests;
- concurrent lock acquisition has exactly one winner, and the loser fails in the
  policy category naming the current holder;
- an interruption at each of the three journal boundaries is reproduced and each
  is diagnosable from the recorded steps;
- an unresolved operation blocks further mutation with the recovery-required
  category;
- repeated initialization with identical configuration produces no second
  commit, and incompatible reinitialization is refused with control head and
  document byte-identical afterwards;
- every commit in control history carries a parseable project document;
- control commits use the fixed `Change Harness <change-harness@local.invalid>`
  identity, so history does not vary with whose shell ran the command.

Deliverables:

- control repository initialization;
- single-writer process lock;
- append-only event commits;
- operation journal;
- atomic temporary-file write and rename;
- expected-control-HEAD compare-and-swap;
- `project init`, `project status`, and `project recover`;
- recovery report for interrupted initialization.

Acceptance:

- concurrent mutation attempts result in one winner and one policy failure;
- a killed command at every journal boundary can be resumed or diagnosed;
- control history contains no partial authoritative record;
- initialization is idempotent for identical configuration;
- incompatible re-initialization fails without alteration.

### WP-130 — Git inspection layer

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Owner | Claude |
| Completed | 2026-07-28 |
| Dependencies | `WP-100` |
| Target release | Single-repository MVP |

Evidence:

- 21 temporary-repository tests cover main, linked, detached, and bare
  classification; worktree inventory including lock state; clean and dirty
  state counting untracked files; in-progress merge detection; ancestry and
  merge base; and rename, deletion, symlink, executable-bit, and gitlink
  detection;
- R-013 has a named regression test: a malformed gitfile is classified
  `git_error` with its diagnostic preserved, never `not_repository`;
- paths containing spaces, newlines, and Unicode are parsed correctly, which is
  why the parser reads `--raw -z` rather than a line-oriented format;
- a shell-metacharacter argument is proven inert rather than interpreted;
- one test snapshots `HEAD`, all refs, status, and the reflog around every
  read-only entry point and asserts nothing moved.

Implementation note: `rev-parse --git-path` resolves relative to the *process*
working directory, not the repository, so in-progress-operation detection must
pass `--path-format=absolute`. Without it the existence check silently tests
the wrong filesystem location and every repository looks settled.

Deliverables:

- typed command runner;
- typed command result retaining exit status, stdout, and stderr;
- Git-version parser;
- repository classification as `repository`, `not_repository`, or `git_error`;
- main, linked, detached, and bare worktree/repository inventory;
- worktree-command capability check;
- ref/object/ancestry inspection;
- clean-state and in-progress-operation detection;
- NUL-safe diff parser;
- rename, deletion, mode, symlink, and submodule detection.

Acceptance:

- no shell command construction;
- paths containing spaces and Unicode work;
- malicious path names do not become arguments;
- a genuine non-repository is reported as `not_repository`;
- `safe.directory`, permission, malformed-gitdir, and other Git refusals are
  reported as `git_error` with a sanitized diagnostic, never as
  `not_repository`;
- extended `doctor` reports minimum-version compliance, worktree support, and
  whether the inspected path is the main worktree, a linked worktree, detached,
  or bare;
- every parser has real Git fixture coverage;
- inspection commands perform no repository mutation.

### WP-200 — Cycle model

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Owner | Claude |
| Completed | 2026-07-28 |
| Dependencies | `WP-120`, `WP-130` |
| Target release | Single-repository MVP |

Evidence:

- 14 workspace tests plus 17 unit tests;
- activation resolves one exact commit from the *authority*, proven by a test
  that advances the candidate first so the two heads differ;
- a frozen baseline never moves: advancing the authority and re-activating is
  refused, and the recorded baseline is unchanged afterwards;
- every Section 11.1 transition is asserted legal and undocumented ones fail in
  the policy category naming the permitted alternatives;
- an abandoned cycle refuses every onward transition;
- status is folded from the event log, and a tampered cached status is reported
  as drift with history winning rather than being silently trusted;
- events and cycle records are versioned while the journal is not.

Note: `WP-120` listed append-only event commits as a deliverable and shipped
without them. The event store was added here, before the cycle model that needs
it, rather than leaving the gap recorded as complete.

Deliverables:

- cycle schema and domain model;
- release invariants;
- frozen baseline;
- atomic-group and card membership validation;
- cycle state machine;
- `cycle create`, `activate`, `status`, `abandon`.

Acceptance:

- activation records one exact authority baseline;
- active cycles cannot silently change baseline;
- invalid transitions fail;
- abandoned cycles cannot accept new cards;
- status is derived from authoritative events.

### WP-210 — Card schema, canonicalization, and digest

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Owner | Claude |
| Completed | 2026-07-28 |
| Dependencies | `WP-200` |
| Target release | Single-repository MVP |

Evidence:

- 14 workspace tests plus 22 unit tests;
- a committed digest vector pins the canonical form, so an accidental change to
  canonicalization fails there rather than silently invalidating stored reviews;
- reordered YAML and JSON drafts produce identical activated digests under a
  fixed clock, and six separate material changes each move the digest;
- activation rejects a missing goal, acceptance behavior, feature gate, write
  scope, review policy, or rollback strategy, and rejects a `base_sha` that is
  not a full object ID;
- unsafe write-scope patterns are rejected: absolute, upward-traversing,
  `.git`-naming, and backslash-separated;
- an activated revision is never rewritten, and `card status` recomputes the
  digest rather than trusting it, so an edited record is detected;
- a revision supersedes rather than replaces: both revision records survive and
  the event records the invalidated revision and digest.

Note on the digest: `created_at` and `base_sha` participate, so the digest
identifies an exact record instance rather than a class of equivalent cards.
Two identical drafts activated at different times digest differently, which is
correct for binding evidence to one exact card.

Deliverables:

- draft YAML input;
- strict activated JSON schema;
- canonical serialization;
- committed digest vectors;
- card revision rules;
- immutable activated record;
- `card create`, `validate`, `activate`, `revise`, `status`.

Acceptance:

- equivalent drafts produce the same activated digest;
- material field changes produce different digests;
- unknown fields fail;
- activation rejects missing goals, acceptance, or gates;
- revisions invalidate prior dependent records.

### WP-220 — Ownership, contracts, resources, and dependencies

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Owner | Claude |
| Completed | 2026-07-28 |
| Dependencies | `WP-210` |
| Target release | Single-repository MVP |

Evidence:

- 13 workspace tests plus 31 unit tests;
- overlapping cards cannot both activate, and the refusal names both patterns
  and the conflicting card;
- excludes override includes, demonstrated by two cards sharing a directory
  because one excludes the subtree the other owns;
- contract overlap is refused with disjoint paths, and the test asserts the
  path check finds nothing first, so only the contract check can catch it;
- a dependency cycle is refused with the route through it, not a bare
  "cycle exists";
- released claims do not block: closed, abandoned, landed, and draft cards all
  free their region, while eight in-flight states still hold it;
- a revision that widens scope into a conflict is refused, so allocation is
  re-checked rather than assumed to hold from activation.

Note: the path matcher is implemented in-tree rather than taken from a crate.
What a pattern means decides what a card may change, so the semantics are
specified and tested here rather than inherited. Pattern intersection
deliberately over-approximates: refusing two cards that might overlap costs one
conversation, admitting two that do costs a silent lost write.

Deliverables:

- include/exclude path policy;
- case-normalized overlap checks appropriate to the host filesystem;
- contract-domain ownership;
- exclusive-resource reservations;
- dependency DAG validation;
- active-card global allocation validator.

Acceptance:

- overlapping cards cannot both activate;
- excludes override includes;
- contract overlap fails even when paths differ;
- dependency cycles fail with an explanatory path;
- inactive/closed allocations do not block new cards.

### WP-230 — Safe worktree allocation and resume link

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Owner | Claude |
| Completed | 2026-07-28 |
| Dependencies | `WP-120`, `WP-130`, `WP-220` |
| Target release | Single-repository MVP |

Evidence:

- 19 workspace tests plus 16 unit tests;
- the branch begins at exactly the card's declared base, asserted against the
  authority head rather than the candidate's;
- duplicate lease, duplicate branch, and duplicate worktree path each fail with
  their own code, and the lease check runs first because it is the actionable
  diagnosis;
- a card whose base is absent is refused with nothing created, proven by
  asserting the branch and worktree do not exist afterwards;
- resume rejects a locator that disagrees with control on card or lease, and
  names the disagreeing field;
- nothing forces: `branch -d` not `-D`, `worktree remove` without `--force`,
  and a test leaves uncommitted work in an allocated worktree and asserts it
  survives a refused re-allocation;
- the `.agent/` rule lands in `info/exclude`, never in the committed
  `.gitignore`, and installing it twice is a no-op;
- allocation postconditions are verified rather than assumed: registration,
  checked-out head, and cleanliness are each confirmed before success.

Deliverables:

- lease record;
- journaled branch/worktree creation;
- locked worktree;
- ignored `.agent/project.json`;
- idempotent `.agent/` common-exclude installation;
- initial progress record;
- `work start`, `status`, `checkpoint`, `resume`, `block`;
- `--dry-run` for every mutating command in this package, as required by
  Section 12.1;
- recovery for every allocation boundary.

Acceptance:

- branch begins at the exact card base;
- duplicate lease, branch, or worktree fails;
- partial allocation is recoverable;
- resume rejects a locator/control mismatch;
- no force deletion occurs.

### WP-240 — Authoritative candidate verification

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Owner | Claude |
| Completed | 2026-07-28 |
| Dependencies | `WP-220`, `WP-230` |
| Target release | Single-repository MVP |

Evidence:

- 13 workspace tests plus 19 unit tests;
- mandatory scenarios 13 through 19 each have a named unit test and, where
  observable end to end, a workspace test;
- a card cached inside the worktree cannot widen the real scope: one test plants
  a permissive card in `.agent/` and asserts the out-of-scope change is still
  refused;
- verification is a pure function of committed objects, so repeated runs are
  byte-identical, which is what makes identical results from another clean clone
  achievable;
- a rename is checked on both sides, so moving a file from outside the scope
  into it is refused on the source;
- an excluded path is reported distinctly from an out-of-scope path, because the
  operator fix differs.

Note: verification diffs the declared base against the candidate, not commit to
commit. A file added and then chmod'd within the candidate is therefore a single
addition at mode 100755, not a mode change. A first test asserted otherwise and
was wrong about the model rather than finding a defect.

Deliverables:

- exact base-to-head diff verification;
- full path/mode/symlink/submodule checks;
- commit-message policy;
- dependency and generated-artifact declaration checks;
- clean-worktree requirement;
- structured policy report.

Acceptance:

- all mandatory scenarios 13 through 19 pass;
- cached worktree card changes cannot alter verification;
- verification produces identical results from another clean clone;
- out-of-scope candidates cannot hand off.

### WP-250 — Exact-SHA handoff

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Owner | Claude |
| Completed | 2026-07-28 |
| Dependencies | `WP-240`, `WP-310` |
| Target release | Single-repository MVP |

Evidence:

- 13 workspace tests plus 11 unit tests;
- **`SPIKE-001` finding F-1 is closed.** A branch amended after delivery is
  refused with `CH-POLICY-DELIVERED-SHA-MISMATCH`, and the refusal names both
  the delivered and the actual SHA. The spike's implementer caught this by
  reading a reflog; it is now a control;
- a declaration missing narrative fields is refused, while empty assumption and
  risk lists are permitted, because an empty list is a claim and an absent field
  is not;
- a dirty worktree, missing gate evidence, and stale gate evidence each refuse
  with their own code;
- a head change or a card revision invalidates an existing handoff, and
  `handoff inspect` names which binding broke and warns that a review recorded
  against it would be reviewing different code;
- the handoff is reproducible: `create` and `inspect` produce identical records
  and digests, and the record names its canonicalization algorithm (F-2);
- revoking returns the card to active work and a revoked handoff never applies
  again.

Deliverables:

- generated factual handoff section;
- required actor-authored declaration;
- actor-declared `delivered_sha`, compared against the branch head at handoff
  creation (`SPIKE-001` finding F-1, D-022, R-017);
- handoff digest;
- branch freeze expectation;
- invalidation event;
- `handoff create`, `inspect`, `revoke`.

Acceptance:

- missing narrative or evidence fields fail;
- a dirty worktree fails;
- stale receipts fail;
- a head change automatically invalidates handoff eligibility;
- a branch rewritten between delivery and handoff creation fails with a
  distinct policy error naming both SHAs, and does not produce a handoff;
- handoff can be reproduced from control and Git objects;
- the handoff record names its canonicalization algorithm and carries or
  references the card, so every digest is independently recomputable
  (`SPIKE-001` finding F-2, R-018).

### WP-300 — Named gate registry

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Owner | Claude |
| Completed | 2026-07-28 |
| Dependencies | `WP-110`, `WP-210` |
| Target release | Single-repository MVP |

Evidence:

- 14 workspace tests plus 17 unit tests;
- a card cannot introduce a command: the schema has no field that carries one,
  and naming an unregistered gate fails activation with the gate named;
- shell strings are refused at registration, including a single argv entry
  containing spaces, pipes, semicolons, and command substitution;
- an argument may still contain spaces, because nothing splits it; only the
  executable is checked for shell syntax;
- working directories outside the evaluation worktree are refused;
- credential variables are denied even when explicitly allowlisted;
- a gate revision moves its digest and records the superseded digest in the
  event, which is what makes an older receipt detectably stale;
- revisions must advance by exactly one, so a receipt traces to a definition
  rather than to whichever version happened to be on disk.

Deliverables:

- gate schema;
- strict registry loader;
- argv-only command contract;
- environment allowlist;
- timeout and retry policies;
- gate digest.

Acceptance:

- cards cannot introduce commands;
- unknown gates fail card activation;
- shell strings are not accepted;
- gate revisions invalidate prior receipts;
- invalid working directories are rejected.

### WP-310 — Gate runner and receipts

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Owner | Claude |
| Completed | 2026-07-28 |
| Dependencies | `WP-300` |
| Target release | Single-repository MVP |

Evidence:

- 13 workspace tests plus 24 unit tests;
- success, failure, timeout, signal, and runner error are separate terminations,
  each asserted: a clean non-zero exit is `completed`, a deadline is `timeout`,
  and a missing executable is a runner error rather than a gate failure;
- every attempt is recorded and numbered, with logs written to per-attempt paths
  so a retry cannot overwrite the evidence of the attempt before it;
- the child starts from an empty environment and receives only what the gate
  names, proven by a gate that cannot see `HOME` unless it is allowlisted;
- gate output never reaches the JSON envelope: a gate that prints JSON to stdout
  is captured to log files while the envelope stays one clean document;
- a receipt goes stale when either the candidate or the gate definition moves,
  and the staleness message names which binding broke;
- a gate passing only beyond its declared attempts is not acceptable evidence.

Two defects were found by tests rather than by inspection:

- reading the child's pipes only after it exits deadlocks any gate that prints
  more than the pipe buffer, roughly 64 KiB. The gate would then hang until its
  own timeout. Fixed by draining both streams concurrently; the bounded-log test
  went from 30 seconds to 1;
- gate logs were being committed to control history. Section 14.3 gives logs
  retention windows rather than permanence, so they are now excluded and the
  receipt carries their location and digest, which is what invariant 7.4.2
  actually requires.

Deliverables:

- process-group execution;
- timeout termination;
- bounded stdout/stderr logs;
- environment fingerprint;
- SHA-256 log/artifact digests;
- retry accounting;
- `gate run`, `gate status`.

Acceptance:

- success, failure, timeout, signal, and runner error are distinct;
- every attempt is recorded;
- no unallowlisted environment variable reaches the child;
- logs do not print through JSON stdout;
- stale receipts are rejected.

### WP-320 — Independent review

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Owner | Claude |
| Completed | 2026-07-28 |
| Dependencies | `WP-250`, `WP-310` |
| Target release | Single-repository MVP |

Evidence:

- 14 workspace tests plus 13 unit tests;
- self-review is refused, and the message says the check is procedural rather
  than claiming an identity proof it does not have;
- **`SPIKE-001` finding F-4 is closed.** Findings carry a disposition, so a
  reviewer can approve while recording that a real problem is accepted or
  unfixable within the card's write scope. Approving over an *open* finding is
  refused;
- **`SPIKE-001` finding F-5 is closed.** Every review states whether the gates
  can observe the acceptance behaviors and how that was established. A review
  reporting inadequate gates still approves, and the shortfall is surfaced as a
  warning rather than buried;
- a candidate change or a card revision invalidates an approval;
- reviewing a superseded handoff is refused, which is `SPIKE-001` F-1's failure
  one stage later;
- a re-review supersedes rather than erases: the earlier finding survives in its
  own record, the later review names what it supersedes, and the test asserts
  the approval came from a different reviewer than the one who raised the
  finding, reproducing spike hypothesis `H-04`.

Gap found and closed while building this: Section 11.2 permits
`changes_requested -> active`, but no command performed it, so a card that
received review feedback could never be handed off again. `work resume` now
performs it. See D-037.

Deliverables:

- review schema;
- reviewer/feature-actor separation check;
- findings and residual-risk structure, with a per-finding `disposition` of
  resolved, accepted as residual risk, or unresolvable within the card's write
  scope (`SPIKE-001` finding F-4, D-025);
- re-review records that supersede a prior review and disposition each of its
  findings;
- required gate-adequacy assessment stating whether the named gates can
  actually observe each acceptance behavior (`SPIKE-001` finding F-5, D-024);
- review gate binding;
- review invalidation;
- `review begin`, `record`, `inspect`.

Acceptance:

- self-review declaration fails;
- approval references exact candidate/card/dependencies;
- a candidate change invalidates approval;
- changes requested return the card to active work;
- findings remain visible after subsequent approval;
- a re-review that leaves a prior finding undispositioned fails;
- an approval whose gate-adequacy assessment reports an unobservable
  acceptance behavior records that fact rather than discarding it.

### WP-400 — Bare authority initialization

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Dependencies | `WP-120`, `WP-130` |
| Target release | Single-repository MVP |
| Evidence | `src/git/authority.rs`, `establish_authority` in `src/commands/project.rs`, `tests/authority.rs` (13 acceptance tests) |

Deliverables:

- compatible bare-repository detection;
- non-destructive initialization;
- explicit authority remote;
- initial protected-branch transfer;
- expected-ref inspection;
- authority health check.

Acceptance:

- existing unrelated directories fail;
- initialization never overwrites a remote;
- protected branch matches the configured baseline;
- candidate repo retains existing remotes;
- rerun with identical state is idempotent.

Delivered notes:

- `project init` gained an `authority-initialized` journal step, so an
  interrupted initialization is attributable to the authority boundary rather
  than guessed at.
- The health check lives in `project status` rather than `doctor`. `doctor`
  inspects a path with no project configuration and therefore cannot know which
  authority to check; `project status` already opens the control repository and
  reads the configuration. It reports the authority as data — reachability,
  bareness, protected SHA, and whether the candidate's remote still points at
  it — and never fails because the authority is unhealthy, since that is
  exactly when the report is needed.
- Seeding transfers objects through a staging ref under
  `refs/harness/incoming` and deletes it afterwards, so a fresh authority ends
  initialization holding exactly one ref: the protected branch.

### WP-410 — Integration plan and dependency order

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Dependencies | `WP-320`, `WP-400` |
| Target release | Single-repository MVP |
| Evidence | `src/domain/integration.rs`, `src/commands/integration.rs`, `cycle declare-group` in `src/commands/cycle.rs`, `tests/integration_plan.rs` (17 acceptance tests) |

Deliverables:

- integration schema;
- selected-card validation;
- a discoverable ready-to-integrate view listing approved cards awaiting
  integration, so no actor has to hold that state out of band
  (`SPIKE-001` finding F-3);
- an explicit statement of whether cards land individually or as a batch;
- topological ordering;
- atomic-group handling;
- integration lease;
- `integration prepare`, `inspect`.

Acceptance:

- only approved exact candidates are selected;
- an actor resuming from a cold context can determine which cards are awaiting
  integration from harness state alone;
- missing dependencies fail;
- ordering is deterministic;
- one integration lease exists per cycle/project;
- prepared state records expected authority baseline.

Delivered notes:

- Closes `SPIKE-001` finding F-3. `integration ready` reports both halves of
  the question: the cards awaiting integration, and — for each card that is
  not ready — why. A bare list of ready cards would leave "approved but the
  branch moved" indistinguishable from "never reviewed", and that distinction
  is the whole diagnosis.
- Readiness is judged against the candidate branch head, not against the SHA
  the handoff recorded. Comparing a record with itself always agrees; a branch
  that gained a commit after approval would otherwise stay ready and integrate
  a commit nobody reviewed. `review record` already applies this rule, so
  selection now applies the same one (D-041).
- The integration lease is the non-terminal integration record itself rather
  than a separate lease file, so the claim cannot disagree with the thing it
  claims (D-040).
- `WP-200` listed atomic-group validation as a deliverable and shipped it
  unreachable: `CycleRecord::validate` checked groups thoroughly, but no
  command could declare one. `cycle declare-group` was added here rather than
  leaving the gap recorded as complete — the same correction `WP-200` itself
  made for `WP-120`'s event store.

### WP-420 — Merge preflight and disposable integration

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Dependencies | `WP-410` |
| Target release | Single-repository MVP |
| Evidence | `src/git/merge.rs`, `src/git/integration_worktree.rs`, `integration preflight` and `merge` in `src/commands/integration.rs`, `tests/merge_preflight.rs` (15 acceptance tests) |

Deliverables:

- disposable integration worktree;
- non-destructive merge preflight;
- deterministic candidate merging;
- conflict classification;
- integration-fix-card route;
- intermediate smoke/contract gates.

Acceptance:

- feature untracked state cannot enter integration;
- textual conflicts block without partial landing;
- semantic conflict resolution cannot be silently committed;
- integration order matches the DAG;
- worktree is clean after each accepted group.

Delivered notes:

- Section 13.4's precondition is met: `merge-tree --write-tree` output and
  exit behavior are covered by unit tests against a real repository, including
  the `-z` framing, the `Auto-merging` records that must not read as
  conflicts, and the exit-128 refusal of unrelated histories. There is no
  fallback path — project validation refuses a Git older than
  `MINIMUM_GIT_VERSION`, which exists for this command.
- The preflight carries state forward with unreachable `commit-tree` objects
  (D-042), so simulating a multi-card sequence still writes no ref, index, or
  worktree.
- The sequence stops at the first conflict and reports how many members were
  left unevaluated. Continuing would merge later candidates against a state
  that will never exist.
- Conflicts are classified textual or structural, and an unrecognized conflict
  token is kept and treated as structural rather than dropped — a conflict the
  code cannot name must not become a clean preflight.
- Intermediate gates run after each candidate merges, not once at the end, so
  a failure names the candidate that broke the combination.
- Note on testing: two cards cannot be made to conflict with each other,
  because `WP-230` refuses two active cards claiming the same path. A conflict
  reaches integration by the other route — the protected branch moving under
  an approved candidate — which is what the acceptance test exercises.
- The integration-fix-card route is guidance, not a command: a conflicted
  preflight and the `CH-CONFLICT-MERGE-FAILED` recovery text both direct the
  actor to resolve it in a new card. The harness never resolves a conflict
  itself, which is what keeps a semantic resolution from being silently
  committed.

### WP-430 — Landing commit construction

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Dependencies | `WP-420` |
| Target release | Single-repository MVP |
| Evidence | `src/git/landing.rs`, `integration land` in `src/commands/integration.rs`, `tests/landing_commit.rs` (13 acceptance tests) |

Deliverables:

- exact tree validation;
- two-parent landing commit;
- commit trailers;
- landing object inspection.

Acceptance:

- landing first parent equals expected authority baseline;
- landing second parent equals integration head;
- landing tree equals the verified integration tree.
- the landing commit is created without changing the protected authority ref;
- projects declaring shared generated artifacts fail with a stable
  `unsupported-until-WP-540` policy error.

Delivered notes:

- The landing commit is built with `commit-tree`, so building it moves no
  branch, and is then held by `refs/harness/landing/<INT-id>` (D-044).
  Section 13.5 requires it to be unreachable *from the protected branch*, not
  unreachable outright; an object nothing points at is a collection candidate,
  and losing it between construction and promotion would mean rebuilding and
  re-verifying everything.
- Exact tree validation compares the recorded integration tree against what
  the integration head carries *now*, not against what the merge reported. If
  they disagree, something rewrote the head after the merge.
- Landing also refuses when the authority has moved since the plan was built,
  because the first parent would then not be the branch promotion updates.
- Trailers follow Git's own convention, so `git interpret-trailers` reads them
  without knowing about this harness. They name the cycle, every card with its
  revision, candidate SHA and approving review, the integration record digest,
  and every gate receipt — enough to explain the commit from the candidate
  repository alone, without the control repository.
- `integration inspect` now surfaces `integration_head`, `integration_tree`,
  and `landing_sha`. Promotion reloads through it, and a field it cannot see
  is a field promotion cannot verify.

`WP-540` extends this path with integration-owned artifact generation. It is
not a prerequisite for MVP projects that declare no shared generated
artifacts.

### WP-440 — Combined verification and integration review

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Dependencies | `WP-420`, `WP-310` |
| Target release | Single-repository MVP |
| Evidence | `VerificationRecord` and `interactions` in `src/domain/integration.rs`, `integration verify` and `review` in `src/commands/integration.rs`, `tests/combined_verification.rs` (13 acceptance tests) |

Deliverables:

- clean-worktree rerun of required feature/review/integration gates;
- cycle-invariant report;
- combined interaction checklist;
- integration review record;
- exact landing-receipt binding;
- `integration verify`, `integration review`.

Acceptance:

- every final receipt evaluates the exact landing SHA;
- stale feature evidence cannot replace integration reruns;
- combined gate failure blocks acceptance;
- dirty post-gate worktree fails;
- integration review records residual risk.

Delivered notes:

- Verification reruns every gate any member names — feature, review, and
  integration alike — against the landing commit in a fresh disposable
  worktree. A feature gate that passed on an isolated candidate proves nothing
  about the combined tree, which is the entire reason the rerun exists.
- The receipt record gained an integration scope (D-046). A combined run
  belongs to the integration, not to one card, and attributing it to a member
  would be a false claim about what was checked. This amends Section 10.6:
  `card_id` and `card_digest` become optional and `integration_id` is added;
  exactly one of the two subjects is always present.
- The worktree is checked for cleanliness *after* the gates as well as before.
  A gate that writes into the tree it is checking invalidates its own result,
  and the diagnosis says so rather than reporting a generic gate failure.
- The combined interaction checklist is derived from the members' declared
  contracts: ordered pairs where what one card changes is what another reads.
  Directions are listed separately, because "A changes what B reads" is a
  different thing to look at than the reverse.
- Cycle release invariants are free text, so no gate can evaluate them. They
  are carried into the verification record marked `machine_checked: false`,
  and `integration review` refuses to record a decision that leaves one
  unconfirmed (D-047). An invariant nobody is shown is an invariant nobody
  checks.
- A failed verification is recorded and committed before the command fails, so
  a retry cannot present itself as the first attempt.
- Section 15.1's independence rule extends to this stage: whoever ran the
  gates cannot also be the actor who judges what they proved.

### WP-450 — Acceptance and promotion

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Dependencies | `WP-400`, `WP-430`, `WP-440` |
| Target release | Single-repository MVP |
| Evidence | `src/domain/acceptance.rs`, `src/commands/acceptance.rs`, `integration promote` in `src/commands/integration.rs`, `tests/promotion.rs` (15 acceptance tests) |

Deliverables:

- acceptance schema and command;
- exact-SHA/digest validation;
- authority expected-old promotion;
- clean main-worktree synchronization;
- partial-success journal and recovery;
- `acceptance record`, `inspect`, `integration promote`.

Acceptance:

- moved authority branch fails before update;
- dirty local main fails before authority update;
- exact accepted landing succeeds;
- authority success/local-sync interruption returns recovery-required state;
- no command rewinds authority automatically.

Delivered notes:

- Every Section 13.6 precondition is checked before the authority moves,
  because that is the only irreversible step in the harness. Anything
  discoverable afterwards is worth discovering first.
- The landing commit's shape is read back from the object — first parent, tree
  — rather than trusted from the record that describes it. Verifying a record
  against itself proves nothing.
- A rejection is recorded but advances nothing. Section 11.3 has no transition
  out of `reviewed` for a refusal, and inventing one would erase the fact that
  the work reached review at all.
- Fixes a real gap this package exposed (D-048): `with_transaction` decided
  between `failed_clean` and `failed_partial` by checking whether the control
  worktree was clean. Promotion's irreversible step happens in the *authority*,
  which that check cannot see, so an authority-promoted, local-sync-failed run
  would have been journaled as a clean failure and `project recover` would have
  stayed silent about it. An error whose category is `recovery-required` is now
  taken at its word.
- The partial-success path is exercised by a real failure — another process
  holding the index lock — not by inspection. Under normal conditions the
  fast-forward cannot fail, since the landing commit descends from the commit
  the precondition check confirmed.

### WP-460 — Archive, close, and cleanup

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Dependencies | `WP-450` |
| Target release | Single-repository MVP |
| Evidence | `src/git/archive.rs`, `src/domain/archive.rs`, `src/commands/archive.rs`, `tests/archive_cleanup.rs` (15 acceptance tests) |

Deliverables:

- archive refs for cards and integrations;
- reachability verification;
- handoff/review/receipt archive index;
- safe worktree removal;
- safe ordinary branch deletion;
- `archive create`, `verify`, `close`.

Acceptance:

- unarchived unique commits block cleanup;
- dirty worktrees block cleanup;
- landed/archived commits remain reachable;
- closed state is terminal;
- repeated close is idempotent.

Delivered notes:

- Candidates are archived as well as the landing commit. A landing tree does
  not contain the individual candidate commits, so deleting the card branches
  without archiving them would lose the record of how the change was made.
- "Unarchived" is computed, not assumed: `rev-list <branch> --not <every other
  ref>` names the commits that would become unreachable, and a non-empty
  result refuses the cleanup. The reachability test asserts survival across
  `git gc --prune=now`, because a ref that merely exists proves less than an
  object that survives collection.
- Fixes a real interaction this package exposed (D-049): `work start` locks
  card worktrees so `git worktree prune` cannot reclaim them, which made every
  removal fail on the lock rather than on dirtiness. Cleanup now checks
  cleanliness itself *before* unlocking, so the refusal never depends on the
  lock and a dirty worktree is never left unlocked on the failure path.
  Removal itself stays non-forcing, as invariant 7.2 requires.
- `archive close` re-verifies every archived ref before removing anything. The
  archive record's existence is not evidence that the refs still resolve.
- A repeated close is a no-op that reports `changed: false` rather than an
  error: an operator unsure whether cleanup finished should be able to ask
  again.

### WP-500 — Recovery and failure injection

Note: `project recover --resume` for the promotion boundary was pulled into the
MVP by D-051, because Section 19.3 requires that state to be recovered and not
merely reached. `WP-500` retains systematic failure injection at every journaled
boundary, and recovery for boundaries introduced after the MVP.

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Dependencies | `WP-460` |
| Target release | Hardened single-repository release |
| Evidence | `INJECT_FAILURE_VAR` and `Journal::step` in `src/control/journal.rs`, `Steps` in `src/commands/transaction.rs`, named boundaries across all ten command modules, `tests/recovery.rs` (8 acceptance tests) |

Deliverables:

- injectable interruption points;
- recovery tests for every mutation journal;
- consistent operation-status inspection;
- operator recovery instructions;
- unresolved-operation blocking.

Acceptance:

- injected failure covers every journaled mutation boundary, extending
  scenarios 34 and 35 beyond the naturally occurring interruptions already
  demonstrated for the MVP gate;
- no interruption silently reports success;
- recovery never deletes ambiguous work;
- completed operations are idempotently recognized.

Delivered notes:

- Injection lives in `Journal::step`, so it reaches every boundary any command
  names, present or future, from one insertion point. The step is written
  before the work it names, which means an injected failure lands in the
  hardest place for recovery: the boundary was recorded and its work did not
  happen.
- The affordance is compiled in rather than hidden behind a feature flag, so
  the code under test is the code that ships (D-055). It can only make a
  command fail at a boundary the harness already has to handle; it cannot
  produce a silent success or a write recovery cannot see.
- `with_transaction` now hands the body a `Steps` recorder. Every mutating
  command names at least one boundary, and a test asserts that against the
  source rather than trusting it — a command that names none has no boundary
  to inject at, which would make the coverage claim false without failing
  anything.
- Allocation names three boundaries rather than one, because the recoveries
  differ: a branch with no worktree is retryable once the branch is removed,
  while a worktree with no lock is already usable and must not be discarded.
- The interrupted-allocation test asserts recovery leaves the half-created
  branch alone. Deleting it would be tidier and wrong: its disposition is
  ambiguous, and cleanup that guesses is indistinguishable from data loss.

| D-055 | Compile failure injection in rather than gate it behind a feature flag | Accepted | A `#[cfg(feature)]` affordance means the binary under test is not the binary that ships, which is precisely the gap this package exists to close. Leaving it compiled in is safe because it can only cause a command to fail at a boundary the harness already journals and already has to handle — never a silent success, and never a write recovery cannot see. It is driven by an environment variable that names one step, so it cannot be triggered by accident in a way that does anything the harness is not already required to survive. |

### WP-510 — Concurrency and lease hardening

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Dependencies | `WP-500` |
| Target release | Hardened single-repository release |
| Evidence | `LockDiagnosis` in `src/control/lock.rs`, `work reclaim` in `src/commands/work.rs`, `tests/concurrency.rs` (11 acceptance tests) |

Deliverables:

- cross-process lock implementation;
- stale-lock diagnosis;
- explicit lease reclaim;
- control-HEAD compare-and-swap;
- concurrent integration rejection.

Acceptance:

- parallel mutation tests have one winner;
- lease reclaim preserves candidate commits;
- PID reuse does not silently steal a lease;
- manual decision is required for ambiguous ownership.

Delivered notes:

- A lock records the holder's process start time alongside its PID. The PID
  alone is not enough: PIDs are recycled, so a lock left by a crashed process
  can appear held by whatever unrelated program later inherited its number, and
  the harness would then wait forever on something with no connection to it.
  Two processes can share a PID but not a PID and a start instant.
- The diagnosis has four outcomes, not two. `Ambiguous` is separate from
  `Stale` on purpose (D-056): clearing a lock whose holder might still be
  writing is how two processes interleave mutations, so an unprovable case
  escalates to a person instead of being resolved by optimism. A lock written
  before start times were recorded is ambiguous, not stale.
- `clear_stale` takes the diagnosis rather than re-deriving it, so no caller
  can skip the check by accident.
- Clearing happens before the journal is consulted. A process killed outright
  leaves a lock and *no* unresolved entry, so gating the clear on the journal
  would have left the commonest stale lock permanent — which the first version
  of this package did, and the acceptance test caught.
- The start time is read with `LC_ALL=C` pinned. `lstart` renders in the
  caller's locale — `Tue Jul 28` against `Di. 28 Juli` for the same instant —
  and it is compared as a string across invocations that may run under
  different environments, such as an interactive shell and a cron job. Without
  pinning, the same live process reads as a different one, the lock is declared
  stale, and recovery clears a lock whose holder is still writing: exactly the
  interleaving D-056 was written to prevent. Caught by writing it down as a
  residual risk on the handoff declaration and then checking it instead of
  accepting it; the handoff was revoked and the fix made under the same card.
- A test named for what it checked but did not check it:
  `a_reclaim_is_recorded_with_the_head_it_preserved` asserted the actor and the
  reason and never the head, so it passed whether or not the head was recorded.
  Found by mutation-testing the review's own gate-adequacy claim instead of
  writing it down — the claim was false, and the test was the reason.
- `work reclaim` touches nothing in the candidate repository. The branch, the
  worktree, and every commit survive: a lease says who is responsible for a
  card, not what the work is worth, and an abandoned lease is a coordination
  problem rather than a reason to destroy code. The preserved head is recorded
  in the event, so the claim is checkable from the log rather than trusted.

| D-056 | Distinguish an ambiguous lock from a stale one | Accepted | The tempting design has two states, held and stale, with anything unprovable treated as stale so the harness never wedges. That trades a rare inconvenience for a rare correctness failure: clearing a lock whose holder is still writing lets two processes interleave mutations, which is the exact hazard the lock exists to prevent. An unprovable lock is therefore refused with its own code and left for a person, who can see the PID and decide. Wedging is recoverable; interleaved writes may not be. |

### WP-520 — Backup and integrity verification

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Dependencies | `WP-460` |
| Target release | Hardened single-repository release |
| Evidence | `src/git/backup.rs`, `src/commands/backup.rs`, `tests/backup.rs` (9 acceptance tests), restore drill documented in `README.md` |

Deliverables:

- backup policy configuration;
- Git bundle or mirror creation;
- independent-destination validation;
- `git bundle verify`/`git fsck` integration;
- receipt/control backup;
- restore drill documentation.

Acceptance:

- same-disk sibling directories do not satisfy independent-backup policy;
- corrupted backup verification fails;
- archive refs exist in the backup;
- one restore drill reconstructs authority and control state.

Delivered notes:

- `git bundle verify` is not sufficient, and finding that out shaped the
  design. It validates the header and the prerequisite commits and then reports
  success — a bundle truncated mid-pack passes it cleanly. The first version of
  this package used it alone and the acceptance test caught it, which is the
  module's own stated failure mode committed by its author.
- Verification therefore restores the bundle into a throwaway repository and
  runs `fsck` over the result (D-057). That reads every object and makes
  "verified" mean what an operator cares about: it restores. The refs in the
  report are listed from the restored copy, so the report describes what came
  out rather than what the file claims.
- The source is `fsck`ed before it is bundled. Backing up an already-damaged
  repository produces a faithful backup of the damage.
- `--allow-same-device` exists because a single-disk laptop is common, and an
  outright refusal pushes people to skip backups rather than take a weak one.
  The weakness is reported as a warning on the successful result either way.
- Independence that cannot be established is refused rather than assumed good,
  for the same reason an unprovable lock is ambiguous rather than stale.
- `tempfile` moved from a dev-dependency to a runtime one, which required
  revising the card: its write scope did not include `Cargo.toml`, and the
  harness would have refused the handoff.

| D-057 | Verify a backup by restoring it, not by calling `git bundle verify` | Accepted | `bundle verify` checks the header and the prerequisite commits, then returns success; a bundle truncated mid-pack passes. A backup verifier that accepts a half-written file is worse than no verifier, because it converts an unknown into a false assurance. Restoring into a scratch repository and running `fsck` reads every object, costs a full read of the backup — the correct price for the claim — and makes the restore drill a property of every verification rather than a separate ceremony someone remembers to do. A regression test pins the reason: it asserts that `bundle verify` alone still accepts a truncated bundle, so if Git ever gets stricter, the restore step can be reconsidered deliberately. |

### WP-530 — Audit report and redaction

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Dependencies | `WP-460` |
| Target release | Hardened single-repository release |
| Evidence | `src/commands/audit.rs`, `tests/audit.rs` (9 acceptance tests) |

Deliverables:

- cycle audit report;
- chronological event reconstruction;
- SHA/digest cross-check;
- log redaction tests;
- residual-risk and rollback summary.

Acceptance:

- report can be reproduced from authority/control state;
- secrets are excluded;
- missing or mismatched evidence is explicit;
- report identifies the exact protected-branch transition.

Delivered notes:

- The report's value is entirely in the discrepancies. A summary of records
  that agree tells a reader nothing they could not get by listing files; what
  they cannot get any other way is whether the evidence still describes the
  objects it names. So a stale digest, a vanished commit, or a missing revision
  is a *finding*, never a line quietly dropped because it could not be
  resolved (D-058).
- A report that found discrepancies exits non-zero. A caller piping the result
  onward must not have to parse prose to learn the answer, which is the same
  reasoning as D-033 for candidate verification.
- Redaction is structural rather than filtered: gate logs live outside control
  history by D-036, and the report names their location and never reads them.
  Nothing needs a secret-scanning heuristic, because the captured third-party
  text is never opened. The test proves the negative properly — it writes a
  recognizable secret through a real gate, asserts it reached disk, then
  asserts it is absent from the report.
- Events are ordered by their monotonic identifiers rather than by timestamp,
  because two events recorded in the same second would otherwise sort
  arbitrarily and the timeline is the point.

| D-058 | Report unresolvable evidence as a discrepancy rather than omitting it | Accepted | The tempting implementation skips what it cannot resolve, which produces a clean-looking report from a damaged record — the worst possible output, because it converts an unknown into a false assurance in exactly the situation an audit exists for. A missing revision, a stale digest, and a vanished commit are all reported with what the record claims and what was found, and the command exits non-zero so a caller does not have to read prose to learn there was a problem. |

### WP-540 — Generated-artifact governance

| Field | Value |
| --- | --- |
| Status | `DONE` |
| Dependencies | `WP-310`, `WP-420` |
| Target release | Hardened single-repository release |
| Evidence | `src/domain/artifact.rs`, validation in `src/domain/card.rs`, enforcement in `src/policy/verification.rs`, `tests/artifacts.rs` (10 acceptance tests) |

Deliverables:

- transient, per-card, shared, and serialized classifications;
- generator gate references;
- deterministic regeneration checks;
- integration ownership;
- snapshot/lockfile policy.

Acceptance:

- each generated path has exactly one class;
- transient outputs cannot be committed;
- per-card outputs require in-scope sources;
- shared outputs are generated once in integration;
- serialized identifiers are allocated before work.

Delivered notes:

- The class is required rather than defaulted. A path with no class is the
  failure this package exists to prevent, and guessing one would reintroduce
  it quietly (D-059).
- `generated_artifacts` changed from `Vec<String>` to a typed declaration,
  which is a change to the Section 10.3 card contract. Every stored card in
  this project declared an empty list, so nothing needed migrating.
- Each class implies an owner, and the scope rules follow from that. A
  per-card artifact generated from sources the card does not own would go
  stale the moment somebody else changed them, with nothing to notice. A
  shared artifact inside a card's write scope is one path with two owners,
  which is the exact collision the classes prevent.
- A transient artifact naming a generator is refused. Not pedantry: a
  generator implies someone believes the file is regenerated and checked, and
  nothing checks a file that is never committed.
- **Not delivered: integration-owned generation.** A shared artifact is
  classified and no card may commit it, but nothing produces it during
  integration yet. The `WP-430` landing guard therefore still refuses a
  shared declaration — narrowed from refusing *all* generated artifacts, which
  was the conservative reading available before classes existed. Deterministic
  regeneration checking is likewise not built. Both are named here rather than
  implied by the package being `DONE`.

| D-059 | Require a class on every generated declaration rather than defaulting one | Accepted | A default would have to be either transient — silently forbidding a file somebody committed on purpose — or per-card, silently granting a card ownership of something integration should produce. Both restore the ambiguity the classification exists to remove, and neither failure is visible at the point it is made. Requiring the class costs one line per declaration and makes the ownership decision explicit where it belongs, in the card under review. |

### WP-600 — Workspace manifest

| Field | Value |
| --- | --- |
| Status | `DEFERRED` |
| Dependencies | Hardened single-repository release accepted |
| Target release | Multi-repository release |

Deliverables:

- multiple project references;
- exact baseline/landing SHAs per repository;
- cross-repository gate list;
- manifest digest;
- combined review/acceptance record.

Acceptance:

- every repository SHA exists and is verified;
- no branch-name-only references;
- manifest changes invalidate acceptance;
- single-repository behavior remains unchanged.

### WP-610 — Cross-repository prepare and landing

| Field | Value |
| --- | --- |
| Status | `DEFERRED` |
| Dependencies | `WP-600` |
| Target release | Multi-repository release |

Deliverables:

- prepare candidate landing objects in every repository;
- exact-SHA workspace checkout;
- cross-repository gates;
- ordered expected-old promotions;
- partial-landing transaction record;
- completion/compensation procedures.

Acceptance:

- the tool never claims Git-level atomicity;
- partial promotion is detected and blocks release;
- documented completion or compensation is required;
- final manifest names actual landed SHAs.

### WP-700 — Runtime-resource adapters

| Field | Value |
| --- | --- |
| Status | `DEFERRED` |
| Dependencies | Hardened single-repository release accepted |
| Target release | Optional isolation release |

Potential adapters:

- port ranges;
- temporary directories;
- database names;
- Docker Compose project names;
- cache namespaces;
- language virtual environments.

Adapters MUST remain project-profile extensions. They cannot enter the generic
engine until two real projects demonstrate the same requirement.

### WP-710 — Constrained gate executor

| Field | Value |
| --- | --- |
| Status | `DEFERRED` |
| Dependencies | `WP-310`, explicit threat-model approval |
| Target release | Optional isolation release |

Potential capabilities:

- network denial;
- filesystem write scopes;
- secret isolation;
- process limits;
- container or OS-user execution.

Configuration declarations alone MUST NOT be described as enforced isolation.

## 18. Dependency and execution order

The required order is:

```text
WP-000
  ↓
SPIKE-001
  ↓
WP-100
  ├─→ WP-110 → WP-120 ───────────────┐
  └─→ WP-130 ────────────────────────┤
                                      ↓
                         WP-200 → WP-210 → WP-220
                                      │        │
                             WP-300   │        ↓
                                ↓     │      WP-230
                              WP-310  │        ↓
                                │     └────→ WP-240
                                └──────────→ WP-250
                                               ↓
                                            WP-320
                                               ↓
WP-120 + WP-130 → WP-400              WP-410
        │                                 ↓
        └────────────────────────────→ WP-420
                                          ├─→ WP-440
                                          └─→ WP-430
                                                │
WP-400 ─────────────────────────────────────────┤
WP-440 ─────────────────────────────────────────┤
                                                ↓
                                             WP-450
                                                ↓
                                             WP-460
```

Permitted parallel work:

- No production work package may begin until `SPIKE-001` is `DONE` with all
  required hypotheses passing.
- `WP-110` and `WP-130` may run after `WP-100`.
- `WP-300` may begin after the card model fields are stable in `WP-210`.
- `WP-400` may run after `WP-120` and `WP-130`.

No other parallel implementation is authorized until the dependency graph is
updated here.

### 18.1 Mandatory dogfooding thresholds

Dogfooding is staged because the tool cannot safely manage lifecycle stages it
has not implemented.

#### Threshold A — Worktree allocation

Trigger: `WP-230` becomes `DONE`. **Reached 2026-07-28.**

Requirement:

- every subsequently started Change Harness work package uses `work start` to
  create its implementation branch and worktree;
- manual recovery is allowed only when the harness operation fails and the
  failure is recorded as a regression;
- card, review, integration, and landing remain manual until their thresholds.

#### Threshold B — Cards, handoffs, and reviews

Trigger: `WP-320` becomes `DONE`. **Reached 2026-07-28.**

Requirement:

- every subsequently started Change Harness work package has an authoritative
  harness card;
- its implementation uses harness gate receipts and handoff;
- review occurs in a fresh agent context and is recorded through the harness;
- integration and promotion may remain manual but must preserve exact reviewed
  SHAs.

#### Threshold C — Complete self-hosting

Status: `DONE`. `SELFHOST-001` ran the full lifecycle against this repository on
2026-07-28 and promoted `c51f2dc` to `main`. Two limitations are recorded rather
than waived: the member-card review used a distinct declared actor but not a
genuinely fresh context, and the three defects the run exposed were fixed by
ordinary commits, since self-hosting becomes mandatory only after the run
passes. Both are stated in `docs/SELFHOST-001.md`.

Trigger: `WP-460` becomes `DONE`.

Requirement:

- execute `SELFHOST-001`, a bounded documentation-only Change Harness card,
  through card activation, worktree allocation, gate receipt, handoff,
  fresh-context review, integration, acceptance, authority promotion, archive,
  and cleanup;
- no manual Git mutation may substitute for a harness command;
- any failed or bypassed lifecycle stage blocks the Single-repository MVP
  release gate;
- after `SELFHOST-001` passes, all Change Harness feature work uses the complete
  harness lifecycle.

## 19. Release acceptance gates

### 19.1 Foundation gate

Status: `DONE`.

- Repository initialized on `main`
- Pinned Rust toolchain
- Read-only doctor command
- Three CLI tests
- Format, test, and strict Clippy pass
- Architecture and agent instructions committed

### 19.2 Walking-skeleton gate

Status: `DONE`. Accepted 2026-07-28 by Alvaro Alvarez.

| Criterion | Status |
| --- | --- |
| `SPIKE-001` is `DONE` | ✅ |
| `H-01` through `H-07` are `PASS` | ✅ all seven, evidence in the spike report |
| `docs/spikes/SPIKE-001-REPORT.md` is accepted | ✅ 2026-07-28, Alvaro Alvarez |
| Prototype head preserved under `refs/archive/spikes/SPIKE-001` | ✅ `1bb3fc8` |
| No prototype implementation present on `main` | ✅ verified with `git ls-tree` |
| Sections 9–15 and the dependency sequence reflect observed evidence | ✅ plan revision 4 |
| `WP-100` is explicitly changed to `READY` | ✅ plan revision 5 |

Acceptance evidence: `cargo fmt --check`, `cargo test` with three passing
integration tests, and strict Clippy all passed from a clean worktree; the
archive ref resolved to the recorded prototype head; `main` contained no
prototype path.

### 19.3 Single-repository MVP gate

Status: `IN_PROGRESS`. Eleven of the twelve criteria are met. Only the
acceptance owner's signature on the release record remains.

All must be true:

- `WP-100`, `WP-110`, `WP-120`, `WP-130`, `WP-200`, `WP-210`,
  `WP-220`, `WP-230`, `WP-240`, `WP-250`, `WP-300`, `WP-310`,
  `WP-320`, `WP-400`, `WP-410`, `WP-420`, `WP-430`, `WP-440`,
  `WP-450`, and `WP-460` are `DONE`;
- all 40 mandatory scenarios pass;
- no unclassified failure path mutates authority;
- exact-SHA review and acceptance invalidation is demonstrated;
- one temporary project completes the full lifecycle twice;
- second lifecycle proves stale-main rejection;
- recovery-required promotion state is demonstrated and recovered;
- audit evidence identifies the exact authority transition;
- no critical or high open defect remains;
- README documents installation and operator workflow;
- `SELFHOST-001` completes every lifecycle stage without manual Git mutation;
- acceptance owner signs the release record.

Progress against each criterion:

| Criterion | Status | Evidence or what remains |
| --- | --- | --- |
| All twenty work packages `DONE` | ✅ | This document's per-package entries |
| All 40 mandatory scenarios pass | ✅ | Section 16.2 carries a per-scenario trace to the test that exercises it |
| No unclassified failure path mutates authority | ✅ | `no_failure_path_in_the_lifecycle_leaves_the_authority_moved`, which also asserts exit 1 is never produced |
| Exact-SHA review and acceptance invalidation demonstrated | ✅ | `an_invalidated_approval_is_reported_as_stale_rather_than_absent`, `an_acceptance_binds_the_exact_landing_commit_and_its_evidence` |
| One temporary project completes the full lifecycle twice | ✅ | `one_project_completes_the_full_lifecycle_twice` |
| Second lifecycle proves stale-main rejection | ✅ | `the_second_cycle_rejects_a_plan_built_against_a_stale_main` |
| Recovery-required promotion state demonstrated and recovered | ✅ | `a_local_sync_failure_after_promotion_requires_recovery_and_does_not_rewind` reaches the state; `a_recovery_required_promotion_can_be_resumed_to_completion` recovers it via `project recover --resume`. Scope decision recorded as D-051 |
| Audit evidence identifies the exact authority transition | ✅ | `audit_evidence_identifies_the_exact_authority_transition` |
| No critical or high open defect | ✅ | No open defect is recorded; every defect found during implementation was fixed in the package that exposed it |
| README documents installation and operator workflow | ✅ | `README.md`: installation, the three-repository model, the eleven-step operator workflow, recovery, and the exit-code table |
| `SELFHOST-001` completes without manual Git mutation | ✅ | Completed on the third attempt; landing commit `c51f2dc` on `main`, archive refs `refs/archive/cards/F-001` and `refs/archive/integrations/INT-001`, integration `archived`, card `closed`. The first two attempts exposed three real defects (D-052, D-053, D-054), which is recorded in `docs/SELFHOST-001.md` rather than smoothed over |
| Acceptance owner signs the release record | ⏳ | Alvaro Alvarez. Every other criterion is now met |

The recovery criterion was the one structural problem: Section 19.3 required a
recovery that `WP-500` owned, and `WP-500` is scoped to the hardened release.
The acceptance owner chose to pull that one recovery path into the MVP rather
than narrow the criterion (D-051), so the gate now reads as written.

### 19.4 Hardened single-repository gate

Status: `IN_PROGRESS`. Six of the seven criteria are met. The one outstanding
needs a second repository.

All must be true:

- `WP-500`, `WP-510`, `WP-520`, `WP-530`, and `WP-540` are `DONE`;
- every mutation boundary has failure-injection coverage;
- backup and restore drill passes;
- generated-artifact governance is demonstrated;
- concurrency tests pass repeatedly;
- one ARTANA profile trial completes without changing the generic engine;
- no critical/high risk remains unmitigated.

Progress against each criterion:

| Criterion | Status | Evidence or what remains |
| --- | --- | --- |
| `WP-500` through `WP-540` are `DONE` | ✅ | This document's per-package entries; all five landed through the harness itself |
| Every mutation boundary has failure-injection coverage | ✅ | `INJECT_FAILURE_VAR` reaches every boundary any command names, and `every_mutating_command_names_at_least_one_boundary` asserts against the source that no command module opens a transaction without naming one |
| Backup and restore drill passes | ✅ | `a_restore_drill_reconstructs_authority_and_control_from_the_backup_alone` deletes both source repositories before restoring, so it cannot pass by reading them |
| Generated-artifact governance is demonstrated | ⚠️ | Classification and ownership are enforced and tested (`tests/artifacts.rs`). Integration-owned generation and deterministic regeneration checking are **not built**, so a shared artifact is classified but cannot land. Whether the criterion means what shipped or the full deliverable list is the acceptance owner's call, recorded here rather than resolved unilaterally |
| Concurrency tests pass repeatedly | ✅ | `many_threads_contending_for_one_lock_produce_exactly_one_winner`, `many_processes_mutating_one_project_produce_one_commit_each_round`, and `a_losing_contender_never_leaves_the_lock_behind` each run 40 rounds of 8 contenders, across threads and across processes. The test that previously claimed this made two sequential calls on one thread and contended for nothing; writing the real one found a genuine race (D-060) |
| One ARTANA profile trial | ⏳ | Not run, and not runnable from this repository — it needs an ARTANA checkout. D-001 keeps the engine independent, so the trial is the check that the independence holds in practice |
| No critical or high risk unmitigated | ✅ | No open defect is recorded; each found during implementation was fixed in the package that exposed it, and the two high-severity review findings this session (`WP-520`'s bundle verification, `WP-510`'s locale comparison) were resolved before their cards landed |

One outstanding criterion remains, and it is an honest gap rather than an
oversight: the ARTANA trial needs a second repository, and it is the criterion
that actually tests D-001 — a profile that required changing the engine would
mean the independence was nominal.

| D-060 | Write the lock file to a scratch path and link it into place | Accepted | `create_new` makes acquisition exclusive, but it also makes the lock file *visible* before its contents are written. A contender reading it in that window sees an unparseable holder and is told the lock's disposition cannot be established — sending an operator to check whether a command is running while one plainly is, which is the worst possible advice at that moment. Writing the contents to a scratch file and `hard_link`ing it in keeps the exclusion, since the link fails when the destination exists, and makes the file complete the instant it appears. The scratch name is unique per attempt rather than per process, because threads within one process share a pid and would delete each other's scratch file. Both defects were found by writing a contention test that actually contends. |

| D-061 | Rename a test whose assertions were narrowed without narrowing its name | Accepted | D-052 correctly stopped `probe_identifies_this_path_as_a_linked_worktree_or_the_main_one` from asserting anything environment-dependent, leaving a single claim: the path is a non-bare repository. The name was not narrowed with it, so the test went on advertising an identification it no longer performed. Renaming costs nothing and removes a false signal from the one place a reader looks first when deciding whether a behaviour is covered. |

#### On tests that claim more than they check

Five instances of one defect surfaced during implementation: two tests pinning
values that depend on where the suite runs, one asserting an actor and a reason
but never the head its name promised, one claiming concurrency while making two
sequential calls on a single thread, and one left advertising a check that a
correct narrowing had removed.

A systematic sweep for the pattern was attempted and largely failed, which is
worth recording. Comparing a test's name against the words in its body flags
almost every test, because English verbs like "refuses" and "reports" do not
appear literally in assertion code. Filtering to tests whose only assertions are
pass or fail flags a hundred and seventeen, nearly all of them legitimately
boolean. The sweep found exactly one real instance — the one recorded above,
and the one this document's own author had introduced.

The four that mattered were found by *running* something: pointing the harness
at itself, mutation-testing a review's own claim, and writing a contention test
that actually contended. The pattern is a mismatch between a claim and an
absent assertion, and no analysis of names detects an assertion that was never
written. The practice that works is the one already recorded against `WP-500`
and `WP-510`: check a claim before writing it down.

| D-062 | Let `CHANGE_HARNESS_CONTROL` supply `--control`, except for `project init` | Accepted | Twenty-one commands require an absolute control path, and eleven self-hosted releases typed it several hundred times. Each repetition is an opportunity to point a command at the wrong project, which no amount of downstream checking catches — the command would be operating correctly on the wrong records. `project init` is excluded on purpose: that flag decides where a control repository is *created*, and inheriting it from a variable exported for a different project is how someone initializes into the wrong place. The missing-argument error names only the flag, because clap does not allow a required argument's error to be customised; `--help` names both, and the acceptance criterion was narrowed to match what is deliverable rather than left standing as unmet. |

### 19.5 Multi-repository gate

Status: `DEFERRED`.

All must be true:

- hardened single-repository release accepted;
- `WP-600` and `WP-610` are `DONE`;
- exact-SHA manifest demonstrated with at least two repositories;
- partial landing is detected and recovered;
- cross-repository test evidence is retained;
- documentation explicitly avoids atomicity claims.

## 20. Current status tracker

### 20.1 Summary

| Area | Status | Evidence | Next action |
| --- | --- | --- | --- |
| Repository foundation | `DONE` | Commit `4729d18` | Preserve |
| Walking-skeleton validation | `DONE` | `SPIKE-001` report accepted 2026-07-28, seven passing hypotheses, archive ref `1bb3fc8` | Preserve |
| Rust toolchain | `DONE` | `rust-toolchain.toml`, Cargo build | Preserve |
| CLI shell | `DONE` | `--help`, `doctor` | Extend in `WP-100` |
| Read-only Git probe | `DONE` | `src/git/`, 21 fixture tests | Preserve |
| Stable command envelope | `DONE` | `WP-100`, 59 passing tests | Extend as commands are added |
| Project configuration | `DONE` | `WP-110`, 28 tests | Preserve |
| Control repository | `DONE` | `WP-120`, 30 tests | Preserve |
| Full Git inspection | `DONE` | `WP-130`, 21 fixture tests | Preserve |
| Cycles | `DONE` | `WP-200`, 31 tests | Preserve |
| Cards | `DONE` | `WP-210`, 36 tests | Preserve |
| Ownership and overlap | `DONE` | `WP-220`, 44 tests | Preserve |
| Worktree allocation | `DONE` | `WP-230`, 35 tests | Preserve |
| Candidate verification | `DONE` | `WP-240`, 32 tests | Preserve |
| Gate registry | `DONE` | `WP-300`, 31 tests | Preserve |
| Gate runner and receipts | `DONE` | `WP-310`, 37 tests | Preserve |
| Handoff | `DONE` | `WP-250`, 24 tests | Preserve |
| Independent review | `DONE` | `WP-320`, 27 tests | Preserve |
| Bare authority | `DONE` | Established, health-checked, and covered | `WP-400` |





| Integration | `DONE` | Plan through archive and close, all covered | — |
| Acceptance/promotion | `NOT_STARTED` | None | `WP-450` |
| Archive/cleanup | `NOT_STARTED` | None | `WP-460` |
| Recovery/concurrency | `NOT_STARTED` | None | `WP-500`, `WP-510` |
| Backup/audit | `NOT_STARTED` | None | `WP-520`, `WP-530` |
| Multi-repository | `DEFERRED` | Architecture only | After hardened release |
| Runtime isolation | `DEFERRED` | Architecture only | After demonstrated need |

### 20.2 Active work

| Field | Current value |
| --- | --- |
| Active work package | None |
| Active implementation branch | None |
| Active implementation worktree | None |
| Active owner | None |
| Active blocker | None |
| Next work package | `SELFHOST-001` |
| Next branch name | `selfhost/SELFHOST-001` |
| Next acceptance evidence | `WP-100` deliverables, unit tests covering every exit-code category, committed JSON round-trip fixtures, and unchanged `doctor` behavior |

The spike-derived corrections are assigned to their owning packages and are not
`WP-100` scope: F-1 to `WP-250`, F-2 to `WP-250`, F-3 to `WP-410`, and F-4 and
F-5 to `WP-320`.

### 20.3 Current demonstrated behavior

The current binary supports:

```bash
change-harness doctor --workspace <path> --format text
change-harness doctor --workspace <path> --format json
change-harness --help
change-harness --version
```

The current `doctor`:

- rejects a missing workspace;
- reports the installed Git version;
- reports the containing Git repository when one exists;
- performs no mutation.

Known limitation:

- any unsuccessful repository `rev-parse`, including Git refusals such as
  `safe.directory`, is currently reported as no repository detected because
  stderr is discarded. `WP-130` owns the typed diagnostic correction and
  regression coverage.

The current binary does not:

- create configuration;
- create a control or authority repository;
- create cycles, cards, leases, branches, or worktrees;
- run project gates;
- create handoffs, reviews, integrations, or acceptances;
- update any Git ref;
- clean up any worktree.

### 20.4 Current test inventory

59 tests pass: 47 unit and 12 CLI integration.

| Area | Tests | Covers |
| --- | --- | --- |
| `domain::ids` | 6 | Documented shapes, wrong prefix, short and non-numeric suffixes, length bound, project slug rejection, JSON round-trip |
| `domain::digest` | 8 | Committed SHA-256 vectors, canonical key ordering, field-order immateriality, material-change sensitivity, parse rejection, round-trip |
| `domain::clock` | 6 | Epoch and known-instant rendering, fixed-clock determinism, RFC 3339 round-trip, malformed-input rejection |
| `cli::exit` | 4 | Every category's documented number, uniqueness, reserved exit 1, name shape |
| `cli::output` | 8 | Every envelope key present, null rather than omitted, warnings placement, text purity, both envelopes round-trip |
| `cli` | 4 | Output-option resolution and the both-options usage error |
| `error` | 6 | Category prefixes, code uniqueness, recovery guidance, no success-category code, per-variant code mapping |
| `commands::doctor` | 5 | Frozen legacy payload, envelope placement under `data`, identical text in both paths, round-trip |
| `tests/cli.rs` | 12 | Help surface, unimplemented commands absent, legacy payload and its warning, envelope output, both-options rejection, exit codes 2 and 4, JSON error envelope |

Last verified commands:

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

All passed. Documentation changes made after that evidence require the same
commands before commit even though Rust behavior is unchanged.

## 21. Risk register

| ID | Risk | Impact | Mitigation | Trigger/stop condition | Status |
| --- | --- | --- | --- | --- | --- |
| R-001 | Same OS account can bypass hooks and modify local state | False security claims | Treat hooks as advisory; exact-SHA verifier; document threat model | Any claim of malicious-actor prevention | Open, accepted for MVP |
| R-002 | Direct ref update desynchronizes checked-out main | Corrupted/confusing worktree state | Canonical bare authority; coherent local fast-forward | Any design using direct `update-ref` on checked-out branch | Mitigated by design; unimplemented |
| R-003 | Control repository is corrupted or rewritten | Loss of workflow authority | Git history, locks, backups, expected-HEAD transactions | Digest/history mismatch | Open |
| R-004 | Candidate tests/build scripts execute untrusted code | Credential or host compromise | No secrets in runner; constrained executor later | Gate requires production credentials | Open; production use blocked |
| R-005 | Path matcher mishandles rename, case, symlink, or Unicode | Scope escape | NUL-safe Git parsing and adversarial tests | Any unsupported path condition | Open |
| R-006 | CLI interruption leaves partial branch/worktree/ref mutation | Stale or ambiguous state | Operation journal and recovery | Incomplete operation detected | Open |
| R-007 | Cross-repository landing partially succeeds | Inconsistent workspace | Exact manifest and explicit completion/compensation | Any partial promotion | Deferred with feature |
| R-008 | Gate retries hide flakiness | False acceptance | Attempt receipts and declared retry policy | Undeclared rerun | Open |
| R-009 | Tool becomes ARTANA-specific | Reuse failure and coupling | Generic schemas/engine; ARTANA profile only | ARTANA name/command in core domain | Actively controlled |
| R-010 | Implementation expands into infrastructure platform before MVP | Schedule failure | Timeboxed walking skeleton, single vertical slice, and deferred capabilities | Work outside active package/dependencies or spike budget | Actively controlled |
| R-011 | Same-disk backup is mistaken for independent backup | Permanent loss | Independent destination validation and restore drill | Backup path shares device | Open |
| R-012 | Actor string is treated as proven identity | Invalid separation claim | Declare identity limits; stronger boundary optional | Security-sensitive approval needs proof | Open, accepted for local MVP |
| R-013 | Repository probe discards `rev-parse` failure diagnostics | Git refusal is misreported as non-repository | Preserve exit/stderr and classify repository versus Git error in `WP-130` | Any nonzero repository probe result | Closed 2026-07-28 by `WP-130`, with a named regression test |
| R-014 | Detailed specification hardens before real agent use | Expensive schemas encode the wrong ceremony | Complete `SPIKE-001` before `WP-100`; keep Sections 9–15 provisional | Production code begins before accepted spike evidence | Actively controlled |
| R-015 | Full-plan reading consumes feature-agent context | Reduced implementation focus and higher error rate | Role-specific reading contract and explicit per-item reading list | Feature agent is instructed to load the complete plan without a coordination role | Mitigated by plan revision 2 |
| R-016 | Review reuses implementation context | Nominally independent review repeats the implementer's assumptions | Fresh session, review packet only, different session ID, no branch edits | Reviewer inherits or forks implementation conversation | Mitigated procedurally; not a security boundary |
| R-017 | Candidate branch is rewritten between actor delivery and handoff creation | A structurally valid handoff describes code the feature actor never delivered, and review evaluates the wrong objects | Record the actor-declared `delivered_sha` and compare it at handoff creation; reject on mismatch | Any handoff whose candidate SHA differs from the declared delivered SHA | Closed 2026-07-28 by `WP-250`, with a named regression test reproducing the spike's amend |
| R-018 | Evidence records cannot be independently verified by their holder | An auditor can check records against each other but cannot recompute any digest, so internal consistency is mistaken for correctness | Records name their canonicalization algorithm and carry or reference the card; commit digest vectors before activation | Any record whose digest cannot be recomputed from the record plus Git objects | Open; observed in `SPIKE-001` finding F-2 |
| R-019 | Independent reviewers reach different verdicts on identical code | Approval depends on which reviewer is assigned rather than on the code | Comparability guidance in Section 15.3; required gate-adequacy output | Same construct flagged in one card and not another | Open; observed in `SPIKE-001` finding F-6 |

## 22. Decision register

| ID | Decision | Status | Rationale |
| --- | --- | --- | --- |
| D-001 | Maintain Change Harness in an independent repository | Accepted | Prevents ARTANA development from blocking or being blocked by harness work |
| D-002 | Keep the engine project- and language-neutral | Accepted | Git workflow mechanics are not language-specific |
| D-003 | Implement the CLI in Rust 1.95 | Accepted | Pinned local toolchain, typed models, safe process invocation, single binary |
| D-004 | Separate control authority from candidate worktrees | Accepted | Candidate actors cannot define their own acceptance policy |
| D-005 | Use a local bare repository as canonical protected-ref authority | Accepted | Avoids direct ref changes that desynchronize working trees |
| D-006 | Treat hooks as advisory only | Accepted | Hooks are bypassable and cannot serve as acceptance evidence |
| D-007 | Bind cards, receipts, reviews, and acceptance to exact digests/SHAs | Accepted | Branch names and narrative claims are mutable |
| D-008 | Cards reference named gates and cannot introduce commands | Accepted | Prevents cards from expanding executable authority |
| D-009 | Deliver one single-repository vertical slice before multi-repository work | Accepted | Proves value before infrastructure expansion |
| D-010 | Authoritative records use strict JSON; draft cards may use YAML | Accepted | Stable machine representation with human-friendly authoring |
| D-011 | Git history is the event integrity chain for MVP | Accepted | Avoids redundant custom hash chaining |
| D-012 | macOS arm64 is the first supported host | Accepted | Matches current development environment |
| D-013 | Same-user local operation is not a hard security boundary | Accepted | Honest threat model |
| D-014 | Licensing remains undecided while `publish = false` | Accepted temporarily | No public distribution has been authorized |
| D-015 | Run a disposable walking skeleton before stabilizing production contracts | Accepted | Tests agent usability, evidence invalidation, and bare-authority landing before infrastructure hardens assumptions |
| D-016 | Dogfood in three thresholds: allocation, review, then complete lifecycle | Accepted | Earlier full self-hosting is impossible because required commands do not yet exist |
| D-017 | Fresh reviewer context is the MVP operational definition of independent agent review | Accepted | Prevents inherited implementation conversation while honestly avoiding a hard identity claim |
| D-018 | Keep Section 7 invariants committed while treating Sections 9–15 contracts as provisional until the spike | Accepted | Preserves safety boundaries while allowing evidence-driven schema correction |
| D-019 | Exempt spike roles from the standing agent reading contract when the tracker names a packet as sole required reading | Accepted | The general contract in `AGENTS.md` forced the implementer to read Sections 1–7 and the spike entry itself, which would have revealed the seeded omission and invalidated `H-01` and `H-02` |
| D-020 | Assign MVP ownership of mandatory scenarios 34 and 35 to `WP-450` and `WP-230`, and scope `WP-500` to injected failure | Accepted | The Single-repository MVP gate required all 40 scenarios while scenario 35 was claimed only by `WP-500`, a hardened-release package, making the gate unsatisfiable as written |
| D-021 | Define the spike packet contract inside the `SPIKE-001` entry rather than Sections 9–15 | Accepted | `H-01` and `H-02` test packet sufficiency, so the contract must exist before the spike, but hardening a general packet schema before evidence is exactly the failure R-014 describes |
| D-022 | Bind the actor-declared delivered SHA to the handoff candidate SHA | Accepted, unimplemented | `SPIKE-001` demonstrated that a branch rewritten between delivery and handoff produces an internally consistent handoff describing code the actor never delivered. Closes a gap between two things the plan already treats as authoritative; requires no new trust model. |
| D-023 | Measure implementation-packet sufficiency as zero blocking clarifications with recorded assumptions, not zero clarification messages | Accepted | Both `SPIKE-001` implementers completed without blocking, chose documented defaults, and recorded genuine packet ambiguities. That behavior is desirable and the original metric penalized it. |
| D-024 | Make gate adequacy a required, recorded review output | Accepted, unimplemented | Both `SPIKE-001` reviewers mutation-tested the gates unprompted and proved in three of three cases that a green receipt was not evidence for the acceptance behavior it appeared to support |
| D-025 | Add per-finding disposition and re-review to the review contract | Accepted, unimplemented | Every `SPIKE-001` review round needed to mark findings resolved, accepted as residual risk, or unresolvable within the card's write scope. Section 15.1 describes only a first review. |
| D-026 | Accept the `SPIKE-001` report and begin production implementation at `WP-100` | Accepted 2026-07-28 by Alvaro Alvarez | All seven hypotheses passed, the acceptance commands passed from a clean worktree, the archived prototype head matched the recorded SHA, and no prototype code reached `main`. Sections 9–15 were corrected from observed evidence before acceptance rather than after. |
| D-027 | Keep `doctor --format json` on its pre-envelope payload rather than making it a strict alias for `--output json` | Accepted | Section 12.1 calls `--format` an alias while `WP-100` acceptance requires existing `doctor` behavior to remain compatible. Emitting the envelope under the old option would move every field under `data` and break existing callers, so the explicit compatibility requirement wins and the option is documented as a shim. Combining both options is a usage error rather than a silent precedence rule. |
| D-028 | Reserve exit code 1 rather than assigning it a category | Accepted | Section 12.2 assigns 0 and 2 through 10. Leaving 1 unused keeps an uncategorized process failure, such as a panic, distinguishable from every classified outcome. |
| D-030 | Accept card drafts in YAML or JSON via `serde_yaml_ng` | Accepted | D-010 permits YAML drafts. `serde_yaml` is archived, so a maintained fork is used. JSON is accepted for free because it is a YAML subset, which suits machine authors without a second code path. |
| D-037 | Make `work resume` perform the `changes_requested`/`blocked` to `active` transition | Accepted | Section 11.2 permits both transitions but Section 11.4 assigned neither to a command, so a card that received review feedback could never be handed off again. Resuming is the actor's own signal that they have picked the work back up, which makes it the honest trigger. `work resume` gains `--dry-run` accordingly. |
| D-035 | Terminate gate process groups by invoking `kill` rather than `libc::killpg` | Accepted | The crate sets `unsafe_code = "forbid"`, and `killpg` requires an unsafe block. Invoking `kill` with a negative process id is the same operation expressed through a process boundary, and keeps the crate free of unsafe code. |
| D-036 | Exclude gate logs from control history | Accepted | Section 14.3 gives logs retention windows rather than permanence, and a passing gate's output is large and uninteresting. Invariant 7.4.2 is satisfied by the receipt, which records each log's location and SHA-256 digest. |
| D-034 | Add `gate validate`, `register`, `list`, and `show` to the Section 12.3 command surface | Accepted | Section 12.3 lists only `gate run` and `gate status`, but gates must be registered before a card can name one, and D-008 makes registration a deliberate trusted act rather than a side effect of authoring a card. |
| D-032 | Add `work verify` to the Section 12.3 command surface | Accepted | `WP-240` produces a structured verification report, and without a command it would only be observable through `handoff create` in `WP-250`. A separate read-only command lets an actor check scope before attempting handoff. |
| D-033 | Treat a failed verification as a policy refusal rather than a successful report | Accepted | Returning exit 0 with `passed: false` would let a caller pipe the result onward and treat an out-of-scope candidate as ready. The verdict is the command's outcome, not its payload. |
| D-031 | Include `created_at` and `base_sha` in the card digest | Accepted | The digest identifies one exact record instance, not a class of equivalent cards. Two identical drafts activated at different times digest differently, which is the correct behavior when the digest is what reviews and receipts bind to. |
| D-038 | Allocate handoff identifiers monotonically rather than deriving them from the candidate SHA | Accepted | `WP-250` derived the identifier as `{card_id}-r{revision}-{sha[..12]}`, which is deterministic but unordered. "The latest handoff for this card" is a question the review path asks constantly, and answering it by sorting those names returns whichever candidate happened to hash higher. A second handoff at the same revision therefore resolved to the wrong record whenever the SHA prefixes sorted against issue order — latent until `WP-400` changed the SHAs. Identifiers are now `H-000001`, allocated like every other record type, so lexical order is issue order. |
| D-039 | Report authority health from `project status` rather than `doctor` | Accepted | `WP-400` requires an authority health check. `doctor` inspects a bare path with no project configuration and therefore cannot know which authority to check, while `project status` already opens the control repository and reads the configuration. The check reports rather than refuses: an unreachable or repointed authority is described in the payload, because a diagnostic that fails when its subject is broken is useless exactly when it is needed. |
| D-040 | Make the non-terminal integration record itself the cycle's integration lease | Accepted | `WP-410` requires one integration lease per cycle. A separate lease file would be a second record asserting the same fact, and the two could disagree after a partial failure. Section 11.3 already gives every integration a status, so "an integration exists in a non-terminal state" is the claim, and it cannot drift from itself. |
| D-041 | Judge integration readiness against the candidate branch head rather than the handoff's recorded SHA | Accepted | Comparing an approval to the handoff it was recorded against is a tautology: they always agree. A branch that gained a commit after approval would stay ready and integrate a commit no reviewer saw, which is `SPIKE-001` finding F-1 one stage later. `review record` already refuses a superseded handoff; selection now applies the same rule. |
| D-042 | Carry the preflight forward with unreachable `commit-tree` objects | Accepted | `merge-tree` produces a tree, but merging the next candidate needs a commit, so simulating a multi-card sequence needs intermediate commits. Writing unreachable objects changes no state a reader can observe — no ref moves, no index or worktree is touched, and `git gc` collects them — which keeps the preflight non-destructive in the sense that matters. The alternative, merging in a real worktree, would make the preflight as risky as the operation it is meant to de-risk. |
| D-043 | Refuse a second `integration merge` on an already-merged plan | Accepted | Merging twice builds a different head from the same plan and overwrites the recorded one, leaving anything `WP-430` or `WP-440` had already done pointing at a commit the record no longer names. Rebuilding requires abandoning the integration and preparing again, which is visible in the record rather than silent. |
| D-044 | Hold the landing commit with `refs/harness/landing/<INT-id>` | Accepted | Section 13.5 requires the landing commit to stay unreachable from the protected branch until accepted, which is not the same as unreachable outright. An object no ref points at can be garbage-collected, and losing the landing commit between construction and promotion would force a rebuild and a full re-verification. A harness ref keeps it alive without putting it anywhere a reader would mistake for promoted. |
| D-045 | Add `integration ready`, `preflight`, `merge`, and `land` to the Section 12.3 command surface | Accepted | Section 12.3 lists `prepare`, `verify`, `inspect`, `review`, and `promote`. `ready` is required by `WP-410` for `SPIKE-001` finding F-3; `preflight`, `merge`, and `land` are the separately observable steps `WP-420` and `WP-430` deliver, and Section 13.5 requires the landing commit to exist before final verification, which is impossible if landing is folded into `verify` or `promote`. |
| D-046 | Give receipts an integration scope, making `card_id` optional | Accepted, amends Section 10.6 | `WP-440` reruns gates against the landing commit, which belongs to every member card and to none of them individually. The original schema required `card_id`, leaving two options: attribute a combined run to an arbitrary member, or run each gate once per card. The first is a false claim about what was checked; the second multiplies a long test suite by the batch size while adding no information, since the gate does not know which card asked for it. Naming the real subject is the honest third option. |
| D-047 | Refuse an integration review that leaves a declared cycle invariant unconfirmed | Accepted | Section 10.2 lets a cycle declare release invariants in free text, which no gate can evaluate. If the review can pass without addressing them, they are decorative. Requiring each to be named explicitly is the only mechanism available for a condition a machine cannot check. |
| D-048 | Journal a `recovery-required` error as a partial failure regardless of control-tree cleanliness | Accepted | `with_transaction` inferred "nothing was written" from a clean control worktree. That inference holds for every command that only mutates control state, but `integration promote` moves the authority branch — a repository the check cannot see. An authority-promoted, local-sync-failed run leaves control clean, so it would have been recorded `failed_clean` and `project recover` would have stayed silent about precisely the state Section 13.6 requires it to surface. An error that declares itself recovery-required is now taken at its word. |
| D-049 | Verify worktree cleanliness before unlocking, rather than relying on the lock to refuse | Accepted | `WP-130` locks card worktrees so `git worktree prune` cannot reclaim them, which meant every cleanup removal failed on the lock instead of on the real question. Unlocking first and letting `git worktree remove` decide would leave a worktree holding uncommitted work unlocked on the failure path — protection removed at exactly the moment it was needed. Cleanup now establishes cleanliness itself, then unlocks, then removes without forcing. |
| D-050 | Check authority freshness at `integration merge`, not only at `land` | Accepted | A plan built against a superseded protected branch is refused at landing and again at promotion, so merging one is not unsafe. It is, however, wasted work that produces an integration head missing whatever landed in the meantime — exactly the kind of object someone inspects and misreads. Every other stage in the harness refuses as early as it can detect a problem; this one now does too. |
| D-051 | Pull promotion recovery into the MVP rather than narrowing the Section 19.3 criterion | Accepted 2026-07-28 by Alvaro Alvarez | The gate requires the recovery-required promotion state to be "demonstrated and recovered", and recovery sat in `WP-500`, a hardened-release package. Narrowing the criterion to "reached and does not rewind" was the alternative. Recovering was chosen because the state is the one place a command can die having already changed something outside the control repository, so leaving it operator-only is the weakest point in the whole lifecycle. `project recover --resume` re-derives what happened from the authority branch rather than from a journal marker, and shares the settlement code with `integration promote`, so a resumed promotion cannot record something subtly different from an uninterrupted one. |
| D-052 | Stop asserting `detached_head` in the repository-probe test | Accepted | The test asserted the source checkout is on a branch, which is a claim about the environment rather than about the code. The harness runs its own integration gates in a detached worktree by design (`WP-420`), so the suite failed the first time the harness was pointed at itself. The comment beside it already applied the right reasoning to `linked_worktree` and then did the opposite for `detached_head`. Found by `SELFHOST-001`, and unreachable by any temporary-project test. |
| D-053 | Add `integration abandon` | Accepted | Section 11.3 permits `abandoned` from every pre-promoted state and `holds_lease` treats it as terminal, but no command could reach it. An integration that failed verification therefore held its cycle's integration lease permanently, with no way to plan another. This is the third instance of the same pattern — a state the model defines and no command reaches — after `WP-120`'s event store and `WP-200`'s atomic groups. Member cards return to `approved` rather than to work, because their approvals remained valid: the combination failed, not the candidates. |
| D-054 | Stop pinning `workspace_role` in the `doctor` CLI test | Accepted | The same defect as D-052 in a second place: the test asserted the role was "main worktree" or "linked worktree", which fails in a detached worktree — where the harness runs its own integration gates. Fixing D-052 alone was not enough, and the second `SELFHOST-001` attempt failed on this one. Both are now stated as "any non-bare role is admissible", and the suite is verified green from an actual detached worktree rather than by inspection. |
| D-029 | Exclude the operation journal from control history | Accepted | A journal entry describes a mutation in flight, so committing it would place non-authoritative state into authoritative history and leave control permanently dirty. Recovery reads the journal from the working tree precisely because a crashed process leaves it there uncommitted. `WP-530` revisits whether the audit report needs operations in history. |

## 23. Decisions required later

These are intentionally time-bounded and do not block `SPIKE-001`.

| ID | Decision needed | Deadline | Blocking effect |
| --- | --- | --- | --- |
| Q-001 | Open-source or proprietary license | Before any public remote, package publication, or external distribution | Blocks public distribution only |
| Q-002 | Minimum supported macOS version | Before hardened release | Blocks compatibility claim |
| Q-003 | Linux support level | Before multi-project external trial | Blocks Linux support claim |
| Q-004 | Cryptographic or OS-backed actor identity | Before security-sensitive multi-user use | Blocks hard authorization claim |
| Q-005 | Sandboxed gate executor technology | Before gates may access sensitive repositories or credentials | Blocks sensitive/production gate use |
| Q-006 | Long-term artifact storage backend | Before one-year landing-log retention is operational | Blocks hardened retention acceptance |

## 24. Definition of done for every work package

Spikes use their explicitly listed hypothesis and disposition gates. They do
not become production implementation by satisfying this section.

A work package is `DONE` only when:

1. its listed deliverables exist;
2. all listed acceptance criteria pass;
3. relevant negative and regression tests exist;
4. `cargo fmt --check` passes;
5. `cargo test` passes;
6. strict Clippy passes;
7. public CLI/schema/documentation changes are documented;
8. no unrelated cleanup is mixed into the package;
9. the branch head is committed;
10. the status and evidence fields in this document are updated;
11. discovered risks and decisions are recorded;
12. the final diff is reviewed against package scope.

## 25. Plan maintenance procedure

When starting a work package or spike:

1. verify dependencies are `DONE`;
2. change package status from `READY` or `NOT_STARTED` to `IN_PROGRESS`;
3. record owner, branch, worktree, and start date;
4. record the exact `Required reading` headings for the assigned role;
5. add or confirm exact acceptance commands;
6. commit the tracker update before implementation diverges materially.

When completing a work package or spike:

1. run all acceptance commands from a clean worktree;
2. record exact command results and relevant artifact paths;
3. record the final implementation commit;
4. update risks and decisions;
5. change status to `DONE`;
6. mark newly unblocked packages `READY`;
7. commit the status update.

When blocked:

1. change status to `BLOCKED`;
2. record the exact condition, evidence, and decision needed;
3. preserve the branch and worktree;
4. do not start dependent packages;
5. resume only after recording the resolution.

When the plan changes:

1. increment the plan revision;
2. record the reason in the decision register;
3. update dependencies, acceptance, status, and release gates together;
4. do not silently reinterpret an existing work package.
