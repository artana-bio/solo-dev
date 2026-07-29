# dependency-shas-never-bound-to-a-review  [high]  fix_accepted=False

## Summary
Neither `HandoffRecord` nor `ReviewRecord` carries any dependency SHA, so an approval survives its dependency being re-reviewed at a different commit — Section 10.7's required `dependency SHAs` field does not exist and invariant 7.3.6 / Section 15.2's `required dependency SHA` invalidation trigger is unimplemented.

## Root cause
Absence, in four places, all verified by reading the code and confirmed by the probes.

1. `/Users/alvaro/Documents/Code/change-harness/src/domain/handoff.rs:125-165` — `HandoffRecord` has no dependency field. `docs/IMPLEMENTATION_PLAN.md:656` lists `- dependency SHAs;` as a required machine-computed handoff field. It was never added.

2. `/Users/alvaro/Documents/Code/change-harness/src/commands/handoff.rs:392-411` — `run_create` builds the record from `record.base_sha`, `record.cycle_id`, `record.named_gates.feature`; it never reads `record.depends_on`. `depends_on` is loaded (it is a field of the `CardRecord` at `src/domain/card.rs:303`) and dropped on the floor.

3. `/Users/alvaro/Documents/Code/change-harness/src/domain/review.rs:219-239` — `is_current_for` and `staleness` compare exactly two things, `candidate_sha` and `card_digest`. The docstring at lines 215-217 says "Section 15.2: approval becomes invalid when the candidate SHA or the card digest changes. Both are checked, because either alone would let a stale approval through." Section 15.2 (`docs/IMPLEMENTATION_PLAN.md:1136-1143`) actually lists seven triggers; `required dependency SHA` is the fourth. The docstring narrows the specification to the subset that was implemented, which is why the gap reads as closed. `HandoffRecord::staleness` at `src/domain/handoff.rs:199-216` has the identical two-way shape.

4. `/Users/alvaro/Documents/Code/change-harness/src/commands/integration.rs:517-547` — `check_dependencies` is the only code in the repository that consults `depends_on` for admission, and it asks a pure identity question: is the dependency in the selection, or is its card state `Landed`/`Closed`. It never asks which commit of the dependency the dependent was reviewed against. `assess_candidacies` at lines 294-320 computes `blocked_by` from candidate SHA and card digest only.

Secondary, same area: `/Users/alvaro/Documents/Code/change-harness/src/commands/handoff.rs:399` reads `baseline_sha: cycle.baseline_sha.clone().unwrap_or(baseline)`. `baseline` is the resolved card `base_sha` returned by `derive_facts` (line 299), and it is the value `commits` and `changed_paths` were diffed from. Because an activated cycle always has a frozen baseline, the `unwrap_or` fallback is dead in the normal path and the derived value is always discarded — so `baseline_sha` can name a commit that is not the base the rest of the record was computed against. Probe 3 shows this concretely.

## Files
src/domain/handoff.rs, src/domain/review.rs, src/commands/handoff.rs, src/commands/review.rs, src/commands/integration.rs, src/error.rs, tests/review.rs, tests/integration_plan.rs, docs/DEFECT-REGISTER.md

## Proposed fix
RESOLUTION RULE (decide this first; everything else follows). A dependency binds to **the candidate SHA of that dependency card's most recent `approved` review**, or `None` when it has none. Not the landing commit, not the branch head, not the card digest. Landing a dependency changes the commit an operator would name while changing nothing a reviewer looked at; probe 4 proves the two values differ. Re-review at a new candidate is exactly when the reviewed content changed.

1. `src/domain/handoff.rs`
```rust
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DependencyBinding {
    pub card_id: CardId,
    /// The commit this dependency was last independently approved at.
    /// `None` when it has no approval yet; see the guard test.
    pub approved_candidate_sha: Option<String>,
}
```
Add `pub dependency_shas: Vec<DependencyBinding>` to `HandoffRecord` (after `commits`), sorted by `card_id` so the digest is a function of the card, not of the author's `depends_on` order. No `#[serde(default)]`: `deny_unknown_fields` plus a required field makes pre-existing on-disk handoffs fail loudly with `InternalControlCorrupt` rather than silently deserialize as "this card had no dependencies", which is the defect itself. Bump `HANDOFF_SCHEMA` to `harness.handoff/v2` and note in the register that the self-hosted control repo needs its records regenerated. No digest vector is pinned for handoffs or reviews (only `src/domain/card.rs:973`), so adding the field breaks nothing else.

