# Implementation packet — card F-002

This packet is the complete assigned context for this task. Do not look for a
broader specification, plan, or issue tracker. If information you need is
missing, report the gap in your final answer instead of assuming a value.

## Repository

| Field | Value |
| --- | --- |
| Repository | `TOY_REPO_PATH` |
| Base commit | `fc03bf893d16a27d4a1bbeae6b2e515a86c1897c` |
| Target branch | `card/F-002-currency` |
| Branch already created | Yes, checked out at the base commit |

The repository is a small Python 3 project with no third-party dependencies.

## Card

| Field | Value |
| --- | --- |
| Card ID | `F-002` |
| Revision | 1 |
| Title | Implement money rounding |
| Risk | Low |
| Change kind | Feature |

### Goal

Implement `round_half_up` in `src/currency.py` so monetary amounts round half
away from zero rather than using Python's default banker's rounding.

### Non-goals

- Do not add a CLI, logging, or configuration.
- Do not add third-party dependencies.
- Do not implement currency symbols, formatting, or exchange rates.
- Do not modify temperature, packaging, or unrelated modules.

## Write scope

Deny by default. You may modify only:

```text
include: src/currency.py
exclude: tests/**
exclude: src/temperature.py
exclude: gates.json
```

Modifying an excluded path fails this card. The test files are fixed inputs.
Do not edit, add, or delete tests.

## Acceptance behaviors

All five MUST hold in the delivered code:

1. `round_half_up(amount, places=2)` returns a `decimal.Decimal`.
2. Rounding is half away from zero, so `"2.345"` becomes `2.35` and `"-2.345"`
   becomes `-2.35`.
3. `places` defaults to 2 and the result is quantized to exactly that many
   decimal places.
4. `amount` accepts `str`, `int`, and `decimal.Decimal`. A `float` argument
   raises `TypeError`, because binary floats cannot represent decimal money
   exactly.
5. A negative `places` raises `ValueError`.

## Acceptance regressions

- Existing tests in `tests/test_currency.py` MUST pass unchanged.
- `src/temperature.py` behavior MUST remain untouched.

## Named gates

Run from the repository root:

| Gate | argv |
| --- | --- |
| `gate.unit.currency` | `python3 -m unittest -q tests.test_currency` |

The gate MUST exit 0 before you report completion.

Note: the gate is the project's existing test suite. Passing it is necessary
but is not by itself proof that every acceptance behavior above is met.

## Commit instructions

- Commit to `card/F-002-currency` only.
- Use one commit with the subject `feat: implement money rounding`.
- Do not merge, rebase, tag, or push.
- Leave the working tree clean.

## Required report

Return exactly these fields:

- `candidate_sha`: the exact commit SHA you produced;
- `behavior_delivered`: what the code now does;
- `implementation_decisions`: choices you made and why;
- `assumptions`: anything you inferred rather than were told;
- `known_limitations`: what you did not do;
- `residual_risks`: what could still be wrong;
- `gate_result`: the gate name, exact command, and exit code;
- `clarifications_needed`: information this packet did not give you, or the
  exact string `none`.
