# Scout webpage intake and qualified Curmudgeon review

Date: 27 October 2026. Status: implementation tasks 1–4 are complete in the
working tree. Deterministic coverage is complete for the implemented paths;
continuous live-model and independent semantic acceptance remain open.

## Outcome

Scout reads retained incoming webpage text, preserves its distinct assertions
and investigable questions, and selects useful verification work for Scholar.
Scholar uses Curmudgeon's objections to improve the actual proposition or seek
better evidence. A supported claim can be accepted with material qualifications
without being promoted as a broader, unconditional fact.

## What the current code and local data establish

- `src/scholar/site_policy.rs` retains fetched external page text before targeted
  extraction. `src/scholar.rs` also retains Wikipedia material. Retention is
  bounded, versioned by content hash, and separate from successful answering.
- The original basket reader extracted assertions and questions with the
  **Scholar** backend. The implementation now shares it through
  `src/research_intake.rs`; Scout reads retained text and selects follow-up
  work.
- Retained assertions in `research_claims` remain provisional and pairwise
  relationship review remains distinct from verification. Scout can now select
  a focused question and create a durable assertion-to-Scholar job link.
- Harvesting now runs in Scout's maintenance work, independent of Scholar's
  foreground and derived-job queue. A persistent five-stage cursor rotates
  sources, feedback, retained questions, retained assertions, and map work.
- Curmudgeon now structures claim acceptance with caveats and material
  objections. Scholar repair/new-source guidance includes the review reason and
  serialized objection; retries retain the existing three-attempt total budget.
- Caveats are stored on the reviewed finding and included in source evidence for
  answer generation. Caveated findings remain in review instead of being
  auto-promoted, so their limits are not lost in the semantic map.
- Source-backed answer generation already receives review feedback, rewrites,
  and re-reviews. Its policy allows useful qualified partial answers. Ordinary
  conversation audits remain advisory after delivery. Neither path should be
  replaced with a new universal approval loop.

Read-only inspection of `state/learning.db` on 27 October found 469 retained
sources: 61 fully harvested and 408 unfinished, including one parked source.
There were 709 retained assertions; 34 pending, 20 queued, 37 researched, and
46 unanswered retained questions; and 58 pending `new_source` feedback tasks.
Of the 20 queued questions, 19 jobs were pending and one was `needs_review`.
All 28 pending Scholar jobs were Scout-originated. The newest retained assertion
was dated 15 October, while sources continued arriving through 24 October.
These records demonstrate earlier extraction and feedback activity and a
backlog, not successful current end-to-end operation. Scheduling code supplies
credible starvation mechanisms; the snapshot alone cannot establish every
cause or whether a worker is currently running.

Baseline validation before implementation: `cargo test --quiet` passed all
**471 tests**. The implementation now uses schema version 69. A synthetic live
model probe and one isolated Wikipedia/SearXNG cycle have been run; concrete
failures and their limits are recorded below. Existing working-tree changes
must be preserved.

## Implementation tasks, in order

### 1. Reproduce scheduling failures and make intake progress observable

- [x] Add controlled regressions for capacity-blocked retained questions,
  interval, daily-budget, duplicate, and challenged/due combinations. A blocked
  queue must not cause the entire Scout pass to return idle without considering
  other eligible work. Reconcile terminal question jobs before checking
  capacity. The regressions cover each admission blocker, plus challenged/due
  map work when question capacity is full.
- [x] Extend the controlled regression coverage to interval, daily-budget,
  duplicate, and challenged/due combinations. A blocked queue must not cause
  the entire Scout pass to return idle without considering other eligible work.
  Reconcile terminal question jobs before checking capacity. The regressions
  cover each admission blocker, plus challenged/due map work when question
  capacity is full.
- [x] Test harvesting with a sustained Scholar job queue, and feedback with a
  sustained derived-question queue. Preserve foreground conversation priority,
  resource budgets, cooldowns, and bounded passes. Quota-deferred work must not
  become a semantic rejection or consume repeated model attempts. A 25-job
  derived queue no longer blocks a source harvest; feedback runs despite a full
  retained-question queue, and assertion admission reuses its selected question
  after interval deferral.
