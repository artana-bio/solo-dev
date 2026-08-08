# Change Harness — Quick Start

**Every command below was run end to end on a fresh project before this was written.** The traps are the ones I actually hit, in the order I hit them.

What you get at the end: a change that landed on your protected branch with a record of which commit was tested, by which gate, who reviewed it, and what mutation the reviewer used to prove the test could fail.

Budget **30–40 minutes** the first time. Ten after that.

---

## 0. Before you start

```bash
curl -fsSL https://raw.githubusercontent.com/artana-bio/solo-dev/main/install.sh | sh
change-harness --version        # change-harness 0.1.4
```

Four platforms: macOS and Linux, Intel and ARM. This exact command was run from a clean machine state to write this guide.

**You need 0.1.4 or later.** `card example` and `handoff example` — the two commands that save you the most guessing — are not in 0.1.3. If `--version` says otherwise, stop.

**Get real exit codes.** A pipe's `$?` is the pipe's, not the command's — this bit me on my first command:

```bash
change-harness gate register --definition gate.yaml > out.log 2>&1; echo "EXIT:$?"
```

**Exit codes:** `0` ok · `2` you typed it wrong · `4` a precondition is missing · `5` a policy refused you.

Every refusal prints a `code:` and a `recovery:`. **Read the recovery — it is usually the literal next command.** Do not reach for raw `git` to get around a refusal; that is the one habit that makes the record worthless.

---

## 1. Adopt a project

Pick something small and real with a test command under a minute. All paths absolute.

```bash
change-harness project init \
  --project-id myapp \
  --repository    /abs/path/to/myapp \
  --control       /abs/path/to/myapp-control \
  --authority     /abs/path/to/myapp-authority.git \
  --worktree-root /abs/path/to/myapp-worktrees
```

Three repositories, deliberately separate: **your code**, the **control** repository that holds the record, and the **authority** repository that owns the protected branch.

```bash
export CHANGE_HARNESS_CONTROL=/abs/path/to/myapp-control
```

New projects always record an explicit final-authorization mode. Without
`--final-authorizer-actor-id`, initialization records `migration_required` and
sealed-cycle acceptance refuses until a policy is installed. Supplying one or
more `--final-authorizer-actor-id` values installs the documented default
policy and enforces it on the sealed-cycle path.

---

## 2. Register **two** gates — not one

**This is the trap that cost me three refusals.** A gate can occupy only one validation stage, *and* the validation policy requires the final-integration stage to have at least one gate. So one gate is never enough.

```bash
change-harness gate example > gate.yaml
```

Edit `gate_id` and `argv` to your real test command. **`argv` is a YAML block list** — replacing just the `argv:` line leaves an orphaned entry behind and the parser will reject it:

```yaml
gate_id: gate.unit
argv:
- python3
- -m
- unittest
- -q
```

Register it, then register a second with a different `gate_id` for the integration stage:

```bash
change-harness gate register --definition gate.yaml
sed 's/gate_id: gate.unit/gate_id: gate.integration/' gate.yaml > gate2.yaml
change-harness gate register --definition gate2.yaml
```

---

## 3. Open a cycle

```bash
change-harness cycle create   --cycle-id C-001 --objective "add a farewell function"
change-harness cycle activate --cycle-id C-001
```

Activation freezes the baseline commit every card in the cycle starts from.

---

## 4. Write the card

```bash
change-harness card example > draft.yaml
```

Edit it: your `card_id`, `cycle_id`, the frozen `base_sha` from step 3, a real `title` and `goal`, your files in `write_scope.include`, and acceptance behaviors a test can actually fail on. Clear the fields that name things your project doesn't have (`depends_on`, `contract_reads`, `exclusive_resources`, `generated_artifacts`).

**Put each gate in exactly one stage:**

```yaml
named_gates:
  feature:
  - gate.unit
  review: []
  integration:
  - gate.integration
```

```bash
change-harness card create   --draft draft.yaml
change-harness card activate --card-id F-001
```

An activated card is immutable. To change it: `card revise --card-id F-001 --draft draft.yaml --reason "..."` — which supersedes the revision and **invalidates any handoff, review, or receipt bound to the old one.**

## Bind the cycle distribution plan

Before work starts or integration is prepared, persist one plan covering every
card in the cycle. Its scope must exactly match each card's canonical
`write_scope`, and each assignment must include the declared actor, principal,
and session:

```json
{
  "schema": "harness.cycle-plan/v1",
  "plan_id": "PLAN-001",
  "cycle_id": "C-001",
  "objective": "add a farewell function",
  "cards": [{
    "card_id": "F-001",
    "card_revision": 1,
    "scope": ["src/farewell.py"],
    "scope_exclude": [],
    "depends_on": [],
    "proof_entries": ["proof-farewell"],
    "mutation_plan": ["remove the farewell assertion"],
    "risk": "low",
    "reviewer_requirements": ["independent"],
    "assignment": "implementer-a",
    "assignment_principal_id": "principal-a",
    "assignment_session_id": "session-a",
    "distribution": "parallel",
    "acceptance_behaviors": ["the farewell function returns the greeting"]
  }]
}
```

```bash
change-harness cycle plan --plan-id PLAN-001 --file plan.json
```

A normal cycle with no bound plan is refused before integration. The only
planless compatibility path is an explicit, pre-existing migration record. It
is refused for cycles created by the current CLI, which carry durable
`plan_required_v1` creation provenance:
`cycle migrate-legacy --provenance legacy_cycle_plan_v1`.

---

## 5. Do the work in the allocated worktree

```bash
change-harness work start --card-id F-001 --actor implementer-a
```

