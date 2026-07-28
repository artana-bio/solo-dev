# Implementation packet — card F-001

This packet is the complete assigned context for this task. Do not look for a
broader specification, plan, or issue tracker. If information you need is
missing, report the gap in your final answer instead of assuming a value.

## Repository

| Field | Value |
| --- | --- |
| Repository | `TOY_REPO_PATH` |
| Base commit | `fc03bf893d16a27d4a1bbeae6b2e515a86c1897c` |
| Target branch | `card/F-001-temperature` |
| Branch already created | Yes, checked out at the base commit |

The repository is a small Python 3 project with no third-party dependencies.

## Card

| Field | Value |
| --- | --- |
| Card ID | `F-001` |
| Revision | 1 |
| Title | Implement temperature conversion |
| Risk | Low |
| Change kind | Feature |

### Goal

Implement the two conversion functions in `src/temperature.py` so the module
converts between Celsius and Fahrenheit and rejects invalid input.

### Non-goals

- Do not add a CLI, logging, or configuration.
- Do not add third-party dependencies.
- Do not implement Kelvin or any other scale.
- Do not modify currency, packaging, or unrelated modules.

## Write scope

Deny by default. You may modify only:

```text
include: src/temperature.py
exclude: tests/**
exclude: src/currency.py
exclude: gates.json
```

Modifying an excluded path fails this card. The test files are fixed inputs.
Do not edit, add, or delete tests.

## Acceptance behaviors

All five MUST hold in the delivered code:

1. `celsius_to_fahrenheit(celsius)` returns the Fahrenheit equivalent.
2. `fahrenheit_to_celsius(fahrenheit)` returns the Celsius equivalent.
3. Both functions return a `float` rounded to 2 decimal places.
4. Both functions raise `ValueError` when the input is below absolute zero.
   Absolute zero is -273.15 °C and -459.67 °F. The check applies to the unit
   of the argument each function accepts.
5. Both functions raise `TypeError` when the input is not an `int` or `float`.

## Acceptance regressions

- Existing tests in `tests/test_temperature.py` MUST pass unchanged.
- `src/currency.py` behavior MUST remain untouched.

## Named gates

Run from the repository root:

| Gate | argv |
| --- | --- |
| `gate.unit.temperature` | `python3 -m unittest -q tests.test_temperature` |

The gate MUST exit 0 before you report completion.

Note: the gate is the project's existing test suite. Passing it is necessary
but is not by itself proof that every acceptance behavior above is met.

## Commit instructions

- Commit to `card/F-001-temperature` only.
- Use one commit with the subject `feat: implement temperature conversion`.
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
