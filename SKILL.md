---
name: change-harness
version: change-harness-skill/v1
description: Drive a bounded change through Change Harness: card, allocated worktree, gates, exact-SHA handoff, independent review, integration, and promotion. Use this guide in any repository governed by Change Harness.
---

# Change Harness operating guide

Change Harness coordinates bounded changes across three repositories:

- **candidate:** the ordinary repository where work happens;
- **control:** the separate Git repository holding cards, reviews, receipts,
  and integration records; and
- **authority:** the bare repository that owns the protected branch and moves
  only on promotion.

Every factual claim binds to an exact commit SHA. This guide is tool-neutral:
it applies equally to people and agents using Codex, Claude, Cursor, or any
other client.

## What this guide is, and is not

This file contains stable operating rules and command shapes. It is **not live
authority** for current assignments, permissions, dependencies, locks,
receipts, card state, or integration readiness. Before a controlled action,
read fresh Harness state with the relevant status command; never act from an
old conversation, a copied response, or this file alone.

Use `--output json` when another program reads a result. Success returns a
`harness.command-result/v1` envelope; failures return
`harness.command-error/v1` with `error.code`, `error.recovery`, and details.

## Ground rules

1. Declare every source, test, and documentation file in the card's
   `write_scope`. Run `work verify` before handoff.
2. `delivered_sha` is the exact `git rev-parse HEAD` at handoff time. Never
   reconstruct it from memory or name a branch instead.
3. Implementer and reviewer are different declared actors in separate, fresh
   tasks. Identity is declared, not proven; preserve the separation anyway.
4. Treat a refusal as invariant information. Read its code and recovery, fix
   the state, then retry. Never bypass a refusal with raw Git.
5. Never hand-edit the control repository, authority, protected branch, or a
   Harness lock. Never force-push or reset state managed by the Harness.
6. A green test is evidence only if it can detect the relevant wrong behavior.
   Make a narrow mutation and confirm the test fails at the intended oracle.
7. A claim about state not freshly read is not a fact. Read the exact tree and
   record before reporting or changing it.

When scope, ownership, risk, acceptance criteria, or product intent is
ambiguous, stop and escalate. Use `work block` when work cannot continue.

## Orient first

On a new machine, run:

```bash
export CHANGE_HARNESS_CONTROL=/path/to/control
change-harness doctor --workspace .
```

At the start of a task, obtain fresh state:

```bash
change-harness project status
change-harness cycle status --cycle-id C-001
change-harness integration ready --cycle-id C-001
change-harness card status --card-id F-001
```

`integration ready` answers what is approved and integrable, and why other
activated cards are not. It is cycle-local: it does not prove that another
cycle is absent. Declare `exclusive_resources` for shared ground and ask
before assuming no other work is active.

## Roles and limitations

| Role | Owns | Declares itself with |
| --- | --- | --- |
| Coordinator | cycles, cards, registered gates, integration | `--actor` / `--actor-id` |
| Implementer | one allocated card and its gates/handoff | `--actor` |
| Reviewer | a candidate it did not implement | `reviewer_actor_id` |
| Acceptance owner | authorization to move a verified landing | `--acceptance-owner` |

Commands being available are not authority to run them. The Harness refuses
self-review, self-integration review, self-acceptance, and self-promotion
where the same declared actor would bless its own implementation. It cannot
prove that a declared actor is a distinct person or session.

Actor flags are intentionally inconsistent: `work`, `gate run`, `handoff`,
and card review use `--actor`; integration commands use `--actor-id`;
integration review uses `--reviewer-actor-id`; acceptance uses
`--acceptance-owner`.

## Exit codes and refusals

| Code | Meaning |
| --- | --- |
| 0 | success |
| 2 | usage error |
| 3 | configuration error |
| 4 | precondition not met |
| 5 | policy refusal |
| 6 | concurrent-state conflict |
| 7 | gate failed |
| 8 | external tool failure |
| 9 | recovery required: run `project status`, then `project recover` |
| 10 | internal error |

Every mutating command has `--dry-run` except `project recover --resume`.
Dry runs usually perform the same checks without mutation. Do not assume a
successful handoff dry run proves receipt evidence: use the real command's
structured result for authority.

## Lifecycle

### 1. Register reviewed gates

Cards can name only registered gates. Register an explicit command definition
and inspect it before use:

```bash
change-harness gate register --definition gate.test.json
change-harness gate list
```

Gates are read-only checks. Do not put deployments, notifications, publishing,
or other external side effects in a gate: they may be retried or rerun on the
landing commit. A declared `network_policy` is not proof that network access
is enforced.

### 2. Create and activate a cycle

```bash
change-harness cycle create --cycle-id C-001 --objective "One-line objective"
change-harness cycle activate --cycle-id C-001
```

Activation freezes the authority baseline. Use that exact SHA as the card's
base unless a fresh, explicit dependency workflow says otherwise.

