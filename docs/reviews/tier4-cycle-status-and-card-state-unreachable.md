# cycle-status-and-card-state-unreachable  [medium]  fix_accepted=False

## Summary
The claim is TRUE AS STATED: no command writes CycleStatus::{Integrating, Accepted, Landed, Closed, Blocked} and no command writes CardState::Abandoned, so a cycle can only ever be draft/active/abandoned and a card the coordinator gives up on has no terminal state and holds its write scope, worktree and branch forever.

## Root cause
Two distinct holes, plus a third that any fix must close in the same pass.

(a) src/domain/cycle.rs:30-46 declares eight `CycleStatus` variants and :86-98 encodes the full Section 11.1 successor table, but src/commands/cycle.rs:27-38 offers only create/activate/declare-group/status/abandon. Nothing writes Integrating (cycle.rs:36), Accepted (:38), Landed (:40), Closed (:42) or Blocked (:44). Three of those five are not merely unwired, they are the wrong states: Integrating, Accepted and Landed restate `IntegrationStatus::{Prepared.., Accepted, Promoted}` (src/domain/integration.rs:40-56), which the integration record already holds — the second-record-asserting-the-same-fact that D-040 explicitly rejected. Worse, `accepts_cards` (src/domain/cycle.rs:63) is false for all three, so wiring `integration prepare -> cycle integrating` would refuse `card activate` for the remainder of the cycle, contradicting the multi-integration design proved by probe (6).

(b) src/commands/card.rs:59-70 has no `Abandon` arm, so `CardState::Abandoned` (src/domain/card.rs:89) is unreachable even though src/domain/card.rs:169-196 permits it from every non-landed state and src/policy/allocation.rs:57-64 documents that reaching it is what releases a card's claims.

(c) src/domain/cycle.rs:299-307 `status_from_events` folds the *last* event whose `next_state` parses as a cycle status name, and src/commands/cycle.rs:191-201 feeds it every event carrying the cycle id — which includes every card and integration event (src/commands/card.rs:409, work.rs:498, review.rs:278, acceptance.rs:314, integration.rs:732, archive.rs:557 all call `.cycle(...)`). The card vocabulary and the cycle vocabulary overlap on draft/active/blocked/closed/abandoned, so this cannot be left alone once `blocked` and `closed` become genuinely reachable on the cycle.

## Files
src/domain/cycle.rs, src/commands/cycle.rs, src/commands/card.rs, src/commands/archive.rs, src/commands/integration.rs, tests/cycle_model.rs, tests/ownership.rs, tests/card_model.rs, tests/dry_run_parity.rs, docs/IMPLEMENTATION_PLAN.md

## Proposed fix
Split repair: remove three cycle statuses because they belong to the integration, add commands for the two that are genuinely the cycle's, add `card abandon`, and fix the event fold in the same pass because the first change makes it dangerous.

=== CYCLE: REMOVE Integrating, Accepted, Landed ===
Delete variants at src/domain/cycle.rs:36,38,40, their `name()` arms (:74-76), their `parse_status` arms (:315-317), and rewrite `successors` (:90-97) to:
    Draft   -> [Active, Abandoned]
    Active  -> [Blocked, Closed, Abandoned]
    Blocked -> [Active, Abandoned]
    Closed | Abandoned -> []
`is_terminal` (:56) and `accepts_cards` (:63) are unchanged. Update the unit tests at :352-359 (transition table), :388 `a_landed_cycle_cannot_be_abandoned` (delete — the state is gone), :399 `only_an_active_cycle_accepts_cards` (drop the three names), :522 `status_from_events` case. Amend docs/IMPLEMENTATION_PLAN.md Section 11.1 to `draft -> active <-> blocked -> closed | abandoned`, with a decision-register entry stating the reason: those three states are the integration's, a cycle runs N integrations (probe 6), and Section 11.1 was transcribing Section 11.3.

=== CYCLE: ADD COMMANDS FOR Blocked AND Closed ===
Add to `CycleCommand` (src/commands/cycle.rs:27) and to `path()` (:47):
  - `cycle block --cycle-id --reason [--dry-run]`  Active -> Blocked, event `cycle.blocked`, `.meta("reason", ...)`.
  - `cycle resume --cycle-id [--dry-run]`          Blocked -> Active, event `cycle.resumed`.
  - `cycle close --cycle-id [--dry-run]`           Active|Blocked -> Closed, event `cycle.closed`.
