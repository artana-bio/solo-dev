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

## What the Harness guarantees

The checks below run against the control and authority repositories the CLI
is pointed at, never against anything outside them. What follows states
exactly what an integrator can rely on, in three tiers: what the Harness
prevents outright, what it detects — and, in one case, now blocks — and what
is left over. Deciding what to trust the Harness for means knowing all three.

### Prevented

The Harness refuses these outright, before anything is written.

A card's convergence budget gates `work start`, `review begin`,
`review record`, `handoff create`, and `card revise`: once the card's
declared risk has a dimension at its configured limit, each of these
refuses, and so does its `--dry-run` preview. `work resume` is the one
deliberate exception — taking a returned card back up is how the next
attempt gets produced in the first place, so resume only re-checks the
budget when resuming is equivalent to starting fresh; the attempt itself is
still caught the moment it is delivered or reviewed.

A cycle's convergence budget gates every step from integration preparation
through promotion the same way: `integration prepare`, `integration merge`,
`integration land`, `integration verify`, `integration review`,
`acceptance record` (final authorization), and `integration promote` — all
refuse once integration failures exhaust the cycle's budget.

A convergence projection that cannot be trusted — a malformed, duplicate,
foreign, or unbound convergence fact — refuses the whole projection rather
than reading as an unspent budget. Every enforcement command above, and
`card status` and `cycle status` themselves, report the corruption (exit 10,
internal error) instead of letting a card or cycle advance, or display, as
though nothing had been recorded against it.

### Detected, and blocking where it counts

Every landing commit anchors the control repository's exact head, at that
moment, in a `Change-Harness-Control` trailer. `audit anchors` walks the
protected branch and reports every anchored head the control repository no
longer contains, or that is no longer an ancestor of its current head:

```bash
change-harness audit anchors
```

Nothing requires an operator to run that. What runs unconditionally:
`integration promote` performs the identical check itself, before its own
convergence budget check and before anything else, and refuses if any anchor
fails — deliberately, with no override:

```bash
change-harness integration promote --integration-id INT-001 --actor-id promoter
```

So this tier is not merely detected. The rewrite itself is still not
prevented — nothing stops an edit to the control repository's own history —
but promoting onto it is: a rewrite blocks exactly the promotion it would
otherwise have gone on to authorize. The refusal names both the anchored
commit and the landing commit that claimed it; its only remedy is restoring
the control repository from a `backup create` archive that contains the
anchored commit.

### Residual

Two things the Harness cannot stop, named rather than left for a reader to
infer:

- An actor who can write to both the control and authority repositories can
  rewrite either's history and keep the other consistent with it. The anchor
  check is a cross-check between exactly those two repositories; it has
  nothing outside them to compare against, so a rewrite that keeps both
  sides in agreement is invisible to it.
- An actor who rewrites the protected branch directly on the authority
  repository — a force-push, or a hand-edited ref, run outside this CLI —
  bypasses the Harness entirely. Promotion's compare-and-swap catches a
  stale plan racing a concurrent change; it is not a standing lock on the
  branch, and it has no way to see, let alone refuse, a Git command that
  never goes through it.

Both assume write access the Harness treats as non-adversarial. That is
D-013: a shared operating-system account is not treated as a security
boundary. `AGENTS.md` states this for people building the Harness; this is
the same position for people using it. Every `--actor`, `--actor-id`,
`--reviewer-actor-id`, and `--acceptance-owner` value is a string the caller
typed, not a proven identity — the CLI's own `--actor` help text says as
much. The Harness refuses self-review, self-integration review,
self-acceptance, and self-promotion when the same declared actor would bless
its own work — that catches the same actor typing the same name twice, not
one willing to type a different one. It is a deliberate architectural
position, not unfinished work: claiming otherwise would promise an identity
or repository-integrity guarantee no Git object can back.

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

## Multi-agent orchestration

Change Harness governs the assignment, worktree, evidence, and state
transitions. It does not launch an AI model. The coordinator uses the agent
host's **create new task/session** operation, then gives that task the bounded
packet below. Do not substitute a shared conversation, an informal request to
"take a look," or a branch name for an assignment.

The operating flow is:

```text
User request
  -> Coordinator proposes a cycle and independently reviewable cards
  -> Owner approves scope when the original request did not already authorize it
  -> Coordinator activates each card and allocates one worktree
  -> One implementer task receives one card and its allocated worktree
  -> Implementer commits, proves the behavior, and creates an exact-SHA handoff
  -> One fresh reviewer task receives authoritative review state and repository objects only
  -> Reviewer records findings, an undeclared mutation, and a verdict
  -> Coordinator integrates only exact candidates whose Harness state is approved
```

### Convert a user request into cards

The coordinator translates intent into governance before starting agents:

