---
name: change-harness
description: Drive a bounded change through the change-harness CLI — card, allocated worktree, gates, exact-SHA handoff, independent review, integration, promotion. Use when a project is governed by change-harness (a control repository exists or CHANGE_HARNESS_CONTROL is set) and you need to land, review, or integrate a change through it.
---

# Driving a change through change-harness

change-harness coordinates bounded changes across three repositories: the
**candidate** (an ordinary repository where work happens), the **control** (a
separate Git repository holding cards, reviews, receipts, and integration
records — the authoritative account of *why*), and the **authority** (a bare
repository owning the protected branch, moved only by promotion). Every claim
binds to an exact commit SHA. The harness automates the mechanical controls;
the judgment stays with you.

## Ground rules

These are the rules the tool cannot enforce, learned from real failures. Break
one and the harness will usually refuse you later, at a worse moment.

1. **Declare every file you will touch** — source, tests, docs — in the card's
   `write_scope` before starting, then run `work verify` before every handoff.
   An out-of-scope path is the single most common authoring failure, and it
   refuses at handoff with exit 5, `CH-POLICY-CANDIDATE-OUT-OF-SCOPE`, naming
   the path. Forgetting the test file you obviously must edit is the classic
   case.
2. **`delivered_sha` is the exact commit you delivered.** The harness refuses a
   handoff whose branch head disagrees. Never write "the branch," a stale
   SHA, or a full hash reconstructed from memory of a short one — run
   `git rev-parse HEAD` in the worktree at the moment you write it. A
   hand-expanded guess is indistinguishable from a fabricated one until the
   harness refuses it.
3. **One actor, one role.** The reviewer is a different actor, in a separate
   task or agent thread that starts cold — not a later turn of the
   conversation that wrote the code. Step 7 says exactly what that thread may
   receive. Identity is declared, not proven (D-013): the harness records it
   and cannot verify it. Honor the separation anyway; independent review bound
   to exact commits is the entire product.
4. **Treat refusals as information.** Exit 5 means a state machine is
   protecting an invariant. Read `error.code` and `error.recovery`, fix the
   state, retry. Never work around a refusal with raw git.
5. **Never hand-edit** the control repository, the authority, the protected
   branch, or a lock file. Never force-push or `reset --hard` anything the
   harness manages.
6. **A review is evidence only if it tried to break something.** Record what
   you tried that failed. Approving because the diff "looks right" is the
   failure mode this tool exists to prevent — and it applies to you before
   you hand off, not only to the reviewer after. Mutate what you wrote and
   confirm the test actually fails; a test written immediately after its own
   implementation is a suspect, not a witness.
