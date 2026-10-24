# Archived code simplification review

Review date: 23 October 2026, with status updated on 24 October. This review was
based on the working tree, including pre-existing grounding/context changes.
The implementation record distinguishes completed refactors from the proposals
that remained at that point.

This file preserves the original review, proposals, and implementation notes.
It is historical reference, not a second active task list. Current work is
tracked only in [nextSteps.txt](../nextSteps.txt); the system overview is in the
[README](../README.md).

The strongest opportunities are to consolidate repeated model-call mechanics,
give evidence one typed representation with explicit views, simplify the
conversation turn, and share research transport and persistence operations.
Preserve the different evidence and review purposes throughout. A smaller codebase
that forgets qualifications or silently weakens review would be a regression.

## What has grown

At the review baseline, the `src/` tree contained 92 Rust files and 47,094 lines, including comments,
blank lines, tests and migrations:

| Area | Files | Lines | Observation |
| --- | ---: | ---: | --- |
| Storage | 39 | 20,397 | Largest concentration; includes substantial tests and migration history. |
| Scholar | 14 | 7,895 | Provider handling, fetching, extraction, repair and retention. |
| Agent | 12 | 4,821 | Turn orchestration, prompts and multiple answer-review paths. |
| Inference | 3 | 2,789 | Two supported backends, routing, caching, locks and tests. |
| CLI | 6 | 2,412 | Command setup, inspection, evaluation and model operations. |

Dedicated test modules account for 9,706 lines; more tests live inline. There
are 450 Rust `#[test]` annotations. Schema code accounts for another 2,039 lines,
with the current database at version 63. These are not all disposable runtime
complexity. Moving code between files alone will not reduce the total.

The main difficulty is repeated concepts and decisions across boundaries:

- Structured model output has many local parsers, validators and retry loops.
- The same proposition/source metadata is manually copied into several Rust
  records, JSON objects and prompt formats.
- `Agent::reply_inner` interleaves interpretation, retrieval, generation,
  research, confirmation, persistence and audit scheduling.
- Scholar handles encyclopedia and discovered sources through partially
  separate paths before bringing them back together for storage.
- JSON-map and SQLite knowledge both participate in retrieval and auditing.
- Current instructions, historical progress and future designs overlap across
  documentation; some current descriptions have drifted from the implementation.

## Meaning of preserving content

For this work, preserve all supported capabilities and all retained information:

- Raw user messages, speaker roles, conversation/session identity and history.
- Original retained source bytes, version hashes, truncation metadata, exact
  quotations and UTF-8 byte offsets. Do not claim to recover text already omitted
  by existing retention limits.
- Distinct propositions, attribution, polarity, modality, validity periods,
  geographic/contextual scope, conflicts and negative results.
- Review provenance, unresolved/rejected states, source-origin relationships,
  correction/supersession history and model lineage.
- Durable jobs, attempt counts, leases, cooldowns, quotas and explicit retry.
- Existing CLI/browser contracts, model assignments, both inference backends,
  adapter restrictions and the training acceptance gate.
- Regression scenarios, raw evaluation failures and historical design decisions.

Repeated transport or rendering code can disappear. Different kinds of evidence,
approval and uncertainty must remain distinguishable. Do not merge claims merely
because their wording or embeddings are similar.

## Proposed work, in order

### 1. Remove small obsolete branches and share setup

Start with changes whose behavior can be checked without a live model.

- In [discovery.rs](../src/scholar/discovery.rs), remove the `SearchProvider`
  trait and its three thin adapters if they remain used only through
  the current direct wrappers. Call the provider functions directly. The current
  trait adds dispatch scaffolding without sharing the provider work.
- In [agent.rs](../src/agent.rs), remove internal plumbing for
  `AgentReply::new_conversation`, which is always `false`. Remove the unreachable
  topic-change notice in [chat.rs](../src/cli/chat.rs). Preserve the response
  field at the browser/JSON boundary until its compatibility requirements are
  established; explicit new-chat behavior must continue to work.
