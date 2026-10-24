# Code simplification implementation history

This dated record preserves the detailed implementation log verbatim as it stood when the active plan was shortened. New slices belong in the active plan until a later dated archive is needed.

## Implementation progress

The working tree has since completed several mechanical slices:

- Removed the thin search-provider trait/adapters while preserving provider
  request and response handling.
- Removed the always-false internal `AgentReply::new_conversation` state and
  unreachable notice. The existing browser and CLI JSON field remains present
  with its compatible `false` value.
- Shared extraction of a JSON object envelope across the existing callers,
  retaining each caller's own diagnostics and schema validation.
- Shared strict object-envelope deserialization across Curator, Curmudgeon and
  basket retention while keeping caller-specific missing, incomplete and parse
  errors. Scholar extraction keeps its distinct flat-shape and delimiter recovery.
- Built only the selected task or factual prompt, and deferred the baseline
  prompt until token comparison needs it.
- Consolidated Interaction, Curmudgeon and Scholar model-handle construction
  for chat, one-shot replies and browser replies in the foreground runtime.
- Loaded expanded and needs-review answer candidates in one SQLite snapshot,
  retaining each population's existing 500-row bound and downstream ranking.
- Shared temporal-limit and conflicting-account metadata predicates between
  model-facing guidance and the literal source-limited path.
- Shared the verified-origin and scope-field contracts across those answer
  views, retaining the literal path's all-origins-must-be-verified condition.
- Added a typed source-use view for status, timing, scope, conflicts and origin;
  answer generation and the literal fallback now share one converted snapshot.
- Reused the citation-list finalizer for ordinary, partial and source-checked
  answers, preserving each existing label and duplicate-avoidance rule.
- Shared source-URL masking for support review, keeping citation labeling,
  reviewer masking and final source-list rendering at one boundary.
- Moved request and source-check assessment into one turn helper, preserving
  model call order, diagnostics, conservative error defaults and the separate
  original/resolved request values.
- Extracted reply persistence and audit scheduling from the turn body while
  retaining save rollback and non-fatal audit enqueue behavior.
- Introduced a typed grounded-answer verdict and assessment record, using the
  same parser for the primary reviewer and reconsideration while preserving
  invalid-output rejection and disagreement handling.
- Corrected the Rust file-guide count and reconciled README model-runtime text,
  linking the browser description to the single cache/lane explanation.
- Reduced the current `nextSteps.txt` from 539 lines to 55 active-priority lines;
  preserved its complete pre-edit working-tree contents in a dated history file.
- Restricted test-only grounding and scholar-task inspection helpers to test
  builds, keeping their test coverage and migration checks.
- Moved the multi-role background worker lifecycle from `curator.rs` into
  `maintenance.rs`, renamed its handle and reply-service hook to
  `MaintenanceHandle`, and kept the bounded wake queue, web productive-pass
  cap and shutdown behavior. Observation extraction remains in Curator.
- Deferred runtime, conversation-state and map loading until a command uses
  them. Database inspection/edit commands can now run without model assignment
  validation or a constructed chat; chat and inference commands still report
  invalid model configuration before doing inference.
- Converted stored Scholar findings directly into the typed source-use view for
  answer generation, keeping JSON conversion for prompt and review boundaries.
- Shared parsing of the current time for Retry-After handling while preserving
  the provider-specific status and retry policies.
- Represented Scholar job completion as explicit finding, no-result and failed
  outcomes after the bounded search stage, retaining the existing defer path for
  backpressure and keeping source-backed completion and feedback transitions in
  their existing branches.
- Shared the reserved-host wait and HTTP metadata field parser across Wikipedia,
  discovered search providers, and policy-checked page fetching. Request
  construction, DNS pinning, source policy, provider quotas, and each caller's
  status/error decisions remain at their respective boundaries.
- Extracted turn context retrieval and selection from `reply_inner`, preserving
  the order, limits and diagnostics for observations, claims, findings,
  challenges and possible user-fact confirmation.
- Reused one Scholar evidence query to derive general and needs-review candidate
  sets for turn selection, retaining each set's separate ranking and filter.
- Extracted research-plan execution, foreground lookup, evidence refresh and
  task/factual fallback handling into a turn helper, keeping attempt-first task
  behavior and factual unresolved outcomes intact.
- Grouped one grounded-draft review into a typed result carrying verdict,
  support/usefulness decisions, reviewer disagreement and structural flags.
  The caller still owns the existing three-attempt transition and publication
  rules; conjecture and useful-partial reviews remain separate.
- Shared the identical bounded-text-field contract across curator extraction,
  scholar intake and retained-basket validation, while leaving each boundary's
  distinct evidence, scope and provenance checks in place.
- Shared the common SQLite projection and row decoder for relevant and all-active
  semantic claims, retaining research-relevance weighting only in the relevant
  view.
- Shared printable-field checks for reviewer reasons while keeping the stricter
  bounded-field check (including special-token rejection) separate for model
  claim fields. Curator, Curmudgeon, Scholar extraction, and storage reviews now
  call the shared contracts.
- Replaced automatic promotion's positional 11-field finding tuple with a named
  record, preserving its source-origin, proposition-scope, and status checks.
- Replaced corroboration matching's positional 10-field row with named evidence,
  origin, support, and provenance fields; its query and independent-source gate
  are unchanged.
- Centralized the identical SQLite `date('now')` query used by claim curation,
  Scholar intake, verified-finding promotion, human source review, claim conflict
  checks and audit date reporting. Supplied-date normalization and SQL temporal
  comparisons remain local because they have different inputs and purposes.
- Shared the root-finding lookup used by durable-feedback queueing and status
  logging. Retry ownership and attempt limits remain unchanged; the focused
  feedback-storage tests pass.
- Shared Tavily's curl stdin configuration between its basic search and
  advanced retry. Request construction, credential handling, retry eligibility,
  response parsing and reservation completion stay at their existing call sites.
- Replaced the 11-position human source-origin review tuple with a named
  `SourceOriginFinding` row record. It carries the same source passage,
  proposition and scope fields through the existing review checks.
- Shared the exact rolling-day and current-month Tavily usage query between
  reservation limits and corroboration admission. Each caller retains its own
  admission threshold and reservation transaction behavior.
- Replaced the deferred Scout lead's five-value tuple with a named
  `DeferredModelProbe` record across storage retrieval and queue resumption.
  Seed, question, topic, and saved raw model output remain separate fields.
- Reused a parameterized Scholar-finding fixture across source-review,
  provenance, source-grounding, claim-observation, identity-scope, migration,
  and scheduling tests; each migrated scenario still supplies its subject,
  relation, object, quote, confidence, source quality, and optional update date.
  The affected modules' 74 tests pass.
- Extended that fixture reuse to the remaining default-metadata findings in
  source-grounding and provenance regressions. Scenarios with explicit validity
  periods, identity scope or attribution remain explicit so their distinguishing
  evidence stays visible.
- Added a focused default-proposal `CandidateClaim` fixture and reused it for
  repeated ordinary user-claim setup. Low-confidence review candidates,
  confirmation-context claims and time-scoped claims keep their explicit records.
- Shared Unix-time acquisition between basket retry scheduling and
  corroboration scheduling while preserving their distinct timestamp-overflow
  errors and millisecond conversions.
- Reused one checked Unix-millisecond-to-`i64` conversion for search
  reservations, request leases, completion records and site-policy cache times;
  each operation keeps its existing overflow error text.
- Shared Wikipedia page revision-timestamp extraction across source retention
  and primary/alternative finding extraction, preserving first-revision
  selection and the existing nullable timestamp behavior.
- Reused one `ScholarEvidence` constructor across encyclopedia results, feedback
  repairs, and discovered pages; each path still supplies its own source fields,
  finding outcome and relevance, with the same default source profile.
- Shared Wikipedia JSON response deserialization and its `InvalidData` mapping
  across search, section metadata and page-content responses; endpoint schemas
  and request construction remain local.
- Re-ran the full Rust suite after these changes: 456 passed, 0 failed. Formatting,
  compilation, and `git diff --check` also pass. The overall Rust line count is
  higher than the planning baseline because this branch contains preserved
  grounding/context work and explicit shared types; it is not evidence of net
  code reduction. Continue measuring duplicate implementations and runtime code
  separately before claiming a size reduction.
- Shared exact evidence byte-offset calculation between Scholar finding
  retention and chunked research-basket ingestion, retaining the chunk cursor
  as an explicit base and preserving UTF-8 byte offsets.
- Combined two retained-claim relationship queries into one ordered `UNION ALL`
  read, while preserving which evidence links contribute to provisional host
  support counts and which appear only as semantic relationships.
- Replaced the retained-claim detail's 11-field positional tuple with a named
  row record, keeping its quote, qualification, hash, and byte-span fields
  distinct at the JSON boundary.
- Reused one candidate row decoder for inbox detail and relevant-candidate
  retrieval by aligning the query projection; chronological ranking still uses
  the same turn number and tie order.
- Shared the byte-preserving HTTP response-marker split across search, Wikipedia
  and site-policy transport, retaining each provider's own metadata parsing,
  status policy, byte limits and request completion behavior.
- Shared curl's common silent/error/failure and time/size-limit setup across
  search, Wikipedia and policy-checked fetches, leaving each provider's timeout,
  byte bound and request/security options explicit at its call site.
- Centralized search-domain normalization for persisted SerpAPI usage and
  discovered result counts, retaining the existing whitespace, trailing-dot,
  case-folding and `www.` behavior.
- Replaced semantic-support retrieval's positional finding/source tuple and
  inline row decoder with a named row record, preserving proposition metadata
  decoding and the original evidence-validation transaction.
- Shared the persisted-string decoding for proposition polarity and modality
  across semantic-support intake and both promotion-review checks, preserving
  unknown-value fallbacks.
- Replaced promotion review's 14-field positional finding tuple with a named
  row record, keeping the same query projection and review transaction.
- Replaced automatic promotion's six-field conflicting-claim tuple with named
  proposition, scope, and validity fields; conflict matching conditions remain
  unchanged.
- Reused one proposition-metadata value for saved-assessment binding and legacy
  passage checks during human promotion review, keeping both checks and their
  distinct evidence sources separate.
- Avoided rechecking source-limited eligibility in the production answer path
  after the typed finding view already rejected it; the standalone JSON review
  helper retains its own check for callers that have not prechecked evidence.
- Replaced candidate resolution's 12-field positional query tuple with a named
  row record; user-observation binding, date checks and transaction boundaries
  stay unchanged.
- Replaced human-promotion conflict scanning's eight-field active-claim tuple
  with named proposition, validity and scope fields, preserving separate
  equivalent-claim and overlapping-conflict predicates.
- Replaced Scholar intake's 11-field existing-claim tuple with a named row and
  its four optional match outputs with one explicit outcome enum. Conflict,
  exact match, qualification and unresolved-scope precedence remains unchanged.
- Replaced the source-expertise review's finding and assessment tuples with
  named records, preserving the separate direct-support, freshness, confidence,
  sensitivity and independence checks.
- Shared the bounded subject/relation/object field check between Scholar finding
  intake and research-basket ingestion; their evidence, scope, confidence and
  qualification checks remain specific to each intake boundary.