7. **Check whether another session already owns the ground you are about to
   cover — and know how far that check reaches.** `integration ready
   --cycle-id <id>` reports every *activated* card in that cycle with its
   current state, not only the ones ready to integrate — a card left in
   `draft` after a failed activation will not appear; an active or
   `review_pending` card that does is someone's work in progress. `cycle
   status` is not this check: its `card_ids` lists every card ever
   activated, abandoned ones included, with no state attached.

   Two cards claiming the same *file* in one cycle are refused at
   activation (`CH-POLICY-OWNERSHIP-OVERLAP`). Two claiming the same ground
   through *different* files are refused too, if both say so:
   `exclusive_resources: ["issue:42"]` on both cards is checked exactly
   like a file path (`CH-POLICY-RESOURCE-CONFLICT`) — declaring the issue or
   ticket a card is for is what makes that catchable, and nothing catches
   it if neither card declares it.

   None of this reaches past the current cycle. Two active cycles can each
   claim the same file or the same resource string with nothing refusing
   either, and there is no command that lists which other cycles exist.
   Look first, declare the resource you are working if you have one, and
   ask whether another cycle might be open before assuming there is only
   one.
8. **A claim about state you did not just read is not a fact.** Writing a
   decision-register row, a defect entry, or a review finding based on what
   you remember from an adjacent worktree or an earlier turn is how a false
   statement gets recorded while you are in the middle of correcting a false
   statement. Read the file you are describing, in the tree you are
   describing, before you write the sentence.

## Setup

Export the control path once instead of repeating `--control` on every
command; an explicit flag still wins where both are present:

```bash
export CHANGE_HARNESS_CONTROL=/path/to/control
```

`project init` deliberately ignores the variable — it decides where a control
repository is *created*, and inheriting that from another project's export is
how you initialize into the wrong place.

Always pass `--output json` when a program is reading the result. Success is a
`harness.command-result/v1` envelope (`status`, `command`, `data`,
`warnings`); failure is `harness.command-error/v1` with `error.code`,
`error.message`, `error.recovery`, and `error.details`. Argument-parsing
failures are wrapped in the same envelope.

Check the host before anything else on a new machine:

```bash
change-harness doctor --workspace .
```

Git ≥ 2.50 is required (for `git merge-tree --write-tree`) and validated
rather than silently degraded.

## Roles, authority, and when to stop

Every command in this CLI is available to you at every moment. That is a
property of a single-user local tool, not a grant. The harness records the
role you declare and cannot verify it (D-013), so most of what follows is
yours to keep rather than something you will be refused for breaking.

| Role | Owns | Declares itself as |
| --- | --- | --- |
| Coordinator | Cycles, cards, gate registration, integration from `prepare` to `promote` | `--actor` on `cycle`/`card`/`gate register`; `--actor-id` on `integration` |
| Implementer | One card at a time — its worktree, its gates, its handoff | `--actor` on `work`, `gate run`, `handoff create` |
| Reviewer | The verdict on a candidate they did not write | the verdict's own `reviewer_actor_id` for `review record`; `--reviewer-actor-id` on `integration review` |
| Acceptance owner | Whether a verified integration may move the protected branch | `--acceptance-owner` |

**A command being available is not authority to run it.** Most of this table
rests on you declaring the role you are actually in. Four cases are refused
(exit 5), and they are the ones where the author would otherwise bless their
own work:

- reviewing a card you handed off — `CH-POLICY-SELF-REVIEW`;
- reviewing an integration you verified — `CH-POLICY-SAME-ACTOR`;
- accepting an integration carrying a card you implemented — `CH-POLICY-SAME-ACTOR`;
- promoting one — `CH-POLICY-SAME-ACTOR`.

The **acceptance owner may promote**, deliberately: acceptance is the
authorization and promotion is executing it, and the one-human-many-agents
model has the same person do both. Comparisons ignore surrounding space and
letter case, so `Reviewer-B` will not pass as someone other than `reviewer-b`.

None of this proves anything about who ran the command. The same person under
two names defeats all four, and is meant to — Q-004 is where declared identity
becomes attested identity.

**Escalate and stop** — do not resolve it yourself — when any of these is
ambiguous: what is in scope, who owns a file or a decision, how much risk a
change carries, what the acceptance criteria actually require, or what the
product is supposed to do. These are the questions where a confident wrong
answer is expensive and a question is cheap. Write down what you were about to
assume, and ask. `work block --actor …` exists for the case where you cannot
proceed at all.

Throughout this guide: a **refusal** is something the tool does, and a **rule**
is something you do. Where the difference matters it is stated. Do not read a
rule as a guarantee that something will catch you.

## The actor-flag split (read this before anything fails)

Different command families name the acting party differently. This is a known,
recorded inconsistency — documented here as it is, not as it should be:

| Command family | Flag |
| --- | --- |
| `work start` / `resume` / `checkpoint` / `block`, `gate run`, `handoff create`, `review begin` / `record`, `card revise` | `--actor` |
| `integration prepare` / `merge` / `land` / `verify` / `promote` | `--actor-id` |
| `integration review` | `--reviewer-actor-id` |
| `acceptance record` | `--acceptance-owner` |

Neither `--actor` determines the reviewer. `review begin`'s is genuinely
used — it is the actor recorded on the `review.begun` event. `review
record`'s is accepted and then read nowhere at all: the `review.recorded`
event's actor is the verdict's own `reviewer_actor_id`, not the flag. Self-
review is refused by comparing that field against the handoff's `--actor`
(from `handoff create`, a different flag on a different command); `review
record`'s own `--actor` plays no part in it and could be omitted from this
table without losing anything true.

`integration preflight` takes no actor at all. `work verify` and the
read-only status commands (`cycle status`, `card status`, `gate list`) accept
an optional `--actor` that defaults to `operator` — you may omit it.

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
| 9 | Recovery required — run `project status`, then `project recover` |
| 10 | Internal error |

Exit 1 is deliberately unassigned so an uncategorized panic stays
distinguishable from every classified outcome.

Every mutating command accepts `--dry-run` (the one exception:
`project recover --resume` has no dry-run form). Most run the real checks
against real state and change nothing, so a dry run that would refuse tells
you so with the same error the real command would give. `handoff create` is
a known exception: its preview does not check feature-gate evidence, so
`--dry-run` can succeed where the real command refuses with
`CH-GATE-EVIDENCE-STALE` over a missing or stale receipt.

## The lifecycle

### 0. Orient

```bash
change-harness project status
change-harness cycle status --cycle-id C-001
change-harness integration ready --cycle-id C-001
change-harness card status --card-id F-001
```

`integration ready` answers "what is approved and integrable, and why is
everything else not" — start there when arriving with no context.

### 1. Gates are registered once, deliberately

A card can only name gates that were registered — it cannot invent a check
nobody reviewed. A definition looks like:

```json
{
  "schema": "harness.gate/v1",
  "gate_id": "gate.test",
  "revision": 1,
  "argv": ["cargo", "test"],
  "working_directory": ".",
  "timeout_seconds": 600,
  "environment": { "allow": ["PATH", "HOME"], "set": {} },
  "network_policy": "denied",
  "retry_policy": { "max_attempts": 1 },
  "artifacts": []
}
```

```bash
change-harness gate register --definition gate.test.json
change-harness gate list
```

Gates run with a cleared environment: only the allowlist survives, and eight
credential variables (`AWS_SECRET_ACCESS_KEY`, `GITHUB_TOKEN`, `NPM_TOKEN` and
the rest) are refused at registration even when a definition names them. That
part is enforced.

`network_policy` is **not**. It records what the gate says it needs; nothing
stops a gate from reaching the network, and `gate show` prints
`denied (declared, not enforced)` for exactly that reason. Receipts carry
`network_enforced=false` beside the declaration. Do not read `denied` as
isolation.

**A gate is a read-only check.** It observes the candidate and reports;
publishing, deploying, notifying, filing, or writing to any system outside the
worktree does not belong in one. Two reasons, and the second is the one that
bites: a gate is rerun against the landing commit during integration and may
be retried on failure, so anything it does externally happens more than once
and at times nobody chose; and a receipt is evidence about a commit, which it
stops being the moment the run also changed the world. The harness cannot
check this — `argv` is whatever was registered — so it is a rule, not a
refusal.

### 2. Open a cycle

```bash
change-harness cycle create --cycle-id C-001 --objective "One-line objective"
change-harness cycle activate --cycle-id C-001
```

Activation freezes the cycle's baseline to the authority's current head.
Every card in the cycle is meant to build from that exact commit — this is a
rule, not a refusal. `base_sha` is validated only as a well-formed
40-character commit id; nothing cross-checks it against the cycle's frozen
baseline, and a card naming a later commit activates and starts without
complaint.

### 3. Author and activate the card

The card is the contract. Write the draft YAML completely — the deserializer
rejects any key it does not know. Not every key below is required: `non_goals`
and `depends_on` are shown but default to empty, same as `contract_reads`,
`contract_changes`, `exclusive_resources`, and `generated_artifacts`, which
are valid keys not shown here at all. `card_id`, `cycle_id`, `title`, `goal`,
`risk`, `change_kind`, `base_sha`, `write_scope`, `named_gates`, `acceptance`,
`review_policy`, and `rollback_strategy` are the ones actually required.
Declare `exclusive_resources` when ground rule 7 calls for it.

```yaml
card_id: F-001
cycle_id: C-001
title: One-line title
goal: >
  What will be true when this lands, stated as behavior.