2. Force the call sites. Change `HandoffRecord::{is_current_for, staleness}` and `ReviewRecord::{is_current_for, staleness}` to take one struct instead of two scalars:
```rust
pub struct CurrentBindings<'a> {
    pub candidate_sha: &'a str,
    pub card_digest: &'a Digest,
    pub dependencies: &'a [DependencyBinding],
}
```
A new *method* would repeat defect 22 (`requires_human_review`: modelled, unit-tested, called from nowhere). A changed signature makes the compiler enumerate every site: `src/commands/handoff.rs:474`, `src/commands/review.rs:395,556,572`, `src/commands/review.rs:234` (`current_approval`), `src/commands/integration.rs:305-306`.
`staleness` gains a third comparison, checked after candidate and card digest:
```
"review approved this candidate against F-001 at <old>, but F-001 is now approved at <new>; the dependency was re-reviewed after this approval"
```
with `<old>`/`<new>` rendered as `none` when the option is empty, and a distinct message when the dependency set itself differs (a card revision changed `depends_on`).

3. `src/commands/review.rs` — add
```rust
pub fn resolve_dependencies(
    control: &ControlRepository,
    depends_on: &[CardId],
) -> Result<Vec<DependencyBinding>, HarnessError>
```
For each id: `reviews_for(control, id)?.into_iter().rfind(|r| r.decision == Decision::Approved).map(|r| r.candidate_sha)`. `reviews_for` returns oldest-first, so `rfind` is the latest. Deliberately does **not** filter by the dependency's *current* card digest: if the dependency card was revised and its approval voided, `check_dependencies` already refuses because its state left `Approved`. Sort by `card_id`; dedup (the card validator at `src/domain/card.rs:435` rejects only self-dependency, not duplicates).

4. `src/commands/handoff.rs` — `run_create` calls `resolve_dependencies(control, &record.depends_on)?` and stores it. It does **not** refuse on `None`; see the overcorrection guard. Separately fix line 399: keep `baseline_sha` as the cycle baseline (integration depends on it) and add `pub base_sha: String` carrying the derived `baseline` from `derive_facts`, so the record states the base its own `commits` and `changed_paths` were computed from.

5. `src/commands/review.rs` — `run_record` copies `handoff.dependency_shas` onto the review, exactly as it already copies `baseline_sha` and `candidate_sha` from the handoff at lines 440-441, so a review is readable in isolation. `require_current_handoff` (line 385) resolves current bindings and passes them.

6. `src/commands/integration.rs` — `assess_candidacies` resolves bindings per card and passes them into `current_approval` and `handoff.staleness`, producing a `blocked_by` naming the dependency and both commits. `check_dependencies` additionally refuses when a selected member's recorded binding for a dependency disagrees with that dependency's approval inside the same selection.

7. `src/error.rs` — new `PolicyDependencyEvidenceStale` → `CH-POLICY-DEPENDENCY-EVIDENCE-STALE`. Extend `ALL` (`78` → `79`, line 199), add a `Policy` category arm and a `recovery()` sentence ending in `.`; the tests at lines 894-928 iterate `ALL` and enforce both.

FAILING-FIRST TEST — `tests/review.rs::a_dependency_re_approved_at_a_new_commit_invalidates_its_dependents_review`. Body is probe (2) verbatim, with the assertions inverted:
```rust
assert_ne!(first, second);
let inspect = workspace.review_json(&["inspect", "--card-id", "F-002"]);
assert_eq!(inspect["data"]["has_current_approval"], false);
let reason = /* stale reason for F-002's approval */;
assert!(reason.contains("F-001"), "must name the dependency: {reason}");
assert!(reason.contains(&first) && reason.contains(&second), "must name both commits: {reason}");
let refused = workspace.integration_raw(&["prepare","--cycle-id","C-001","--actor-id","coordinator"]);
assert_eq!(refused.status.code(), Some(5));
assert_eq!(error_code(&refused), "CH-POLICY-DEPENDENCY-EVIDENCE-STALE");
```
Against today's code this fails at the first assertion (probe measured `true`) and again at the exit code (probe measured a successful `prepare` listing both members).

MUTATIONS THAT MUST FAIL IT, run in this order:
- Make the new dependency branch of `ReviewRecord::staleness` `return None`. The test must fail at `has_current_approval == false`, not at the error-code line — if it fails only at the error code, the invalidation is living entirely in `check_dependencies` and the review record is still lying.
- Make `resolve_dependencies` return the dependency's *landing* commit. Guard G1 below must fail.
- Change `resolve_dependencies` to bind every card in the cycle rather than `depends_on`. Guard G2 must fail.