- Shared the active-claim proposition read used by both sides of manual
  supersession, retaining the caller-specific missing-claim errors and the
  same-subject/relation, changed-object validation.
- Replaced retained-question queueing's three-field positional row with a named
  question record, preserving source/job linkage and queue priority behavior.
- Grouped original input, resolved request, factual-lookup need and source-check
  constraint into one turn request passed through context selection, prompt
  selection and research resolution; user-facing work reads original wording
  from that record while research reads the resolved wording. Request assessment
  now produces this record directly, removing the intermediate assessment type.
- Shared JSON-envelope parsing and delimiter-only recovery between temporal
  assessment and evidence-selector repair; each retains its existing schema
  validation, retry behavior and caller-specific recovery diagnostics.
- Added a parser regression for valid JSON, syntax-only closure and malformed
  content; the full Rust suite now passes 457 tests.
- Replaced research-source claiming's five-field positional row with a named
  source record; lease selection, chunk boundaries and cursor updates remain
  unchanged. The focused basket tests and full suite pass.
- Reused a named low-confidence review-candidate fixture for repeated ordinary
  user-claim setup while preserving each scenario's text, proposition and
  reason. High-confidence, confirmed and time-scoped candidates remain explicit.
- Composed answer evidence's attribution, polarity, modality and two scope fields
  into the existing `PropositionMetadata` record. SQL columns and model-facing
  JSON keys remain unchanged; polarity/modality still render as their stored
  strings at those boundaries.
- Passed the existing typed source-use view through final answer review instead
  of reparsing its model-facing JSON to recreate the same view. Review/generation
  JSON remains at the model boundary; JSON-to-view helpers are now test-only.
- Ran the Python grounding fixture-runner tests: 3 passed.
- Reused a single-source test fixture across ordinary grounded-reply and reviewer
  scenarios, retaining each source URL and passage as explicit inputs. All 67
  grounded-reply tests and the full 457-test Rust suite pass.
- Mapped the promotion paths before consolidating policy: initial intake applies
  finding/source assessment plus provenance, conflict and model-lead overrides;
  human and AI expertise reviews share a method-tagged writer; automatic
  reconsideration only clears stale map-conflict gates; corroboration promotion
  requires distinct findings and human-verified origins. These authorities are
  intentionally not collapsed into one approval verdict.
- Shared strict typed JSON-envelope deserialization across Curator, Curmudgeon
  and basket retention, preserving their caller-specific parser errors and
  leaving Scholar's shape repair and delimiter recovery distinct.
- Verified 10 provenance, 8 source-review, 6 semantic-support and 19 claim/
  observation tests; focused identity-scope/source-grounding and
  Curator/Curmudgeon/retention tests; all 29 Scholar and 67 grounded-reply
  tests; and the full Rust suite (457 passed, 0 failed). Formatting, offline
  compilation and `git diff --check` pass.
- Replaced research-basket completion's three-value source-state tuple with a
  named harvest-state row. Passage identity, UTF-8 offsets, job linkage and
  cursor validation remain unchanged; all 13 focused basket tests and the full
  Rust suite (457 passed, 0 failed) pass. At that point the source tree was 92 Rust
  files and 47,685 lines; this is 591 lines above the original review baseline,
  including the grounding/context work present before this plan was implemented.
- Composed the shared proposition fields in automatic-promotion and human-review
  finding records into `PropositionMetadata`; database projections, columns and
  stored strings are unchanged. Source-review (8), provenance (10), identity-
  scope (9) and full Rust (457) tests pass.
- Shared the contiguous SQLite row decoding of `PropositionMetadata` across
  answer evidence, finding intake, automatic promotion and human review. The
  schema-specific projection offsets remain explicit at each query; stored
  values and unknown-value handling are unchanged.
- Composed Scholar intake's existing-claim row from the same proposition type,
  moving its internal `relation` projection after the contiguous metadata
  fields. Match, conflict and unresolved-scope precedence is unchanged; the
  full Rust suite passes. The source tree is now 47,657 lines.
- Corrected the Rust file guide's inference entry to describe Candle and MLX
  and link to the README runtime section. Updated the README to describe chat's
  all-role maintenance worker and its background model lane; the runtime section
  remains the single detailed explanation.
- Shared curl process-launch failure handling between Wikipedia API requests
  and policy-checked page fetches. Both still record the same failed host
  reservation before returning the original process error; provider response
  parsing and access policies remain local. Added a reservation-state regression;
  the focused test and full Rust suite (458 passed) pass.
- Replaced guided-feedback session tuples and duplicate row decoders with one
  named session state used by retry admission and status logging. Retry counts,
  missing-session errors, guidance and logged fields remain unchanged. Feedback
  lifecycle tests and the full Rust suite (458 passed) pass.
- Reused prompt-data decoding in task and request tests instead of repeating
  JSON parsing at fixed message positions. Each test still asserts its own
  request, context, and omission conditions; both affected test groups pass.
- Centralized `ReplyServices` assembly for interactive chat, one-shot `say`, and
  browser replies. Each caller still owns its existing model, storage, session,
  and maintenance lifetimes. Focused Agent and web tests and the full Rust suite
  (458 passed) pass.
- Extracted initial prompt assembly, baseline token sizing and first-draft
  generation from `reply_inner` into one helper. Task/factual routing, challenge
  placement, the empty-evidence token comparison, and the research-backed answer
  path are unchanged. The challenge message is now computed once and reused in
  the evidence prompt and baseline. The full Rust suite (458 passed), offline
  check, formatting and `git diff --check` pass.
- Extracted turn research planning so task attempts still plan from the draft
  and explicit source-check constraint, while factual questions plan from
  selected source evidence alone. The distinct inputs and model route are
  unchanged. The full Rust suite (458 passed) and offline checks pass.
- Removed the redundant partial history rollback inside `deliver_reply`. Its
  sole caller, `reply`, already restores the complete prior conversation on any
  error, including save failure. The outer rollback and save error propagation
  remain unchanged; the complete 458-test suite passes.
- Combined response framing and HTTP metadata parsing for Wikipedia, public
  page fetches, and search providers. Each caller keeps its current missing or
  invalid status handling, body fallback, response decisions, and durable
  completion behavior. The full 458-test suite, offline check, formatting and
  `git diff --check` pass.
- Reused `valid_proposition_fields` for Curator and Scholar extraction instead
  of repeating its three bounded-field checks. Their boundary-specific error
  text and evidence/token checks remain. The full 458-test suite, offline check,
  formatting and `git diff --check` pass.
- Reused `valid_printable_field` for human/AI candidate review and model-claim
  approval, removing two copies of its whitespace, length and control-character
  checks while retaining each caller's validation error. The full 458-test
  suite, offline check, formatting and `git diff --check` pass.
- Centralized SQLite's repeated 12-month publication-date predicate for finding
  intake and source-expertise reviews. Their separate assessment gates remain
  local. Added regression coverage for current, cutoff, future, invalid and
  absent dates. The full 459-test suite, offline check, formatting and
  `git diff --check` pass.
- Reused the shared ordinary-candidate fixture in the disjoint user-employment
  period regression, leaving both validity dates explicit. Its targeted test,
  formatting and `git diff --check` pass; the full 459-test suite passed before
  this test-only fixture edit.
- Centralized the deterministic validity-period-and-evidence check used by
  Curator, Scholar extraction/intake, Curmudgeon, and candidate storage. Each
  boundary retains its own response to invalid dates (drop, reject, hold for
  review, or error). Added direct predicate coverage. The full 460-test suite,
  offline check, formatting and `git diff --check` pass.
- Composed `StoredScholarFinding`'s proposition fields with the existing typed
  `PropositionMetadata` and row decoder. Schema migrations constrain persisted
  polarity/modality values and backfill missing values as unknown, so this does
  not normalize away valid stored content. Updated metadata assertions. The full
  460-test suite, offline check, formatting and `git diff --check` pass.
- Extracted final answer finishing from `reply_inner`: task limitations are
  appended first, the uncertain personal-fact check runs only when a candidate
  exists, a positive check replaces the draft and clears thinking, and empty
  replies still fail. Added orchestration coverage. The full 461-test suite,
  offline check, formatting and `git diff --check` pass.
- Shared printable-field error construction across candidate review, source
  reviews, claim review, and model-claim approval. Each caller keeps its exact
  error text and 500-character contract; existing non-review guidance and model
  assessment checks remain local. The full 461-test suite, offline check,
  formatting and `git diff --check` pass.
- Reused `unix_time_ms_i64` for corroboration queue timestamps, removing its
  duplicate time acquisition and conversion while preserving its exact system
  time mapping and corroboration-specific overflow error. Basket scheduling
  keeps its distinct overflow behavior. The full 461-test suite, offline check,
  formatting and `git diff --check` pass.
- Shared exact quoted-scope validation across alias extraction, alias storage,
  and Scholar intake. Each boundary retains its existing context and source
  checks, including intake's separate control-token restriction. The focused
  identity-scope tests (10) and full Rust suite (461) pass; formatting and
  `git diff --check` pass. The nine-case live baseline is retained separately;
  one case timed out and none received independent semantic adjudication.
- Reused the shared printable-field predicate for Curmudgeon's parsed
  provenance rationale and SQLite's provenance-review write boundary. Their
  verdict allowlists and error behavior remain local. The provenance parser
  regression and full 461-test suite pass; formatting and offline compilation
  pass.
- Merged the two provenance-link and copied-passage warning branches during
  Scholar intake. Both retain their exact warning text and priority, and the
  shared-source branch remains separate; the evidence-policy verdict and gate
  rules are unchanged. All 10 provenance tests and the full 461-test suite pass,
  along with formatting and `git diff --check`.
- Shared Unix-millisecond acquisition and checked `i64` conversion across
  storage queue timestamps, basket scheduling, Scholar host waits and search
  scheduling, and Retry-After date handling. Callers retain their prior
  overflow error mappings and `Option` behavior. The full 461-test suite,
  offline check, formatting and `git diff --check` pass. The tree now has 92
  Rust files and 47,835 lines.
- Shared bounded contiguous-quote validation for Curmudgeon's suggested repair
  evidence and SQLite's feedback write boundary. Both still require the exact
  quote in the retained source passage and keep their own error text. The two
  focused regressions and full 461-test suite pass, as do the offline check,
  formatting and `git diff --check`. The tree has 92 Rust files and 47,836
  lines.
- Reused the already computed source-independence request result for each
  evidence item in the answer view, avoiding a repeated request-only check
  without changing which fields are exposed. The 39 grounded-reply tests and
  full 461-test suite pass; offline compilation, formatting and `git diff --check`
  pass. The Rust line count remains 47,836.
- Reused the rendered model-facing source view when the bounded rewrite loop
  regenerates an answer. Its inputs are the same evidence snapshot, request and
  currentness assessment, so rebuilding it could only repeat preparation; the
  reviewer still receives the separately sanitized view. All 67 grounded-reply
  tests and the full 461-test suite pass; offline check, formatting and
  `git diff --check` pass. The tree contained 92 Rust files and 47,838 lines.