non_goals:
  - What this card deliberately does not do.
risk: low            # low | medium | high | critical
change_kind: feature # a free string, not an enum — feature | fix by convention
base_sha: <the cycle baseline, 40 hex>
depends_on: []       # other card ids this builds on, if any
write_scope:
  include:
    - "src/thing.rs"
    - "tests/thing.rs"      # forget this and handoff refuses later
    - "docs/CHANGES.md"
  exclude: []
named_gates:
  feature: [gate.fmt]       # must pass before handoff
  review: []
  integration: [gate.test]  # rerun against the landing commit
acceptance:
  behaviors:
    - "Observable statements a gate or test could actually fail."
  regressions:
    - "What must keep working."
review_policy: independent
rollback_strategy: revert the landing commit on the protected branch
```

```bash
change-harness card create --draft F-001.yaml
change-harness card activate --card-id F-001
```

Activation freezes the card into an immutable, digested revision. To change it
afterward use `card revise --card-id F-001 --draft new.yaml --reason "..."`,
which supersedes the revision, invalidates existing approvals, and returns the
card to `ready` — run `work resume` to step it back to work.

Base the new draft on the card's *current* revision, not the file you first
wrote. The control repository stores each revision separately
(`cards/<id>/rN.json`), and `draft.json` is permanently revision 1 — `card
status --card-id F-001` reports which `N` is current. Editing `draft.json` to
build revision 3 silently reverts every field revision 2 already changed;
nothing in the CLI catches this today, so diff every stored revision before
submitting if you did not author the one you are building on.

Scope authoring guidance: paths are matched exactly or by glob. Both sides of a
rename are checked. Include every test and doc file the change needs; a scope
you have to revise later costs a re-handoff and a fresh review.

`risk: high` and `risk: critical` require an *approving* review verdict to
declare `human_reviewer: true` — enforced, not advisory. `changes_requested`
and `blocked` verdicts do not require it: the gate is on what may approve a
high-risk card, not on every word said about one along the way.

### 4. Work in the allocated worktree

```bash
change-harness work start --card-id F-001 --actor implementer-a
```

This leases the card to one actor and allocates a branch (`card/F-001`) and a
dedicated worktree; the result names the path. Work there, not in your main
checkout. Commit everything — an untracked file blocks handoff by design,
because an untracked file can silently become part of a candidate. If a gate
writes output (caches, coverage, `__pycache__`), the *project's* `.gitignore`
must cover it or the handoff will refuse on a dirty tree.

### 5. Run the feature gates

```bash
change-harness gate run --card-id F-001 --gate-id gate.fmt --actor implementer-a
```

The receipt binds to the exact commit and records whether the worktree was
clean. A receipt from a dirty tree does not count as evidence about the
commit — iterating dirty is fine, but the final run must be on committed
state. Every gate the card names under `feature:` needs a passing receipt
before handoff.

### 6. Verify, then hand off

**Always run this first** — it is the same check handoff performs, and it
names any out-of-scope path while fixing it is still cheap:

```bash
change-harness work verify --card-id F-001
```

Then write the declaration (exact keys, exact SHA):

```yaml
delivered_sha: <git rev-parse HEAD in the card worktree>
behavior_delivered: >
  What the candidate actually does, stated as behavior.