- Share foreground model/service construction between `chat`, `say` and
  [browser_reply](../src/web.rs), preserving the different lifetimes of an
  interactive chat, one-shot reply and browser session. Keep formatting at the
  frontend boundary.
- Simplify startup so inspection commands do not need a fully assembled chat
  object. Keep current configuration validation and command errors explicit.
- Move the worker lifecycle currently called `CuratorHandle` into
  [maintenance.rs](../src/maintenance.rs) or a maintenance worker module. It
  drives all four maintenance roles, while curator should own observation
  extraction. Preserve wake coalescing, productive-pass limits and shutdown.
- Resolve the three compiler warnings accurately: `claim_is_grounded`,
  `ScholarTaskTarget` and `scholar_task_target` are used in tests. Move narrow
  test conveniences into test support or mark them appropriately; keep the
  task-target data and the migration regressions.

Treat these as independent small batches, not one broad edit. **Result:** fewer
wrappers, stale branches and repeated frontend setup. These changes will only
remove a small part of the total code.

### 2. Share structured-output mechanics, keeping distinct decisions

Introduce one small structured-output module, initially extracted from
[research/json.rs](../src/research/json.rs). Migrate existing callers gradually:
[curator.rs](../src/curator.rs), [curmudgeon.rs](../src/curmudgeon.rs),
[scholar/extraction.rs](../src/scholar/extraction.rs),
[scholar/retention.rs](../src/scholar/retention.rs),
[research/request.rs](../src/research/request.rs), and
[agent/task.rs](../src/agent/task.rs).

- Share JSON envelope handling and typed deserialization. Preserve each caller's
  current acceptance rules initially: strict whole-object parsing, object
  extraction and delimiter-only recovery currently accept different inputs.
  Make those differences explicit rather than silently broadening all parsers.
- Use a small bounded-attempt helper only where retry mechanics actually match.
  Callers provide their schema validator, repair instructions and attempt/token
  budget. Retention's 1,024/2,048/4,096 budgets remain different from a one-repair
  intent assessment.
- Share attempt diagnostics, bounded output excerpts and error categories.
  Distinguish malformed output, model failure, uncertain assessment and a valid
  negative verdict.
- Replace repeated `Value` indexing with typed local verdict records and enums
  where this removes repeated field checks. Preserve serialized field names,
  required fields and legacy aliases.
- Keep exact quotation validation and claim/evidence binding mandatory at their
  respective boundaries. Parsing JSON cannot authorize an answer or promotion.

Do not build a configurable agent/workflow framework. A parser and a modest
attempt helper should remove code; if their configuration becomes larger than
the loops they replace, retain the local loops.

**Result:** one maintained implementation of common protocol mechanics, with
fewer opportunities for callers to interpret missing fields differently.

### 3. Consolidate evidence data and model-facing views

Use [ScholarFinding and PropositionMetadata](../src/storage.rs) as the starting
point, not a new competing claim system. Compose existing records from shared
proposition/source components where their fields actually mean the same thing.
Keep separate types for extracted candidates, stored findings and selected
answer spans because they carry different guarantees.

- Create one explicit conversion from stored findings to answer evidence.
- Give that evidence deliberate views for selection, answer writing, support
  review and diagnostics. Keep source URLs masked in support review and keep
  internal review rationales/scores out of ordinary generation.
- Derive source-use restrictions once from structured evidence state, then
  render the required view. Consolidate the overlapping temporal, conflict and
  origin checks in [source_view.rs](../src/agent/research_reply/source_view.rs),
  [source_limits.rs](../src/agent/research_reply/source_limits.rs),
  [research_reply.rs](../src/agent/research_reply.rs) and the older agent prompt
  builder without dropping their deliberate visibility differences.
- Reuse the original-span utility in [scholar/evidence.rs](../src/scholar/evidence.rs)
  where the same quote-recovery contract applies. Keep stricter exact-quote
  checks strict where review requires literal quotations.