- Shared the exact query-word scoring and deterministic tie-break logic used
  for Scholar evidence and semantic-claim retrieval. Each caller retains its
  own search-text projection, relevance source and SQL population. A focused
  regression covers relevance weighting and the default for unknown relevance;
  formatting, the complete 462-test suite, offline compilation and
  `git diff --check` pass. The tree contains 92 Rust files and 47,893 lines.
- Unified accepted grounded-answer finalization so ordinarily accepted answers
  and separately reviewed useful partials share citation-list rendering. Partial
  assessment conditions and its diagnostic remain unchanged. All 67 grounded-
  reply tests and the full 462-test suite, offline compilation, formatting and
  `git diff --check` pass. The tree contains 92 Rust files and 47,891 lines.
- Derived Scholar search-attempt status, result count and failure text from the
  existing explicit research outcome instead of separately inspecting the raw
  result three times. Found, no-result and failed outcomes retain their exact
  recorded values. The focused outcome regression and full 463-test suite pass,
  as do offline compilation, formatting and `git diff --check`. The tree
  contained 92 Rust files and 47,916 lines.
- Loaded a human-reviewed finding's source title and evidence text in the initial
  row read, then reused one typed finding for the saved semantic-support and
  legacy direct-support checks. Their separate acceptance gates, evidence bytes,
  and error outcomes remain unchanged. Source-review (8), provenance (10), and
  the full 463-test suite pass, as do offline compilation, formatting and
  `git diff --check`. The tree contains 92 Rust files and 47,895 lines.
- Extracted Scholar job completion into one helper that records the explicit
  search outcome, commits evidence or defers/fails the job, and completes
  linked feedback. Result logging, job statuses, error messages, audit events,
  and feedback outcomes are unchanged. All 31 Scholar tests and the full
  463-test suite pass, as do offline compilation, formatting and
  `git diff --check`. The tree contained 92 Rust files and 47,906 lines.
- Shared the exact last-day Scout investigation-count query between admission
  checks and status reporting. The threshold and status fields remain owned by
  their callers. All 5 Scout-admission tests and the full 463-test suite pass,
  along with offline compilation, formatting and `git diff --check`. The tree
  contained 92 Rust files and 47,902 lines.
- Shared the conversation research-job insert between ordinary and planned
  research enqueueing. Turn conversion, the plan insert, transaction ownership,
  and conflict behavior remain at their original boundaries. The focused Scout
  queue and source-review tests and full 463-test suite pass, along with offline
  compilation, formatting and `git diff --check`. The tree contains 92 Rust
  files and 47,912 lines.
- Shared retrieval of a finding's saved semantic-support assessment between
  review context construction and human acceptance review. Each caller retains
  its own parsing, source binding and decision checks. Source-review,
  semantic-support and the full 463-test suites pass, as do offline compilation,
  formatting and `git diff --check`. The tree contains 92 Rust files and
  47,908 lines.
- Shared the timestamped task-target stop-reason update across job failure,
  finding intake and human review. Each path still chooses its own reason and
  transaction. Source-review (8), Scout-admission (5), and full Rust (463) tests
  pass, along with offline compilation, formatting and `git diff --check`. The
  tree contains 92 Rust files and 47,915 lines.
- Reused the validated original observation text when an accepted candidate's
  evidence link is written, removing a second read in the same transaction.
  Date validation remains before curation, and rejected candidates do not fetch
  the text. Candidate/observation (19) and full Rust (463) tests pass, as do
  offline compilation, formatting and `git diff --check`. The tree contains 92
  Rust files and 47,911 lines.
- Shared the required latest-proposal lookup between human review and automatic
  promotion while preserving the same missing-row error. The similar intake
  lookup remains optional. Source-review (8), provenance (10), and full Rust
  (463) tests pass, along with offline compilation, formatting and
  `git diff --check`. The tree contained 92 Rust files and 47,915 lines.
- Shared the timestamped active-claim update between intake auto-promotion and
  human review. Their distinct approval checks still determine whether the
  update runs. Source-review (8), provenance (10), and full Rust (463) tests
  pass, as do offline compilation, formatting and `git diff --check`. The tree
  contains 92 Rust files and 47,912 lines.
- Shared Scholar semantic-claim insertion between evidence intake and automatic
  corroboration promotion, including all proposition and validity fields. Each
  path retains its eligibility gates, lookup and source choice. Source-grounding
  (6), provenance (10), identity-scope, and full Rust (463) tests pass, as do
  offline compilation, formatting and `git diff --check`. The tree contains 92
  Rust files and 47,947 lines.
- Shared the raw observation row reader between extraction finalization and
  candidate review. Both still consume the same stored user text and map a
  missing observation to the same database error. Candidate/observation (19),
  migration, and full Rust (463) tests pass, as do offline compilation,
  formatting and `git diff --check`. The tree contains 92 Rust files and
  47,946 lines.
- Shared the Scholar job-processing update between ordinary job claims and
  feedback repairs. Both callers retain their own transaction and selection
  rules. Research (24), feedback (10), and full Rust (463) tests pass, as do
  offline compilation, formatting and `git diff --check`. The tree contains
  92 Rust files and 47,954 lines.
- Shared the SQLite date-time validity query for claim reviews and review
  schedules. Each caller keeps its existing validation scope and error text.
  Maintenance (4) and full Rust (463) tests pass, as do offline compilation,
  formatting and `git diff --check`. The tree contains 92 Rust files and
  47,954 lines.
- Shared the exact SQLite current-time query between Scholar retry timing and
  network usage reporting, preserving its whole-second millisecond value and
  database clock. Research (24), network (6), and full Rust (463) tests pass,
  as do offline compilation, formatting and `git diff --check`. The tree
  contains 92 Rust files and 47,943 lines.
- Shared the reserved-search completion write for SerpAPI and Tavily. Both
  retain their table, provider-specific pending-reservation error, outcome,
  timestamp and 500-character error limit. Network (10) and full Rust (463)
  tests pass, as do offline compilation, formatting and `git diff --check`.
  The tree contains 92 Rust files and 47,949 lines.
- Shared the exact semantic-claim challenged-state update between candidate
  conflict resolution and Scholar review. Intake's guarded update remains
  separate. Candidate/observation (19), source-review (8), and full Rust (463)
  tests pass, as do offline compilation, formatting and `git diff --check`.
  The tree contains 92 Rust files and 47,954 lines.
- Named the conversion from stored Scholar findings to the grounding-review
  evidence view. It retains every field and remains separate from the narrower
  answer-generation view. Grounded-answer (67) and full Rust (463) tests pass,
  as do offline compilation, formatting and `git diff --check`. The tree
  contains 92 Rust files and 47,964 lines.
- Added a named active-semantic-claim row reader for the identical six-column
  projection used in candidate curation and candidate conflict handling. Both
  query order and conflict decisions remain unchanged. Claim-observation (19)
  and full Rust (463) tests pass, as do offline compilation, formatting and
  `git diff --check`. The tree contains 92 Rust files and 47,968 lines.
- Shared the latest-provider-search timestamp query between corroboration
  readiness and the network status API. Each caller retains its provider
  validation and error message, and the lookup remains optional. Network (10),
  corroboration (7), and full Rust (463) tests pass, as do offline compilation,
  formatting and `git diff --check`. The tree contains 92 Rust files and
  47,968 lines.
- Shared the superseded-claim write between candidate conflict resolution and
  Scholar review. Each caller retains its own reason text and transaction.
  Claim-observation (19), source-review (8), and full Rust (463) tests pass, as
  do offline compilation, formatting and `git diff --check`. The tree contains
  92 Rust files and 47,990 lines.
- Shared the feedback-session state lookup between queueing and feedback event
  logging. Both paths retain the required-row behavior and common row decoder.
  Feedback storage (7), Scholar feedback (1), and full Rust (463) tests pass,
  as do offline compilation, formatting and `git diff --check`. The tree
  contains 92 Rust files and 47,994 lines.
- Shared the parameterized schema-column existence query across three recent
  database upgrades. Each migration keeps its current column definitions and
  version handling. Migration (8) and full Rust (463) tests pass, as do offline
  compilation, formatting and `git diff --check`. The tree contains 92 Rust
  files and 47,986 lines.
- Shared the optional SQLite table-definition lookup across two constraint
  upgrades. Their distinct constraint checks and replacement schemas are
  unchanged. Migration (8) and full Rust (463) tests pass, as do offline
  compilation, formatting and `git diff --check`. The tree contains 92 Rust
  files and 47,983 lines.
- Shared the identical decision/rationale response record between Curator's
  claim and observation reviews. Allowed decisions, prompts, and caller-specific
  validation remain separate. Curator (10) and full Rust (463) tests pass, as
  do offline compilation, formatting and `git diff --check`. The tree contains
  92 Rust files and 47,977 lines.
- Reused the structured-output decision/rationale type in Curmudgeon's candidate
  review as well as Curator's claim and observation reviews. Their valid
  decisions, prompts and caller-specific checks remain distinct. Curator (10),
  Curmudgeon (17), and full Rust (463) tests pass, as do offline compilation,
  formatting and `git diff --check`. The tree contains 92 Rust files and
  47,972 lines.
- Shared the idempotent candidate-to-semantic-claim source link between
  candidate review and equivalent-source attachment. Both paths retain their
  own candidate selection and evidence-link behavior. Claim-observation (19)
  and full Rust (463) tests pass, as do offline compilation, formatting and
  `git diff --check`. The tree contains 92 Rust files and 47,981 lines.
- Reused the shared raw-observation reader in equivalent-source attachment,
  completing its use across extraction finalization, candidate review and
  source attachment. Claim-observation (19) and full Rust (463) tests pass, as
  do offline compilation, formatting and `git diff --check`. The tree contains
  92 Rust files and 47,975 lines.
- Reused one test helper for the identical retained-claim byte-span query in
  Unicode and source-chunk-boundary tests. Both tests keep their separate
  assertions. The full Rust suite (463) passes, as do formatting and
  `git diff --check`; the tree contains 92 Rust files and 47,972 lines.
- Selected factual-turn knowledge-map claims once and reused that exact ranked
  snapshot for context diagnostics, the answer prompt and its token-comparison
  baseline. Ranking limit, prompt contents, and diagnostics are unchanged; the
  agent tests (93) and full Rust suite (463) pass, as do locked offline
  compilation, formatting and `git diff --check`. The tree contains 92 Rust
  files and 48,008 lines.
- Reused the Scholar semantic-claim insertion helper in human finding review,
  extending it to carry the already-computed optional supersession link. Claim
  fields, timestamps, provenance columns and review gates are unchanged. Source
  review (8) and full Rust (463) tests pass, as do locked offline compilation,
  formatting and `git diff --check`. The tree contains 92 Rust files and
  48,013 lines.

Formatting, offline compilation, focused agent/Scholar/storage tests and the
complete 463-test suite pass after these changes. The remaining plan is still
open, including broader proposition/source records, further turn-flow
simplification, provider request mechanics beyond response framing, storage
deduplication, evaluation and documentation alignment. The model-dependent
interpretation and architectural reductions still require their proposed
benchmark and compatibility evidence. No migration, ranking-policy change,
model-policy change or benchmark claim is included in these slices.

## Follow-up progress preserved from the active plan

These entries were moved verbatim from the active plan to keep its current status and remaining scope concise.