implementation_decisions:
  - "Choices a reviewer should know about, with the why."
assumptions:
  - "What this relies on being true."
known_limitations:
  - "What this deliberately does not handle."
residual_risks:
  - "What could still go wrong, honestly."
rollback_notes: revert the landing commit on the protected branch
```

```bash
change-harness handoff create --card-id F-001 --declaration decl.yaml --actor implementer-a
```

The harness refuses if the branch head is not `delivered_sha`, if the tree is
dirty, if a feature gate receipt is missing or stale, or if any changed path
falls outside the card's write scope.

### 7. Review — a separate task, a fresh context

For every card with `review_policy: independent`, the implementer does not
review. Create or request a **separate reviewer task or agent thread**. Not a
new section of the same conversation, not a subagent handed your working
state — a thread that starts cold.

The reviewer receives exactly:

- the activated card revision and its review criteria;
- the handoff declaration;
- the exact delivered commit, and the baseline it builds from;
- the complete diff, and any contract-domain changes in it;
- the feature gate receipts;
- the repository files and reproducible evidence needed to evaluate the change.

The reviewer does **not** receive the implementation conversation, your
private reasoning, your working summary, or unfiltered author context. Nor an
instruction to approve: no "this is ready, please sign off", no framing that
supplies the conclusion. Handing the reviewer your reasoning is how a review
becomes a second reading of the same argument, which is worth nothing. What
survives contact with a cold reader is the point.

The harness enforces the mechanical half — exact-commit binding, stale-review
rejection, and a reviewer actor different from the handoff actor (exit 5,
`CH-POLICY-SELF-REVIEW`). It cannot enforce the rest. **Actor identity is
declared, not proven** (D-013, D-017): a fresh context is a strong review
practice, not an attested one, and nothing here can tell whether you really
opened a new thread. Honor it anyway; independent review bound to exact
commits is the entire product.

**Decide your stopping rule before round one, and write it down.** Nothing in
this tool bounds how many review rounds a card goes through — this is a rule,
not a refusal. "Land unless the next review finds an exploitable bypass,"
decided before you start, is one you can hold yourself to under pressure.
Deciding it after round three, once you are tired of finding things, is not
the same rule.

```bash
change-harness review begin  --card-id F-001 --actor reviewer-b
change-harness review record --card-id F-001 --verdict verdict.yaml --actor reviewer-b
```

Review discipline: verify claims by breaking things — apply the mutation a
test claims to catch and confirm it fails at the assertion that matters; drive
the real binary against the real behavior. The same applies to documentation:
a decision-register row or defect-log entry is a claim about what the code
does, and rereading it only checks that it reads well — reproduce the
scenario it describes against the actual code before you trust the sentence.
Then write:

```yaml
reviewer_actor_id: reviewer-b
decision: changes_requested # approved | changes_requested | blocked
human_reviewer: true        # required to approve a high/critical card, not for changes_requested or blocked
findings:
  - severity: medium       # critical | high | medium | low
    location: src/thing.rs
    detail: What is wrong, concretely.
    disposition: open      # open | still_open | resolved | accepted_risk | out_of_scope
