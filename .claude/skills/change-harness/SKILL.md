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
   handoff whose branch head disagrees. Never write "the branch" or a stale
   SHA.
3. **One actor, one role.** The reviewer must be a different actor in a fresh
   context from the implementer. Identity is declared, not proven (D-013) —
   the harness records it and cannot verify it. Honor the separation anyway;
   independent review bound to exact commits is the entire product.
4. **Treat refusals as information.** Exit 5 means a state machine is
   protecting an invariant. Read `error.code` and `error.recovery`, fix the
   state, retry. Never work around a refusal with raw git.
5. **Never hand-edit** the control repository, the authority, the protected
   branch, or a lock file. Never force-push or `reset --hard` anything the
   harness manages.
6. **A review is evidence only if it tried to break something.** Record what
   you tried that failed. Approving because the diff "looks right" is the
   failure mode this tool exists to prevent.
7. **Never put a credential in a record.** Cards, handoffs, reviews, and gate
   definitions are committed to the control repository, and control history is
   the integrity chain — nothing can remove one later without breaking what
   the record proves. The harness refuses the shapes it recognizes (exit 5,
   `CH-POLICY-SENSITIVE-VALUE`, naming the field and not the value), but it
   recognizes *shapes*: an internal token with no distinctive prefix passes
   straight through, so the rule is yours to keep. A gate that needs a
   credential names it under `environment.allow` and lets the host supply it —
   `environment.set` holds literal values and is committed with the
   definition.

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

## The actor-flag split (read this before anything fails)

Different command families name the acting party differently. This is a known,
recorded inconsistency — documented here as it is, not as it should be:

| Command family | Flag |
| --- | --- |
| `work start` / `resume` / `checkpoint` / `block`, `gate run`, `handoff create`, `review begin` / `record`, `card revise` | `--actor` |
| `integration prepare` / `merge` / `land` / `verify` / `promote` | `--actor-id` |
| `integration review` | `--reviewer-actor-id` |
| `acceptance record` | `--acceptance-owner` |

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
`project recover --resume` has no dry-run form). It runs the real checks
against real state and changes nothing. A dry run that would refuse tells you
so with the same error the real command would give.

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
isolation, and do not put a credential in `environment.set` on the theory that
the gate cannot phone home with it.

### 2. Open a cycle

```bash
change-harness cycle create --cycle-id C-001 --objective "One-line objective"
change-harness cycle activate --cycle-id C-001
```

Activation freezes the baseline to the authority's current head. Every card in
the cycle builds from that exact commit.

### 3. Author and activate the card

The card is the contract. Write the draft YAML completely — the deserializer
rejects unknown fields, so use exactly these keys:

```yaml
card_id: F-001
cycle_id: C-001
title: One-line title
goal: >
  What will be true when this lands, stated as behavior.
non_goals:
  - What this card deliberately does not do.
risk: low            # low | medium | high | critical
change_kind: feature # feature | fix
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

Scope authoring guidance: paths are matched exactly or by glob. Both sides of a
rename are checked. Include every test and doc file the change needs; a scope
you have to revise later costs a re-handoff and a fresh review.

`risk: high` and `risk: critical` require the eventual review verdict to
declare `human_reviewer: true` — enforced, not advisory.

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

### 7. Review — different actor, fresh context

```bash
change-harness review begin  --card-id F-001 --actor reviewer-b
change-harness review record --card-id F-001 --verdict verdict.yaml --actor reviewer-b
```

The reviewer reads the card revision, the handoff, and the candidate at its
exact SHA. Review discipline: verify claims by breaking things — apply the
mutation a test claims to catch and confirm it fails at the assertion that
matters; drive the real binary against the real behavior. Then write:

```yaml
reviewer_actor_id: reviewer-b
decision: approved         # approved | changes_requested | blocked
human_reviewer: true       # required when card risk is high or critical
findings:
  - severity: medium       # critical | high | medium | low
    location: src/thing.rs
    detail: What is wrong, concretely.
    disposition: open      # open | resolved | accepted_risk | out_of_scope
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
- An approval goes **stale** if the candidate changes or the card is revised;
  staleness is reported, and integration refuses stale members.

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
branch. `verify` reruns every member-named integration gate *against the
landing commit*, because a gate that passed on an isolated candidate proves
nothing about the combined tree.

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
| `CH-POLICY-SENSITIVE-VALUE` (exit 5) | A record carries what looks like a credential; the field is named, the value never is | Remove it from that field and rotate it — it has been on this machine in plaintext |
| `CH-POLICY-INVALID-TRANSITION` (exit 5) | The command is not legal from the current state; permitted states are listed | `card status` to see where you are; after `changes_requested`, `work resume` first |
| `CH-POLICY-NOT-INTEGRABLE` (exit 5) | A named card is not approved-and-current | `integration ready` explains exactly why |
| `CH-POLICY-OWNERSHIP-OVERLAP` (exit 5) | The card's write scope overlaps another active card's | Narrow one scope (`card revise`), or wait for the other card to land — overlap is refused at activation, not discovered at merge |
| Handoff refuses a dirty worktree | Uncommitted or untracked files | Commit them, or add gate outputs to the project's `.gitignore` |
| Handoff refuses the SHA | Branch head ≠ `delivered_sha` | Re-read HEAD, fix the declaration |
| An approval is reported stale | Candidate or card changed after the review | New handoff, fresh review — the old approval is not deleted, it just no longer applies |
| Lock held by another command | Concurrent mutation | Wait; `project status` diagnoses a stale holder; **never delete a lock file by hand** |
| Exit 9 from anything | An operation was interrupted mid-mutation | `project status`, then `project recover` |

## Recovery

Mutating commands journal every step before performing it, so an interruption
is attributable rather than guessed at. `project recover` reports; its
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
is refused (`expected an absolute path, found `.``); your working directory is
never consulted, so it makes no difference whether you run this from inside
the project or anywhere else.

| Flag | What it points at |
| --- | --- |
| `--repository` | The project you already cloned. Must exist, be a Git repository, and have at least one commit on the protected branch. |
| `--control` | **New.** Cards, reviews, receipts, integration records. Must be absent or empty. |
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

Nothing is created inside the repository being governed. Control and authority
are siblings, not subdirectories, and the candidate gains exactly one thing: a
remote named `harness-authority`. An existing remote of that name is never
repointed.

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

An authority that already exists and is compatible is adopted unchanged, and
then *it* supplies the baseline — the candidate's branch is not read at all.

### Init is a one-time registration

Re-running it with identical configuration reports there is nothing to do.
Re-running it against a control repository bound to a *different*
configuration refuses rather than rebinding, and it refuses a control
directory that already holds files nobody checked.

Once it succeeds, work stops happening in your original checkout: register
gates (step 1), open a cycle, and take each card into the worktree the harness
allocates for it.