1. Restate the observable outcome and explicit non-goals.
2. Inspect repository guidance and fresh project, cycle, and ownership state.
3. Split work by independently reviewable outcome, not by file count or by how
   many agents are available.
4. For every card, declare exact write scope, contract reads and changes,
   dependencies, exclusive resources, risk, named gates, acceptance behaviors,
   regression behaviors, and rollback strategy.
5. Keep coupled changes in one card when neither half can be tested or reviewed
   honestly on its own. Use separate cards when each outcome has its own proof
   and neither requires overlapping ownership.
6. If the user's request did not already authorize implementation, show the
   proposed cycle and cards before activation. Approval of the product request
   is not permission to silently widen its scope.

### Plan before executing

Planning and execution are separate phases. Before starting an implementer
task, the coordinator proposes the complete card set currently known for the
cycle; it does not let agents create cards opportunistically while coding.
Each proposed card must have the fields above and an explicit execution
relationship: no dependency, depends on another card, or shares an exclusive
resource. If implementation reveals work outside an activated card, stop at
the boundary and propose a new card (or a revised card requiring approval)
before doing that work.

Execute the approved set according to those relationships:

- use serial execution when a declared dependency or exclusive resource
  requires it;
- use parallel execution only for cards with accepted disjoint ownership,
  dependencies, and resources.

An unrelated card need not wait for another card to finish. "Serial" describes
the order of card execution, not the creation of the plan; "parallel"
describes independent card tasks, not shared ownership of one card.

One implementation assignment means **one activated card, one lease, one
allocated worktree, and one feature actor**. An agent may complete several
cards over time, but each card gets a separate task and handoff. Cards may run
in parallel only after the Harness accepts their ownership, dependency, and
exclusive-resource boundaries. The coordinator owns fan-out and fan-in; an
implementer does not recruit another agent or split its own card silently.

### Start an implementer task

Read the authoritative card, allocate its worktree, and use the returned path
as the new task's working directory:

```bash
change-harness card status --card-id F-001
change-harness work start --card-id F-001 --actor implementer-a
change-harness work status --card-id F-001
```

The implementation packet contains only what the task needs to deliver the
card:

- project and control-repository locator;
- cycle ID and exact baseline SHA;
- activated card ID, revision, digest, and complete card body;
- lease ID, allocated worktree path, and feature actor ID;
- repository guidance files the task must read;
- named gates and any approved focused test commands;
- the reporting contract below; and
- explicit confirmation that this packet is the complete assigned context.

Do not paste the coordinator's private reasoning, unrelated card discussions,
or a broad conversation transcript. Repository files may be read when the card
or repository guidance requires them; another agent's conversation is never a
repository dependency.

Copy and fill this prompt when creating the implementer task:

```text
Role: Implementer for card <card-id> as actor <feature-actor-id>.

Authority:
- Control repository: <absolute-control-path>
- Cycle: <cycle-id>; baseline: <exact-baseline-sha>
- Card: <card-id> revision <revision>; digest: <card-digest>
- Lease: <lease-id>
- Worktree: <absolute-allocated-worktree-path>

Complete assigned context:
- Activated card: <attach or paste the complete authoritative card>
- Required repository guidance: <exact file list>
- Named gates and focused checks: <exact names or commands>

Rules:
1. Work only in the allocated worktree and only within the card's write scope.
2. Do not edit the control repository, authority, protected branch, or another card.
3. Do not widen scope or delegate part of this card. Ask the coordinator when a
   missing decision would change behavior, scope, contracts, risk, or dependencies.
4. Make bounded assumptions only when they stay inside the card; report them.
5. Add or update focused tests, run an intended-oracle mutation, commit all work,
   run the named gates, verify scope, and create the exact-SHA handoff.

Reporting:
- Progress: current phase, completed evidence, next action, and any risk discovered.
- Clarification: one decision needed, why it blocks, and the smallest safe options.
- Blocked: Harness state or refusal code, evidence, and required recovery or decision.
- Complete: candidate SHA, handoff ID, changed files, checks and mutation results,
  assumptions, limitations, residual risks, and checks not run.

This packet is the complete assigned context. Do not infer authority from prior chat.
```

### Progress, clarification, and completion

The implementer reports after orientation, after a material proof or failure,
before any scope-changing decision, and at handoff. Progress is informational;
it never changes Harness state by itself.

If missing information does not change the card boundary, the implementer may
make a conservative assumption and record it. If it changes behavior, scope,
contracts, dependencies, risk, acceptance, or rollback, the implementer stops
that path and asks one focused question. When no in-scope work can continue,
record the block:

```bash
change-harness work block --card-id F-001 --actor implementer-a --reason "Product decision required"
```

The coordinator answers in the implementer task, revises the card when its
immutable contract must change, and never uses an informal message to override
the activated card. At completion, verify the report from Harness and Git
state; do not accept "done" in chat as a handoff.

