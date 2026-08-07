# The thesis, and the experiment that can kill it

This file records the product argument in the form that survived two rounds
of external adversarial review (2026-08-06), what each round conceded or
corrected, and the one empirical question that remains capable of killing the
idea — with its kill thresholds stated in advance, because a threshold chosen
after the data arrives is not a threshold.

## The argument

Orchestration mechanics are becoming commodity — worktrees, subagents, and
dependency-aware fan-out ship inside the coding tools themselves. What scales
with them is the volume of claims a person must validate: a worker saying
tests passed, a supervisor saying the patch is safe, and neither statement
becoming evidence because a second model produced it. A verifier adds trust
only when it holds independent evidence and a separate authority boundary,
and that — not another judge — is what Change Harness supplies. It changes
the unit of authorization: not "an agent reviewed this diff," but "this exact
artifact satisfied a previously declared argument for promotion."

A card declares that argument before work starts — scope, registered gates,
observable acceptance behaviors — and stays frozen for purposes of the
current authorization. It can be revised, but revision invalidates prior
approval and spends a governed budget, so discovery is priced rather than
forbidden or silent.

Evidence **validity** is mechanical: receipts bind to exact commits, record
tree cleanliness, and are re-verified against the actual merge candidate, not
the isolated branch. Evidence **adequacy** is judgment — but judgment that
can only nominate, never promote: a reviewer must structurally answer whether
the gates could have observed each acceptance behavior, an approval cannot
carry an open finding, and absence of evidence is a first-class failure
state. Unproven refuses exactly as failed does, where a language model would
collapse uncertainty into plausible approval.

The gate is non-conversational, the policy changes only through its own
governed step, and the record lives outside the candidate in a control
repository whose rewrite blocks the exact promotion it would have authorized.

The claim is bounded: the harness proves that the *required* argument for the
change is complete *under the declared contract and policy*, and that it is
about the artifact that actually lands. Nothing more — and nothing less is
mechanical.

## Three kinds of truth, and which ones this tool provides

| | Statement | Who can establish it |
| --- | --- | --- |
| Execution truth | Gate `G` ran command `X` against commit `C` in a clean tree and exited 0 | The harness, mechanically |
| Policy truth | Every claim the contract requires has current, independently produced evidence, and the versioned policy accepts the set | The harness, mechanically |
| Product truth | The change satisfies the user's actual intent, preserves unstated expectations, and is safe in production | Nobody, mechanically |

Change Harness guarantees the first two and only *reduces uncertainty* about
the third. That is the honest boundary of the product, stated as scope rather
than discovered as disappointment. The contract itself can omit a critical
requirement; every registered claim can be evidenced while the argument is
substantively inadequate. What the harness removes is the possibility of a
*skipped step* or of *evidence drifting from its artifact* — not the
possibility of a badly written contract.

## What the reviews conceded, and what they corrected

Both reviewers arrived attacking and left refining. What their attacks
established:

- **"Coordination is largely solved" was too strong.** Spawning, isolating,
  and scheduling agents is commoditizing; decomposition quality, correlated
  errors, and integration semantics are not solved. The argument now claims
  only the mechanics.
- **"Non-probabilistic" was the wrong word.** An exit code is discrete, not
  trustworthy: tests flake, skip, run against the wrong commit, or are
  weakened by the same agent that wrote the implementation. The defensible
  properties are *mechanically verifiable* and *non-conversational*.
- **"The record cannot be revised" was an overclaim.** Git refs rewrite. The
  true property is tamper-*evident* within a stated boundary: every landing
  commit anchors the control head it was authorized under, and `integration
  promote` refuses — with no override — when an anchor no longer holds. A
  coordinated rewrite of both control and authority is invisible to this
  check, and SKILL.md's Residual section says so. Under the single-operator
  trust model (D-013), that is scope, not shortfall; D-095 records why
  unattended signing would not raise the boundary (the agent holds the key,
  so whoever controls the agent signs the rewrite too). An external
  append-only anchor is the upgrade path consistent with D-095's own
  reopening condition, deferred because it strengthens custody, not the
  promotion argument.
- **A supervising agent is not useless — it is insufficient alone.** A
  verifier with independent evidence, separate tooling, and a separate
  authority boundary adds real trust even if it is a model. The harness does
  not compete with supervisor agents; it is what makes their verdicts mean
  something, and what stops their approval from being the thing that moves
  the ref.
- **What the reviewers proposed as the roadmap was largely the shipped
  part.** The change contract is the card; claim-to-evidence adequacy is the
  verdict's `gate_adequacy`; merge-candidate re-verification is
  `integration verify`; evidence-absence-as-refusal is how corrupt
  convergence projections, dirty receipts, and unknown liveness already
  behave; contract revision with invalidated approval and a priced budget is
  `card revise` plus the `material-scope-revisions` dimension.