Model each on `run_abandon` (:574-624): `with_transaction`, `steps.at("control-write")`, `previous.check_transition(next)?`, `store`, `events.append`, `control.commit`. Each needs a real `--dry-run` arm that runs the same `check_transition` and the same preconditions — `tests/dry_run_parity.rs` exists because seven previews skipped checks (defect 24), so add all three to that suite.
`cycle close` preconditions, each refused with its own named reason:
  - every id in `cycle.card_ids` resolves to a state that `is_terminal()` (closed or abandoned); name the first that does not;
  - no integration for the cycle satisfies `status.holds_lease()`; name it and point at `integration abandon` or `archive close`. Reuse `integrations_for` (src/commands/integration.rs:397) — promote it to `pub(crate)`.
`blocked` needs no separate enforcement: `accepts_cards` is already false for it, so a blocked cycle refuses `card activate` for free, which is the entire point of the state.

=== CYCLE: STOP FOLDING FOREIGN EVENTS ===
Change `status_from_events` (src/domain/cycle.rs:299) to take `(event_type, next_state)` pairs and skip any event whose type does not start with `"cycle."`, keeping the filter inside the domain function that has the unit tests. Update the caller at src/commands/cycle.rs:191-201 to pass `(&event.event_type, event.next_state.as_deref())`. Do NOT filter on `card_id.is_none()` instead — `integration.prepared`, `acceptance.recorded` and `archive.closed` all have `card_id == null` (confirmed in the probe event dump) and would still be folded.

=== CARD: ADD `card abandon` ===
Add `Abandon(AbandonArgs)` to `CardCommand` (src/commands/card.rs:59) and `"card.abandon"` to `path()` (:80). Args: `--card-id --reason [--dry-run]`. No change to `CardState` or `successors()` — Section 11.2 already permits abandonment from every non-landed state.
Preconditions, in this order:
  1. `state.state.check_transition(CardState::Abandoned)?` — this alone refuses landed, closed, abandoned.
  2. Refuse when the card is a member of any integration whose `status.holds_lease()` is true. Name the integration and direct the operator to `integration abandon`, which already returns members to `approved` (D-053).
  3. `clean_up_card(control, &config, &card_id)` — promote src/commands/archive.rs:333 to `pub(crate)` or lift it into a shared module. It already refuses a dirty worktree (`CH-PRECONDITION-WORKTREE-DIRTY`) and a branch holding commits reachable from nowhere else (`CH-PRECONDITION-UNMERGED-WORK`), then unlocks and removes the worktree and deletes the branch. Those refusals are correct here: giving up on a card must not destroy work.
Then `store_card_state(control, &record, &state, CardState::Abandoned)` (src/commands/card.rs:236) and emit `card.abandoned` with `.card(id, revision, digest)`, `.cycle(...)`, `.transition(Some(previous.name()), "abandoned")`, `.meta("reason", ...)`, `.meta("removed", ...)`. Mark `steps.outside_control("cleanup-started")` before the Git work, as `archive close` does at archive.rs:537 — the worktree and branch removal lands outside control, and defect 11's fix requires that boundary be journaled.

=== FAILING-FIRST TESTS ===
T1 `a_card_can_be_abandoned_and_releases_its_scope` (tests/ownership.rs) — the reachable twin of `an_abandoned_cards_claims_are_released` (:317). Identical, except `workspace.tamper_card_state("F-001", "abandoned")` becomes `workspace.card(&["abandon", "--card-id", "F-001", "--reason", "superseded"])`. Fails today with clap's unrecognized-subcommand error. Keep the tampering original too, renamed to say it tests the allocator arm rather than the command. Mutation that must fail T1: delete the `Abandon` dispatch arm. Second mutation, already verified by me: dropping `CardState::Abandoned` from `holds_claims` (src/policy/allocation.rs:62) fails `an_abandoned_cards_claims_are_released` with CH-POLICY-OWNERSHIP-OVERLAP, so that arm is genuinely load-bearing.