FIXTURE REPAIR REQUIRED. Under the new rule a dependent approved *before* its dependency binds `None` and goes stale the moment the dependency is approved — correct under invariant 7.3.6, but two fixtures encode the opposite order and will break: `tests/integration_plan.rs:158` and `tests/integration_plan.rs:186` both call `approve_card("F-002")` before `approve_card("F-001")`. Swapping the two lines keeps `dependencies_are_merged_before_their_dependents` green (verified). While there, fix the same test's real weakness: see overcorrection_risk.

## Over-correction risk
Three ways a too-aggressive fix refuses everything, each with the guard test that holds it open. All three are new tests in `tests/review.rs`.

G1 — binding to the landing commit instead of the approved candidate. `landing_a_dependency_does_not_invalidate_its_dependent`: F-001 and F-002 (`depends_on: [F-001]`) both approved; F-001 alone is prepared, merged, landed, verified, reviewed, accepted and promoted; F-002's approval must still be current and F-002 must still appear in `integration ready`. This is the single most likely wrong fix, because "which commit is the dependency at" reads as "what is on main". I built exactly this scenario in `probe_landing_a_dependency_does_not_move_its_approved_candidate_sha` and measured the two values diverging (approval 7b86895e…, authority head cc66809d…), and `integration ready` still listing F-002, so the guard has teeth today: a landing-commit fix flips it to failing. Without G1, every dependent's evidence is voided at the exact moment nothing about the dependency changed, and no multi-card cycle can ever land in two batches.

G2 — binding more than `depends_on` (every card in the cycle, or transitive closure). `an_undeclared_card_moving_does_not_invalidate_a_review`: three cards, F-002 declares `depends_on: [F-001]` only; F-003 is revoked and re-approved at a new commit; F-002's approval must stay current and `integration prepare` must still succeed. Without it the cycle collapses into a serial queue where any card moving re-reviews all the others.

G3 — refusing `handoff create` when a dependency has no approval yet. `a_dependent_card_may_be_handed_off_before_its_dependency_is_approved`: F-002 depends on F-001; F-001 is activated and never worked; `handoff create` for F-002 must exit 0 and its record must contain `{"card_id":"F-001","approved_candidate_sha":null}`. Recording `None` is the whole point — it is a fact about what the reviewer saw, not an error. Refusing here would forbid a dependent card from ever reaching handoff until its dependency finished, serializing precisely the parallel work the harness exists to coordinate.

And the opposite of the opposite — a fix that refuses nothing. Recording `dependency_shas` while no `staleness` path consults it is a cosmetic fix that satisfies Section 10.7 and leaves invariant 7.3.6 exactly as unimplemented as it is today. The first listed mutation is what catches that, and it must fail at `has_current_approval`, not at the error code.

Separate finding that belongs in the register, discovered while checking existing coverage. `tests/integration_plan.rs:158 dependencies_are_merged_before_their_dependents` does not test what its name says, and neither does anything else. Its comment claims F-002 "must merge second despite sorting first among the ready set only by identifier" — but `topological_order` (`src/domain/integration.rs:315`) breaks ties by ascending card identifier, and F-001 < F-002, so the dependency already sorts first and the right answer comes out for the wrong reason. I mutated `src/commands/integration.rs:614` to pass `Vec::new()` in place of `candidacy.record.depends_on.clone()`, deleting dependency ordering from the merge plan entirely, and ran the whole suite: **all 25 test binaries passed, 0 failures, exit 0.** Nothing in 801 tests observes it. Repairing the fixture so the dependent sorts first — activate F-002, make F-001 `depends_on: [F-002]`, assert `merge_order == ["F-002","F-001"]` — is a two-line change; I verified it passes against real code and fails against the mutation with `left: ["F-001","F-002"], right: ["F-002","F-001"]` at `tests/integration_plan.rs:182`.

## VERIFIER objections
THE DEFECT IS REAL — I reproduced it independently, from scratch, in my own worktree.