- [x] Expose source backlog, oldest eligible age, cursor progress, parked/error
  counts, derived-work backlog, feedback age/route, and actual selection or
  deferral reason through existing status/diagnostic surfaces. Distinguish
  extracted, skipped, proposed, reviewed, waiting for evidence, and verified.

### 2. Make Scout responsible for source-derived leads

- [x] Extract the existing basket reader into a cohesive shared module and
  invoke page harvesting with Scout's backend from Scout's maintenance work.
  Scholar retains fetching, targeted extraction, repair, and corroboration.
  Preserve `model probe-baskets` through the shared reader, making its selected
  role explicit. Avoid two competing harvest owners or duplicate extraction.
- [x] Use a bounded, durable scheduling rotation/age policy among eligible
  source intake, existing feedback, derived leads, and map gaps. State and test
  an upper bound in completed eligible background rounds for each class to get
  a turn. A source chunk remains one bounded unit of work; do not add an
  unbounded page read or extra inference beyond the maintenance budget.
  User research takes precedence; model-invented exploration remains last.
- [x] Reuse source leases, retained bytes/hashes, cursor transactions, exact
  evidence offsets, deduplication, output repair limits, and explicit parking.
  Bounded overlapping windows recover a mid-sentence boundary case and retain
  correct Unicode byte offsets. This is representative coverage, not a proof for
  every page structure or extremely long assertion.
- [x] Add a durable link from selected retained assertions to Scholar jobs,
  alongside the existing question path. A contextual Scout judgment selects
  public, concrete, useful verification questions, retaining the original
  assertion, qualifications, source version, exact basis, and selection reason.
  A second contextual alignment review rejects generated questions that change
  the proposition. Asserted premises remain provisional. Do not queue every
  assertion blindly. This same-model alignment review still needs live replay.
- [x] Apply existing investigation admission limits and semantic duplicate/
  coverage judgment to assertion-derived work. Preserve deferred leads for
  later admission, bound derivation depth, and prevent already-researched
  premises and paraphrases from creating endless verification chains. Lexical
  indexing may shortlist candidates; it must not decide semantic equivalence.
  Previously collected assertions must be eligible without re-fetching pages.
  Scout supplies up to 20 recent assertion checks to its contextual selector;
  it preserves a deferred selected question and caps assertion derivation at
  depth three. A migration adds depth with a legacy default; local tests cover
  paraphrase context and chain exhaustion. Semantic judgment still needs live
  and independent evaluation.
- [x] Keep raw assertions outside the semantic map. Scholar's evidence,
  source-suitability, provenance, conflict, freshness, and promotion checks
  apply to every derived verification job.

### 3. Make review feedback actionable and separate its decisions

- [x] Extend the structured assessment to separate passage entailment, relevance
  to the question, source suitability, material limitations, and promotion
  eligibility. A claim can faithfully reflect one source while still waiting
  for independent evidence. Missing corroboration or low extraction confidence
  must not be represented as proof that the claim is false. Curmudgeon now emits
  separate entailment and relevance fields; enforced evidence gates continue to
  determine promotion eligibility.
- [x] For objections, require the disputed assertion/span, why the issue is
  material to this request, its evidence basis or explicit evidence gap, and
  an action: repair the same passage, seek specific evidence, retain a useful
  qualified result, or stop with an unresolved outcome. Unsupported hypothetical
  exceptions are not established counterevidence and should not veto a claim
  that does not assert universality. Judge these distinctions contextually;
  add no runtime phrase lists or topic examples to make individual cases pass.
- [x] Carry the original question/context, full structured objection, original
  source, previous candidate, and suggested exact span into Scholar repair and
  subsequent review. Preserve source profile, dates, version identity, and
  qualifications. Treat reviewer text as untrusted guidance, never evidence or
  instructions that can override policy.
- [x] Keep the current bounded retry chain across repair and new-source routes.
  Detect unchanged proposals/objections and record no progress instead of
  repeatedly seeking agreement. Exhaustion means unresolved, not disproven.
  Repeated repair with the same proposition, reviewer guidance, and passage now
  exhausts the chain with an explicit no-progress reason. Waiting for
  corroboration is a separate state from repeated bad extraction.