- Factual turns now select knowledge-map claims once and reuse the same ranked
  snapshot for diagnostics, generation and token comparison. Agent (93) and full
  Rust (463) tests pass.
- Human Scholar finding review now shares the semantic-claim insertion helper,
  including its optional supersession link. Source review (8) and full Rust
  (463) tests pass. Locked offline compilation, formatting and
  `git diff --check` pass for both changes; the tree contains 92 Rust files and
  48,013 lines.
- Centralized bounded text excerpts and reused the Unicode-safe helper for
  repeated 1,000-, 1,200-, 1,500-, 2,000- and 6,000-character model-output,
  answer and passage excerpts. The Unicode boundary regression and full Rust
  suite (464) pass, as do locked offline compilation, formatting and
  `git diff --check`; no direct 1,200-character truncations remain in `src/`.
- Shared the guarded Scholar-finding promotion update between automatic
  two-source promotion and later corroboration. Callers retain their distinct
  reasons, source-link audits and eligibility checks. Provenance (10) and full
  Rust (464) tests pass, as do locked offline compilation, formatting and
  `git diff --check`; the tree contains 92 Rust files and 48,037 lines.
- Centralized idempotent Scholar-finding-to-claim link insertion across
  evidence intake and both promotion paths. Relationship, rationale and
  assessment-method values remain caller-specific. Provenance (10) and full
  Rust (464) tests pass, as do locked offline compilation, formatting and
  `git diff --check`; the shared SQL now has one production definition.
- Shared the identical curl response-metadata trailer between Wikipedia and
  policy-checked page fetching. Their marker values, request options, and
  response handling stay local; search providers retain their shorter trailer.
  The focused contract test and full Rust suite (465) pass, as do locked offline
  compilation, formatting and `git diff --check`. The tree contains 92 Rust
  files and 48,047 lines.
- Centralized the shared `research_sources` row insert behind a named source
  record. Ordinary retention and finding archival keep separate hashes,
  truncation limits, scopes, provider metadata and capture statuses. The full
  Rust suite (465) passes, as do locked offline compilation, formatting and
  `git diff --check`; the table insert now has one production definition. The
  tree contains 92 Rust files and 48,092 lines.
- Shared the named semantic-claim scope row and decoder between source-review
  conflict resolution and automatic promotion. Their query filters and caller
  decisions remain separate; the common ID/object/validity/geographic/context
  fields are read once. Source review and full Rust (465) tests pass, as do
  locked offline compilation, formatting and `git diff --check`. The tree
  contains 92 Rust files and 48,089 lines.
- Centralized the identical SQLite-generated Scholar job ID query used by
  Scout admission, corroboration queueing and feedback repair. Each path keeps
  its own transaction and job creation fields. The full Rust suite (465) passes,
  as do locked offline compilation, formatting and `git diff --check`; the
  query now has one definition for these job paths.
- Replaced two `SELECT last_insert_rowid()` round trips with rusqlite's direct
  transaction method. Both operations retain the same transaction and inserted
  claim; candidate/observation (15) and full Rust (465) tests pass, as do
  locked offline compilation, formatting and `git diff --check`. No SQL
  `last_insert_rowid()` queries remain, and the tree is 48,086 Rust lines.
- Shared the maintenance scan-cursor read between rotating claim scans and the
  status report. The scan still defaults a missing row to zero; status still
  reports the missing-row database error. Formatting, locked offline
  compilation, all four focused maintenance tests and the full Rust suite
  (465) pass; `git diff --check` is clean. The tree contains 92 Rust files and
  48,083 lines.
- Moved ordered deduplication of repeated source URLs into the shared citation
  list helper and removed the repair path's duplicate loop. Distinct sources
  retain their order and labels. Citation (3), grounded-reply (39) and full
  Rust (465) tests pass, as do formatting and `git diff --check`; the tree has
  92 Rust files and 48,087 lines.
- Centralized the shared human/AI accept/reject policy-outcome mapping used by
  candidate-claim and Scholar-finding review. Their review gates, transaction
  updates and resulting states remain caller-owned. Candidate/observation (19),
  source-review (8) and full Rust (465) tests pass, as do locked offline
  compilation, formatting and `git diff --check`. The tree has 92 Rust files
  and 48,072 lines.
- Shared answer-token encoding, end-of-message handling, and token/context
  bounds between Candle and MLX training. Each backend still builds its prompt
  and performs training through its own implementation. Locked offline
  compilation, formatting, `git diff --check` and the full Rust suite (465)
  pass; the tree has 92 Rust files and 48,069 lines.
- Replaced repeated `InvalidData` constructors and one identical local closure
  with a shared error helper. Centralized the matching JSON parse-error
  constructor while retaining serde errors as their sources. `InvalidInput`
  and storage's module-local serde wrapper remain separate. Locked offline
  compilation, formatting, `git diff --check` and the full Rust suite (465)
  pass; the file guide covers all 93 Rust files, totaling 48,043 lines.
- Consolidated 38 test-only temporary-path builders into one fixture helper,
  preserving each test's path prefix, database/directory use, and cleanup.
  The full Rust suite (465), locked offline compilation, formatting and
  `git diff --check` pass. The file guide covers all 94 Rust files; the tree
  contains 47,836 Rust lines.
- Shared the reserved curl response-envelope handling between Wikipedia and
  policy-checked page fetching. Callers retain their own missing-status,
  content-type, transport, cache and retry decisions. The regression for a
  missing metadata trailer and the full Rust suite (466) pass, as do locked
  offline compilation, formatting and `git diff --check`. The tree contains
  94 Rust files and 47,873 lines.
- Composed the proposition subject, relation, validity and geographic/context
  scope into one named record across human source-origin review and automatic
  promotion. The existing review and promotion SQL gates remain separate.
  Both origin/promotion regressions, locked offline compilation, formatting,
  `git diff --check` and the full Rust suite (466) pass.
- Reused one curl response-body decoder for Tavily, its advanced retry, SearXNG
  and SerpAPI. Providers retain their response schemas and status/retry
  handling. Trailer and missing-trailer cases plus the full Rust suite (466)
  pass; formatting, locked offline compilation and `git diff --check` pass.
  The tree contains 94 Rust files and 47,866 lines.
- Switched search providers to the shared curl metadata formatter. Discovery's
  prior trailer omitted the content-type field expected by the shared parser,
  so `Retry-After` was read from the wrong position. The formatter contract
  and retry-after fixture tests, full Rust suite (466), locked offline
  compilation, formatting and `git diff --check` pass. The tree contains
  94 Rust files and 47,864 lines.
- Reused the shared character-count excerpt helper for the two provenance
  passages and metadata fields. Their 1,400- and 2,500-character limits and
  fallback text are unchanged. Formatting, `git diff --check` and the full Rust
  suite (466) pass; the tree contains 94 Rust files and 47,861 lines.
- Reused the shared serde-to-`InvalidData` conversion across conversation
  review, entity extraction, semantic review, Scholar decoding and HTTP status
  serialization/parsing. The serde error remains the source; other error
  categories retain their existing constructors. Locked offline compilation,
  formatting, `git diff --check` and the full Rust suite (466) pass. The tree
  contains 94 Rust files and 47,860 lines.
- Composed semantic-claim scope and Scholar proposition scope from the same
  object/validity/geographic/context record, with one row decoder for those
  shared fields. The claim ID and finding subject/relation remain distinct.
  Origin/promotion regressions, locked offline compilation, formatting,
  `git diff --check` and the full Rust suite (466) pass. The tree contains
  94 Rust files and 47,871 lines.
- Composed the identical archived quote fields in stored findings and answer
  evidence into `ScholarEvidenceSpan`, with one row decoder. The two enclosing
  types remain separate so stored status/history and answer eligibility retain
  distinct guarantees; archive IDs, byte offsets, URLs and exact quote text are
  unchanged. The full Rust suite (466), locked offline compilation, formatting
  and `git diff --check` pass. This explicit shared type adds 29 net Rust lines;
  the tree contains 94 Rust files and 47,900 lines.
- Shared the direct passage-support decision between Scholar intake and legacy
  human review. Both write boundaries still independently require the exact
  evidence quote in the retained passage; semantic assessment and other review
  gates remain separate. Added a regression for encyclopedia title identity
  and exact text. The full Rust suite (467), locked offline compilation,
  formatting and `git diff --check` pass. The tree contains 94 Rust files and
  47,927 lines.
- Parsed each successful discovery response's body and retry metadata in one
  pass. Failed requests retain the metadata-only handling path. Trailer,
  retry-after and missing-trailer fixtures, locked offline compilation,
  formatting, `git diff --check` and the full Rust suite (467) pass. The tree
  contains 94 Rust files and 47,948 lines.
- Replaced request-intent and property-span JSON indexing with typed response
  records. Raw intent JSON remains in diagnostics, and individual non-string
  spans continue to be ignored while valid anchored spans are retained. The
  new schema regression and full Rust suite (468), locked offline compilation,
  formatting and `git diff --check` pass. The tree contains 94 Rust files and
  47,996 lines.
- Shared exact geographic/context scope equality across finding review and
  automatic-promotion conflict checks through `ScopedProposition`. Validity
  periods, object matching, source eligibility and review authority remain on
  their existing paths. The focused storage/research tests (24) and full Rust
  suite (471) pass, as do locked offline compilation, formatting and
  `git diff --check`. The tree contains 94 Rust files and 48,041 lines.
- Replaced 26 identical in-memory SQLite test constructors across storage,
  Scholar HTTP and grounding tests with one shared test fixture. Each call
  still opens an independent store; file-backed and restart tests retain their
  own setup. The focused storage/research tests (24), Scholar HTTP regression
  and full Rust suite (471) pass, as do formatting and `git diff --check`.
  The tree contains 94 Rust files and 48,050 lines.
- Removed the duplicate policy-page `USER_AGENT` constant and reused Scholar's
  shared value, already used by Wikipedia. Site-policy tests (9) and the full
  Rust suite (471) pass, as do formatting and `git diff --check`. The tree
  contains 94 Rust files and 48,050 lines.
- Composed repeated provider/job/query fields into one `SearchAttempt` context
  for discovery-ledger writes across Tavily, its advanced retry, SearXNG and
  SerpAPI. Providers retain their own outcome mappings, errors, quota completion
  and persistence decisions. Discovery tests (7) and the full Rust suite (471)
  pass, as do locked offline compilation, formatting and `git diff --check`.
  The tree contains 94 Rust files and 47,925 lines.
- Shared the result-count/error mapping that records `error`, `no_result` or
  `results` for provider search attempts. Added a ledger regression preserving
  result counts and error text across all outcomes. Discovery tests (8) and the
  full Rust suite (472) pass, as do locked offline compilation, formatting and
  `git diff --check`. The tree contains 94 Rust files and 47,941 lines.
- Reused the exact-evidence scope validator for attribution as well as
  geographic/context scope during extraction. All four metadata fields keep the
  same trimming, 240-character, literal-evidence and control-character rules.
  Focused proposition tests (2), full Rust (472), locked compilation, formatting
  and `git diff --check` pass. The tree contains 94 Rust files and 47,930 lines.