When the cycle's card set is complete, freeze that membership before preparing
an integration:

```bash
change-harness cycle seal --cycle-id C-001
```

Sealing prevents new card activation in the cycle but does not stop existing
cards from being worked, handed off, or reviewed. It is not authorization to
integrate or promote.

### 3. Write a bounded card

A card is a contract. It needs one observable outcome, a narrow scope, named
gates, acceptance behavior, independent review policy, and rollback plan.

```yaml
card_id: F-001
cycle_id: C-001
title: One-line title
goal: What will be true when this lands.
non_goals: []
risk: low
change_kind: feature
base_sha: <exact 40-hex cycle baseline>
depends_on: []
write_scope:
  include: ["src/thing.rs", "tests/thing.rs", "docs/CHANGES.md"]
  exclude: []
exclusive_resources: []
named_gates:
  feature: [gate.fmt]
  review: []
  integration: [gate.test]
acceptance:
  behaviors: ["Observable behavior a test can fail."]
  regressions: ["Behavior that must keep working."]
review_policy: independent
rollback_strategy: revert the landing commit on the protected branch
```

```bash
change-harness card create --draft F-001.yaml
change-harness card activate --card-id F-001
```

If one card has multiple independent outcomes, unrelated risk surfaces, or no
rapid narrow proof, return it for slicing rather than starting it. Revising a
card supersedes its immutable revision and invalidates previous approval.

### 4. Work only in the allocated worktree

```bash
change-harness work start --card-id F-001 --actor implementer-a
```

Work in the path returned by the command, not in the primary checkout. Commit
all candidate changes. A dirty or untracked worktree blocks handoff by design.

### 5. Prove the narrow feature claim first

```bash
change-harness gate run --card-id F-001 --gate-id gate.fmt --actor implementer-a
```

Each receipt binds to the exact commit and records cleanliness. For a material
claim, state the invariant, precondition, visible assertion, and a small
mutation that should make the focused test fail. Run expensive broader suites
only after the narrow proof passes, at the schedule the Harness authorizes.

### 6. Verify and hand off

```bash
change-harness work verify --card-id F-001
```

Then submit a declaration bound to the exact commit:

```yaml
delivered_sha: <git rev-parse HEAD in the allocated worktree>
behavior_delivered: What the candidate actually does.
implementation_decisions: []
assumptions: []
known_limitations: []
residual_risks: []
rollback_notes: revert the landing commit on the protected branch
```

```bash
change-harness handoff create --card-id F-001 --declaration decl.yaml --actor implementer-a
```

The handoff must honestly state scope, invariant, focused proof, mutation,
exact evidence, known limits, residual risk, and checks not run.

### 7. Independent review in a fresh task

The reviewer receives the activated card, handoff, exact delivered and base
SHAs, complete diff, relevant contract changes, gate receipts, and reproducible
evidence. They do not receive the implementer's private reasoning or an
instruction to approve.

```bash
change-harness review begin --card-id F-001 --actor reviewer-b
change-harness review record --card-id F-001 --verdict verdict.yaml --actor reviewer-b
```

The reviewer should try to break the claim, including the declared mutation.
An approval cannot include an open finding. A candidate or card revision makes
approval stale; re-handoff and fresh review are required.

### 8. Integrate the exact approved candidates

```bash
change-harness integration ready --cycle-id C-001
change-harness integration prepare --cycle-id C-001 --actor-id coordinator
change-harness integration preflight --integration-id INT-001
change-harness integration merge --integration-id INT-001 --actor-id coordinator
change-harness integration land --integration-id INT-001 --actor-id coordinator
change-harness integration verify --integration-id INT-001 --actor-id verifier
change-harness integration review --integration-id INT-001 --reviewer-actor-id int-reviewer
```

Read `warnings` from `prepare`: omitted cards are named with a reason. Landing
verification reruns the union of member gates against the combined landing
SHA, not merely the isolated candidates.

### 9. Accept, promote, and archive

```bash
change-harness acceptance record --integration-id INT-001 --acceptance-owner "Name"
change-harness integration promote --integration-id INT-001 --actor-id promoter
change-harness archive create --integration-id INT-001
change-harness archive verify --integration-id INT-001
change-harness archive close --integration-id INT-001
```

Acceptance is bound to one landing SHA. Promotion uses compare-and-swap.
Archive before close so the landed work remains reachable.

## Convergence budgets and escalation

A convergence policy is optional. Once a project configures one, every
review return, repair attempt, gate failure, and material scope revision on
a card — and every integration failure on its cycle — becomes a counted
attempt against a budget. Running out is called escalation, and escalation
is designed to be answered, not worked around.

### Budgets

A convergence policy registers per-risk limits on four card dimensions —
review returns, repair attempts, gate failures, and material scope
revisions — plus one cycle dimension, integration failures. A card's
declared `risk` selects which set of limits applies to it. No budget exists
unless a policy is configured: a project without one reports
`legacy_unassessed`, and nothing is enforced.