### 4. Add acceptance with caveats without approving a broader claim

- [x] Add an explicit `accept_with_caveats` review outcome with validated,
  claim-bound caveats. Each caveat identifies the affected proposition, its
  material scope/uncertainty, and supporting source span or an explicitly
  unresolved evidence limit. An asserted exception needs evidence; absence of
  verification does not establish an exception or its location.
- [x] When the candidate **already expresses** the supported scope, accept that
  exact proposition with its material qualifications. A hypothetical edge case
  outside its asserted scope should not force another rewrite or more research.
- [x] When adding a caveat **changes the proposition**, route a replacement
  through Scholar and a fresh semantic/source review. Bind approval to the
  replacement's complete fields and evidence. Do not mutate a promoted claim
  in place or carry the broader claim's approval onto its replacement. A
  reviewer suggestion alone is not an accepted corrected claim. Curmudgeon
  must reject a broader candidate for repair; Scholar's replacement is a new
  finding with a new review lifecycle.
- [x] Validate cross-field consistency: malformed, uncertain, contradictory,
  or evidence-free caveated approvals stay unresolved. Keep uncertainty about
  source support separate from a supported statement describing uncertainty.
- [x] Persist material caveats with the reviewed finding/version and preserve
  them through retrieval, answer evidence, and any eventual map promotion.
  Use existing proposition fields for expressed scope/stance; add dedicated
  review metadata only where those fields cannot represent the limitation.
  Evidence limits such as single-source status remain evidence metadata.
  Caveated findings stay out of the semantic map until it can preserve their
  limits. Migration 66 adds review metadata with a legacy empty-list default;
  migration history remains append-only.
- [x] Review answer rendering against original passages: put necessary caveats
  beside the affected assertion, preserve useful supported content, and omit
  unrelated hypothetical limitations. A general disclaimer cannot rescue a
  definite unsupported assertion. Every changed final answer still passes its
  existing review; ordinary dialogue audits remain advisory. Validated caveats
  now pass through the sanitized answer evidence view, and a regression checks
  both the generated context and final answer review.

### 5. Validate orchestration and semantic behavior separately

- [x] Exercise focused isolated-database paths for retained webpage
  harvesting, assertion selection and Scholar admission, caveat
  persistence/retrieval, and answer-generation caveat handoff. A continuous
  replay from fetch through rejection, improved candidate, final answer and
  persisted retrieval remains open; component tests do not establish that
  complete lifecycle.
- [x] Cover blocked admission, retry exhaustion, malformed review fields,
  prompt injection in page content, and unchanged-candidate loops. Existing
  lease-expiry recovery is covered in storage scheduling tests. No test or
  policy promotes basket assertions or reviewer text alone.
- [ ] Add isolated duplicate-page-version and semantic question-paraphrase
  replay cases. Scout now receives prior assertion-check context for contextual
  review; the live semantic behavior is not established by that prompt or its
  orchestration test.
- [ ] Add semantic cases across astronomy, cooking, engineering, history, and
  public services, with paraphrases, negation, attribution, dates, locality,
  conflicting sources, and genuinely unsupported generalizations. Include a
  supported ordinary claim challenged by an irrelevant imagined exception,
  and a superficially similar claim whose missing scope really changes it.
- [x] Run live-model tests on synthetic sources and a bounded live
  Wikipedia/SearXNG cycle in an isolated database. Record source content,
  structured decisions, model/prompt versions, repairs, latency, coverage,
  unsupported approvals, unnecessary refusals, and queue waiting times.
  Existing live runs are failure-bearing samples; a fixed before/after
  comparison with archived outputs and broad coverage metrics remains open.
- [ ] Obtain independent judgments of false approvals and false rejections.
  Model doubles establish control flow; same-model agreement does not establish
  semantic correctness. Preserve failing runs and the existing grounding
  acceptance gate; training stays gated.
- [x] Run `cargo fmt --check`, relevant tests, the full Rust suite for the
  scheduler/storage integration changes, and `git diff --check`. Update
  `nextSteps.txt`, progress notes, and basket documentation with measured
  outcomes and remaining limits. Do not clear queues, bulk retry parked
  sources, restart the user's service, or modify the live database to make the
  demonstration pass.