- Compute the selected-passage character count, diagnostic excerpt and
  truncation flag once, then reuse them in extraction-repair and final-decision
  events. The truncation regression and full Rust suite (472) pass, as do locked
  compilation, formatting and `git diff --check`. The tree contains 94 Rust
  files and 47,934 lines.
- Reused the storage enum converters for extracted polarity and modality after
  preserving extraction's trim/lowercase normalization. A focused regression
  covers padded mixed-case values; proposition tests (2), full Rust (472),
  locked compilation, formatting and `git diff --check` pass. The tree contains
  94 Rust files and 47,926 lines.
- Shared the named Wikipedia page-title predicate between candidate filtering
  and relevance scoring, preserving exact and parenthesized title matches. All
  Scholar tests (33), the serial full suite (472), locked compilation,
  formatting and `git diff --check` pass. The parallel full suite had one
  unrelated inference lease test fail with `WouldBlock` (471/472). The tree
  contains 94 Rust files and 47,924 lines.
- Reused one exact-source-quote predicate in bounded quote validation, research
  basket validation and Scholar intake. Existing quote length limits, ingestion
  controls, proposition checks and confidence gates remain at their original
  boundaries. Storage tests (108), the serial full suite (472), locked
  compilation, formatting and `git diff --check` pass. The tree contains 94
  Rust files and 47,924 lines.
- Shared the literal-evidence and printable-length check used for proposition
  metadata extraction and stored scope validation; stored scopes still must
  also occur in the full source. The focused scope regression and serial full
  suite (472) pass, as do locked compilation, formatting and `git diff --check`.
  The tree contains 94 Rust files and 47,921 lines.
- Unified the two identical 32,000-character research-source retention limits
  as one storage invariant. The harvest-source truncation regression preserves
  Unicode counts, full-input hashes and retained bytes; the focused regression
  and serial full suite (472) pass, as do locked compilation, formatting and
  `git diff --check`. The tree contains 94 Rust files and 47,921 lines.
- Centralized source-body truncation, Unicode counts, full-input hashing and
  checked integer conversion for Scholar intake and retained research sources.
  Their 32,000-character limit is shared; capture status and source scopes stay
  caller-specific. The truncation regression and serial full suite (472) pass,
  as do locked compilation, formatting and `git diff --check`. The tree
  contains 94 Rust files and 47,934 lines.
- Composed the identical finding status, stored passage and exact evidence quote
  into one typed record shared by source-expertise and source-origin review.
  Confidence, source kind, proposition scope and both review gates remain
  distinct. All 8 source-review tests and the serial full suite (472) pass, as
  do locked compilation, formatting and `git diff --check`. The tree contains
  94 Rust files and 47,949 lines.
- Reused that evidence component in ordinary finding review as well, so all
  three review paths now share one status/passage/quote record. Their differing
  decision data and policies remain separate. The serial full suite (472),
  locked compilation, formatting and `git diff --check` pass. The tree contains
  94 Rust files and 47,949 lines.
- Replaced the four-string tuple in retained-source import with a named row
  record for job, title, URL and body. Import defaults and the existing excerpt
  hash, review status and stored text remain unchanged. The import regression
  and serial full suite (472) pass, as do locked compilation, formatting and
  `git diff --check`. The tree contains 94 Rust files and 47,956 lines.
- Shared the finite 0–1 confidence validator between basket claims and Scholar
  findings. It preserves the same inclusive bounds and rejects NaN/infinity at
  both write boundaries. Basket-item and source-grounding tests, the serial full
  suite (472), locked compilation, formatting and `git diff --check` pass. The
  tree contains 94 Rust files and 47,958 lines.
- Flattened the nested publication-date validation branch in Curmudgeon and
  inlined the nested formatting used to build the research-reply review prompt.
  Both preserve their original conditions and prompt text. Curmudgeon tests
  (14), research-reply review tests (9), targeted Clippy formatting validation,
  `cargo fmt --check` and `git diff --check` pass. These changes reduce the
  Rust tree to 94 files and 47,955 lines.
- Flattened nested guards in SerpAPI's structured-result traversal and added a
  regression for nested candidate fields and URL cleanup. The discovery tests
  (9), serial full suite (473), locked compilation, formatting and
  `git diff --check` pass. The tree contains 94 Rust files and 47,972 lines.
- Flattened nested conditions for research chunk trimming, stored review-date
  validation, and recovery of finished Scholar feedback. Short-circuit order
  and existing state transitions are unchanged. The serial full suite (473),
  locked compilation, formatting and `git diff --check` pass. The tree contains
  94 Rust files and 47,971 lines.
- Flattened possessive-name validation/deduplication and the empty research
  selection error guard. Existing name filters, fallback selections and error
  conditions are unchanged. Entity tests (7), research tests (24), the serial
  full suite (473), locked compilation, formatting and `git diff --check` pass.
  The tree contains 94 Rust files and 47,970 lines.
- Aligned the README and three current architecture/operations notes with the
  implemented schema version 63. Kept schema 44–47 as the original rollout
  history and retained the documented evidence, audit, claim and JSON-map
  compatibility statements. `git diff --check` passes; this documentation-only
  correction does not change the Rust tree (94 files, 47,958 lines).
- Flattened nested guards in Wikipedia API error handling, fetched-candidate
  processing, provenance URL normalization and Scholar extraction/repair. Their
  evaluation order and outcomes are unchanged. Focused Wikipedia (8), provenance
  (17) and extraction (15) tests, locked compilation, formatting and
  `git diff --check` pass. The tree contains 94 Rust files and 47,958 lines.
- Unified semantic-claim row decoding by giving active-claim queries an explicit
  `NULL` relevance column. Relevant claims still read the stored relevance;
  active-claim records still expose none. The full serial Rust suite (473),
  locked compilation, formatting and `git diff --check` pass. The tree contains
  94 Rust files and 47,950 lines.
- Flattened the site-policy cache fast path and Wikipedia page-retention guard.
  Cache lookup ordering, response-size limits, URL eligibility and source
  retention remain unchanged. Site-policy (9) and Wikipedia (8) tests, focused
  collapsible-if Clippy, formatting and `git diff --check` pass. The tree
  contains 94 Rust files and 47,950 lines.
- Returned shared named HTTP response metadata from discovery's error-path
  parser instead of a positional status/retry tuple. Missing metadata still
  yields no status or retry delay. Discovery tests (10), the full serial suite
  (473), locked compilation, formatting and `git diff --check` pass. The tree
  contains 94 Rust files and 47,953 lines.
- Shortened the active simplification plan by removing its duplicate progress
  log after verifying that all 29 entries from its staged version are retained
  verbatim in the dated history. Preserved nine additional entries that were
  absent from that archive. The active plan is 173 lines shorter; no progress
  evidence was discarded.


## Earlier progress entries retained during plan cleanup

- Replaced retrieval-expansion and source-check intent `Value` lookups with
  typed response records. Retrieval still allows an empty list, returns strings
  exactly as supplied, and enforces the same three-item/120-character bounds;
  source-check intent still rejects missing or non-boolean values. Added a
  retrieval-contract regression; the source-check tests and full Rust suite
  (469) pass, as do locked offline compilation, formatting and
  `git diff --check`. The tree contains 94 Rust files and 48,020 lines.
- Removed a duplicate semantic-support predicate evaluation during Scholar
  intake and reused the same optional assessment reference for validation and
  fallback selection. Its assessment remains an acceptance gate, and legacy
  direct passage support remains the fallback only when no assessment exists.
  Grounding tests and the full Rust suite (469) pass, as do locked offline
  compilation, formatting and `git diff --check`. The tree contains 94 Rust
  files and 48,020 lines.
- Replaced section-selection JSON indexing with a typed response record while
  retaining fenced-object parsing, the two-section limit, valid-index checks,
  ordered de-duplication and rejection of non-string indices. The focused
  regression and full Rust suite (469) pass, as do locked offline compilation,
  formatting and `git diff --check`. The tree contains 94 Rust files and
  48,034 lines.
- Normalized Tavily, SearXNG and SerpAPI results into one search-candidate
  record and shared their candidate-persistence loop. Provider parsing,
  categories, structured SerpAPI data, quotas and completion/error paths remain
  distinct. Provider regressions and the full Rust suite (470) pass, as do
  locked offline compilation, formatting and `git diff --check`. The tree
  contains 94 Rust files and 48,018 lines.
- Centralized shared `ScholarEvidenceForAnswer` fixture defaults for research
  selection and Agent prompt tests. Archive spans, confidence, review state,
  source origin and conflicts remain explicit where each scenario differs.
  Focused Agent/research tests and the full Rust suite (470) pass, as do locked
  offline compilation, formatting and `git diff --check`. The tree contains
  94 Rust files and 48,026 lines.
- Updated the Rust file guide to describe the answer-evidence fixture now shared
  by tests, alongside its existing unique temporary-path helper.
- Combined Tavily response deserialization and normalization for its basic and
  advanced search paths. Their separate retry, completion and error-recording
  decisions are unchanged. All seven discovery tests and the full Rust suite
  (470) pass, as do locked offline compilation, formatting and
  `git diff --check`. The tree contains 94 Rust files and 48,024 lines.
- Reused one normalized content type and HTML/XHTML predicate for policy-page
  provenance extraction and text conversion. Plain text and unsupported-content
  outcomes remain unchanged. Site-policy tests (9) and the full Rust suite
  (470) pass, as do locked offline compilation, formatting and
  `git diff --check`. The tree contains 94 Rust files and 48,018 lines.
- Shared Wikipedia's non-content section predicate between API-section ranking
  and article-passage filtering. Both still exclude the same reference and
  navigation sections. Added a regression that checks each path; Wikipedia
  tests (3) and the full Rust suite (471) pass, as do locked offline compilation,
  formatting and `git diff --check`. The tree contains 94 Rust files and
  48,030 lines.
- Consolidated provider response status and retry metadata into the shared HTTP
  metadata record and routed six identical discovery host-completion calls
  through one helper. Provider result/error outcomes remain local. Discovery
  tests (10), the full serial Rust suite (473), locked compilation, formatting
  and `git diff --check` pass. The tree contains 94 Rust files and 47,956 lines.
- Moved reserved HTTP response completion into one Scholar helper shared by
  search, Wikipedia and policy-page fetching. Search still ignores ledger-write
  failures; Wikipedia and policy fetches still propagate them. Focused discovery
  (10), Wikipedia (8) and site-policy (9) tests and the full serial suite (473)
  pass, as do compilation, formatting and `git diff --check`. The tree contains
  94 Rust files and 47,951 lines.
- Shared the literal evidence-in-passage operation across source-expertise,
  source-origin and legacy finding review. Callers retain their original quote
  values, review-specific rejection paths and promotion gates. Source-review,
  source-grounding and full serial Rust tests (473) pass, as do locked
  compilation, formatting and `git diff --check`. The tree contains 94 Rust
  files and 47,953 lines.
- Reordered finding-review query columns so it reuses the same `FindingEvidence`
  row decoder as source-expertise and source-origin review. Proposition fields,
  selected values and review gates are unchanged. Source-review and
  source-grounding tests plus the full serial Rust suite (473) pass, as do
  locked compilation, formatting and `git diff --check`. The tree contains 94
  Rust files and 47,949 lines.
