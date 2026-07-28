# SPIKE-001 report — disposable end-to-end walking skeleton

## 1. Disposition summary

| Field | Value |
| --- | --- |
| Outcome | All seven hypotheses `PASS`. Not yet `DONE`: acceptance-owner approval is outstanding. |
| Recommendation | Preserve the design. Two required corrections, several simplifications. |
| Production implementation | `WP-100` stays `NOT_STARTED` until the acceptance owner approves this report |

The planned workflow survived contact with real agents. Fresh-context review
found a deliberately seeded acceptance omission that a green gate could not
see, exact-SHA binding invalidated superseded evidence correctly, two
independent candidates combined into one explainable landing commit, and
expected-old-SHA promotion both succeeded and then correctly refused a stale
replay without moving the authority ref.

One gap in the planned design was discovered that the hypotheses did not
anticipate and that no listed work package currently closes. See finding F-1.

## 2. Execution record

| Field | Value |
| --- | --- |
| Start | 2026-07-28 14:47:34 -0700 (first toy commit) |
| Finish | 2026-07-28 15:16:29 -0700 |
| Elapsed | 29 minutes |
| Active engineering hours | Under 1, against a 16-hour budget |
| Prototype size | 219 non-generated executable lines, against a 300-line budget |
| Prototype branch | `spike/SPIKE-001-walking-skeleton` |
| Prototype head | `1bb3fc81195c43c27fc01488e3efa7a2bc3ae377` |
| Archive ref | `refs/archive/spikes/SPIKE-001` → `1bb3fc81195c43c27fc01488e3efa7a2bc3ae377` |
| Prototype on `main` | None. Verified with `git ls-tree -r --name-only main`. |

The prototype is Python, not Rust, on purpose: D-002 claims the engine is
language-neutral, so the walking skeleton drove a non-Rust toy project through
a non-Rust driver. Nothing about the workflow required the harness and the
target repository to share a language.

### Toy object identities

| Object | SHA |
| --- | --- |
| Toy baseline | `fc03bf893d16a27d4a1bbeae6b2e515a86c1897c` |
| F-001 delivered by implementer | `55de96d37e4cf3f19580c50e734c44a9a382e8a0` |
| F-001 candidate v1, omission injected | `813eaebba44ccb84cc94601ff3a193f5232993f8` |
| F-001 candidate v2, corrected and approved | `1bba43daff4df3760c68258f940c91feeff17b1a` |
| F-002 candidate v1 | `778a0155a23cdc0000e956a2aef3f7bd954d3197` |
| F-002 candidate v2, corrected and approved | `0dbb1c69633ce813ed934b55b7db86b85abbc1c2` |
| Integration head | `b80b8d6d228aa7cb15604ebba4aa171897355a17` |
| Landing commit | `125a3dce02656be584b0be27c90e3bd978617706` |
| Landing tree | `16d7b9e6c52b656be74bcc9d51842cf586375e10` |
| Authority `main` before promotion | `fc03bf893d16a27d4a1bbeae6b2e515a86c1897c` |
| Authority `main` after promotion | `125a3dce02656be584b0be27c90e3bd978617706` |

### Actors

| Actor | Role |
| --- | --- |
| `implementer-session-1` | F-001 implementation and correction |
| `implementer-session-2` | F-002 implementation and correction |
| `reviewer-session-A` | F-001 candidate v1 review |
| `reviewer-session-B` | F-001 candidate v2 review |
| `reviewer-session-C` | F-002 candidate v1 and v2 review |
| Coordinator | Packet authoring, omission injection, landing, promotion |

Each actor ran in a separate agent session with no inherited implementation
conversation. Packets were delivered directly and were the sole context, per
the `AGENTS.md` spike exception added in plan revision 3.

### Packets

Reproduced verbatim in `docs/spikes/SPIKE-001-PACKET-F-001.md` and
`docs/spikes/SPIKE-001-PACKET-F-002.md`. Review packets are reproduced in the
evidence trail on the archived spike branch.

## 3. Hypothesis results

### H-01 — a bounded packet is sufficient for implementation: `PASS`, qualified

Both implementers completed their assigned candidate from the packet alone.
Neither blocked, neither asked a question before delivering, and neither
modified an excluded path. Both ran the named gate and reported exit 0.

The qualification matters. The pass condition reads "zero
requirement-clarification messages," and neither implementer returned the
`clarifications_needed: none` the packet asked for. Both instead completed the
work, chose a documented default, and recorded the ambiguity:

| Actor | Ambiguities reported |
| --- | --- |
| `implementer-session-1` | `bool` is an `int` subclass, so acceptance behavior 5 is ambiguous; rounding mode unstated; non-finite float handling unstated |
| `implementer-session-2` | non-int `places` exception class; malformed-string exception class; `bool` for both parameters |

These are real defects in the packet, not agent confusion. Both implementers
independently flagged the `bool`-as-`int`-subclass problem, which is genuinely
underspecified by "not an `int` or `float`."

The finding is that "zero clarification messages" is the wrong metric. It
conflates a blocking question, which costs a round trip, with a recorded
assumption, which costs nothing and improves the audit trail. The behavior
observed here is the desired behavior. See recommendation R-1.

### H-02 — fresh-context review adds semantic value: `PASS`

`reviewer-session-A` identified the seeded omission as its first finding,
severity critical, without being told its location, nature, or existence, and
recorded `changes_requested`.

The seeded omission was the absolute-zero guard removed from
`fahrenheit_to_celsius` only, leaving `celsius_to_fahrenheit` intact and
leaving the Fahrenheit docstring still claiming the check. The gate passed 8/8
throughout.

The reviewer went well beyond the planted defect:

- it identified the dead `ABSOLUTE_ZERO_FAHRENHEIT` constant as the static tell;
- it flagged the docstring asserting behavior the code lacked;
- unprompted, it mutation-tested the gate and proved the defective revision
  passes 8/8, establishing that a green receipt is not evidence for acceptance
  behavior 4;
- it observed that the card's own write scope excludes `tests/**`, so the
  implementer could not have closed that gap, and correctly assigned the defect
  to the card's scope definition rather than to the implementer;
- it flagged that the implementer's own declaration asserted a behavior the
  code did not have.

`reviewer-session-C` independently did the same class of work on F-002 without
any seeded defect, mutation-testing two wrong implementations that both pass
the gate 6/6.

**Important caveat on how the omission arose.** The spike design assumed an
implementer coding against a gate would naturally skip the untested acceptance
behavior. That did not happen. Both implementers read the acceptance list and
implemented every behavior correctly, including the untested one. The
coordinator therefore injected the omission by amending
`55de96d` into `813eaeb`, deleting exactly the five-line guard. H-02 tested a
coordinator-injected defect, not a naturally occurring one. The
hypothesis is still satisfied — the reviewer had no knowledge of the injection
— but the stronger claim, that agents under-implement untested acceptance
criteria, was **not** observed and is contradicted by this sample.

### H-03 — evidence is bound to exact code: `PASS`

After the correction moved the branch from `813eaeb` to `1bba43d`, both the
prior handoff and the prior review were rejected as stale, exit code 6:

```text
STALE handoff-F-001-v1.json
  - candidate SHA changed: 813eaeb... -> 1bba43d...
STALE review-F-001-v1.json
  - candidate SHA changed: 813eaeb... -> 1bba43d...
```

The check discriminates rather than blanket-failing: F-002's untouched
evidence verified `CURRENT` in the same run. Card-digest invalidation was
tested separately by bumping the card to revision 2, which produced a distinct
digest and a second staleness reason on the same record.

Self-review separation was also verified: a review recorded with reviewer and
feature actor equal was rejected before any record was written. This is
mandatory scenario 22.

### H-04 — corrected work can be independently approved: `PASS`

`reviewer-session-B`, a different session from both the implementer and
`reviewer-session-A`, approved candidate `1bba43d` with an empty findings list
after dispositioning each prior finding individually. F-002 followed the same
shape through `reviewer-session-C`, which approved `0dbb1c6` after running a
differential harness over 34 amounts and 14 `places` values and classifying
zero regressions.

Both corrections were new commits, not amendments, preserving the review trail
on the branch.

### H-05 — candidate combination is understandable: `PASS`

The landing commit was verified against plain git, independently of the
prototype that produced it:

| Property | Verified |
| --- | --- |
| First parent equals expected authority baseline | `fc03bf89...` ✅ |
| Second parent equals integration head | `b80b8d6d...` ✅ |
| Landing tree equals verified integration tree | `16d7b9e6...` both ✅ |
| Both approved candidates reachable | ✅ |
| Not reachable from authority `main` before acceptance | ✅ |
| Both cards' work present in the landing tree | ✅ |
| `gate.unit.all` on the integration worktree | exit 0, 14 tests ✅ |