## Acceptance and suggested delivery boundaries

First deliver intake scheduling and assertion-to-verification wiring (tasks
1–2), then structured objections and qualified acceptance (tasks 3–4), then
integrated evaluation (task 5). Keep each change reviewable and preserve the
existing uncommitted work. No commits, staging, remote actions, deployments, or
model training are requested by this handoff.

Completion requires traceable examples of both derived assertions and questions
reaching Scholar; bounded progress for eligible queues; objections demonstrably
changing or narrowing candidates; and material caveats surviving storage,
retrieval, and answer review. No raw or broadened assertion may bypass the
existing evidence gates. Report semantic failures explicitly rather than
claiming success from the automated suite alone.

## Implementation validation record

- `cargo fmt --check` passed. The full Rust suite passed **490 tests** serially.
  Curmudgeon parser/repair tests passed after rejection was made to require a
  structured objection. `git diff --check` is run after the final documentation
  edits.
- A parallel full-suite run previously had one unrelated model-lease lock test
  fail with `WouldBlock`; that test passed in isolation and the complete serial
  suite passed. The serial result is the reliable validation for this workspace.
- The 490 Rust tests establish deterministic orchestration, persistence, and
  schema behavior. They do not establish semantic correctness of live model
  judgments.
- In a synthetic Scout basket probe, the configured Qwen/Qwen3.5-2B INT8 model
  returned exact source spans and a grounded question. It also extracted an
  explicit statement of unknown information as a provisional claim; extraction
  confidence does not make that claim true.
- In a synthetic Curmudgeon probe, the first output accepted a site-specific
  finding without qualifying the broader Finland question. A strengthened
  general scope comparison yielded `accept_with_caveats`. The parser rejected
  invalid scope binding. A later valid output kept broader operation unknown,
  but included a redundant positive caveat as well as the useful limitation.
  This is one case, and caveat quality is not established. The updated schema
  has now had a bounded live replay, with the result recorded below.
- One isolated live Wikipedia/SearXNG job found an allowed NASA Titan page;
  robots/parser policy skipped another candidate. Scout read a retained page and
  extracted 17 provisional assertions. Its first proposed verification question
  introduced a diameter comparison absent from the selected assertion. A
  bounded alignment review was added before admission; it has not been replayed
  live yet. The derived Scholar run surfaced off-topic candidates and at least
  one weak semantic entailment rationale. The findings stayed pending review or
  evidence gates; this run is a failure-bearing integration sample, not a pass.
- After the final passage-entailment/relevance schema, an isolated live
  Curmudgeon probe correctly separated a Sweden passage as entailed from a
  Finland-only question as irrelevant and routed it to new-source work. In a
  second test, a Sweden passage was directly relevant to a question asking
  whether deployment existed in both Sweden and Finland, but the model judged
  it irrelevant and rejected it. Its first output also contradicted itself
  (`next_action=repair`, objection action `new_source`); the bounded repair
  repeated the mismatch and was rejected by the parser. This is a concrete
  partial-answer/reviewer-consistency failure, not a passing caveat result.
  Rejection now requires a structured objection, and cross-field action
  consistency remains enforced so this output stays unresolved.
- Independent human judgment from the user on this case: the Sweden statement
  is partial at best and may be reported as such; the answer should say only
  that the evidence does not establish Finland deployment. This confirms the
  expected disposition for this one case, but does not independently validate
  other topics, paraphrases, or model behavior across a benchmark.
- The live run used `/tmp/lyra-scout-live.RCqDiI/learning.db`, with separate
  knowledge/state paths. It did not touch `state/learning.db`, production queues,
  or the running service. The temporary database retains the fetched page and
  provisional findings for inspection; the full CLI stream was not archived.
- One user judgment is recorded for the Sweden/Finland partial-answer case;
  broader independent judgments, an adversarial cross-domain benchmark, a
  complete repair-to-final-answer replay, and a training-gate result remain
  unavailable. Same-model agreement and passing orchestration tests do not
  establish semantic accuracy. Keep these acceptance items open and retain the
  existing grounding gate.