T2 `abandoning_a_live_integration_member_is_refused` — full flow to `integration prepare`, then `card abandon` the member. Assert exit 5 and that the message names the integration id. Mutation: delete precondition 2; the test must then fail on the *refusal* assertion, not on `check_transition`, which permits Integrating -> Abandoned and would let the abandonment through. Without this guard the plan references a terminal card and `integration promote` attempts Abandoned -> Landed, wedging the integration exactly as `card revise` from `landed` did (defect 19).

T3 `a_cycle_closes_only_when_every_card_is_finished` — two halves in one test so neither can be satisfied vacuously. Half A: full lifecycle to `archive close`, then `cycle close` succeeds and `cycle status` reports `closed` with `status_matches_history == true`. Half B: a second card still `active` in the same cycle, `cycle close` refused with exit 5 naming that card. Mutation: delete the card-state precondition -> half B fails. Mutation: make the precondition require zero cards -> half A fails.

T4 `card_and_integration_events_do_not_move_the_cycle` — activate cycle, activate card, `work start`, `work block`, then drive an integration all the way to `acceptance record`. After every step assert `cycle status` reports `active`, `status_matches_history == true`, and `warnings` is empty. Fails today at the very first card step (`derived="draft"`, spurious drift warning) and again after acceptance (`derived="accepted"`). Mutation: remove the `"cycle."` prefix filter -> fails. Mutation: replace the prefix filter with `card_id.is_none()` -> the acceptance step fails, which is the point of driving the integration and not just the card.

T5 (guard, see overcorrection) `a_blocked_cycle_can_be_resumed_and_still_accepts_cards`.

=== REPLACE THE TESTS THAT CANNOT SEE ANY OF THIS ===
Verified by mutation: replacing the body of `status_from_events` with `CycleStatus::Active` leaves `tests/cycle_model.rs:184 status_is_derived_from_authoritative_events` and `tests/cycle_model.rs:202 a_stored_status_that_disagrees_with_history_is_surfaced_not_trusted` both PASSING (12 passed, 2 failed — only `a_dry_run_changes_nothing` and `an_abandoned_cycle_is_terminal_and_accepts_nothing_further` caught it). Both tests are named for the derivation and neither exercises it: every cycle_model fixture has zero cards, and both assert the value `"active"`, which the mutation returns. Rewrite :184 to assert a status the mutation cannot produce and to include a card, and rewrite :202 to tamper to `"closed"` on a cycle that has actually been closed by `cycle close`, so the drift is between two real values.

## Over-correction risk
Five opposite failures, each with the guard that holds it open.

1. TOO MUCH REMOVAL. Deleting all five statuses and adding no command leaves `abandoned` as a cycle's only ending, so a coordinator who finishes a cycle must record abandonment — a false entry in the audit trail the product exists to produce. Guard: T3, whose success half must run a real full lifecycle through `archive close` and then observe `cycle status` report `closed`. A fix that removed `Closed` cannot compile T3.

2. TOO MUCH ADDITION. Wiring `integration prepare -> cycle integrating` (the obvious reading of Section 11.1) makes `accepts_cards()` false for the rest of the cycle, so no further card can be activated once any integration is planned, and `integration abandon` leaves the cycle in `integrating` with no route back to `active` (Section 11.1 gives Integrating only Accepted/Blocked/Abandoned). Guard: a test that prepares an integration, abandons it, then activates a new card in the same cycle and prepares a second integration to completion — the two-integration flow my probe already showed works today (INT-001 and INT-002 both archived in C-001). This must keep passing.

3. `card abandon` TOO PERMISSIVE. Abandoning a member of a live integration wedges that integration permanently: the plan pins a card whose state is terminal, and promote's `Accepted -> Landed` can never fire. This is defect 19's exact shape and `is_revisable` (src/domain/card.rs:135-141) already draws the same line for `card revise`. Guard: T2, and its mutation must fail on the refusal assertion rather than on `check_transition`.

4. `card abandon` TOO DESTRUCTIVE. A version that force-removes the worktree and deletes the branch destroys uncommitted work and commits reachable from nowhere else — the loss `clean_up_card` was written to prevent (archive.rs:343-354, 364-372). Guard: two tests, one leaving an uncommitted file in the card's worktree and asserting `card abandon` refuses with `CH-PRECONDITION-WORKTREE-DIRTY`, one leaving a commit on `card/F-001` unreachable from any archive ref and asserting `CH-PRECONDITION-UNMERGED-WORK`. Both must name the path or the branch.