- Consolidate citation labeling, URL masking and final rendering so ordinary
  acceptance and partial-answer acceptance share the same citation operation.

Keep JSON conversion at model/storage boundaries. Do not use an arbitrary
metadata map as the internal claim representation, and do not serialize every
internal field into every prompt.

**Result:** less field copying, repeated formatting and drift when a scope or
provenance field is added. This should remove more code than another file split.

### 4. Make a conversation turn readable and avoid repeated preparation

Refactor [Agent::reply_inner](../src/agent.rs) into a short orchestration sequence
with cohesive helpers for interpretation, context selection, task/factual
answering, research and delivery. Use a request record that preserves original
input, resolved request and applicable constraints separately.

- Fetch research candidates once per evidence snapshot. The current
  `scholar_evidence_candidate_sets` query derives the general and review-only
  sets from one result; `semantic_research_evidence` keeps relevance weighting
  and general-evidence ranking in memory. Preserve both contributions if this
  path changes; do not combine their policies.
- Reuse request-only assessments within a turn when the request and context are
  unchanged. Invalidate evidence-dependent results after research; never reuse
  review approval for a changed draft, source span, proposition or scope.
- Build only the prompt needed by the selected route. This is implemented. The
  separate baseline remains only for the model's additional-token comparison
  when initial evidence is empty; removing it requires preserving that sizing
  behavior.
- For factual lookup, assess the supplied evidence and resolve necessary research
  before generating/reviewing the final answer where possible. The current path
  can fully compose/review an answer, plan research, then compose/review again.
  This ordering change needs replay validation, beyond the structural refactor.
- Preserve attempt-first behavior for constructive tasks. If research fails,
  retain the useful attempt with the specific remaining gap.
- Keep persistence and audit enqueueing in one delivery helper, retaining current
  history rollback, save-failure and audit-failure behavior.

Do the behavior-preserving extraction first. Make reordering and model-call
removal a separate batch so latency changes and semantic regressions can be
attributed. This can support the existing investigation-controller proposal
without adding another model role or an additional planning call by default.

**Result:** one visible turn flow and less duplicate retrieval, prompt work and
potentially inference. Actual call/latency savings must be measured.

### 5. Make grounded-answer review one explicit bounded process

The main loop in [research_reply.rs](../src/agent/research_reply.rs) combines
ordinary review, fresh reconsideration, qualified conjecture, useful partials,
structural repair and source-literal recovery. Retain these capabilities while
making transitions and publication conditions explicit.

- Use a typed attempt/outcome record to replace loosely coupled booleans and
  repeated verdict extraction. Preserve existing branch-specific attempt limits.
- Keep one finalization operation for accepted answers and one explicit unresolved
  outcome. A rejected draft must never escape through a rendering fallback.
- Generate a fresh candidate after reviewer disagreement, then review that
  candidate. Do not reinterpret same-model agreement as independent evidence.
- Keep source-limited rendering's current conditions explicit. Consolidating it
  must not make every quoted passage automatically acceptable or remove the
  separate review required by final structural recovery.
- Extract shared grounding policy into cohesive prompt sections for generation
  and review. Remove repeated instructions only after mapping each obligation
  to its remaining home and checking the rendered prompts.

First preserve the transition graph; only later remove a review pass if retained
and fresh evaluations show it adds no useful protection. Sharing mechanics does
not justify combining conversation audits, source-support review, source-quality
assessment and knowledge-promotion approval into one verdict.

**Result:** fewer duplicated acceptance/fallback paths and clearer review bounds.

### 6. Share research transport and source processing

In [discovery.rs](../src/scholar/discovery.rs),
[wikipedia.rs](../src/scholar/wikipedia.rs) and
[site_policy.rs](../src/scholar/site_policy.rs), consolidate common HTTP response
metadata, process/error handling and bounded response operations. Reuse the
existing curl transport initially; a networking-library migration is separate.