gate_adequacy:
  gates_observe_acceptance: true
  unobserved_behaviors: []
  basis: Specifically what you ran to establish this.
residual_risks: []
```

Rules the harness enforces:

- An **approval may not carry an `open` finding**. `findings: []` is a valid
  clean approval; `changes_requested` requires at least one finding.
- A **re-review may not silently drop a superseded review's open findings** —
  each must reappear at the same `location` with an explicit disposition.
  `still_open` is for the common case that is neither settled nor new: worked
  on, not yet closed. It accounts for the prior finding without claiming it
  is resolved, and still blocks an approval exactly as `open` does.
- An approval goes **stale** if the candidate changes or the card is revised.
  A third, narrower condition covers dependencies: not any change to a
  dependency's standing, but specifically when the commit this card recorded
  as incorporated is no longer contained in what the dependency currently
  stands approved at, or when a declared dependency has no recorded binding
  at all. A dependency merely losing its own approval does not by itself
  stale this one. Staleness is reported, and integration refuses stale
  members.

After `changes_requested`: `work resume --card-id F-001 --actor implementer-a`
(the card cannot hand off from `changes_requested` directly), fix, re-run
gates, `handoff create` again (a new handoff), fresh review.

### 8. Integrate

Flags switch to `--actor-id` here.

```bash
change-harness integration ready     --cycle-id C-001
change-harness integration prepare   --cycle-id C-001 --actor-id coordinator
change-harness integration preflight --integration-id INT-001
change-harness integration merge     --integration-id INT-001 --actor-id coordinator
change-harness integration land      --integration-id INT-001 --actor-id coordinator
change-harness integration verify    --integration-id INT-001 --actor-id verifier
change-harness integration review    --integration-id INT-001 --reviewer-actor-id int-reviewer
```

`prepare` pins each approved candidate and orders them topologically; cards it
leaves out are named in `warnings` with the reason — read them, silence is
never the signal. `preflight` simulates the whole merge without touching a
ref. `merge` combines in a disposable worktree. `land` builds the landing
commit — baseline as first parent, integration head as second, moving no
branch. `verify` reruns the union of every member's `feature`, `review`, and
`integration` named gates *against the landing commit* — not only the ones
named under `integration:` — because a gate that passed on an isolated
candidate proves nothing about the combined tree.

### 9. Accept, promote, archive

```bash
change-harness acceptance record   --integration-id INT-001 --acceptance-owner "Name"
change-harness integration promote --integration-id INT-001 --actor-id promoter
change-harness archive create --integration-id INT-001
change-harness archive verify --integration-id INT-001
change-harness archive close  --integration-id INT-001
```

Acceptance binds one exact landing commit and is the only thing that
authorizes moving the protected branch. Promotion moves the authority with a
compare-and-swap and fast-forwards the local protected worktree. Archive refs
keep every landed commit reachable after `close` removes the card branches and
worktrees; `close` refuses to delete anything that would become unreachable.

## Common refusals

| You see | It means | Do |
| --- | --- | --- |
| `CH-POLICY-CANDIDATE-OUT-OF-SCOPE` (exit 5) | The candidate touches a path outside `write_scope`; the path is named | Drop the change, or `card revise` with the corrected scope, `work resume`, re-gate, re-handoff |
| `CH-POLICY-INVALID-TRANSITION` (exit 5) | The command is not legal from the current state; permitted states are listed | `card status` to see where you are; after `changes_requested`, `work resume` first |
| `CH-POLICY-NOT-INTEGRABLE` (exit 5) | A named card is not approved-and-current | `integration ready` explains exactly why |
| `CH-POLICY-OWNERSHIP-OVERLAP` (exit 5) | The card's write scope overlaps another active card's | Narrow one scope (`card revise`), or wait for the other card to land — overlap is refused at activation, not discovered at merge |
| Handoff refuses a dirty worktree | Uncommitted or untracked files | Commit them, or add gate outputs to the project's `.gitignore` |
| Handoff refuses the SHA | Branch head ≠ `delivered_sha` | Re-read HEAD, fix the declaration |
| An approval is reported stale | Candidate or card changed after the review | New handoff, fresh review — the old approval is not deleted, it just no longer applies |
| Stale specifically over a dependency | The dependency's standing approval no longer contains what this card recorded as incorporated, or has no recorded binding | A new handoff alone recomputes the binding; if the candidate does not yet build on the dependency's current commit, incorporate it first, then hand off and review again |
| Lock held by another command | Concurrent mutation | Wait; `project status` diagnoses a stale holder; **never delete a lock file by hand** |
| Exit 9 from anything | An operation was interrupted mid-mutation | `project status`, then `project recover` |

## Recovery

Mutating commands journal every step before performing it, so an interruption
is attributable rather than guessed at — `backup create` is the exception; it
writes bundle files directly and is not journaled, so an interruption there is
not something `project recover` can attribute. `project recover` reports; its
`--resume` completes only an interrupted promotion (the one operation it can
safely finish) and refuses anything else by name — disposition of other
partials is an operator decision.

One state is deliberate: if promotion moves the authority and the local
fast-forward then fails, the command exits 9 and records
`authority_promoted_local_sync_pending`. The authority is **not** rolled
back — rewinding a published branch is worse than a recoverable gap. Run
`project recover --resume` to finish the local sync.

## Adopt an existing repository

`project init` puts a repository you already have under governance. It does
not create your project, move it, or rewrite one commit of its history — it
registers the checkout and builds the two repositories the harness owns
around it.

Four paths, three of them new, and **all of them absolute**. `--repository .`
is refused (`` expected an absolute path, found `.` ``); your working
directory is never consulted, so it makes no difference whether you run this
from inside the project or anywhere else.

| Flag | What it points at |
| --- | --- |
| `--repository` | The project you already cloned. Must exist, be a Git repository, and have at least one commit on the protected branch. |
| `--control` | **New**, usually. Cards, reviews, receipts, integration records. An empty or absent directory initializes fresh; one already bound to an identical configuration is accepted as a no-op (see "Init is a one-time registration," below); one bound to a different configuration, or holding files nobody checked, is refused. |
| `--authority` | **New.** The bare repository that owns the protected branch. |
| `--worktree-root` | **New**, optional. Where card worktrees are allocated; defaults to `<the control's parent>/<project-id>-worktrees`. |