5. THE EVENT FILTER TOO BROAD OR TOO NARROW. Too narrow (`card_id.is_none()`) still folds `acceptance.recorded` and `archive.closed`, which carry no card id — the visible symptom I reproduced (`derived="accepted"`) would survive the "fix". Too broad (dropping the fold entirely and trusting the stored field) discards the WP-200 acceptance criterion that history is authoritative, and would let `a_stored_status_that_disagrees_with_history_is_surfaced_not_trusted` become vacuous. Guard: T4 drives a full integration to acceptance and asserts the cycle still reads `active`, which the narrow filter fails; and the rewritten cycle_model.rs:202 tampers a genuinely-closed cycle and requires the derived value to win, which the broad fix fails.

## VERIFIER objections
THE FACTS CHECK OUT. I re-ran everything with my own probes against the real binary and `tests/support::Workspace::initialized()`.

- Enumeration: only Draft (src/commands/cycle.rs:244), Active (:326), Abandoned (:599) are ever written; the only other `CycleStatus::` hits are the parse_status unit test at :634-641. `CardState::Abandoned` appears outside src/domain/card.rs only at src/policy/allocation.rs:62 and :245, both reads.
- Full lifecycle probe: `[after card create] derived="draft" stored="active" matches=false`, `[after acceptance record] derived="accepted" stored="active" matches=false`, and after `archive close` the stored record still reads `"status": "active"` while the card reads `closed`. Event dump confirms the mechanism: `card.created` carries cycle=C-001 next_state="draft", `acceptance.recorded` carries cycle=C-001 card=null next_state="accepted".
- Wedge probe: F-001 `blocked`, F-002 refused exit 5 CH-POLICY-OWNERSHIP-OVERLAP, `cycle abandon` exit 0, F-001 still `blocked`.
- Two full integrations in one cycle: INT-001 and INT-002 both reached `archive close` under C-001.
- Mutation of `status_from_events` to a constant `CycleStatus::Active`: 12 passed, 2 failed — exactly the numbers claimed, and the two survivors are `status_is_derived_from_authoritative_events` (tests/cycle_model.rs:184) and `a_stored_status_that_disagrees_with_history_is_surfaced_not_trusted` (:202), the two named for the derivation. That claim is correct.