`/Users/alvaro/Documents/Code/change-harness/.claude/worktrees/wf_81894ca4-835-11/tests/probe_depsha.rs` (run: `cargo test --test probe_depsha -- --nocapture --test-threads=1`). Measured, not quoted:
- F-002's handoff and review JSON contain no occurrence of `F-001` and no occurrence of F-001's approved candidate SHA. Both assertions that the strings are ABSENT pass.
- F-001 revoked, re-worked, re-gated, re-handed-off and re-approved: `--- F-001 was 80688a29…, is now 381baea2…` then `--- F-002 has_current_approval after dependency moved: true`, and `integration prepare` exits 0 planning both members.
Code reading confirms all four root-cause sites verbatim: `src/domain/handoff.rs:123-165` (no dependency field), `src/commands/handoff.rs:392-411` (`record.depends_on` never read), `src/domain/review.rs:219-239` (two comparisons; the docstring narrows Section 15.2's seven triggers to the two implemented), `src/commands/integration.rs:517-547` (`check_dependencies` asks identity, never which commit). `src/commands/handoff.rs:399` does discard the derived baseline: `cycle.baseline_sha` is `Some` for every activated cycle (`src/commands/cycle.rs:327`), so `unwrap_or(baseline)` is dead and `commits`/`changed_paths` are diffed from a base the record does not state.
The SECONDARY finding also reproduces, at full strength. I applied the stated mutation (`candidacy.record.depends_on.clone()` → `Vec::new()` at `src/commands/integration.rs:613`) and ran the ENTIRE suite: 31 binaries, every one `test result: ok`, 0 failures. Nothing in the suite observes dependency ordering in the merge plan.

THE PROPOSED FIX IS NOT CORRECT. Six objections, the first decisive.

1. THE FAILING-FIRST TEST CANNOT PASS AFTER THE PROPOSED FIX. This is the exact trap the brief warns about, and it is measurable today. `select` (`src/commands/integration.rs:486-495`) treats a blocked card as *not selected* rather than as a refusal when `--card-ids` is omitted: `assessed.iter().filter(|c| c.blocked_by.is_none())`. I built a cycle where F-002 is blocked in precisely the shape the fix would produce and ran `integration prepare --cycle-id C-001 --actor-id coordinator` with no `--card-ids` (`tests/probe_depsha2.rs`). Measured: `--- prepare exit: Some(0)`, `members` = `[F-001]` only, `"warnings": []`. So after the fix the proposed `assert_eq!(refused.status.code(), Some(5))` fails — prepare succeeds. `check_dependencies` cannot save it: it only walks `selected`, and F-002 is not in it. And even with `--card-ids F-001 F-002` the refusal that fires is `PolicyNotIntegrable` / `CH-POLICY-NOT-INTEGRABLE` raised at lines 507-510 from `blocked_by`, never the new `CH-POLICY-DEPENDENCY-EVIDENCE-STALE`, because the step-2 staleness change pre-empts the step-6 `check_dependencies` addition. The test's last two assertions are unreachable under its own fix.

2. THE FIX MAKES A SILENT SCOPE REDUCTION ROUTINE. Because of (1), the normal consequence of the fix is not a refusal but a batch that quietly ships fewer cards than the coordinator asked for, with an empty `warnings` array. Today that path is rare; the fix makes it the standard outcome of the ordinary "approve dependent, then approve dependency" sequence. Nothing in the proposal touches `select` or adds a warning.

3. G3 IS VACUOUS AND THE FIX REINSTATES THE SERIALIZATION IT CLAIMS TO PREVENT. G3 permits handing off a dependent before its dependency is approved and recording `None` — but under the stated resolution rule that handoff and any review on it die the instant the dependency is first approved (`None` ≠ the new SHA). The fix therefore forces every dependent to be handed off and reviewed *after* its dependency's final approval, which is the serialization G3's own rationale calls unacceptable, moved one stage later. The fixture repair the finding prescribes (swapping `approve_card` order at `tests/integration_plan.rs:171,197`) is the proof: the fix cannot tolerate the order those fixtures use.

4. THE SCHEMA CHANGE BREAKS EVERY EXISTING RECORD FOR NO BENEFIT, AGAINST THIS REPO'S OWN PRECEDENT. Nothing reads the `schema` string — `HANDOFF_SCHEMA` and `REVIEW_SCHEMA` are written at `src/commands/handoff.rs:393` and `src/commands/review.rs:434` and never compared anywhere — so bumping to `harness.handoff/v2` is cosmetic. What actually bites is the deliberate omission of `#[serde(default)]` on a required field under `deny_unknown_fields`: every handoff already on disk becomes undeserializable, so `handoff inspect`, `review begin/record`, `integration prepare` and `audit` all fail with `InternalControlCorrupt` on any historical card, and the proposed remedy ("the self-hosted control repo needs its records regenerated") means rewriting evidence records, which is the one thing this system exists to prevent. The repo's own most recent field addition — `human_reviewer`, from the defect-22 fix — used `#[serde(default)]` (`src/domain/review.rs:181`) and did not bump the schema string. Step 5 also adds the field to `ReviewRecord` (also `deny_unknown_fields`, `src/domain/review.rs:146`) while mentioning neither `REVIEW_SCHEMA` nor the same breakage.

5. THE RESOLUTION RULE IS NOT THE SPEC'S. Section 10.2 (`docs/IMPLEMENTATION_PLAN.md:510`) says "Dependent cards use exact accepted dependency SHAs declared in the card" — the binding is the card's declared `base_sha`, an immutable field of an activated card, not a snapshot synthesized at handoff time. Under the fix, a dependent whose `base_sha` is the cycle baseline (the default, and the only thing `activate_card_depending_on` produces) gets a recorded `dependency_shas` entry naming a commit whose content is nowhere in its diff — the record asserts a relationship the reviewer never saw, which is the same class of over-claim the register was written about. A check comparing `card.base_sha` against the dependency's currently-accepted candidate needs no schema change, no new required field, and is enforceable at `card activate` as well as at integration. It also covers the case with real content consequences, which the proposal never mentions: a dependent branched on a *superseded* dependency commit carries that commit into the merge alongside the dependency's new one.

6. `resolve_dependencies` HAS A HOLE. `reviews_for(...).rfind(|r| r.decision == Approved)` returns the last approval even when a *later* review on the same card recorded `changes_requested` — the approval is superseded but the binding still names it. It has neither `is_current_for` (deliberately dropped) nor a supersession check, so it can pin a dependency SHA whose approval no longer stands.

Minor: `ErrorCode::ALL` is indeed `[Self; 78]` at `src/error.rs:199` and the `recovery()`-ends-with-`.` and category-prefix tests at 894-928 do iterate it, so that part of the plumbing is accurate. The claim that `check_dependencies` is "the only code in the repository that consults `depends_on` for admission" overlooks `src/policy/allocation.rs:135-167`, the activation-time declaration and DAG check — the natural, cheaper enforcement point for objection 5.

## VERIFIER missed
1. THE DEFECT IS ALREADY IN THE REGISTER. `docs/DEFECT-REGISTER.md`, Tier 4, states verbatim: "Dependency SHAs are never bound to a review." The finding lists the register among its files but never says it is re-confirming a recorded open item rather than discovering one. The parent should treat this as evidence-with-a-repro for a known entry, not a new defect.

2. `select`'s drop-the-blocked branch (`src/commands/integration.rs:490-495`). Not examined at all, and it is what invalidates the proposed test and creates the silent-partial-batch hazard. Any fix here has to decide what `prepare` with no `--card-ids` does with a card blocked on dependency evidence, and `prepare` currently emits no warning for a card it drops.

3. `src/policy/allocation.rs:135-167` — the activation-time dependency check. Section 10.2's binding is declared on the card, so activation is where a declared-base-vs-accepted-dependency check belongs; the finding asserts `check_dependencies` in integration is the only site and never considers this one.

4. `REVIEW_SCHEMA` / `ReviewRecord`'s own `deny_unknown_fields`, despite step 5 adding a field to it. And the `human_reviewer` precedent (`src/domain/review.rs:181`) that settles the `serde(default)` question the opposite way.

5. Interaction with the defect-17 fix. If a dependent really is based on its dependency's accepted SHA (the Section 10.2 path), and the dependency is re-approved at a rewritten commit, the dependent's branch still carries the superseded commit into the merge. That is the concrete harm scenario and the strongest argument for the defect's severity — the finding never states it, arguing only from evidence hygiene.

6. G1's measurement channel. `review inspect`'s `current_approval` is gated on `held_lease` (`src/commands/review.rs:551-559`): a card with no lease reports `has_current_approval: false` for reasons having nothing to do with dependencies. A G1 test must assert on F-002 (lease still held) and must not read F-001's inspect output after F-001 lands, or it will pass for the wrong reason.

7. The word "relevant" in invariant 7.3.6 ("A relevant dependency SHA change invalidates dependent evidence", `docs/IMPLEMENTATION_PLAN.md:264`). The proposal treats every dependency re-approval as relevant and never engages with what the qualifier was there to exclude. Section 16's numbered scenario list (lines 1180-1240) contains no scenario for this invariant at all, which is worth recording as its own gap: the coverage trace cannot be checked for 7.3.6 because it was never enumerated.

Artifacts left in my worktree for re-running: `/Users/alvaro/Documents/Code/change-harness/.claude/worktrees/wf_81894ca4-835-11/tests/probe_depsha.rs` and `/Users/alvaro/Documents/Code/change-harness/.claude/worktrees/wf_81894ca4-835-11/tests/probe_depsha2.rs`. `src/` is unmodified (mutation reverted; `git diff --stat src/` empty).