```bash
change-harness project init \
  --project-id       example \
  --repository       /abs/path/to/your-existing-repo \
  --control          /abs/path/to/example-control \
  --authority        /abs/path/to/example-authority.git \
  --worktree-root    /abs/path/to/example-worktrees
```

Nothing is created inside the repository being governed — that nesting is
refused. Control and authority are meant to be siblings, but nothing stops
either from being nested inside the *other*; the check only guards the
candidate. The candidate gains exactly one thing: a remote named `harness-authority`. An
existing remote of that name is never repointed.

### Where the baseline comes from, and when

**`project init` reads the candidate's protected branch and seeds the empty
authority with it.** That is the moment your history is captured.

`cycle activate` later freezes the cycle baseline to the *authority's* head —
deliberately not the candidate's, because the candidate's branch is whatever a
local actor last did, while the authority's is what has been accepted. So if
your `main` gains commits between `project init` and your first
`cycle activate`, the baseline is the commit **init** saw, not the newer one.
There is no confirmation step for this; the sequence is the record. If you
want the newer commit as your baseline, promote it through the harness, or
initialize after it exists.

An authority that already exists, is compatible, and already holds the
protected branch is adopted unchanged, and then *it* supplies the baseline
rather than the candidate. A compatible authority that exists but does not
yet hold that branch — an empty bare repository, say — is not left empty:
init seeds it from the candidate, the same as a brand new one. "Compatible"
and "already has the branch" are two different conditions, and only meeting
both skips the seed. The candidate's protected branch is still resolved and
checked either way — initialization validates it
in every case and refuses a repository that has no commit on it — it is
simply not what the authority gets seeded from.

### Init is a one-time registration

Re-running it with identical configuration reports there is nothing to do.
Re-running it against a control repository bound to a *different*
configuration refuses rather than rebinding, and it refuses a control
directory that already holds files nobody checked.

Once it succeeds, work stops happening in your original checkout: register
gates (step 1), open a cycle, and take each card into the worktree the harness
allocates for it.