### Escalation

A dimension is exhausted at `count >= limit` — the limit is how many
attempts you get, and the last one spends it. An escalated card refuses
delivery and review; an escalated cycle refuses integration preparation, the
merge/land path, verification, integration review, final authorization, and
promotion.

Read the state before guessing at it. `card status` and `cycle status`
report the budget, the evidence behind each count, and the next permitted
action:

```bash
change-harness card status --card-id F-001
change-harness cycle status --cycle-id C-001
```

### The six card dispositions

An escalated card has exactly six authorized ways forward. Get the
distinctions right — they are the whole point.

- `renew` grants exactly one more configured budget in the one dimension
  that is exhausted. Further attempts still count, and can escalate the
  same dimension again.

  ```bash
  change-harness disposition renew --card-id F-001 --dimension repair-attempts --actor coordinator --rationale "one more repair attempt is warranted"
  ```

- `accept-risk` lets the card proceed with **no** further budget. The count
  keeps climbing, but the dimension stops escalating, because an authorized
  actor accepted that risk. It requires both what risk is accepted and why.

  ```bash
  change-harness disposition accept-risk --card-id F-001 --dimension gate-failures --risk "the flaky integration gate may fail once more" --rationale "shipping now is safer than another repair cycle" --actor coordinator
  ```

- `split` moves the remaining work behind one exhausted dimension to an
  **already-existing** follow-up card in the same cycle, and waives that
  dimension the same way `accept-risk` does. Create the follow-up card
  first:

  ```bash
  change-harness card create --draft F-002.yaml
  change-harness disposition split --card-id F-001 --dimension material-scope-revisions --follow-up-card-id F-002 --actor coordinator --rationale "the remaining scope is a distinct outcome"
  ```

- `abandon` permanently ends an escalated card. There is no way back.

  ```bash
  change-harness disposition abandon --card-id F-001 --actor coordinator --rationale "the approach will not converge"
  ```

- `redesign` also permanently ends the card, because the approach itself was
  wrong, and names the exact card that replaces it. The replacement may sit
  in a different cycle.

  ```bash
  change-harness disposition redesign --card-id F-001 --replacement-card-id F-010 --actor coordinator --rationale "the approach was wrong; replaced by F-010"
  ```

- `rebaseline` is project-wide, not per-card: it retires the currently
  configured convergence policy digest, installs a new one, and re-pins
  every non-terminal cycle to it, all in one transaction.

  ```bash
  change-harness disposition rebaseline --policy new-policy.json --actor coordinator --rationale "the configured limits were miscalibrated project-wide"
  ```

### Who may record a disposition

Authorization is `final_authorization_policy.authorizer_actor_ids` — the
same set that authorizes a sealed cycle's final acceptance. Every
disposition needs a non-blank `--rationale`.

Four of the six refuse a repeat: `accept-risk` and `split` each refuse a
second call on a dimension whose escalation is already waived, and `abandon`
and `redesign` refuse a second call outright, because the card is already
terminal. `renew` is the exception by design — the same dimension can be
renewed again the next time it escalates.

### The cycle-level budget

A cycle carries its own budget, on integration failures, and there is no
cycle-scoped `renew` — every disposition except `rebaseline` takes
`--card-id`, not `--cycle-id`. An escalated cycle has exactly two exits:

```bash
change-harness disposition rebaseline --policy new-policy.json --actor coordinator --rationale "retire the digest so the old integration failures stop counting"
change-harness cycle abandon --cycle-id C-001 --actor coordinator --reason "the cycle cannot integrate; ending it"
```

`rebaseline` retires the digest so the old `integration_failure` facts stop
counting; `cycle abandon` ends the cycle outright. Both are heavy by design:
a cycle that cannot integrate is a design signal, not something to patch
indefinitely.

## Recovery and safe refusals

Common responses are simple:

- out-of-scope candidate: revise scope, resume work, re-gate, re-handoff;
- stale approval: create a new handoff and obtain fresh review;
- lock conflict: wait and use `project status`; never remove a lock manually;
- exit 9: use `project status`, then `project recover`.

Mutating commands journal state. If promotion moved the authority but local
fast-forward failed, do not rewind the authority; `project recover --resume`
finishes the local synchronization.

## Repository adoption

`project init` governs an existing repository without rewriting its history.
All paths are absolute:

```bash
change-harness project init \
  --project-id example \
  --repository /abs/path/to/project \
  --control /abs/path/to/control \
  --authority /abs/path/to/authority.git \
  --worktree-root /abs/path/to/worktrees
```

Initialization seeds a new authority from the candidate's protected branch.
Later cycle activation freezes the authority head, not whichever commit happens
to be in a local checkout. Re-running init accepts identical configuration but
refuses rebinding or unchecked control content.