- Unified evidence persistence in Scholar outcome finalization for found,
  no-result and fetched-source fallback outcomes. Failure and deferral paths
  still skip that persistence, and feedback completion remains unchanged. The
  Scholar tests and full serial Rust suite (473) pass, as do locked compilation,
  formatting and `git diff --check`. The tree contains 94 Rust files and
  47,947 lines.
- Replaced identical due/challenged maintenance topic tuples with one named
  `ClaimTopic` record and row decoder. Query filters and ordering remain distinct;
  Scout continues using the same claim fields in its questions. Scout tests (24),
  the full serial suite (473), locked compilation, formatting and
  `git diff --check` pass. The tree contains 94 Rust files and 47,975 lines.
- Replaced the latest-feedback diagnostic's positional three-field tuple with
  `FeedbackTaskSummary` and its row decoder. Feedback ID, route and exclusion
  reason remain the exact selected values. Feedback tests (10), the full serial
  suite (473), locked compilation, formatting and `git diff --check` pass. The
  tree contains 94 Rust files and 47,991 lines.
- Centralized the repeated checked conversion from storage query limits to
  SQLite integers, retaining the same `limit too large` error. Zero-limit
  behavior and the bounded maintenance scan remain unchanged. Storage and Scout
  tests plus the full serial suite (473) pass, as do compilation, formatting and
  `git diff --check`. The tree contains 94 Rust files and 47,977 lines.
- Routed challenged and stale claim topics through one Scout queue/deduplication
  helper. Their questions, reasons, source ordering and skip behavior remain
  unchanged. Scout tests (24), the full serial suite (473), locked compilation,
  formatting and `git diff --check` pass. The tree contains 94 Rust files and
  47,984 lines.
- Centralized strict JSON-object-to-type conversion for property-span and
  source-check assessments. Envelope extraction, syntax diagnostics and type
  errors retain the same behavior; paths that need the parsed object or
  delimiter recovery remain specialized. Request tests (22), source-check tests
  (3), the full serial suite (473), locked compilation, formatting and
  `git diff --check` pass. The tree contains 94 Rust files and 47,988 lines.
- Unified missing/invalid HTTP-status reservation completion for Wikipedia and
  public-site fetches. The callers keep their exact error text and prior check
  order. A regression confirms the reservation completes exactly once. HTTP
  status (1), Wikipedia (8), site-policy (9), the full serial suite (474),
  locked compilation, formatting and `git diff --check` pass. The tree contains
  94 Rust files and 48,024 lines.
- Replaced the AI review queue's positional `(finding_id, job_id)` row with a
  named internal record and updated Curmudgeon and the restart/retry regression
  to use its fields. Queue eligibility, ordering and persistence are unchanged.
  The retry test, Curmudgeon tests (18), the full serial suite (474), locked
  compilation, formatting and `git diff --check` pass. The tree contains 94
  Rust files and 48,041 lines.
- Replaced assistant reply attention's positional triple with a named
  `{ reply_id, status, error }` record. The CLI retains the same values and
  formatting. Its mapping regression and the full serial suite (475), locked
  compilation, formatting and `git diff --check` pass. The tree contains 94
  Rust files and 48,075 lines.
- Replaced independent-corroboration's four-field positional tuple with a
  named record for finding ID, job ID, source URL and retained excerpt. The
  promotion and provenance updates retain the same values and ordering. The
  two-source review/promotion regressions, full serial suite (475), locked
  compilation, formatting and `git diff --check` pass. The tree contains 94
  Rust files and 48,080 lines.
- Replaced cached site-policy's positional `(status, content_type, body)` row
  with named fields and updated its consumer and restart/cache assertions. The
  cache hit path preserves the same complete response values. The focused cache
  test, full serial suite (475), locked compilation, formatting and
  `git diff --check` pass. The tree contains 94 Rust files and 48,097 lines.
- Replaced cached source pages' positional `(content_type, body)` result with
  named fields, updated the site-policy fetch path, and kept the full-body cache
  and expiry regression on the same content. The focused cache test, full serial
  suite (475), locked compilation, formatting and `git diff --check` pass. The
  tree contains 94 Rust files and 48,112 lines.
- Routed the three capped network/corroboration query limits through the shared
  checked SQLite conversion. Their caps are unchanged; the rotating maintenance
  scan keeps its separate bounded fallback. Network tests (17), corroboration
  tests and the full serial suite (475), locked compilation, formatting and
  `git diff --check` pass. The source line count remains 48,112.
- Simplified currentness error side-effect handling with `inspect_err` and
  replaced two closure-based character splitters with direct character
  patterns. Error values and delimiters are unchanged. Currentness tests (10),
  property-repair tests (3), full serial suite (475), locked compilation,
  formatting and `git diff --check` pass. The tree contains 94 Rust files and
  48,111 lines.
- Removed needless borrows in robots-policy and page-selection paths; retained
  the required borrow around the owned robots-rule vector. Focused Clippy checks
  for needless borrows, manual inspection and character-pattern comparisons
  pass, as do site-policy tests (9), the full serial suite (475), locked
  compilation, formatting and `git diff --check`. The source line count remains
  48,111.
- Grouped Scholar's per-search inputs in a named `ResearchRequest`, leaving the
  mutable store and model explicit and reducing `research` from nine arguments
  to three. Search selection, evidence retention and extraction remain the
  same. Locked compilation, formatting, `git diff --check` and the full serial
  suite (475) pass. The tree contains 94 Rust files and 48,126 lines.
- Grouped the conversation resolver's request, pending observations, initial
  evidence, plan, answer and thinking trace into a borrowed `ResearchTurn`
  snapshot. The resolver now has four inputs including `self`; the plan,
  evidence reads and answer updates are unchanged. Locked compilation,
  formatting, `git diff --check` and the full serial suite (475) pass. The tree
  contains 94 Rust files and 48,140 lines.
- Replaced two single-branch `match` expressions with `let...else` for missing
  provider credentials and a failed model checkpoint. Queue error recording,
  checkpoint cleanup and returned errors remain the same. The focused provider
  and model tests, locked compilation, formatting, `git diff --check` and
  Clippy with `-D clippy::manual_let_else` pass. The tree contains 94 Rust
  files and 48,131 lines.
- Replaced paired `map`/`unwrap_or_else` calls with `map_or_else`, changed
  method-only closures to method references, and converted the Scholar section
  fallback to `map_or`. Each option/result fallback and conversion is preserved.
  Formatting, locked compilation, strict Clippy for both lint classes,
  `git diff --check` and the full serial suite (475) pass. The tree contains
  94 Rust files and 48,143 lines.
- Simplified verified source-origin selection to one `Option::filter`, keeping
  both the verification requirement and nonempty-key check. The four source
  limitation tests, locked compilation, formatting and `git diff --check` pass.
  The tree contains 94 Rust files and 48,141 lines.
- Routed CLI's model, adapter, state, knowledge and database paths through one
  lazy environment/default resolver. The model-relative adapter fallback and
  all configured/default paths are unchanged. Locked compilation, formatting,
  CLI tests (5) and `git diff --check` pass. The tree contains 94 Rust files and
  48,140 lines.
- `Agent::initial_draft` now returns through the existing-evidence answer route
  before building task/factual prompt messages, avoiding an unused prompt
  construction. The empty-evidence path retains the same baseline token sizing
  and backend generation. Locked compilation, formatting, `git diff --check`
  and the full serial suite (475) pass. The tree contains 94 Rust files and
  48,135 lines.
- Storage count fields now share one SQLite `i64` to `usize` conversion and
  retain the `usize::MAX` overflow fallback. The distinct processed-count zero
  fallback is unchanged. Storage tests (148), full serial suite (475), locked
  compilation, formatting and `git diff --check` pass. The tree contains 94
  Rust files and 48,136 lines.
- Retrieval expansion, article-section selection and entity-alias decoding now
  use the shared JSON-object deserializer instead of repeating object extraction
  and typed conversion. Caller-specific bounds and validation are unchanged.
  Formatting, `git diff --check` and the full serial suite (475) pass. The tree
  contains 94 Rust files and 48,133 lines.
- Conversation-audit decoding now uses the same helper and avoids an intermediate
  JSON value and clone. Verdict validation and the uncertain fallback remain
  unchanged. Its two focused tests, formatting and `git diff --check` pass. The
  tree contains 94 Rust files and 48,131 lines.
- Replaced a test-only cloned one-element slice with `std::slice::from_ref` and
  removed a redundant UTF-8 conversion after slicing a string at stored byte
  offsets. The conjunction-boundary and retained-byte-span tests pass, as do
  formatting, targeted Clippy and `git diff --check`; the source line count is
  unchanged at 48,131.
- Scholar finding assessment now takes a named input record for request, source,
  finding, support, freshness, profile, publication recency, corroboration and
  relevance instead of nine positional parameters. The policy calculation and
  evidence-ledger writes are unchanged. The evidence-policy tests (6), a retained
  source byte-span regression, formatting and `git diff --check` pass. Strict
  Clippy no longer reports this function; seven other production long-argument
  functions and two test-only intake overloads remain. The tree contains 94
  Rust files and 48,140 lines.
- Search-attempt persistence now accepts a named ledger-event input instead of
  seven positional values. Both provider-wrapper and direct Wikipedia callers
  use it; database fields, outcome classification and diagnostics are unchanged.
  Search-ledger tests (2), locked compilation, formatting, `git diff --check` and
  the full serial suite (475) pass. Six production long-argument functions and
  two test-only intake overloads remain. The tree contains 94 Rust files and
  48,157 lines.
- Search candidates now share one input record across Scholar discovery and the
  storage writer, which accepts the candidate as a value instead of five
  positional fields. The stored projection, bounds and diagnostics are
  unchanged. Candidate persistence, migration and source-archive regressions,
  locked compilation, formatting, `git diff --check` and the full serial suite
  (475) pass. Five production long-argument functions and two test-only intake
  overloads remain. The tree contains 94 Rust files and 48,179 lines.
- Foreground chat, one-shot and browser replies now split `ReplyModels` into the
  interaction backend and `ReplyServices` through one shared constructor. CLI
  chat and say share learning-database error formatting. Maintenance lifetimes,
  browser IO errors and CLI errors remain unchanged. CLI (5) and web (6) tests,
  locked compilation, formatting, `git diff --check` and the full serial suite
  (475) pass. The tree contains 94 Rust files and 48,181 lines.
- Grounded answer review now takes a named context for its question, draft,
  masked review view, evidence views, currentness and attempt number. Verdict
  transitions, repair decisions and attempt limits are unchanged. The 67
  research-reply tests, formatting, locked compilation, `git diff --check` and
  the full serial suite (475) pass. Four production long-argument functions and
  two test-only intake overloads remain. The tree contains 94 Rust files and
  48,196 lines.
- Scholar semantic-claim insertion now takes a named record for claim identity,
  source, temporal bounds, supersession and proposition metadata instead of
  eight positional values. The insert projection, transaction ownership and
  promotion paths are unchanged. Locked compilation, formatting,
  `git diff --check` and the full serial suite (475) pass. Three production
  long-argument functions and two test-only intake overloads remain. The tree
  contains 94 Rust files and 48,216 lines.