The integration worktree was disposable, created detached at the authority
baseline and removed afterward. It was clean before and after the gate.

### H-06 — bare-authority promotion is safe: `PASS`

```text
PROMOTED
  main: fc03bf89... -> 125a3dce...

PROMOTION REJECTED
  expected main: fc03bf89...
  actual main:   125a3dce...
  authority unchanged
  (exit 6)
```

The compare-and-swap is `git update-ref refs/heads/main <new> <old>` against
the bare authority, which is atomic and refuses on mismatch. Objects were
transferred first to a temporary `refs/harness/incoming/INT-001` ref so that
the transfer could not touch the protected branch, and that ref was deleted
afterward. The authority ended with exactly one ref and no residue.

### H-07 — context recovery is practical: `PASS`

A fresh agent given only the original packet, the repository, and the evidence
directory correctly determined the current candidate SHA, reconstructed both
review rounds and which records were current versus superseded, verified all
five acceptance behaviors by direct execution, identified the correct next
action and its owner, and reported `blocked_on: none`.

It exceeded the bar in two ways. It independently recomputed the gate
receipt's stdout and stderr SHA-256 digests and matched them byte-for-byte,
corroborating the receipt rather than trusting it. And it noticed that
`bool` acceptance was flagged on F-002 but not on F-001, catching a review
consistency problem across sessions that neither reviewer saw.

It reported four honest information gaps, all of which are real plan gaps.
See findings F-2 and F-3.

## 4. Findings

### F-1 — the delivered SHA is never bound to the reviewed SHA

**Severity: high. No current work package closes this.**

`implementer-session-1` detected the coordinator's injection unprompted. Its
report stated that the reviewed candidate `813eaeb` was not the commit it
produced, that its commit was `55de96d`, and that the reflog showed an external
`commit (amend)` roughly 90 seconds later which deleted exactly the five-line
guard. It was correct in every particular.

This is the most valuable result of the spike, and it was produced by the
system detecting an attack on itself.

The plan binds evidence to exact SHAs from the moment a handoff exists.
`WP-250` invalidates a handoff when the head changes afterward. But nothing
records what the feature actor claims it delivered. Between the actor
finishing and the harness computing a handoff, the branch can move, and the
resulting handoff will be internally consistent and completely wrong about
what was reviewed.

The implementer's own proposed fix is the right one: record the
actor-declared candidate SHA at delivery and compare it when the handoff is
created. A mismatch is a policy failure, not a merge conflict.

This does not require a new trust model. It closes a gap between two things
the plan already treats as authoritative.

### F-2 — evidence records are not self-describing

The H-07 agent could not recompute `card_digest`, `handoff_digest`, or
`gate_digest`, because neither the canonicalization algorithm nor the card
record itself was present in the evidence it was given. It verified those
fields only for mutual consistency across records.

Section 10.1 requires "one documented canonical JSON algorithm," but
documenting it in the plan does not help a reader holding only the records. An
auditor with the control repository and Git objects must be able to
independently recompute every digest. The gate receipt was the one field it
could corroborate, and it did, which shows the difference clearly.

### F-3 — nothing consumes an approved review

The H-07 agent reported: "no record documents what consumes an `approved`
review to trigger the merge," and separately could not determine whether cards
land individually or as a batch.

Section 11.4 has `integration prepare` requiring all selected cards be
`approved`, but selection is unmodeled. There is no queue, no
ready-to-integrate view, and no way for an actor to discover that a card is
waiting. In this spike the coordinator held that state in its head, which does
not survive the context reset that H-07 exists to test.

### F-4 — the review schema cannot express a finding's disposition

Section 10.8 gives a review `findings[]` and `residual_risks[]`, with no way to
mark a finding resolved, accepted as residual risk, or out of scope. Every
review round in this spike needed exactly that.

The packets had to invent a `prior_findings_disposition` field for the
re-review rounds, and it proved essential — `reviewer-session-B` used it to
correctly report that prior finding 3 was `not_resolved` and *not resolvable*
within the card's write scope, which a binary approve/reject would have erased.
`reviewer-session-B` also caught the implementer misnumbering which finding it
had fixed, and noted precisely why it mattered: "a ledger that records finding
3 as resolved would overstate the evidence backing behavior 4."

Section 15.1 also does not describe re-review at all. It describes a first
review only.