NOT A NEW FINDING. docs/DEFECT-REGISTER.md Tier 4 already records this verbatim: "Cycle status folds card events. Five cycle statuses and one card state are unreachable." The investigator lists the register in `files` and never says the register already names both halves as open items. (The briefing's "24 defects, all now fixed" is not true of the Tier 4 tail.)

THE FIX IS WRONG IN FIVE PLACES.

1. `card abandon` as specified REFUSES THE SCENARIO THAT MOTIVATES IT. Precondition 3 is `clean_up_card` (src/commands/archive.rs:333), whose first check is `unarchived_commits`. I re-ran reproduction (4) with realistic committed work and ran the equivalent `rev-list refs/heads/card/F-001 --not <every other ref>` — it returns one commit. So `card abandon` fails with CH-PRECONDITION-UNMERGED-WORK and the message "archive them before cleanup". Nothing can archive a non-integration card branch: `archive create` (src/commands/archive.rs:41) takes `--integration-id` and requires a promoted integration. The operator is instructed to do something no command can do. Worse, overcorrection guard 4 makes this refusal a required test, so the proposal actively pins the hole open. The guard and the motivating scenario are the same case, and the finding never notices.

2. THE CLEANUP COUPLING IS UNNECESSARY. `Claim::holds_claims` (src/policy/allocation.rs:57-64) reads `CardState` alone — the transition by itself frees the region. Making worktree/branch removal a hard precondition of the transition is what creates objection 1. Correct shape: transition always, cleanup opt-in or best-effort; or archive the branch under a card-scoped archive ref first so `unarchived_commits` is satisfied.

3. "NO COMMAND ABLE TO RELEASE IT" IS FALSE. I drove the escape: `card revise --card-id F-001` narrowing `src/shared/**` to `src/dead-end/**` returns exit 0 and state `ready`; F-002 then activates over `src/shared/**`, exit 0. The wedge is ugly but escapable, so the stated consequence ("locked with no command able to release it") is overstated and the severity with it.

4. `cycle block` AND `cycle close` WOULD BE DECORATIVE, AND THE PROPOSAL SAYS THE OPPOSITE. The only enforcement site for cycle status is `card activate` (src/commands/card.rs:290). I read `build_plan` (src/commands/integration.rs:838ff): it checks the integration lease and never reads `cycle.status`. So a "blocked" cycle would still permit work, gates, handoff, review, prepare, merge, land, verify, acceptance, promote and archive. The proposal asserts "`blocked` needs no separate enforcement ... which is the entire point of the state" — that is wrong, and after `cycle close` the same hole lets a closed cycle land work. The finding's own probe (5) demonstrates this and the fix does not address it.

5. DELETING THREE SPECIFIED STATES RESTS ON A CONTESTABLE READING PRESENTED AS SETTLED. Two integrations per cycle does work today — but that is an unchecked behaviour, not a recorded decision. D-053's "no way to plan another" is about a *failed* integration; `build_plan`'s one-open-integration rule is equally consistent with "one integration lands per cycle, retries allowed". Section 11.1 is the specification, WP-200's recorded evidence is "every Section 11.1 transition is asserted legal", and the glossary (plan line 191) defines a cycle as "A bounded integration period", singular. The symmetric repair — refuse `integration prepare` once an integration has archived, and wire integrating/accepted/landed — is never weighed. Also, removing variants narrows what `harness.cycle/v1` deserializes with no schema bump proposed, in a repo that versions records deliberately.

6. COMPOUND TRAP: `cycle close` requiring every card in `card_ids` to be terminal, plus objection 1, means a cycle holding one given-up card with committed work can never close either.

WHAT DOES HOLD UP. I applied the `"cycle."` prefix filter at the caller and ran the entire suite: 801 tests, zero failures — it breaks nothing, and no fixture asserts the contaminated fold. The `card_id.is_none()` alternative would indeed miss `acceptance.recorded` (card=null, next_state="accepted") — confirmed in my event dump. T4 fails today at the first card step and again after acceptance, as claimed. T2's caveat is right: `CardState::Integrating.successors()` contains `Abandoned` (src/domain/card.rs), so without precondition 2 the abandonment passes `check_transition` and the test must assert the refusal.

## VERIFIER missed
1. `card revise` performs an UNCHECKED transition — the same class as defect 19, in the function the investigator read. `run_revise` (src/commands/card.rs:672) calls `write_revision(control, &record, &digest, CardState::Ready)` with no `check_transition`; defect 19's fix added `is_revisable` and never restored the check. `Blocked -> Ready` is not in Section 11.2 (Blocked's successors are Active, Integrating, Abandoned) and I drove it for real: exit 0, state `ready`. `Active -> Ready`, `HandedOff -> Ready`, `ReviewPending -> Ready`, `ChangesRequested -> Ready` are all equally unchecked.

2. src/commands/audit.rs:242 and :288 read the STORED `cycle.status`, not the derived one. Fixing the fold leaves the audit report saying `active` forever. audit.rs is not in the finding's file list.

3. `archive create` is integration-scoped only, so there is no way to preserve an abandoned card's commits. This is the missing piece that makes objection 1 unfixable as designed — a `card abandon` that archives the branch first would work.

4. No check anywhere that `integration prepare` or `integration promote` refuse a terminal cycle. This is what would give `cycle abandon` (and any future `cycle close`) teeth; the finding observes it in probe 5 and drops it.

5. docs/DEFECT-REGISTER.md Tier 4 already records both halves of this finding as open. The investigator should have said so rather than presenting it as fresh confirmation.

6. Interaction not examined: `existing_claims` (src/commands/card.rs:302) iterates `cycle.card_ids`, which is populated only at `card activate` (CycleRecord::declare_card, src/domain/cycle.rs:279-288). Good news for the proposed `cycle close` precondition — a created-but-never-activated draft is not in `card_ids`, so it cannot trap the check — but the investigator asserted the precondition without establishing it.