It prints a worktree path. **Work there, not in your checkout.** Commit normally with `git` — that is not an escape, it is the point.

> **If you `card revise` while holding a lease**, the card drops back to `ready` and `handoff create` will refuse. The fix is `work resume --card-id F-001 --actor implementer-a`, which returns it to `active`. `work start` will not do it — it refuses because the lease is still held.

For a `joint_integration` plan, individual starts are refused. Allocate the
complete joint set atomically:

```bash
change-harness work start-batch \
  --card-id F-001 --card-id F-002 \
  --actor-principal-id principal-a --actor-session-id session-a
```

---

## 6. Run the feature gate

**Reserve first, then run.** `gate run` alone is refused.

```bash
change-harness gate reserve --card-id F-001 --gate-id gate.unit --actor implementer-a
change-harness gate run --card-id F-001 --gate-id gate.unit \
  --reservation-id VR-000001 --actor implementer-a
```

> **Known bug ([#146](https://github.com/artana-bio/solo-dev/issues/146)):** if a reservation was already settled, `gate reserve` exits **0** and prints `settled: validation reservation VR-000001` — the *old* id, granting nothing. Check the line says `reserved:`, not `settled:`.

**Do not try to run your integration-stage gate here.** It refuses, correctly: final-integration gates run automatically during `integration verify`, against the landing commit.

---

## 7. Hand off the exact commit

```bash
change-harness handoff example > decl.yaml
```

Set `delivered_sha` to `git rev-parse HEAD` **in the allocated worktree**, and fill every list honestly — empty ones are refused, deliberately, because a reviewer cannot tell an empty field from an unconsidered one.

```bash
change-harness handoff create --card-id F-001 --declaration decl.yaml --actor implementer-a
```

---

## 8. Review — a different actor

```bash
change-harness review begin --card-id F-001 --actor reviewer-b
change-harness review example > verdict.yaml
```

Fill it in, and **actually do the mutation you write down**: break the thing the test claims to check and confirm the test fails. That field is the review's evidence, and it is required.

```yaml
reviewer_actor_id: reviewer-b
decision: approved
findings: []
gate_adequacy:
  gates_observe_acceptance: true
  unobserved_behaviors: []
  basis: ran gate.unit and mutated farewell to confirm the test observes it
  mutation_evidence:
    status: demonstrated
    mutation: made farewell() return "Hello, {name}!" instead of "Goodbye"
    failing_test: T2.test_farewell
    oracle: gate.unit
```

```bash
change-harness review record --card-id F-001 --verdict verdict.yaml --actor reviewer-b
```

`--actor` must match `reviewer_actor_id`, and it must differ from the implementer.

---

## 9. Integrate and promote

```bash
change-harness integration prepare    --cycle-id C-001 --actor-id coordinator   # -> INT-001
change-harness integration preflight  --integration-id INT-001
change-harness integration merge      --integration-id INT-001 --actor-id coordinator
change-harness integration land       --integration-id INT-001 --actor-id coordinator
change-harness integration verify     --integration-id INT-001 --actor-id verifier
change-harness integration review     --integration-id INT-001 \
  --reviewer-actor-id int-reviewer --invariant-holds "greet() still returns Hello" --residual-risk "none"
change-harness acceptance record      --integration-id INT-001 \
  --authorizer-actor-id owner --accept --rollback-reference "revert the landing commit"
change-harness integration promote    --integration-id INT-001 --actor-id promoter
```

> **The actor flag is named differently on almost every command** — `--actor`, `--actor-id`, `--reviewer-actor-id`, `--authorizer-actor-id`. And `acceptance record` takes `--accept`, not `--decision`. This cost me three exit-2s. When in doubt: `<command> --help`.

`land` builds the landing commit **without moving your branch**. `verify` runs the integration gates against it. Only `promote` moves the protected branch.

---

## When you get stuck

```bash
change-harness project status    # leases, stranded reservations, unresolved operations
change-harness project recover   # names the exact command that clears an interruption
```

If a gate run is interrupted, `project recover` prints the literal `gate settle --reservation-id … --outcome abandoned` you need. Two independent testers resolved this unaided.

---

## Two things not to rely on yet

**Final-authorization policy ([#177](https://github.com/artana-bio/solo-dev/issues/177)).** A sealed cycle now prepares as the final integration by default, so acceptance requires the configured final-authorizer policy. The only non-final sealed-cycle compatibility path requires the exact `--legacy-migration-provenance legacy_cycle_plan_v1` marker; it is not a general bypass. New projects without an authorizer remain explicitly `migration_required` and refuse sealed-cycle acceptance until the policy is installed.

**`--dry-run` as a safety check ([#189](https://github.com/artana-bio/solo-dev/issues/189)).** Review recording now runs the same candidate, attestation, mutation-evidence, and receipt validation sequence as the real command and persists nothing. Other dry-run commands may still have command-specific limitations; use the real command's structured result for authority.

---

## What you have at the end

```
$ git log --oneline -2
7c41a86 Land INT-001 (1 card, individual)
1890424 integrate F-001 into INT-001
```

And in the control repository: the card's frozen contract, the receipt proving `gate.unit` ran against that exact commit in a clean tree, the handoff binding the reviewed SHA, the reviewer's identity, and the mutation they used to prove the test could fail.

Run `change-harness audit probes --output json` for the required negative
assurance checks. Record executable mutation evidence with `mutation create`,
then use `audit report --cycle-id <cycle-id> --output json`; unsupported claims
remain explicitly `not_tested`.

That last one is the difference between "a test passed" and "a test could have caught this."