- Scholar site-policy review and candidate-fetch persistence now share one
  candidate-review record, keeping status, nullable policy reason, fetched URL,
  excerpt, retained passages and provenance together. Existing skip/failure
  states and SQL null-versus-value behavior are preserved. Site-policy (9),
  storage network (10), storage provenance (10), locked compilation, formatting,
  `git diff --check` and the full serial suite (475) pass. Two production
  long-argument functions and two test-only intake overloads remain. The tree
  contains 94 Rust files and 48,240 lines.
- Property repair now receives the shared extraction context instead of eight
  positional arguments. Request, source, profile, timestamp and reviewer
  guidance are unchanged, as are repair prompts, acceptance and diagnostics.
  Controlled-repair tests (15), locked compilation, formatting,
  `git diff --check` and the full serial suite (475) pass. One production
  long-argument function and two test-only intake overloads remain. The tree
  contains 94 Rust files and 48,252 lines.


## Recent implementation record (2026-10-23)

The latest behavior-preserving slice routes retrieval expansion, article-section
selection, entity-alias decoding and conversation-audit decoding through the
shared JSON-object deserializer. Caller-specific limits, validation and the
uncertain audit fallback remain in place. Two test-only copies/conversions were
also removed while keeping the Unicode byte-span assertion intact. Scholar
finding assessment now receives a named input record instead of nine positional
arguments; its policy and persistence decisions are unchanged. The focused
conversation-audit, conjunction-boundary, evidence-policy (6) and retained-byte-
span tests pass, as do formatting and `git diff --check`. Search-attempt writes
now take a named ledger-event input shared by the provider wrapper and direct
Wikipedia path; persisted fields and diagnostics are unchanged. Search-ledger
tests (2), locked compilation, formatting and the full serial suite (475) pass.
Search-candidate persistence now consumes the shared candidate input record
directly instead of unpacking and forwarding five fields. The provider result
shape is shared between discovery and the storage boundary. Foreground chat,
one-shot and browser replies now share model-role splitting and ReplyServices
construction; chat and say also share database-open error formatting. Their
maintenance lifetimes and browser/CLI error boundaries remain distinct. Grounded
answer review now receives a named attempt context for draft, evidence views,
currentness and attempt number; verdict transitions and repair limits are
unchanged. Semantic-claim insertion now receives one named record for claim
identity, source, temporal bounds, supersession and proposition metadata; the
transaction and persisted projection are unchanged. The 67 research-reply
tests, locked compilation, formatting and the full serial suite (475) pass.
Scholar page-policy review and storage now share the same candidate-review
record, so its status, nullable policy reason, fetched text, excerpts and
provenance remain in one object through persistence. Skip/failure states and
the SQL null-versus-value behavior are preserved. Site-policy (9), storage
network (10), storage provenance (10), locked compilation, formatting and the
full serial suite (475) pass. Property repair now takes a shared extraction
context instead of eight positional inputs; the question, source, profile,
timestamp and reviewer guidance remain unchanged. Controlled-repair tests (15),
locked compilation, formatting and the full serial suite (475) pass. The tree
contains 94 Rust files and 48,252 lines. Strict Clippy now identifies one
production long-argument function and two test-only intake overloads.
Source-view JSON now omits the same three conditional fields in one retention
pass instead of three separate object mutations. The focused research-reply
tests (39), full serial suite (475), formatting and `git diff --check` pass.
The tree then contained 94 Rust files and 48,248 lines.
`SiteManifest` now supplies the same empty defaults through `Default`, replacing
three repeated initializers across site guidance and JSON/YAML parsing. The nine
site-policy tests and full serial suite (475) pass, as do formatting and
`git diff --check`. The tree then contained 94 Rust files and 48,234 lines.
The review-candidate storage test fixture now reuses the shared candidate
constructor and overrides only review-specific state. All 20 claim/observation
tests pass, along with formatting and `git diff --check`; no scenarios or
expected states were removed. The tree now contains 94 Rust files and 48,225
lines. `ScholarCandidateReview::fetch_failed` now reuses `failed`'s shared
state construction and changes only the policy status and reason. The focused
site-policy tests (9), full serial suite (475), formatting and `git diff --check`
pass. The tree now contains 94 Rust files and 48,218 lines.
Curator's three test-only `CandidateClaim` literals now use one fixture with
the same shared confidence, empty-context and date defaults; each test retains
its own fields and status. Curator tests (10), formatting and `git diff --check`
pass. The tree now contains 94 Rust files and 48,216 lines.
The shared test support now owns the common `ScholarFinding` fixture used by
storage, Scholar, Scout, maintenance and agent tests. Nine repeated struct
literals and the former storage-local fixture were consolidated with confidence,
source quality, dates and default proposition metadata preserved. The full
serial suite (475), formatting and `git diff --check` pass. The tree now contains
94 Rust files and 48,179 lines.
Curator tests now build their `Observation` records through one helper for the
shared status, timestamp, and field conversions, while each scenario supplies
its own identity, turn, conversation, and raw text. All 10 Curator tests pass,
along with formatting and `git diff --check`. The tree now contains 94 Rust
files and 48,168 lines.
Curator's repeated `ExtractedClaim` test records now share one constructor for
the same no-context, no-resolution, 0.99-confidence defaults; the date scenario
sets its two explicit bounds afterward. All 10 Curator tests pass with formatting
and `git diff --check`. The tree now contains 94 Rust files and 48,151 lines.

The preceding behavior-preserving slices flattened nested guards in Wikipedia API
error handling, fetched-candidate processing, provenance URL normalization and
Scholar extraction/repair, plus site-policy cache reuse and Wikipedia page
retention. They preserve evaluation order and outcomes. The semantic-claim row
decoder now uses one fixed projection shape for both relevance-aware and active
claim reads. Discovery uses one metadata record for response status/retry timing,
and Scholar's Wikipedia, policy and search requests share host-response
completion. Review paths share their exact quote-presence operation while
retaining caller-selected quote text and separate decision gates. Finding review
now uses the existing evidence row reader after aligning its internal column
projection. Scholar now persists collected evidence once for all usable outcomes;
no-result notes and failed/deferred transitions remain separate. Due and
challenged claim topic queries share a named `ClaimTopic` row decoder instead of
parallel positional tuples. Focused Scout tests (24), Wikipedia (8), provenance
(17), extraction (15), site-policy (9), discovery (10), source-review and
source-grounding tests, feedback tests (10), the full serial suite (473), locked
compilation, formatting, focused collapsible-if Clippy and `git diff --check`
pass. Storage now also shares the checked `usize` to SQLite `i64` query-limit
conversion and its error. Duplicate
progress entries were moved to the dated history, removing 173 lines from this
active plan while retaining all logged content. The tree contains 94 Rust files
and 47,988 lines. Scout now routes both challenged and stale claim topics
through one queue/deduplication helper; question wording, queue reasons and
ordering remain distinct. Scout tests (24), the full serial suite (473), locked
compilation, formatting and `git diff --check` pass.
The strict JSON-object-to-type conversion shared by property-span and source-
check assessment now has one helper; callers that retain parsed fields for
diagnostics or use delimiter recovery keep their specialized paths. Request
tests (22), source-check tests (3), the full serial suite (473), locked
compilation, formatting and `git diff --check` pass.
Wikipedia and public-site fetching now share missing/invalid HTTP-status
reservation completion, retaining each exact error and preserving the public-
site content-type check order. The new regression verifies a missing status
completes the host reservation exactly once. HTTP status (1), Wikipedia (8),
site-policy (9), the full serial suite (474), locked compilation, formatting
and `git diff --check` pass. The tree contains 94 Rust files and 48,024 lines.
The AI review queue now returns a named `{ finding_id, job_id }` record rather
than a positional pair, and Curmudgeon and the restart regression use its named
fields. Queue eligibility, order and persistence are unchanged. The retry test,
Curmudgeon tests (18), the full serial suite (474), locked compilation,
formatting and `git diff --check` pass. The tree contains 94 Rust files and
48,041 lines.
Assistant reply attention rows now use a named `{ reply_id, status, error }`
record instead of a positional triple; the CLI keeps the same displayed values
and formatting. Its mapping regression and the full serial suite (475), locked
compilation, formatting and `git diff --check` pass. The tree contains 94 Rust
files and 48,075 lines.
The independent-corroboration findings now use a named record for finding ID,
job ID, source URL, and retained excerpt instead of a four-field positional
tuple. Promotion and provenance updates retain the same values and order. The
two-source review/promotion regressions, full serial suite (475), locked
compilation, formatting and `git diff --check` pass. The tree contains 94 Rust
files and 48,080 lines.
Cached site-policy rows now use named `status`, `content_type`, and `body`
fields instead of a tuple. The cache hit path and restart checks retain the same
response values. The focused cache test, full serial suite (475), locked
compilation, formatting and `git diff --check` pass. The tree contains 94 Rust
files and 48,097 lines.
Cached source pages now use a named `content_type` and full `body` record; the
site-policy fetch path and cache-expiry regression use those fields. Cache
content and expiry behavior remain unchanged. The focused cache test, full
serial suite (475), locked compilation, formatting and `git diff --check` pass.
The tree contains 94 Rust files and 48,112 lines.
Currentness error diagnostics now use `inspect_err`, preserving the original
error; sentence and clause splitting use direct character patterns with the
same delimiters. Currentness tests (10), property-repair tests (3), full serial
suite (475), locked compilation, formatting and `git diff --check` pass. The
tree contains 94 Rust files and 48,111 lines.
Removed redundant borrows in robots-policy and page-selection paths, keeping
the borrow required for the owned robots-rule vector. Focused Clippy checks for
needless borrows, manual inspection and character-pattern comparisons pass;
site-policy tests (9), the full serial suite (475), locked compilation,
formatting and `git diff --check` pass.
The tree remains 94 Rust files and 48,111 lines.
The three remaining capped network/corroboration query limits now use the same
checked conversion; their caps are unchanged. The rotating maintenance scan
keeps its distinct bounded fallback. Network (17), corroboration, and the full
serial suite (475), locked compilation, formatting and `git diff --check` pass.
Scholar's single search path now passes its per-query inputs through a named
`ResearchRequest` record, reducing a nine-argument function to the explicit
store, model and request. Search selection, evidence retention and extraction
are unchanged. Locked compilation, formatting, `git diff --check` and the full
serial suite (475) pass. The tree contains 94 Rust files and 48,126 lines.
The conversation research resolver now takes a cohesive `ResearchTurn` snapshot
for the request, pending observations, evidence, research plan and draft state.
It no longer threads those six values separately; evidence remains borrowed and
the same plan branches and answer updates are retained. Locked compilation,
formatting, `git diff --check` and the full serial suite (475) pass. The tree
contains 94 Rust files and 48,140 lines.
The no-provider-credential and failed-checkpoint branches now use `let...else`
in place of single-purpose `match` expressions, preserving queue recording,
resource cleanup and errors. The two focused regressions, locked compilation,
formatting, `git diff --check` and Clippy with `-D clippy::manual_let_else` pass.
The tree contains 94 Rust files and 48,131 lines.
Option and Result mapping now use `map_or_else` directly instead of paired
`map`/`unwrap_or_else` calls, and method-only closures use method references.
The response-section fallback received the equivalent `map_or` form. Fallbacks
and converted values are unchanged. Formatting, locked compilation, strict
Clippy for both lint classes, `git diff --check` and the full serial suite (475)
pass. The tree contains 94 Rust files and 48,143 lines.
Verified source origin filtering now checks the verification flag and nonempty
key in one `Option::filter`, removing the intermediate `then_some` and `flatten`.
The verified-origin/source-limitation tests (4), locked compilation, formatting
and `git diff --check` pass. The tree contains 94 Rust files and 48,141 lines.
CLI startup now resolves all five configurable paths through one lazy
environment/default helper, including the model-relative adapter fallback.
Environment values and defaults are unchanged. Locked compilation, formatting,
CLI tests (5) and `git diff --check` pass. The tree contains 94 Rust files and
48,140 lines.
`Agent::initial_draft` now selects the existing-evidence answer route before
building task/factual prompt messages. That route no longer constructs and
discards an unused prompt; the empty-evidence route retains its baseline token
comparison and generation behavior. Locked compilation, formatting,
`git diff --check` and the full serial suite (475) pass. The tree contains 94
Rust files and 48,135 lines.
Storage count fields now share the same `i64` to `usize` conversion with its
existing `usize::MAX` overflow fallback. The one processed-count conversion
with a distinct zero fallback remains unchanged. Storage tests (148), the full
serial suite (475), locked compilation, formatting and `git diff --check` pass.
The tree contains 94 Rust files and 48,136 lines.