### Stop and untangle a bottleneck

Start from deterministic state rather than waiting for a conversation to notice
the pattern:

```bash
change-harness card status --card-id F-001 --output json
change-harness project status --output json
```

An activated card's `data.bottleneck` is a
`harness.bottleneck-projection/v1` document. It combines declared scope,
review trend, configured convergence-attempt counters, and hard escalation.
`project status` collects every non-clear card under `data.bottlenecks`.
Models should match the closed `status`, signal `kind`, `recommended_action`,
and `authority_action` fields; prose `detail` is for explanation, not control.

`attention_required` and `stop_required` recommend
`convene_bottleneck_group`. `stop_required` also preserves the Harness's real
next authority action, such as `record_authorized_disposition`; diagnosis
does not release that refusal. Attempt-based detection reports
`legacy_unassessed` when no convergence policy exists. The Harness can count
repeated recorded attempts, but it cannot determine whether two
natural-language hypotheses are materially similar. The implementer must
still apply that semantic stop rule.

Do not let a card consume repeated attempts without changing what the team
knows. Pause ordinary implementation when either condition is true:

- two materially similar attempts fail to resolve the same problem; or
- the next attempt would repeat the same hypothesis without new evidence.

Formal convergence-budget exhaustion also triggers this protocol, but it is a
backstop, not a reason to wait. The implementer stops changing the candidate,
records `work block` when no other in-scope work can continue, and returns a
short bottleneck packet: the exact card and candidate SHA, observed failure,
attempts already made, gate output and review findings, hypotheses disproved,
constraints that must remain true, and the smallest unresolved question.

The coordinator creates a temporary **bottleneck group** of fresh diagnostic
tasks. Use agents with greater reasoning capacity or relevant specialist
experience when available. Give each task the same authoritative bottleneck
packet and ask it to investigate independently. These are diagnostic tasks,
not extra implementers: they do not edit the candidate, share a worktree,
silently widen the card, or approve the work.

Ask the group to return evidence, a root-cause assessment, and ranked recovery
options. At least one task must challenge the current approach rather than
debug it in place. Options may include:

- resume with a new, falsifiable hypothesis and a narrow proof;
- replace a flawed implementation inside the existing card boundary;
- revise or split the card when the outcome or proof is too broad;
- redesign the approach behind a replacement card; or
- abandon the card when further investment is not justified.

The coordinator synthesizes the reports, records the decision and rationale,
and obtains any authority required by the card or convergence policy before
implementation resumes. A stronger agent is not permission to bypass scope,
ownership, evidence, review, or a Harness refusal. Repeated renewal without a
new hypothesis is not recovery; choose `split`, `redesign`, or `abandon`.

### Start a genuinely fresh reviewer task

For every review attempt, create a new task/session that is **not forked,
cloned, resumed, or summarized from the implementer task**. Do not attach the
implementer's conversation, private or hidden reasoning, scratch notes,
working summary, or the coordinator's desired verdict. If the agent host
cannot prevent inherited implementation context, the review is not
independent: do not record it as approval.

The fresh task receives the control-repository locator, card ID, reviewer actor
ID, required repository guidance, and the prompt below. The reviewer runs
`review begin` inside the fresh task so the authoritative packet — activated
cycle and card, exact baseline and candidate SHAs, complete diff, contract
changes, receipts, and handoff decisions, assumptions, and limitations — comes
from current Harness state rather than a coordinator's summary:

```bash
change-harness review begin --card-id F-001 --actor reviewer-b
change-harness review example
change-harness review record --card-id F-001 --verdict verdict.yaml --actor reviewer-b
```

Copy and fill this prompt when creating the reviewer task:

```text
Role: Independent reviewer for card <card-id> as actor <reviewer-actor-id>.

Start from authority:
- Control repository: <absolute-control-path>
- Card: <card-id>
- Required repository guidance: <exact file list>
- Run `review begin` yourself and use the exact packet and Git objects it names.

Independence rules:
1. This is a new task with no inherited implementation conversation.
2. Do not request or inspect the implementer's conversation, private or hidden
   reasoning, scratch notes, or working summary.
3. Do not edit the candidate branch or repair the implementation yourself.
4. Do not assume approval is desired. Findings and evidence come first.

Review requirements:
1. Inspect the complete base-to-candidate diff and the relevant repository code.
2. Evaluate every acceptance behavior, regression, contract change, and the
   review criteria in the authoritative packet.
3. Check whether tests can pass while the required behavior is wrong.
4. Run at least one narrow mutation the implementer did not declare and record
   the changed mechanism, command, intended oracle, and observed result.
5. Record `approved`, `changes_requested`, or `blocked` through the Harness.
   Locate every finding and disposition every prior authoritative finding.

Return: decision, findings first, mutation result, gate-adequacy conclusion,
verdict/review ID, residual risks, and limits of what you verified.
```