- Keep provider request construction and response schemas local to each provider.
- Share the response envelope, HTTP status/retry-after parsing and completion
  bookkeeping where they match.
- Preserve authentication redaction, byte/time limits, caching, provider quotas,
  host reservations and request-attempt records.
- Keep distinct access policies explicit: public-site address/DNS/redirect checks,
  Wikipedia requests and permitted local SearXNG access are different contracts.
- Normalize successful provider results into the existing source/candidate
  concepts, then share the applicable fetch/extract/retain/ingest operations.
  Preserve provider-specific provenance and keep retaining useful source text
  even when answer extraction fails.
- Simplify [Scholar's orchestration](../src/scholar.rs) around explicit outcomes
  such as findings, no result, deferred and failed, while retaining foreground
  priority and the different feedback/corroboration/harvest responsibilities.

**Result:** provider differences remain visible while common request and source
handling is maintained once. This is a larger code-reduction opportunity.

### 7. Simplify storage operations without erasing the ledger

Work primarily in [intake.rs](../src/storage/research/intake.rs),
[reviews.rs](../src/storage/research/reviews.rs),
[promotion.rs](../src/storage/research/promotion.rs),
[baskets.rs](../src/storage/research/baskets.rs),
[queries.rs](../src/storage/queries.rs) and [storage.rs](../src/storage.rs).

- Extract the common deterministic claim/field/span validation contract, with
  explicit differences for each intake boundary. Preserve validation at public
  write boundaries even if extraction already checked it.
- Replace long positional query tuples and repeated row decoding with named
  internal records and small row readers where projections genuinely match.
- Compose duplicated proposition fields in internal records while keeping
  database columns and serialized formats compatible.
- Consolidate repeated promotion eligibility checks behind one explicit policy
  decision. First inventory every intake, human review, AI review and automatic
  promotion path: differences in authority are intentional and must survive.
- Keep transaction ownership explicit in helper signatures. Preserve atomic
  ledger/state changes; do not hold transactions across model or network waits.
- Move test-only convenience overloads out of the runtime interface where useful.
  Several `finish_scholar_job_with_*` methods already serve tests only; do not
  mistake them for several independent production ingestion pipelines.

Do not replace this with an ORM or a generic repository layer merely to reduce
SQL repetition. Keep migration history intact during the refactor. A fresh-schema
snapshot would add another schema definition to maintain unless accompanied by a
carefully validated upgrade strategy.

The current gate inventory is intentionally not one shared approval policy:

- Intake in `research/intake.rs::ingest_source` validates retained evidence and
  records source quality, freshness, support, scope, provenance and conflict
  assessments. A finding remains provisional unless its own policy conditions
  are met.
- Source-expertise review in `research/reviews.rs` records human or automatic
  (AI) method, topic suitability, source kind and the supplied publication date.
  It can change expertise eligibility, but does not itself approve the finding.
- Finding review in `research/reviews.rs::review_scholar_finding_with_method`
  records human or AI acceptance/rejection for one finding, rechecks saved
  semantic support and legacy direct support, and handles scoped conflicts and
  claim-ledger updates.
- Automatic promotion in `research/promotion.rs` requires two distinct,
  high-confidence, directly supporting secondary findings, suitable topic
  reviews, human-verified origins, no unresolved provenance warning, and no
  active conflicting claim. `automatically_review_pending_scholar_evidence`
  only clears a stale conflict gate; it explicitly does not promote findings.

**Result:** less repeated validation, field mapping and policy plumbing while
retaining the complete evidence/review history.

### 8. Consolidate interpretation and replace semantic heuristics separately

This has strong potential but changes model behavior. It follows the mechanical
work and must not be presented as a safe deletion-only cleanup.

- Evolve the existing request assessment into one structured interpretation
  record reused by the turn: resolved request, deliverable, public/private fact
  needs, source-check requirements, temporal needs and relevant constraints.
  Extend it incrementally instead of asking a small model for one large object
  immediately. Keep high-consequence evidence requirements explicit.
- Preserve explicit verification assessment against the original message/history.
  The progress notes retain a failure where a combined classifier missed it;
  merging that call back in requires demonstrating the failure is fixed.
- Replace the meaning-based lists in `private_fact_request` and
  `source_independence_request`, curator confirmation/pronoun/possessive checks,
  Scout topic grouping, Curmudgeon relation/claim interpretation, storage
  time/consequence screening and answer structural heuristics with evaluated,
  structured contextual judgments.
- Retain deterministic schemas, allowed transitions, exact evidence identity,
  numerical operations on established quantities, limits and review requirements.
  Replacing semantic interpretation does not require deleting those contracts.
- Keep the original failures and generalize their scenarios across unrelated
  topics, paraphrases, negation and ambiguity. Uncertain/invalid judgments remain
  unresolved; fewer checks must not mean easier approval.

**Result:** fewer overlapping interpretations and maintained language lists.
There may be more model work in some cases; the code/quality/latency trade-off
must be reported, not hidden with replacement phrase matching.

### 9. Evaluate two larger architectural reductions after the core cleanup

**One knowledge authority.** JSON map claims and SQLite semantic claims are both
read by Agent, Scout and Curmudgeon. The map has its own editing, retrieval,
revision and retraction code; `import_knowledge` already supports limited import.

Prefer a common read/view interface first. Then evaluate SQLite as the canonical
live ledger with JSON retained as a compatible import/export format. Preserve
manual editing workflows and source authority, map/DB ID namespaces, retractions,
supersession links and historical references. Existing import refuses conflicting
IDs/content, so it is not a complete migration solution. Only retire parallel
live retrieval after lossless round-trip and collision tests on copies. If
independent JSON-map operation remains required, keep that capability.

**Clear inference ownership.** `LocalModel` currently represents both concrete
backends and a routed handle, while the pool itself caches `LocalModel`. Evaluate
a concrete Candle/MLX backend enum inside the cache and a separate routed handle
outside it. This can simplify dispatch/ownership; it is worthwhile only if it
reduces the interface and match plumbing. Preserve both backends, role-specific
assignments, foreground/background lanes, locks, adapter lineage and recovery.
Do not collapse the two lanes or introduce Tokio as part of this cleanup.

These are conditional follow-ups, not prerequisites for the earlier reductions.
Do not remove a supported backend, training/rebuild capability or independent
storage workflow to meet a line-count target.

**Evaluation outcome (23 October 2026).** A shared read facade for the knowledge
stores would still need to expose different trust and lifecycle behavior. The
JSON `KnowledgeMap` is an editable lexical graph with source strings,
supersession and retraction; SQLite `SemanticClaimDetail` carries scoped and
time-bounded claims, evidence, review state, provenance and ledger history.
Agent prompt construction distinguishes editable map entries from unpromoted
research evidence, while Curmudgeon namespaces references as `json:<id>` and
`db:<id>` and applies database temporal filtering. A common facade would move
these distinctions into the facade without removing the callers' policy code.
Keep the two reads separate until a concrete consumer can use a shared view
without losing authority, scope or history; no storage migration is justified.

The model-pool concern is real but the proposed enum split does not yet show a
net reduction. `ModelPool` caches `LocalModel`, but checkpoint loading can only
produce Candle or MLX; `ModelRuntime::model` constructs the `Shared(RoutedModel)`
variant. A concrete-backend cache would make that invariant more explicit, but
would add a second enum and forwarding conversions unless inference ownership is
redesigned. Preserve the current boundary until a measured dispatch/API cleanup
can remove those matches without changing leases, lanes, recovery or lineage.

### 10. Simplify tests and documentation without discarding their content

- Extend existing shared fixtures only for repeated setup: temporary stores,
  isolated locks, evidence records and scripted model responses. Preserve explicit
  scenario inputs/expected states and keep adversarial assertions readable.
- Prefer model doubles that check the requested operation/data over brittle
  global call positions where practical, while still checking mandatory order,
  bounded retries and review of each changed candidate.
- Retain the 450 Rust test annotations recorded at the review baseline unless a
  coverage map demonstrates exact redundancy. The current binary runs 476
  tests; moving or parameterizing tests is not permission to lose cases.
- Keep `nextSteps.txt` as the concise current plan and use one current
  architecture map. The baseline file guide count has since been corrected to
  the current 94 Rust files.
- Use one maintained README runtime explanation for model lanes. The baseline's
  conflicting cache/blocking descriptions have since been reconciled; keep
  command and storage descriptions consistent with it.
- Keep dated progress notes and older proposals as linked historical records.
  Label superseded guidance; do not delete retained failures or flatten the
  history into an apparently successful implementation summary.

**Result:** less repeated setup and less documentation to reconcile when making
a change, with the diagnostic value and acceptance limitations preserved.

## Delivery and validation

Use small reviewable batches in the order above. Within each area, separate code
movement/protocol consolidation from changes to routing, prompts, model passes,
ranking, policy or persisted schemas. Avoid mixing an architectural migration
with semantic cleanup.

Before implementation, record the current working-tree baseline and run the
existing Rust/Python tests with isolated state and inference locks. Add focused
characterization coverage only for important behavior that lacks coverage.
Preserve staged/unstaged user work; no index, commit or shared-repository action
is implied by this plan.

For behavior-preserving batches:

1. Run `cargo fmt --check`, relevant Rust tests and `git diff --check`.
2. Run the full Rust suite when a shared parser, evidence type, turn flow,
   storage operation or runtime interface changes. Run the Python fixture-runner
   tests when its contracts are affected.
3. Check malformed/missing fields, unknown verdicts, bounded retries, disputed
   review, explicit verification, save failures, queue recovery and migrations.
4. Keep logs and public field/command compatibility, or document a deliberate
   compatibility change with its replacement before removal.

For semantic or model-call changes, also replay the fixed nine-case corpus, the
cross-domain grounding fixtures and retained multi-turn failures. Include useful
negative/historical answers, conflicting dates, scoped identity, ambiguous names,
prompt injection, user-vs-assistant attribution and explicit source checks.
Retain raw before/after runs and independently adjudicate quality. Track
unsupported assertions, unnecessary abstentions, latency, model calls and repair
counts. Unit tests and same-model review do not establish semantic acceptance.

For any storage migration, use copies. Compare retained bytes/hashes, record IDs
and references, qualification fields, review/status history, queue state and
round-trip exports. Check fresh creation, upgrades, restart and rollback before
cutover. No production data migration is part of the initial cleanup.

Measure net runtime-code reduction separately from moved code, tests and
migrations. Also count duplicate implementations removed and model calls per
representative turn. Do not promise a reduction percentage before the first
batches: types and explicit state may initially add lines while removing risk.
Stop extracting helpers when they add navigation/configuration without removing
an actual repeated responsibility.

## Status recorded at archival date

Behavior-preserving slices across steps 1–5 have been implemented, along with
shared HTTP response framing and explicit Scholar search outcomes from step 6.
The final audit of steps 6–7 found no additional shared contract worth
extracting: provider request/ledger behavior, source-access policy, review
authority, and the remaining query projections differ in meaningful ways. The
matching transport, validation, row-mapping and promotion rules are shared as
described below. Source
quotation byte-offset conversion is now shared across Scholar intake and
research-basket harvesting, preserving each path's overflow outcome. Source
review now also shares the pending-status and exact-quote checks while retaining
operation-specific errors and authority. Test-only job-ingestion adapters now
live in shared test support instead of storage intake. Wikipedia lead and
section captures now share their source-record construction. Retryable Scholar
search outcomes are explicit before completion handling while preserving the
existing search ledger contract. Grounded-answer publication, repair and
unresolved transitions now use a typed action while preserving their attempt
and evidence conditions. The Scholar feedback task mapping uses a named row
reader without changing its SQL projection. Scholar intake and research-basket
harvesting share the proposition-field and exact-source-quote validation
contract while each boundary continues to enforce it alongside its distinct
rules. Basket test fixtures now share constructors for repeated claim, question
and ID-pair defaults while preserving each scenario input. Automatic-promotion
candidate eligibility now shares one SQL filter across promotion and origin
review, while caller-specific authority, origin, provenance and conflict checks
remain separate. Grounded-answer attempt count and reviewer disagreement now
travel together in `DraftReviewState`; two remaining multi-column research query
tuples now use named row records. Step 1's inventory is now verified: provider
adapters and internal new-conversation plumbing are absent,
chat, `say`, and browser replies use the shared model split, startup remains
command-lazy, and maintenance owns the worker lifecycle. The current tree
contains 94 Rust files and 48,296 lines; the earlier 47,094-line figure is
the planning baseline. The refactors reduce duplication and improve boundaries,
but have not yet reduced total Rust line count.

Blind review packets are now prepared for the eight completed baseline
observations under
`evals/results/code-simplification-baseline-2026-10-23/blind-review-packets/`.
They omit expected/required/forbidden rubric keys. A later controlled rerun
completed all nine fixed cases, and all 16 cross-domain grounding fixtures now
have archived observations and blind review packets. The controlled benchmark
still fails its rubric; both runs and their limits are recorded in the
[24 October progress note](code-simplification-progress-2026-10-24.md). No
human judgments have yet been supplied. A separate-model review of the 16
cross-domain cases found two likely unnecessary abstentions, one extraction
that lost numbers despite a correct answer, and one possible completeness
overstatement; it is advisory and not human acceptance. The architecture review
in step 9 is complete, with both
larger refactors deferred on the evidence above. Intake, human source review,
AI review, and automatic promotion have different authority and eligibility
requirements; their gates remain separate. The repeated fixtures and current
documentation identified in step 10 are aligned, with dated history retained.
Step 8 now has complete baseline observations. Implementation can proceed in
reviewable candidates; acceptance requires retained before/after benchmark,
cross-domain and multi-turn replays with independent adjudication. This is an
acceptance requirement, not a requirement for human permission before editing.
The first candidate removes source-reference phrase matching that rewrote
extracted claim meanings. The existing bounded semantic review and repair path
now handles those candidates. Live after-change validation and independent
judgments remain pending, so step 8 is not complete.

The original planning review ran `cargo check --locked --offline` and recorded
three warnings for test-used helpers. Those helpers have since been restricted
to test builds. Current validation runs `cargo fmt --check`,
`cargo check --locked --offline`, focused agent/Scholar/storage tests and the
full Rust suite (476 passed serially in the latest run). The earlier parallel
run passed 471/472; the
inference lease test intermittently failed with `WouldBlock`. Current Python
validation runs 15 `unittest` tests across the controlled-learning evaluator,
learning-quality evaluator and grounding fixture runner.
The nine-case live baseline was run on 23 October 2026 and is retained under
`evals/results/code-simplification-baseline-2026-10-23/`. Eight cases completed
with complete archive audits; `crisps_vs_renewable_energy__1` timed out at 180
seconds before producing its archive. The runner marked semantic quality as
unreviewed, so this is execution/transport evidence, not a passing benchmark.
No server restart, production-data operation or migration was run. Training
remains gated as recorded in the existing working notes.

Recent behavior-preserving refactors and test-fixture consolidations are
recorded in the [23 October history](code-simplification-progress-history-2026-10-23.md)
and [24 October progress note](code-simplification-progress-2026-10-24.md).
Formatting, `git diff --check`, offline `cargo check`, the full serial suite
(476) and all 15 Python `unittest` tests pass. Keep future progress evidence in
the dated history so this plan stays focused on current scope.