### F-5 — gate adequacy is the highest-yield review activity and is not required

Both reviewers mutation-tested the gates without being asked. Reviewer A proved
the defective F-001 revision passes 8/8. Reviewer C proved that an
implementation ignoring `places` entirely, and one calling `.normalize()`,
both pass F-002's suite 6/6.

Evaluation criterion 6 in Section 15.1 asks whether "tests could pass while
behavior remains wrong." In practice this was the criterion that found real
defects. It deserves to be a required, recorded output rather than one item in
a list of ten.

### F-6 — review outcomes are not comparable across reviewers

`bool` accepted as an `int` subclass appears identically in both cards.
`reviewer-session-C` flagged it as a low finding on F-002. `reviewer-session-B`
did not flag it on F-001. Both approvals are defensible; neither is wrong.

Section 15.3 sets a minimum number of reviewers by risk but says nothing about
making outcomes comparable. For a low-risk card this is noise. For a card
where one reviewer's low finding is another's blocker, it is not.

### F-7 — artifact volume is high for small cards

Two single-function cards produced 13 control records plus 2 packets, across
17 prototype invocations. Every record was used and none was ceremonial, but
the ratio is worth watching. If a real card produces 13 records, a 20-card
cycle produces 260, and Section 14.3's retention rules become the operative
constraint on the control repository rather than an afterthought.

No ceremony was observed to be redundant or unsafe. The loop that ran twice —
handoff, review, changes requested, correction, new handoff, new review,
approval — was the most valuable part of the exercise and should not be
simplified.

## 5. Contracts that changed or need to change

| Section | Change |
| --- | --- |
| 10.7 Handoff | Add an actor-declared `delivered_sha`, compared at handoff creation. F-1. |
| 10.6 Receipt | Confirmed sufficient. Digests were independently reproducible. |
| 10.8 Review | Add per-finding `disposition` and a `supersedes` link to the prior review. Add a required gate-adequacy assessment. F-4, F-5. |
| 10.1 Canonicalization | Records must name their canonicalization algorithm and carry or reference the card, so digests are independently recomputable. F-2. |
| 11.2 Card states | `approved` needs a discoverable ready-to-integrate view. F-3. |
| 12.2 Exit codes | Exit 6 served both stale evidence and stale promotion cleanly. No change. |
| 15.1 Independent review | Add re-review. Currently describes only a first review. F-4. |
| 15.3 Risk policy | Add guidance on cross-reviewer comparability. F-6. |

Nothing in Section 7 was contradicted. Invariant 7.1.7, that no branch name is
accepted as proof of code identity, was directly vindicated by F-1.

## 6. Recommendations

- **R-1.** Redefine the H-01-style success metric as zero *blocking*
  clarifications with all assumptions recorded, rather than zero clarification
  messages. The observed behavior — complete, choose a documented default,
  record the ambiguity — is what the workflow should want.
- **R-2.** Close F-1 in `WP-250`, or in `WP-230` if the delivered SHA is
  recorded at checkpoint time. This is a required correction, not an
  enhancement.
- **R-3.** Make gate adequacy a required review output. It is cheap and it
  found real defects three times out of three.
- **R-4.** Keep the correction loop exactly as designed. It is the part that
  worked best.
- **R-5.** Do not expand schemas beyond the changes in Section 5 above. The
  spike validated the existing shape; the risk now is over-correction.

## 7. Acceptance checklist

| Requirement | Status |
| --- | --- |
| All seven hypotheses pass | ✅ |
| Report contains every required field | ✅ |
| No prototype implementation on `main` | ✅ verified |
| Archive ref resolves to recorded prototype head | ✅ `1bb3fc8` |
| Sections 9–15 updated from observed evidence | ✅ plan revision 4 |
| Acceptance owner approves the report | ⬜ **outstanding**, Alvaro Alvarez |
| `WP-100` changed to `READY` | ⬜ blocked on the line above |

Two criteria remain. The spike is not `DONE` and `WP-100` is not `READY`.
Section 2 makes `DONE` binary, and Section 17 lists acceptance-owner approval
as an acceptance criterion, so the coordinator cannot close this out. Section
6.1 keeps production implementation blocked until the report is accepted.

To accept: confirm the hypothesis evidence in Section 3 and the required
corrections in Section 5, then set `SPIKE-001` to `DONE` and `WP-100` to
`READY` per the Section 25 completion procedure.