For a re-review, the fresh reviewer task also receives the authoritative prior
findings because it must disposition them. It still receives no implementation
conversation. Requested changes return to the original implementer through the
review record; the reviewer and coordinator do not patch the candidate.

### Coordinator integration boundary

Before integration, the coordinator independently reads `card status`,
`handoff inspect`, and `integration ready`. Integrate only when the Harness
reports the exact candidate approved. A reviewer message, an implementer
summary, or a green command copied from another task is not sufficient.

The coordinator may fan out independent cards, but fans them in only through
Harness state:

- `changes_requested`: return the structured findings to that card's original
  implementer task, then require a new commit, handoff, and fresh review;
- `blocked`: resolve the named authority, environment, or product decision
  without editing the candidate on the reviewer's behalf;
- `approved`: confirm the approval still binds the current card digest,
  candidate SHA, dependencies, and receipts before integration; and
- mixed outcomes across cards: integrate only an authorized complete set;
  never hide an unfinished dependency by omitting it silently.

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
and inspect it before use. `gate example` prints a complete valid definition to
start from — it is serialized from the same type `gate register` deserializes,
so it cannot describe a shape the command would refuse:

```bash
change-harness gate example
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
`card example` prints a complete valid draft to start from — it is
serialized from the same type `card create` deserializes, so it cannot
describe a shape the command would refuse. Its values are illustrative, not
a recommendation to copy verbatim; the warning it prints says exactly which
ones to replace.

```bash
change-harness card example
```

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

`gate run` requires a live reservation. Reserve the exact gate before running
it: a reservation authorizes one expensive validation attempt, not standing
permission to run the gate repeatedly.

```bash
change-harness gate reserve --card-id F-001 --gate-id gate.fmt --actor implementer-a
change-harness gate run --card-id F-001 --gate-id gate.fmt --reservation-id VR-000001 --actor implementer-a
```

Each receipt binds to the exact commit and records cleanliness. For a material
claim, state the invariant, precondition, visible assertion, and a small
mutation that should make the focused test fail. Run expensive broader suites
only after the narrow proof passes, at the schedule the Harness authorizes.

### 6. Verify and hand off

```bash
change-harness work verify --card-id F-001
```

Then submit a declaration bound to the exact commit. `handoff example` prints
a complete valid declaration to start from, serialized from the same type
`handoff create` deserializes; its `delivered_sha` is a placeholder that must
be replaced with this exact commit before the document will be accepted:

```bash
change-harness handoff example
```

```yaml
delivered_sha: <git rev-parse HEAD in the allocated worktree>
behavior_delivered: What the candidate actually does.
implementation_decisions: ["A choice you made and why."]
assumptions: ["Something inferred rather than specified."]
known_limitations: ["Something deliberately not done."]
residual_risks: ["Something that could still be wrong."]
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

`review example` prints a complete valid verdict to start from, serialized from
the type `review record` deserializes. Do not hand-write the verdict from this
guide's prose: the shape carries fields no sentence here names, including
`gate_adequacy` and a per-finding `disposition`.

```bash
change-harness review example
change-harness review begin --card-id F-001 --actor reviewer-b
change-harness review record --card-id F-001 --verdict verdict.yaml --actor reviewer-b
```

The reviewer must run at least one mutation the implementer did **not**
declare. Re-running the declared mutation only re-checks a report you already
have; a mutation the implementer never tried is the only kind that can find
what their evidence could not.

The valuable outcome is a mutation that **survives**. A green suite while the
behaviour can be silently inverted means the guard is real but undefended —
that is a finding to report, not a badly chosen mutation. Say what survived
and what it proves.

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

`project example` prints a complete valid policy, serialized from the type
`project set-convergence-policy` deserializes. The same document shape is what
`disposition rebaseline --policy` takes:

```bash
change-harness project example
change-harness project set-convergence-policy --policy policy.json
```

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

`project example-final-authorization` prints a complete valid policy,
serialized from the type `project set-final-authorization-policy`
deserializes:

```bash
change-harness project example-final-authorization
change-harness project set-final-authorization-policy --policy policy.json
```

Its two `authorizer_actor_ids` are placeholder slot names, not real actors:
replace both with actor ids this project has actually declared before
installing the document, or the installed policy authorizes nobody. `project
init --final-authorizer-actor-id <id>` (repeatable) installs the same policy
shape at adoption time instead — see "Repository adoption".

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

Repeat `--final-authorizer-actor-id <id>` to install a final-authorization
policy at the same time — the same set of actor ids that later authorizes a
disposition and a sealed cycle's final acceptance; see "Who may record a
disposition" for the document shape and the route to install or change one
afterwards.