Storage tests now build repeated findings from the shared fixture, overriding
validity periods, scope, source quality and proposition metadata where those
values are part of each scenario. The confirmation observation also reuses the
shared candidate fixture while retaining its message links and rationale.
Identity-scope tests (9), claims/observations tests (20), formatting,
`git diff --check` and the full serial suite (475) pass. The tree contains 94
Rust files and 48,130 lines.

- Consolidated ten CLI no-argument checks in one helper. Each command keeps its
  original validation order and exact error message; argument-parsing commands
  remain unchanged. This removes 21 Rust lines. Formatting, the focused CLI
  tests (5), `git diff --check` and the full serial suite (475) pass. The tree
  contains 94 Rust files and 48,109 lines.
- Added one `ScholarEvidence` constructor for the default source profile and
  used it in production Scholar paths and four storage-test records. The
  evaluation evidence with a verified underlying-source profile and the
  custom-profile intake test remain explicit. Scholar tests (84), formatting,
  `git diff --check` and the full serial suite (475) pass. The tree contains 94
  Rust files and 48,105 lines.
- Unified the identical 13-column row decoders used to claim and list Scholar
  provenance reviews. Query projections, transaction scope and result ordering
  are unchanged. Provenance tests (10), formatting, `git diff --check` and the
  full serial suite (475) pass. The tree contains 94 Rust files and 48,093
  lines.
- Added one `Observation` row reader for the identical six-column projections
  used by inbox, detail and claim-queue reads. The separate `last_error` field
  and invalid-turn-number error are preserved. Storage tests (109), formatting,
  `git diff --check` and the full serial suite (475) pass. The tree contains 94
  Rust files and 48,076 lines.
- Removed ten redundant `Vec<String>` to iterator to `Vec` round trips in CLI
  argument handling, cutting nine source lines; `say` now joins its owned
  argument vector directly. Parsing order and command behavior are unchanged.
  CLI tests (5), formatting, `git diff --check` and the full serial suite (475)
  pass. The tree contains 94 Rust files and 48,067 lines.
- Shared the source-evidence byte-offset calculation and checked SQLite integer
  conversion for harvested claims and questions. Missing spans and overflow
  still increment each path's own skip count and leave cursor handling intact.
  Basket tests (13), formatting, `git diff --check` and the full serial suite
  (475) pass. The tree contains 94 Rust files and 48,066 lines.
- Unified persisted source-capture status classification across retained-source
  metadata and Scholar finding intake. Truncation still takes precedence over
  excerpt/section scope. The classification test, existing archive test,
  formatting, `git diff --check` and full serial suite (476) pass. The current
  tree contains 94 Rust files and 48,090 lines.
- Changed the shared CLI no-argument check to inspect each argument slice
  directly, removing the consuming iterator adapters while preserving validation
  order and exact errors. CLI-focused tests (5), formatting, `git diff --check`
  and the full serial suite (476) pass. The tree remains 94 Rust files and
  48,090 lines.
- Routed search failure metadata through the shared search-response parser,
  preserving its missing-marker fallback while removing a duplicate parse path.
  Discovery tests (9), formatting, `git diff --check` and the full serial suite
  (476) pass. The tree contains 94 Rust files and 48,088 lines.
- Grouped source-retention inputs into `ResearchSourceInput`, keeping source
  validation and retention work within `LearningStore`. This removes the
  production too-many-arguments warning; two test-only intake overload warnings
  remain. Archive and Scholar tests (85), formatting, `git diff --check`, Clippy
  and the full serial suite (476) pass. The tree contains 94 Rust files and
  48,093 lines.
- Built the factual-answer and token-baseline prompts before inserting the same
  challenge message into each in one branch. The additional-prompt-token sizing
  path is unchanged. Agent tests (93), formatting, `git diff --check` and the
  full serial suite (476) pass. The tree contains 94 Rust files and 48,090
  lines.
- Kept source conflicts borrowed as strings through source-use preparation and
  serialized them only in the model-facing view, removing intermediate JSON
  value construction and deep clones. Research-reply tests (67), formatting,
  `git diff --check` and the full serial suite (476) pass. The tree contains
  94 Rust files and 48,091 lines.
- Reused the source-independence request decision across answer generation,
  source rendering, literal fallback and all review passes within one answer.
  The existing classifier and policy text are unchanged; separate standalone
  test adapters still classify their own requests. Research-reply tests (67),
  formatting, `git diff --check` and the full serial suite (476) pass. The tree
  contains 94 Rust files and 48,116 lines.
- Reused the same request-only decision across evidence-property request
  matching and per-finding relevance checks, so provenance intent is assessed
  once per retrieval batch. Property-selection rules remain unchanged. Research
  tests (24), formatting, `git diff --check` and the full serial suite (476) pass.
  The tree contains 94 Rust files and 48,123 lines.
- Reused `ScholarEvidence::new` in the test-only source-profile intake adapter,
  then applied the requested custom profile. Source-review tests (8), provenance
  tests (10), formatting, `git diff --check` and the full serial suite (476)
  pass. The tree contains 94 Rust files and 48,120 lines.
- Shared checked conversion of source quotation byte spans to SQLite offsets
  across Scholar intake and research-basket harvesting. Intake still propagates
  overflow, while harvesting still skips the invalid item and advances normally.
  Source-grounding tests (7), Unicode harvest-offset tests (2), formatting,
  `git diff --check` and the full serial suite (476) pass. The tree contains 94
  Rust files and 48,125 lines.
- Shared the pending-status and exact evidence-quote checks used by source
  expertise, source-origin and finding review, retaining each operation's error
  message and the finding-review path's acceptance-only quote check. Source-review
  tests (8), provenance tests (10), formatting, `git diff --check` and the full
  serial suite (476) pass. The tree contains 94 Rust files and 48,128 lines.
- Moved the three `finish_scholar_job_with_*` test adapters out of production
  intake and into the shared test-support extension trait. Existing test call
  sites, default source profile/relevance values, and the production
  `finish_scholar_job_with_evidence` transaction path are unchanged. Production
  `cargo check`, the full serial suite (476), formatting, and `git diff --check`
  pass. Clippy's remaining test-only argument-count warnings now point to
  `test_support.rs`; no runtime API contains these adapters. The tree contains
  94 Rust files and 48,145 lines.
- Consolidated Wikipedia lead-excerpt and selected-section source-record
  construction in one helper. Both paths retain their prior URL handling,
  provider, scope, query, body, and revision timestamp; discovered-page access
  and retention remain separate. The full serial suite (476), formatting, and
  `git diff --check` pass. The tree contains 94 Rust files and 48,162 lines.
- Distinguished retryable Scholar search outcomes as `Deferred` before job
  completion, while keeping the persisted search-attempt status/error contract
  and the fetched-evidence fallback unchanged. The focused outcome test, full
  serial suite (476), formatting, and `git diff --check` pass. The tree contains
  94 Rust files and 48,178 lines.
- Made discovery's metadata-only response adapter use the shared HTTP envelope
  parser directly, avoiding conversion of the unused response body to an owned
  string while preserving the missing-marker fallback. Both metadata tests, the
  full serial suite (476), formatting, and `git diff --check` pass. The tree
  contains 94 Rust files and 48,181 lines.
- Made grounded-answer review transitions explicit with a typed action for
  publication, partial recovery, structural repair, standard repair, or unresolved
  output. The same acceptance, disagreement, evidence, and attempt conditions
  remain in force. The focused reply tests (39), full serial suite (476),
  formatting, and `git diff --check` pass.
- Moved the 15-column Scholar feedback task mapping into a named storage row
  reader. The SQL projection and every field's column mapping remain unchanged.
  Feedback lifecycle and migration tests (7), the full serial suite (476),
  formatting, and `git diff --check` pass.
- Shared the deterministic proposition-field and exact-source-quote checks used
  at Scholar intake and research-basket harvesting. Each write boundary still
  performs validation, with its distinct support, qualification, and confidence
  rules intact. Focused invalid-harvest and source-grounding tests, the full
  serial suite (476), formatting, and `git diff --check` pass.
- Replaced eight repeated `RetainedClaim` and several `RetainedQuestion` test
  literals with test-only constructors for their shared empty-qualification and
  field-conversion defaults. Every scenario keeps its own claim/question,
  evidence, and confidence values. All 14 basket tests, formatting, and
  `git diff --check` pass.
- Consolidated the identical automatic-promotion candidate filter into one SQL
  CTE shared by origin-verified promotion and source-origin review. Source kind,
  direct support, independence, confidence, status, and latest topic-suitability
  requirements stay identical; human-origin verification, pair provenance,
  proposition matching, and active-conflict checks remain caller-specific.
  Promotion/review regressions, the full serial suite (476), formatting, and
  `git diff --check` pass.
- Grouped grounded-answer review attempts and accumulated reviewer disagreement
  in `DraftReviewState`, and replaced the remaining multi-column research query
  tuples with named row records. Review-action behavior and SQL projections are
  unchanged. The full serial suite (476), `cargo check --locked --offline`,
  formatting, `git diff --check`, and all 15 Python `unittest` tests pass. The
  current source tree contains 94 Rust files and 48,302 lines.
- Completed the conditional architecture review: JSON-map and SQLite claims
  still carry different authority/lifecycle contracts, and a concrete-backend
  cache would add forwarding structure without a measured reduction. Recorded
  the reasons in the plan and deferred both migrations. Semantic changes remain
  gated because one live-baseline case timed out and independent judgments for
  the other eight have not been supplied.
- Added a test-only `ClaimPair` constructor for repeated ID-only basket review
  fixtures. Each test still supplies its own pair IDs and assertions. Basket
  tests (14), the full serial suite (476), formatting, offline `cargo check`,
  all 15 Python tests and `git diff --check` pass; the source tree contains 94
  Rust files and 48,296 lines.