One of those concessions describes an architecture the shipped code has not
finished catching up to: the sentence "it is what makes their verdicts mean
something" is true of the design, not yet of the artifact. A review today
records its verdict but not the mutation it was tested against (#95);
nothing yet binds a supervising agent's approval to the specific check that
earned it. That makes the claim aspirational in exactly the place this
document is otherwise scrupulous about the distinction. The gap is not
theoretical: in the 2026-08-07 wave, five cards were independently reviewed
with a mutation different from the implementer's; three of the five —
#144, #145, #142 — turned up a real, previously unpinned gap and required a
repair before merge. The other two, #143 and #153, did not: the
implementer's own tests already caught the reviewer's mutation. A
supervising agent's verdict is already carrying weight the control
repository has no field yet to record.

## The killer question

> Can enough economically useful software changes be described by contracts
> and observable acceptance behaviors, strongly enough, that the operator
> genuinely stops reading their diffs?

If yes, the delegation ceiling moves and the tool is an amplifier. If no —
if most meaningful changes still require a human to understand the
implementation — this is well-built assurance infrastructure and the ceiling
barely moves. Every other objection raised across both reviews is either
answered by shipped code, adopted into the wording above, or out of scope
under the declared trust model. This one is empirical, and it runs on what is
already shipped: no signing, no RBAC, no orchestration layer is needed to
answer it.

The self-hosting record is one existing data point and a biased one: the
governed project is the harness itself, driven by its author, and
documentation and defect-fix cards are the easy class. The real experiment is
the next governed project that is not this one.

That data point has an ending. The control repository's last governed action
was `integration promote INT-035` on 2026-08-02, after 37 cards and 35
integration cycles (34 promoted, one — INT-032 — abandoned). `origin/main`
has taken 215 commits since with no governed integration behind any of them
(measured 2026-08-07). By the Protocol's own definition below, every one of
those 215 commits is a bypass, set against 34 promoted integrations — a
ratio nobody would call reassuring. Being biased and easy-class does not
make the number smaller: a threshold chosen after the data arrives is not a
threshold, and the same discipline applies to explanations chosen after the
data arrives. Why self-hosting stopped is not established. The third
cold-start run's counter C names candidates, not conclusions: ghost
`gate reserve` exit-0 loops (#146), card-shape thrash with no `card example`
generator (#180), and gate-stage rules an operator can learn only by being
refused.

### Protocol

For every unit of work in a governed project over a fixed window, record at
card-writing time and at close:

1. **Expressible** — could acceptance be stated as behaviors a gate can
   observe, honestly, in under ~15 minutes? (`yes` / `partially` / `no`;
   `partially` is the interesting bucket)
2. **Bypassed** — did the change land outside the harness, and why. This is
   the revealed-preference measure of where the contract model breaks.
3. **Read anyway** — was the diff read line-by-line despite fully evidenced
   promotion. The delegation ceiling is measured by this number falling.
4. **Escaped defects** — severity-weighted, on unread cards versus read ones.

The killer question above is single-operator: whether one operator stops
reading diffs. The product goal is a fleet: many agents building cards in
parallel, unattended, overnight, with the operator reading no code. These
four measures cannot see concurrency at all — they could hold perfectly and
the fleet could still be out of reach. Two more measures record what they
miss:

5. **Serial human minutes per card** — wall-clock a human spends on the
   steps that do not parallelize: freezing a contract, reviewing with a
   mutation different from the implementer's, merging. Neither the control
   repository nor GitHub's PR metadata records this — both preserve only
   the instant a decision is committed, not the deliberation before it — so
   this measure needs a timer an operator keeps, not a git log. As an
   unmeasured estimate, an operator's own sense of the work is something
   like 30 minutes a card; the argument does not depend on the exact
   figure, only on it being nonzero — because while it is, agent count is
   not the constraint: three agents and ten agents finish twenty cards in
   the same wall clock.
6. **Concurrent cards sustained** — how many cards' worth of gates a shared
   test suite absorbs before its own wall-clock time, not contract quality,
   becomes the limit. Recording it needs several complete suites started
   together on an otherwise idle machine, each timed to completion. The
   only attempt made for this amendment was a solo run on a machine already
   carrying other sessions' real load — it took closer to 18 minutes and,
   being neither idle nor concurrent, confirms nothing either way. The only
   figures available — roughly 8 minutes solo, roughly 25–40 minutes with
   three running at once — are a single unreproduced estimate, not a
   recorded result. Taken at face value anyway: something like 4–6
   concurrent cards before the suite dominates, roughly 3x throughput, not
   10x.

Neither carries a kill threshold. A number fixed today, from either, would
be reverse-engineered from an estimate nobody has actually measured yet —
the same failure the rule against post-hoc thresholds exists to block, one
step earlier: there is not yet even a data point to fix a threshold after.
Measure 6 is additionally bottlenecked today by test-suite wall-clock time,
an infrastructure property, not a claim about whether contracts substitute
for a read diff; thresholding it would grade the thesis on CI speed
instead. The same caution applies to a later change in it: a rise in
measure 6 more plausibly means the test suite got faster than that
delegation got stronger, and should be read as the former until shown
otherwise.

Measure 3 assumes someone was there to do the reading. Unattended,
`read anyway = 0%` stops being able to distinguish triumph from negligence —
it reads the same whether the operator trusted the evidence enough to skip
the diff, or was asleep and skipped it by default. Measure 4 carries the
weight in that regime instead: an escaped defect can be found and attributed
after the fact, on nobody's schedule, in a way a diff that nobody was ever
going to read cannot. This is a limit of the measurement, not a new
threshold.

### Kill thresholds, fixed in advance

The thesis **fails** if, over the window:

- fewer than half of real changes are contract-expressible (`yes` +
  `partially`); or
- diffs are read anyway on more than 80% of fully evidenced cards; or
- the escaped-severe-defect rate on unread cards exceeds the rate on read
  ones; or
- bypasses cluster on exactly the changes that matter most, so the harness
  governs only what was already safe.

The thesis **holds** if roughly two-thirds of changes are expressible and
diff-reading concentrates in the unproven/partial bucket — because then the
operator's attention is going exactly where the evidence says it should, and
the number that answers every future critic exists: *X% of changes promoted
unread, at no increase in escaped severe defects.*

A comparison that would strengthen either outcome: the same window's changes
under well-configured native tooling alone (branch protection, stale-review
dismissal, merge queue) — because the bar is not "better than trusting an
agent's prose," it is "better than the best configuration of what already
exists."
