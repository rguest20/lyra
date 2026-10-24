# Grounding generalization: 13 October 2026

The broader runs found general problems in source interpretation, rather than a
single crisp-related path. The fixes preserve useful source facts while limiting
what Lyra can infer from them. This is progress toward reliable grounding;
training remains closed and the independent benchmark acceptance is unfinished.

## What changed

- **Separate entailment from answering.** The entailment model now sees the
  actual claim and source, without being primed by the user's completeness or
  currentness requirements. Historical facts can be faithfully extracted even
  when they cannot settle a current question. Bounded extraction repair and
  retrieval share a literal property/constraint decomposition.
- **Preserve qualified propositions.** Flattened JSON recovery, review context
  and stored support refresh now retain attribution, polarity, modality,
  geography and context. Semantic approvals bind those fields, and their source
  quotations must include explicit attribution/scope. Scoped identity approvals
  cannot be turned into global aliases by removing their scope afterward.
- **Anchor retrieval in source text.** A requested place or other qualifier can
  occur outside a valid claim's subject/object. Retrieval now checks the actual
  quotation. A conservative support check rejects turning a listed item into
  its containing category, such as extracting “puffins include Arctic terns.”
- **Guard generated additions.** Source conjunctions cannot silently become new
  list boundaries. Additional checks catch unseen names inside sentences,
  source-literal causal strengthening and relabelling a mentioned actor as a
  reporting source. These are conservative checks, not complete semantic proof.
- **Keep useful limited answers.** Bounded source-literal replies retain dates,
  verified shared-origin limits, conditional statements and explicit negative
  facts. They avoid inventing either an outcome, its definite absence, or a
  causal explanation. Conflicted evidence and omitted material qualifiers keep
  the full review path. Evidence promotion requirements are unchanged.
- **Disambiguate currentness in replies.** A small request-only assessment
  distinguishes the quantity “current” from a request for up-to-date information.
  This controls narration and bounded rendering; stored gates remain unchanged.
- **Restore presentation without rewriting evidence.** Quote matching can
  recover whitespace and internal typographic apostrophes while returning the
  original contiguous source bytes. Case, other punctuation, quotation delimiters
  and mathematical prime marks remain exact. Extraction and selection use the
  same recovery. Source-label resolution also handles capitalization without
  changing existing URLs or partially matching unknown source IDs.

The new checks use source structure, literal spans and general language
relationships. They introduce no crisp-equivalence table. English phrasing and
small-model intent extraction remain limitations.

### Follow-up work (2026-10-14)

The two-hour grounding pass exercised the fixed nine-case corpus and an
unrelated 14-case synthetic sweep. The original corpus exposed the same slow
wrong-property retrieval on the Walkers/Smith's case and a timeout on its nearby
question case; the previous structured-property change then reduced the
Walkers case to a 37-second, safe abstention with no unrelated evidence. The
broader sweep completed with all source archives intact and no HTTP requests.
It confirmed correct cross-entity rejection, source-lineage handling, a
supported negative result, the island-scoped bird list, a researcher/date fact,
and selection of the rural-health use over an unrelated founding fact.

The sweep also reproduced ordinary-path weaknesses: a historical museum claim
was extracted but rejected when matching a current collection; one conditional
question and one title-list question abstained; a physical definition and a
fictional scoped identity acquired redundant narration; and ambiguous title
conjunctions required a safe fallback. These are retained as failures, not
counted as a pass. One run's answer prompt caused the model to treat
human-readable qualification strings inside evidence JSON as instructions,
leading to a 300-second repair and fallback on the prompt-injection fixture.

The answer path now encodes source, currentness, conflict, independence and
scope qualifications as compact status codes, with their meanings in the
system prompt. The answer repair no longer reinserts its failed draft, and the
general reply-style check detects instruction-following commentary. New tests
cover the qualification contract and a rewrite that must omit a bad draft. A
fresh three-case replay returned concise answers for the prompt-injection and
scoped-identity cases; the ambiguous-title case preserved the exact conjunctive
source wording but still added unnecessary caveats about missing publication
dates. The property fallback now removes entity metadata and obvious temporal
modifiers from the matching target while preserving exact question-anchored
terms. The museum recheck retains the 2012 instruments and states that current
status is unknown; it completed in 28 seconds. This fixes the retrieval failure
without calling the historical display current.

Validation currently passes 412 Rust tests, `cargo fmt --check`, the Python
evaluation test suite, and a development build. MLX build warnings remain
for a task-target accessor and type used only by tests. The 14-case run is
archived at `evals/results/grounding-two-hour-generalization-2026-10-14`; the
three-case recheck is at
`evals/results/grounding-two-hour-recheck-2026-10-14b`; museum rechecks are at
`evals/results/grounding-two-hour-museum-recheck-2026-10-14b`; the original
nine-case replay is at `evals/results/grounding-two-hour-v1-final-2026-10-14`;
the revised conflict replay is at
`evals/results/grounding-two-hour-conflict-recheck-2026-10-14b`; the updated
injection replay is at
`evals/results/grounding-two-hour-injection-recheck-2026-10-14`; and the
thinking-disabled style-repair replay is at
`evals/results/grounding-two-hour-cookie-recheck-2026-10-14`. The latest full
14-case replay is at
`evals/results/grounding-two-hour-generalization-final-2026-10-14`, with the
negation and duplicate-evidence narration rechecks at
`evals/results/grounding-two-hour-final-fixes-2026-10-14`. All use synthetic
source transport, not live search or production database reads. Independent
semantic adjudication remains undone and the grounding benchmark is not passing.

The original nine-case replay selected the right flavour source rather than the
renewable-energy distractor, abstained on the Walkers/Smith's entity mismatch,
found the penicillin fact, and recovered the malformed-source flavour list.
It still produced irrelevant currency caveats, repeated copied-source claims,
and selected only the 2000 account in a two-date conflict. A conflict-aware
selection instruction now requires retaining relevant differing values for the
same property. Its fresh 67-second replay selected both sources and answered
with both dates in parallel, explicitly saying the evidence did not resolve
which date was correct. This is one controlled conflict case, not broad conflict
acceptance.

Presentation repairs now leave the initial answer's thinking enabled for
debugging but generate a detected style rewrite with thinking disabled; any
captured initial thoughts are retained for inspection. In the fresh cookie
replay, the 2B model first emitted a self-check, then the filter triggered a
rewrite that returned only the supported fact; total time fell from 196 to 162
seconds. The fresh injection case also returned a concise answer without
instruction leakage. The nine-case run predates the latest conflict-selection
and thinking-disabled-rewrite prompts, so its findings are a baseline for those
changes rather than their validation.

The final 14-case replay is complete. It answers the title-list, researcher,
physical-definition, geography, rural-health, historical-display and scoped
identity questions without unrelated evidence. The historical display keeps
the 2012 date and withholds current status. The negative superconductivity case
still abstained because selection treated a negated answer as irrelevant; the
property prompt now explicitly treats polarity as separate from relevance, and
the fresh 29-second replay returns the exact qualified negative passage. A
style guard also removes an extra sentence that merely said an answer was
supported by its source; the list-use recheck returns one sentence. The list
conjunction ambiguity is surfaced as unresolved rather than split into invented
titles, though that case still takes 168 seconds.

The initial freshness/sensitive-topic audit found substring collisions: `tax`
inside “syntax” or `law` inside unrelated longer words could classify a claim
as consequential. Topic terms now match token boundaries with bounded common
inflections, while short acronyms such as GPU still require a whole token.
Regressions cover both the false positives and positive terms such as symptoms,
laws, taxes and investments. This is a first audit, not a complete taxonomy or
a substitute for the remaining high-stakes review tests.

## Validation and limits

**412 Rust tests and 12 Python tests pass.** New regressions were observed failing
before their fixes, including dropped proposition fields, stale approval binding,
scope removal, list restructuring, invented locations/causes/attribution and
typographic quotation rejection. Formatting and whitespace checks also pass.

Retained local model runs include the unchanged nine original scenarios,
14 non-food cases across books, founders, instruments, geography, experiments,
conditional proposals, fictional identity and prompt injection, and three longer
excerpts copied read-only from the existing database.

The 14-case corpus was exercised twice in candidate E and three times in
candidate F. Those are intermediate results: E exposed unsupported causal and
negative conclusions; F blocked them but repeatedly refused the useful negative
experiment answer. The later source-stance rendering preserves that answer
without inventing its cause. All runs remain available, including the failures.

The subsequent candidate completed all nine original cases, all 14 non-food
cases, the three stored-excerpt cases and three repeats of the historical range
question. The historical range repeats retained the dated examples and explicitly
withheld currentness. The stored excerpts exposed two additional problems:

- Internal apostrophe typography caused a valid source quotation to be rejected;
  the final recovery change addresses this while preserving the original bytes.
- Scholar selected a true nearby chronology statement instead of the paragraph's
  explicit statement about uncertain discovery dates. Retrieval rejected the
  wrong property, so the answer was an unhelpful refusal. This remains a failure
  to repair generically, not a reason to loosen the relevance gate.

The final stored-source replay retains these limitations. The quotation mismatch
was followed by a different failure: the model invented ISO claim-validity dates
from an article publication date. The existing date gate rejected those fields.
The longer nebula description also acquired a constellation assertion absent
from its selected quotation, and the answer reviewer approved it. That is an
unsupported addition even though the full archived article contains more context;
the generation/review path received only the selected quotation. These results
make source-bound answer planning and same-passage extraction repair the next
priorities. They are not a passing real-paragraph grounding test.

The final four-case smoke retained the physical-current definition, dated museum
example, negative experiment statement and conditional proposal with their
appropriate source wording and limits. All seven final presentation-replay
databases passed full source-retention and zero-request transport checks.

These runs exercise real local Qwen3.5-2B extraction, persistence, retrieval and
reply grounding with substituted source transport. They do not establish live
search quality, frontend conversation-boundary behavior, harvesting coverage or
background scheduling correctness. Synthetic reference quality and declared
lineage are fixture assumptions. Three actual stored paragraphs were replayed
with the same fixture assumptions; their real-world truth was not adjudicated.

The same-weight local judge remains advisory and sometimes returns invalid
labels or quotations. No model-generated score here is a production accuracy
claim or permission to open the training gate. Some generated answers still add
unnecessary commentary or give incomplete lists; semantic reliability remains
unfinished.

The production database was inspected read-only. Three pending approvals lacked
the newly bound qualification fields; the review path refreshes such approvals
when those findings are processed. No production database content was changed.

## File organization

| File | At the start of this block | After |
| --- | ---: | ---: |
| `src/scholar.rs` | 2,692 | 1,328 |
| `src/storage/research.rs` | 2,417 | 1,077 |
| `src/storage/tests.rs` | 5,985 | 76 |

Scholar extraction, property repair and tests now have their own modules.
Storage research is split into network accounting, discovery audit, model
probes and existing review/promotion components. Storage's 101 integration tests are grouped into
12 behavioral modules, sharing the original fixtures and database ID allocator;
the largest group is under 1,000 lines. Semantic assessment, conservative guards
and tests are separate, with 328 lines remaining in `src/semantic.rs`.

Further splits should follow responsibilities. The network accounting module
is still about 1,050 lines; Scholar orchestration and the main agent also remain
worth reviewing. Line count alone should not drive another abstraction.

## Reproduce the source-fixture runs

The reusable runner and runtime inputs are outside the ignored `evals/` tree:

```sh
cargo build
python3 scripts/run_grounding_fixtures.py --output evals/results/my-generalization-run --repeat 3
python3 scripts/run_grounding_fixtures.py --fixtures tests/fixtures/learning-quality-v1-runtime.json --output evals/results/my-v1-run
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_*.py'
```

Existing output directories are refused. Each run snapshots the executable,
fingerprints its inputs, retains model calls and replies, and uses a fresh
database for every sample. The audit checks that every supplied source body was
archived intact and that no host HTTP requests were recorded. A successful exit
means execution and transport checks passed; semantic quality still needs review.

Artifacts from this block use `evals/results/grounding-two-hour-*`. The final
broader replay uses `final-v1`, `final-generalization`, `final-stored-sources`
and `final-completeness`; the subsequent presentation fix uses
`presentation-stored` and `presentation-smoke` in those directory names.

## Next grounding work

1. Finish and independently review same-passage property repair on the stored
   multi-fact source fixtures. The latest source32 replay now extracts and
   returns the exact qualified statement that most element discovery dates
   cannot be accurately determined. Earlier runs still show the model can
   repeat a mismatched claim, emit duplicate keys, or misclassify the property,
   so repeatability and independent review remain open.
2. Add production-path dialogue replays for pronouns, corrections and topic
   changes, then exercise live search and the background repair/harvest cycle.
3. Improve source-bound answer planning and list coverage so free-form generation
   needs fewer structural rescues. Reassess readiness with independent review,
   while keeping training and scaling behind the evidence boundary.

### Follow-up repair notes (2026-10-13)

The bounded Scholar repair now compares the requested answer type with an
accepted claim. When they differ, it uses the original question verbatim and
exact source spans ranked from the same archived passage; source validation
still checks every returned quote against the full passage. A second property
check gates acceptance. Repair is no longer skipped when a separate
property-paraphrase call returns an empty result. The property checker has
explicit different-answer-type guidance plus a guard for questions about
knowability/determinability. Semantic support sees only the extractor's
complete quoted span so it cannot borrow a related assertion from elsewhere in
the article. Both reviewers require each JSON key only once.

The controlled regression tests cover a chronology/determinability mismatch,
bounded repair, preserving the original source-supported claim after a rejected
repair, exact-quote semantic review, part/whole inference, and the literal-claim
fallback. Property repair now has a separate module and the extraction tests
live in `src/scholar/extraction/tests.rs`; the extraction implementation is split
into a 680-line orchestration/validation file and a 270-line repair module.
Currentness intent is shared by Scholar's historical-property retry and
answer rendering, so physical uses such as a measured current or “electric
current” do not trigger a temporal repair or freshness caveat. The full suite
passes (396 tests), as do the Python fixture-runner tests and the
release build. A source-grounded metadata cleanup drops bare event years from
validity fields while retaining the exact dated quote, and clears attribution
or scope values absent from the cited words.

Fresh stored-source replays return the qualified statement about most element
discovery dates and the Orion-location statement, while abstaining on the
ambiguous Orion definition case. That last replay had previously accepted an
unsupported part-to-whole inference; the general semantic-review instruction
now explicitly checks that relationship. These are small local observations,
not a semantic benchmark. Runs are under
`evals/results/grounding-*2026-10-13`; they use synthetic transport.

The 14-case non-food sweep confirmed the broader question and evidence paths,
but initially abstained on an inline bird list after the model turned one list
member into the subject of another. A syntax-bound fallback now preserves the
source's containing category and complete list, and requires any literal named
entity in the question to match that subject. Its replay answers the Alder
Island question with the exact puffin and Arctic tern list while rejecting the
nearby Birch Island list. The answer is correct, though its extra source
commentary remains an area for presentation cleanup.

Two more local replays exercised time handling outside the crisps cases. The
museum replay preserved the 2012 exhibit and correctly said current status was
not established, even when the extractor supplied an invalid year bound and a
scope phrase copied from the question. The measured-current replay returned
the amperage without adding a current-status caveat; the electric-current
definition replay likewise no longer adds one. Runs are under
`evals/results/grounding-metadata-normalization-2026-10-13` and use synthetic
transport. The inline-list and current-definition follow-up is under
`evals/results/grounding-inline-list-recovery-2026-10-13`.

### Agnostic cleanup candidates

- `epistemic_property_mismatch` still relies on hand-maintained English cue
  stems for questions about knowability and claims about certainty. It backs up
  a known model-classification failure, so removing it would reopen that gap;
  a reusable typed answer-property representation would be a better eventual
  replacement.
- Currentness detection handles common explicit time cues and asks the model
  to disambiguate “current”, with a measurement-context guard for physical
  quantities. The related property-span normalizer has its own short list of
  temporal words. Consolidating these into one intent result with normalized
  spans would avoid drift and cover paraphrases such as “up to date”.
- Event-year cleanup currently recognizes a bare four-digit year after “in” or
  “during”. If sources expose other date constructions, extend a shared date
  expression parser rather than adding topic-specific exceptions.
- Source-reference cleanup now handles common document-reference forms rather
  than only the exact phrase “by the source”, but its reference nouns and verb
  prefixes remain allowlists. Broaden this only when varied model outputs show
  a real pattern; avoid adding one-off strings.
- The older `explicit_list_finding` fallback still has a fixed list-topic
  vocabulary. The new inline-list recovery covers sentence lists; extend the
  older fallback by deriving section categories from question and heading text
  if fixtures show a gap, rather than growing `LIST_TOPICS` one noun at a time.
- Property-span ranking uses punctuation as a lightweight sentence/clause
  splitter. If passages in other languages or dense scientific prose become a
  target, replace it with a shared sentence boundary utility and add fixtures
  first.

### Task performance and thinking recovery — 13 October

Reproduced the user's refusal-history case with an additional distracting claim
that Python is a popular programming language. Merely adding an instruction to
try the task was insufficient: the model attributed its ability to that claim,
repeated its example and exposed process commentary. The failing outputs remain
under `evals/results/thinking-and-task-routing-2026-10-13` and
`evals/results/thinking-task-followup-2026-10-13`.

Direct tasks now use a short, separate prompt with the complete conversation
encoded as fallible context, user inputs and any specifically researched
prerequisite passages. Topic-only knowledge-map retrieval no longer substitutes
for task performance. A semantic assessment of the attempted answer can request
a targeted public-fact lookup; research results then inform another attempt at
the original task. Failed research retains the attempt and identifies the
unresolved gap. Factual questions and recommendations retain the source-grounded
answer path. No topic names or example-specific equivalence lists were added.
The new orchestration helpers live in `src/agent/task.rs` rather than expanding
the already large agent prompt builder.

MLX now extends exhausted reasoning without restarting it, then closes the
reasoning phase at the bounded enlarged budget and continues into the answer.
Debug visibility is independent of answer reasoning. Answer rewrites also use
thinking; JSON review remains separate. Malformed thought delimiters are rejected,
and a replacement answer no longer displays an unrelated earlier draft's trace.

All 401 automated tests pass, including continuation token preservation,
structured review versus answer generation, strict intent parsing and targeted
task-gap planning. The task-context test also checks that older conversation
inputs are retained. Live isolated runs under
`evals/results/task-attempt-first-2026-10-13` passed the seeded refusal case,
Spanish translation and arithmetic without queuing research. The refusal case
returned one short answer with one Hello World program; executing that program
printed the expected text. These are small regression checks, not a general
capability guarantee. The active development server was left running throughout;
its shared inference lease caused substantial waits for some checks.

The explicit current-documentation case exposed two additional problems: a
combined intent classifier missed the source-check requirement, and the task
planner could emit invalid JSON. Source-check intent is now assessed separately
from routing and from the generated draft. Required lookups cannot be skipped
because an attempted answer looks plausible. The planner gets one bounded repair
attempt, and harmless trailing question punctuation is normalized in search
queries. A research-failure qualification is appended after style repair so it
cannot disappear during rewriting.

The final live case under
`evals/results/task-source-check-query-repair-2026-10-13` generated an HTTPX
example, queued focused documentation queries after repairing an incomplete JSON
object, and retained the example with an explicit unresolved-gap notice. External
search credentials were disabled in this isolated test; Wikipedia returned no
useful result. This verifies task-to-research routing and honest failed-lookup
handling, not successful retrieval of current API documentation. Earlier failed
source-check runs remain under `task-source-check-repair-2026-10-13` and
`task-source-check-independent-2026-10-13` for comparison.

A separate runtime issue was observed in the previous development server log:
Scholar encountered an MLX broken pipe. The old server subsequently was no
longer running; the cause of its exit was not established. This change does not
implement dead-worker recovery. The release binary was rebuilt successfully;
the local server must start with that binary to use the updated behavior.

### Scholar basket output recovery — 13 October

Replayed source 34 after the reported `invalid claims/questions basket JSON`
error. The first response used strings in `questions`, where the schema requires
objects. Its old repair attempt was truncated by the 1,024-token output cap.
Diagnostics now preserve the specific serde error and bounded output excerpts,
so format errors and truncation can be distinguished.

Basket extraction now makes at most three fresh attempts, at 1,024, 2,048 and
4,096 output tokens. Repair instructions preserve all distinct assertions,
require exact source quotations and explicitly require structured questions.
Other extraction operations retain their existing cap. A generic inference
method accepts a bounded extraction budget; no topic-specific logic was added.

After three invalid responses, harvesting parks that source for explicit retry
rather than repeating the same cycle every five minutes. Its body and cursor
remain intact. Transient inference errors and storage errors retain the normal
cooldown. Parking uses the existing retry timestamp and the existing
`scholar baskets retry SOURCE_ID` command, without a schema migration.

All 403 automated tests pass. New regression coverage checks both increases,
rejection after the third invalid field, absence of partial results, source
preservation and resumption after explicit retry. Live read-only source replays
are under `evals/results/basket-output-growth-2026-10-13`: source 34 returned a
valid basket on its third attempt; an unrelated, denser passage hit all three
output limits and correctly stopped. Increasing the cap does not guarantee
successful extraction. No probe output was inserted into the actual baskets.
`model probe-baskets TITLE PASSAGE` exposes this same extraction path for future
diagnosis. The earlier failing source-34 replay is under
`evals/results/basket-json-diagnosis-2026-10-13`.

### Two-hour grounding and dialogue pass — 14 October

The follow-up pass made generic changes across retrieval, answer construction,
fixture handling and freshness policy. Property extraction now has a conservative
question-anchored lexical fallback when the model omits usable terms; this let
the museum fixture retrieve the dated display fact without treating it as current.
Selection retains differing values for the same property and treats polarity as
part of the answer rather than a reason to discard a yes/no response. Fixture
scope metadata no longer contaminates the question sent to retrieval. Freshness
and sensitive-topic matching now uses token boundaries and bounded inflections,
with false-positive regressions for incidental substrings.

The answer path uses compact qualification codes, omits failed drafts from repair
prompts, and catches several forms of process leakage and redundant source
commentary. A no-thinking style rewrite can clean a detected bad first draft
while retaining the initial thinking trace for debugging. Short conversational
statements now have a bounded acknowledgment rewrite and last-resort short
fallback. Tests cover the generic categories rather than crisp-specific wording.

The final synthetic 14-case replay and targeted rechecks now retain the useful
historical display, negative experimental result, conflicting dates, and concise
answers for several previously noisy cases. All 412 Rust tests and 12 Python
evaluation tests pass; formatting and diff whitespace checks pass. Archived runs
are listed above. These are local synthetic-source tests, not live search.

Two isolated web API checks preserved conversation continuity and a corrected
telescope name, but exposed response-style and routing failures: one correction
produced an overlong answer with an unsupported example, and a later simple
personal statement was misread as a request about Lyra's physical capabilities.
The short-statement style guard was added after those runs; it has unit coverage
but has not yet passed a fresh live dialogue recheck. The curator also held the
personal fact rather than promoting it. These checks used isolated evaluation
databases; the production database was not touched. The local test server is
stopped.

Part 1 remains open. Next work should independently adjudicate the retained
cross-topic cases, rerun the isolated correction dialogue against the latest
style guard, and investigate why personal conversational facts are not becoming
useful context. Keep the benchmark's unsupported-answer and unnecessary-refusal
counts visible; unit tests alone do not meet grounding acceptance.

### Further Part 1 review — 14 October

A fresh isolated three-turn web dialogue now preserves continuity through a
personal statement, correction and pronoun follow-up. It stores the first fact
as “the user's telescope is named Marigold,” records the correction on the same
subject, marks Marigold superseded, and answers “The telescope is named Juniper.”
The correction does not queue Scholar; the question-shaped follow-up is rejected
by curation as a claim. The live artifact is under
`evals/results/grounding-part1-output-cap-replay-2026-10-14`.

An earlier replay exposed the wrong-subject extraction (“the user is named
Marigold”) and an unnecessary promise in the correction reply. The generic
possessive-subject gate now blocks that mapping at candidate validation and at
AI review; tests cover named objects and preserve “my name is Ryan.” A short
statement with an unrelated answer now falls back to “Understood.” The corrected
reply is brief and states the updated value.

The first chat request against a newly initialized isolated database also raced
the maintenance worker: both opened the database while migrations were pending,
and the request hit `table observations already exists`. Storage opens now
serialize connection setup and migrations within the process. An eight-thread
first-open regression reproduces and checks this initialization path.

Freshness and consequence screening remain deterministic lexical backstops.
The audit caught that generic short-prefix matching made historical “mayoral
service” look like an undated current office claim, so arbitrary prefixes were
replaced with bounded inflection forms and the one-off office-title expansion
was removed. Existing freshness and high-consequence checks retain token
boundaries. Semantic risk and time-scope classification remains open; expanding
keyword lists would not solve the generalization problem.

Source-check and acknowledgment routing no longer use action/source vocabularies
or opening-phrase allowlists. The request is classified in its conversation
context; short-statement replies receive a separate relevance and safety review,
with rejected or malformed reviews falling back to a brief acknowledgment.
Regression examples remain in tests rather than serving as runtime phrase
gates. This adds a small-model classification call on conversational turns, so
latency and live intent errors still need measurement. Other deterministic style
checks remain and need an inventory before claiming all phrase gates are gone.

An earlier live run on the same personal statement reached
`finish_reason: max_tokens` after a thinking budget expansion. Lyra recovered
through the style fallback, but the inference layer had accepted the truncated
attempt. Generation now treats that finish
reason as incomplete, retries without thinking with a doubled output budget,
then doubles once more before failing. The thinking trace is retained and a
truncated draft is never returned as the answer. Mock-worker tests cover both a
successful retry and abandonment after both increases. The latest live dialogue
did not hit the cap, so the retry branch has not yet been observed live.

Independent adjudication of the museum and cross-topic benchmark has not been
performed. These remain acceptance blockers; no passing benchmark is claimed.
The latest verification passed 422 Rust tests, 12 Python tests, build, format
and diff checks. One earlier full-suite run had a transient inference lock test
failure; the isolated test and subsequent full run passed.

### Part 1 continuation — semantic fallbacks — 14 October

Currentness is now classified by request meaning on every invocation, rather
than bypassing the model for a fixed set of temporal words or physical-quantity
phrases. The structured result distinguishes time-sensitive, not time-sensitive
and uncertain, with a brief reason. Invalid or uncertain output gets one bounded
reconsideration; unresolved output stays unresolved. A source-limited historical
reply no longer infers currentness from question words. When currentness is
unresolved and the evidence is time-scoped, that literal-answer shortcut yields
to the reviewed answer path so its time limitation remains available.

Property spans from the model remain intact after validation. The lexical
property-span fallback and post-extraction removal of temporal words are gone;
if semantic extraction fails, evidence selection uses the original request.
Personal-fact follow-up routing now asks a contextual classifier whether the
user seeks a previously reported personal fact, with one retry for invalid or
uncertain output. The phrase list that rewrote selected “I don't know” answers
has been removed; the ordinary contextual answer review and user-evidence prompt
now handle that meaning.

Scholar no longer reconstructs list claims through question-cue, topic and
predicate word lists. Lists go through model extraction, source-entailment
review and the existing bounded extraction repair. A focused regression shows
repair can correct a mistaken member-to-member claim into a source-supported
containing list, while an unrelated entity remains ungrounded. This may reduce
list recall when the model repeatedly fails; those cases now remain visible as
extraction failures rather than silently converting markup to a claim.

Focused validation passed: 73 agent tests, 13 extraction repair tests, 70
Scholar tests and the request-classification test; formatting and diff checks
passed. These tests use model doubles and do not measure live model accuracy or
latency. Consequential-topic policy, semantic stance guards, date/scope conflict
replays, independent benchmark adjudication and a fresh live dialogue replay
remain open. The broader acceptance benchmark is still not claimed as passing.

### Semantic word-list sweep — 14 October

Ordinary generated answers now receive a contextual structured style verdict
(`accept`, `revise` or `uncertain`). The reviewer judges relevance, requested
format, useful detail and conversational fit by meaning rather than forbidden
phrases. A rejected draft gets one rewrite and a second review; malformed,
uncertain or rejected outcomes fail closed and are logged. Research answers
also require an explicit structured reviewer verdict. Phrase-based gates for
stock refusals, echoing, apology counts and “best flavour” source dumps were
removed. Regression fixtures vary the wording while keeping the semantic
judgment constant.

The full Rust suite passes (417 tests), as do formatting and `git diff --check`.
These unit tests use review-model doubles, so they establish verdict handling
and plumbing rather than the 2B model's semantic accuracy. A varied adversarial
evaluation, including generator/reviewer shared misunderstandings, remains a
prerequisite before claiming that model-only review is reliable.

The source sweep found additional runtime language lists that still carry
semantic risk: personal-question and personal-knowledge routing in `agent.rs`;
time-sensitive and consequential-topic classification in `storage.rs`; and
currentness hints in research selection and source limits. The stance checks in
`semantic/guards.rs` have now been removed. Every valid semantic verdict
receives a second adjudication against the original source and claim, with the
first assessment exposed as something to challenge rather than as proof. The
review can correct false approvals and false rejections; it omits the research
question to avoid turning a historical property into a present-day claim.
Uncertainty,
polarity, attribution, scope, entity transfer, list-member roles and unsupported
generalization are reviewed as semantic examples, not phrase triggers. A
malformed audit fails closed and the decision plus raw review output is logged.
The former serialized `stance_override` field remains readable as an alias while
new review decisions use a general field name. A literal-token-sequence escape
that could reinstate an exact claim after semantic rejection has also been
removed; parseable claims reach semantic review, while the complete structural
schema, exact-evidence, safe-field and date checks still run before acceptance.
Focused and full model-double
regressions pass (412 tests); live accuracy and the risk of shared
misunderstanding by the same model remain unmeasured. This adds one model call
for every valid initial assessment, so review latency still needs measurement.
The word lists that
currently protect medical, legal and financial review still require special care
before replacing them with a model classifier: classification uncertainty must
not silently become approval.

Two direct 2B probes exercised the semantic path. The model rejected a
conversation-style personal preference as evidence for what Walkers sells. A
straightforward Earth-orbits-the-Sun claim passed both the initial assessment
and adversarial review; the two inferences took 1.58 s and 1.38 s after model
startup. These are smoke checks, not a representative accuracy or latency
benchmark. The crisp probe was rejected on the initial pass, so it did not test
whether the second pass catches a false-positive first verdict.

Other lists found in the sweep define machine contracts or bounded data
handling: verdict/status enums and JSON field names, URL schemes and sensitive
query parameters, stop words used for lexical ranking, HTML consent/footer
markers, CLI arguments, and Scout's deliberate cross-domain seed mix. They are
not direct phrase-based judgments of user meaning. Keep deterministic checks
for these system rules and test their boundaries. The current next steps track
the remaining semantic candidates and an adversarial evaluation separately.

### Part 1 continuation — historical partial answers — 14 October

The repeated live museum fixture exposed two linked gaps. First, the 2B model
sometimes returned a semantically useful historical-context assessment with a
missing final JSON delimiter; asking it to repair the syntax led it to repeat
the same truncated output. The relevance parser now closes only missing JSON
delimiters when every value is complete, without creating or changing any field.
If a value itself is malformed, one bounded model repair remains available and
all semantic anchors and limitations are still validated.

Second, Scholar selected the 2012 museum display as a historical-context result,
but a separate currentness classifier labeled the same request
not-time-sensitive. That disagreement made Lyra ignore the partial evidence
and abstain. A validated historical-context relevance decision now carries its
own date limitation into the source-limited answer, regardless of the separate
currentness result. The live replay now answers with the two instruments, cites
the fixture source and states that the 2012 display does not establish the
present collection.

The currentness classifier's miss remains a recorded model error; the answer
path is robust to it because the property reviewer separately established the
historical relationship. Regression coverage verifies both syntax-only recovery
and currentness-classifier disagreement. The full Rust suite passes (416 tests),
with formatting and diff checks clean. This synthetic fixture verifies the
production answer path and exact source handling, not the factual quality of a
real museum source. Independent adjudication and cross-topic partial-answer
replays remain open.

Three cross-domain live fixture replays after this change retained the bounded
answer behavior: the negative experiment was quoted with its measured result
and no invented cause; the island list stayed scoped to Alder Island; and the
Clark Kent/Superman relation stayed scoped to Earth-One. The runs completed in
34 s, 190 s and 73 s respectively. These are positive smoke checks for
grounding and answer form, not independent semantic adjudication or a
representative latency benchmark.

### Part 1 continuation — dated comparisons and reviewer false rejections — 14 October

Two additional production-path fixtures exercise differing dates. For a
conflicting start year, the final live answer attributes 2000 and 1998 to their
respective passages and says the evidence does not resolve which account is
correct. A second case asks for Willow Bay's population in 2010 and 2020; after
bounded extraction and property reconsideration, both census figures are
selected and reported with their dates.

The conflict answer exposed a reviewer false rejection: the model's first
critique said the draft failed to state that the dates were unresolved when the
draft did say so. The answer-review prompt now separates passage support from
agreement between sources, and a rejected first assessment receives one
fresh-context semantic reconsideration. A model-double regression verifies
that the reconsideration can correct a false rejection. The live conflict
replay passed review after the clarified support definition; the second-review
recovery itself has not yet been observed in a live run.

The changed-date comparison still receives an unnecessary currentness caveat.
The currentness classifier and its second semantic review both confuse
researching historical figures with needing information about the present.
Repeated prompt clarification did not fix the 2B model's judgment, so this is
recorded as a live semantic failure for further work rather than hidden by a
phrase rule. Separately, relation/object normalization differs between the
two live conflict claims, so storage leaves both awaiting corroboration rather
than linking them as a conflict. The answer path handles the disagreement, but
claim-level conflict metadata still needs repair.

The fixture set now includes both date cases. Relevant replays are archived at
evals/results/grounding-date-scope-replays-v5-2026-10-14 and
evals/results/grounding-date-scope-replays-v6-2026-10-14. Unit coverage now
includes year-only validity normalization, partial components of multi-part
questions, confident currentness reconsideration, and answer-review
reconsideration. Independent adjudication and broad dialogue review remain
open. At the start of the follow-up, the full `cargo test` run had 407 passing
tests and 14 failures. Most failing reply tests assume a previous model-call sequence and
need their model doubles updated for the added bounded reconsideration; the
follow-up below records their repair and subsequent validation.


### Follow-up hour — review contracts and concrete temporal needs — 14 October

The failing tests were repaired without removing their rejection assertions.
Their model doubles now include the bounded reconsideration steps. The audit
also found real orchestration defects: initial review fields were not fully
validated before reconsideration, and the second reviewer lacked some of the
first reviewer's policy. Both passes now enforce the same grounding contract.
A disagreement requires a fresh answer and final review instead of publishing
the rejected draft. Both critiques remain in diagnostics; the rewriter gets
neutral reconstruction guidance, because a live run copied a false scope
explanation from the first critique. The second reviewer receives original evidence without the
prior rationale. Review feedback is serialized as untrusted JSON data for the
answer repair, keeping discarded drafts out of that input. Regression coverage
checks malformed reviews, uncertainty, disagreement and feedback instructions.
The review code is extracted into a cohesive module, and answer instructions
are shorter without a fixed sentence count that compresses multi-part answers.

Direct probes showed that the model could correctly identify past dates while
still applying the broad `time_sensitive` label incorrectly. Temporal intent
now uses two concrete model decisions: whether present-state facts are needed,
and whether the newest available knowledge is requested. Code combines these
boolean outcomes; it does not interpret dates, topics or wording itself. A
fresh second assessment must agree, otherwise the result stays unresolved.
Malformed assessment excerpts and all resolved decisions are observable.
Existing syntax-only JSON delimiter completion is shared with relevance review;
it cannot invent missing fields, boolean values or unfinished strings.

Eight direct-worker probes are retained in
`evals/results/currentness-semantic-probes-2026-10-14` and
`evals/results/currentness-concrete-probes-2026-10-14`. The original broad labels
misread the historical population, current museum, implicit office holder and
latest historical scholarship requests. Concrete flags improved the first
three, but one office-holder response omitted a closing delimiter, and the
latest-scholarship request remained misread. Some reasons also included
irrelevant or ungrounded assertions. These diagnostics use a zero adapter and
are not acceptance evidence or facts eligible for learning. One thinking probe
exhausted 1,400 reasoning tokens in 46 seconds without a final answer; the rest
of that diagnostic mode was stopped. This does not establish that thinking
should be disabled for ordinary answers.

Production-path replays are retained in
`evals/results/grounding-review-contract-2026-10-14` and
`evals/results/grounding-temporal-needs-2026-10-14`. The museum now gets correct
currentness routing and retains its 2012 instruments with a present-status
limit. The dated electrical measurement stays historical. The population
comparison retains both figures without the incorrect currentness caveat, but
adds irrelevant provenance narration and calls different periods a conflict;
its model reviewer accepted that mistake. The conflicting-date replay remains
unstable, including overlong drafts and unnecessary abstention. These results
are retained as failures of semantic interpretation, not hidden by phrase rules.

The live conflict replay also exposed a false rejection by the attribution
speech-word guard: it treated an ordinary source reference as an invented named
speaker. That guard and its runtime speech list are removed. Attribution is
explicitly assessed by the structured semantic reviewer, with model-double
coverage for false attributions, evidenced speakers and ordinary source
references. This increases reliance on model review; it does not establish that
the shared model will catch every attribution error. Other lexical answer guards
remain inventoried in the working plan.

The storage audit found that differing relation strings were automatically
recorded as `does_not_address`. They now remain `unresolved` pending semantic
review. Schema 63 reopens only links created by the old lexical rule; reviewed
relationships, source claims and promotion states are preserved. Fresh-store
and upgrade regressions cover paraphrased properties alongside truly different
properties without inferring their meaning from string inequality. Actual
semantic conflict linking between unverified findings remains unfinished.


A further source-level run is retained in
`evals/results/grounding-semantic-attribution-2026-10-14`. Its unrelated
observatory answer correctly gives the founder and 1974 with its source. The
conflicting-date answer now gets through the source-reference issue, but the
shared reviewer accepts invented scope and unhelpful provenance commentary.
The population reply again retains both dated figures without the currentness
caveat, but repeats those same facts under a redundant "Material limits" list.
These remain answer-quality failures. The latest full Rust suite passes
429 tests; the Python fixture adapter's three tests also pass. Formatting and
diff checks are clean. Unit agreement and successful execution do not imply
a passing semantic benchmark.


The final neutral-guidance conflict replay is retained at
`evals/results/grounding-neutral-review-repair-2026-10-14`. It withheld the
answer after 195 seconds instead of publishing the invented explanation. This
is a remaining unnecessary-abstention failure, not a successful resolution.
The disagreement repair instruction now asks the writer to recheck each
statement independently, preserve every useful source-supported account with
its attribution even when accounts conflict, and remove only unsupported or
irrelevant additions. The follow-up live replay showed this instruction alone
did not fix the unnecessary abstention: the repaired answer retained only one
date and the final review rejected it. Disagreement cases now get one additional
evidence-checked repair after that failure, while ordinary answer repairs
retain their existing bound. A regression exercises this retry sequence; it
passes. The first production replay after the retry change still abstained; its
rewrite chose one date, then reviewers again disagreed about the corrected
answer. A second replay after hiding unrelated source-origin metadata retained
both attributed dates, but still abstained after the reviewers disagreed about
whether the answer addressed the request and whether the date was unresolved.
The metadata change removed the unsupported independence claim from the review
evidence, but answer review remains inconsistent. A thinking-enabled replay
timed out at 360 seconds while composing the initial answer; thinking stays
enabled for the initial answer, while review and repair remain no-thinking to
avoid multiplying that latency. The earlier population replay still contains
duplicated facts and an unnecessary source-independence note; its global
reviewer accepted both, reinforcing the need for assertion-level review.
A new replay after making source-relationship metadata and corroboration
instructions request-specific returned both dated population figures with
their sources and no repeated limitation list or independence note. The first
conflicting-date replay under the request-scoped prompts still abstained: the
reviewer alternately called an attributed, unresolved answer insufficient and
treated “the evidence does not settle the conflict” as an unsupported
definitive claim. The review policy was then revised to require attributed
accounts and an unresolved conclusion, and to explain scope only when the
passages establish it. The follow-up replay at
`evals/results/grounding-conflict-policy-replay-2026-10-14` answered with both
dates, matched each to its source, and said the evidence does not settle the
disagreement. The answer review accepted it on the first attempt. The newer
population answer is also shorter and omits independence commentary, but
repeats the figures once in a summary and again in its source bullets.
Assertion-level concision review remains necessary.
Temporal-intent uncertainty also still sometimes reflects missing factual
knowledge rather than ambiguity about the requested time; uncertain initial
assessments are now recorded explicitly for diagnosis. A missing-checkpoint
unit test's request lock is isolated from live inference after a concurrent
run delayed the suite; production locking behavior is unchanged. Review
contract tests now have their own module, and Python bytecode caches are ignored.

### Chat quiet-window repair — 14 October

The initial two-minute quiet window exempted only the Interaction agent role.
That was incorrect: browser replies also use Curmudgeon for intent, source-check
and answer-review passes, and may use Scholar for foreground research. Those
calls could pause the response they were meant to help. Foreground access now
belongs to the chat turn's runtime handle, independently of agent role. Its
models share the same warm pool as upkeep; the background handle still yields.
Startup also establishes a two-minute quiet window before upkeep can take over.

A scheduling regression verifies that foreground Curmudgeon and Scholar handles
reach backend access while a background Curmudgeon handle remains paused, without
loading a real model. The full suite passes 433 tests and the release rebuild
succeeds. This is orchestration validation, not a live response-quality result.
The sampled running server still had chat's source-check pass waiting for the
pool while Scholar harvested a source. An already-running inference cannot be
interrupted by this gate; the rebuilt server needs a restart to use the fix.
Moving ordinary conversation audits to quiet time is separately proposed in
the runtime roadmap, with knowledge-promotion review retained.

### Conversational delivery and context repair — 14 October

The pasted clarification failure exposed three separate problems. A synchronous
style gate replaced the generated reply with a literal validation-failure
message. Automatic topic detection archived and cleared the session on a
clarification request, leaving an empty conversation in the answer prompt. A
separate acknowledgment route could also send ordinary requests to Curmudgeon
for generation using an acknowledgment-only prompt.

Ordinary dialogue now uses Lyra's Interaction backend and the actual session
history. Session resets are controlled by the browser's explicit new-chat action;
topic interpretation cannot clear the transcript. The acknowledgment route and
literal repeated-answer retry are removed. Conversation-quality audits use the
existing background reply queue, store structured advisory verdicts and historical
context in the proposal ledger, and cannot rewrite delivered text or approve
factual claims. Invalid audit output remains uncertain. Source-backed answer
review and knowledge-promotion gates remain enforced. No database migration is
needed for this audit record.

Live HTTP replays are retained in `state/runtime-chat-replay-2026-10-14.json`
and `state/runtime-chat-final-replay-2026-10-14.json`, with logs alongside them.
The first replay confirmed delivery without the style gate but exposed the
acknowledgment misrouting. The second kept context and generated through Lyra,
but a greeting still took 39 seconds and a clarification took 148 seconds with
an overly generic answer. General interview advice was misclassified as a
factual lookup and returned a source-verification abstention after 170 seconds.
These are retained model failures, not a passing grounding benchmark.

That long response also outlasted the original quiet deadline. Foreground runtime
handles now hold upkeep paused for the entire turn, including cloned model
handles, then start a fresh two-minute idle window. A regression exercises that
lifetime without loading a model. The intent prompt now requires a deliverable
rationale before choosing direct performance or external factual lookup, and
records the assessment for diagnosis. It preserves explicit verification and
consequential-guidance evidence requirements. The full suite passes 431 tests;
latest intent validation also rejects missing or empty rationales.

The deliverable-first assessment then routed interview advice to direct
performance and produced useful advice in 44 seconds. Its clarification still
focused on source verification, exposing source instructions and empty source
fields in a prompt with no evidence. Those instructions and fields are now
included only when relevant source data exists; user-report guidance is similarly
scoped to supplied reports. The final live replay in
`state/runtime-chat-context-scoped-replay-2026-10-14.json` gives concise interview
preparation advice in 43.03 seconds and, on a clarification request, restates
the same four points in 43.48 seconds. Both turns retain session history and
avoid research, style rejection and provenance narration. The actual generation
uses Interaction; background upkeep remains paused. This is a successful small
conversation replay, not independent benchmark acceptance. Reasoning latency and
broader semantic reliability remain open.

The patched development server is running on loopback port 8080. Restarting a
test server also exposed a shutdown wait while upkeep was paused; interruptible
shutdown is recorded in the runtime roadmap. Prior failed replay logs and session
archives remain intact. Stale resident locks from stopped test processes were
removed without touching the live runtime's locks.

### Partial-answer review and retained failures — 15 October

The latest ordinary source-backed reply changes keep review rationales out of
rewrites and present evidence limits as source guidance rather than internal
status labels. Rewrites retain the prior user-facing draft as fallible context.
This improves direct answers but has not eliminated irrelevant caveats or
repeated conclusions. Source guidance remains data and cannot override policy.

The ambiguous-title replay `part1-grounding-2026-10-15m` preserved the exact
passage and acknowledged that item boundaries were unresolved. Subsequent code
review found that this final recovery could bypass semantic review. Two new
regressions reproduced that gap before its fix. The source-literal candidate now
requires a separate structured useful-partial assessment. Rejected, malformed,
or uncertain assessments leave the answer unresolved. Review validates a
nonempty rationale and exact nonempty answer/source quotations, applies
source-state policy, and masks source URLs so their paths cannot act as evidence.
These are review-contract checks, not deterministic proofs of semantic quality.

A newly added repeated-conjunction shape heuristic was removed during cleanup.
It inferred too much about human meaning from a narrow token pattern. Shortened
names instead rely on semantic review; a regression verifies that a semantic
rejection produces a fresh reviewed answer. This does not prove the local model
will identify every such error. The archived shortening failure remains useful
adversarial evidence. Older literal restructuring/name/causality guards remain
explicit follow-up work, including their false-rejection risk.

An uncertain ordinary answer review can also trigger this bounded assessment.
It independently checks whether all factual assertions are supported or
attributed, useful information addresses the request, and unresolved conclusions
remain explicit. It authorizes conversational delivery only; it cannot promote
claims or establish source independence. Review prompts distinguish confidence
in answer acceptability from uncertainty of the subject. Temporal classification
now explicitly separates request intent from ability to supply its factual answer.

The `part1-grounding-2026-10-15q` BF16 run reached both conflicting dates with an
explicit unresolved conclusion, but repeated itself under unnecessary headings.
It then timed out at 600 seconds during the extra partial review, without a
completed answer. The runner stopped its process group and no fixture worker
remained. This is a retained failure, not a pass. The configured BF16 browser
server was left running. The live test findings do not establish that model
contention caused the timeout.

The full Rust suite passed 446 tests; 65 focused reply tests subsequently passed
after the final-review policy/masking changes. A development build succeeds with
the existing three dead-code warnings. The independent blind benchmark and the
ordinary-dialogue acceptance gate remain open; same-model reviews and mocked
orchestration tests do not satisfy them.

The `15r` production-lock replay was deliberately interrupted without a completed
sample, retaining an `interruption.json` record and its raw traces. Some request
waits exceeded two minutes while inference itself took about twenty seconds.
The stopped fixture's resident marker was removed after confirming its process
had exited. During the separate chatbot replay, process samples showed the web
thread waiting for an MLX response and the background thread inside an already
started Scholar harvest. They did not show the web thread waiting for upkeep's
Rust pool lock. The samples are archived alongside `15r`; this does not establish
which hardware/resource effect accounts for all latency.

A full-suite run during active chat also exposed test isolation, not a scheduling
regression: the chat-priority test isolated only its background request lock and
could block on the live application's chat lock. Both test locks are now private.
The subsequent full run passed all 446 tests while the dialogue replay continued.

The dialogue replay then exposed an input-provenance failure. Asked to draft an
introduction using supplied details and placeholders, Lyra converted its own
research advice into an unsupported claim that the user had already researched
the role. It also claimed to have used placeholders when its preceding answer
contained none. The code turn supplied the correct example but added a false
limitation based on the preceding interview topic. The raw conversation remains
in `state/part1-dialogue-replay-2026-10-15.json`. A shared context-use policy now
keeps supplied facts distinct from assistant suggestions/examples in both
conversation and task prompts, and limits earlier constraints to their actual
scope. A wiring regression preserves original speaker roles. These prompt
changes are not yet demonstrated semantic fixes.

The final dialogue turn answered the library question but queued an unrelated
combined research objective about both code and placeholders. Inspection found
the phrase-matching follow-up helper appending the previous two user questions.
That helper is removed. The existing intent pass now emits a bounded standalone
request along with its routing decision, preserving role/context data and
repairing incomplete output once. Source-check intent uses the original message
and context separately so request rewriting cannot waive explicit verification.
The schema and orchestration regressions do not prove semantic resolution quality.

Final validation passes **450 Rust tests**, three Python fixture-runner tests,
formatting and whitespace checks, and the release build. Two additional test
isolation issues were corrected without changing production policy: checkpoint
fixture directories now have a counter as well as a timestamp, and database
fixture filenames resist stale files after PID reuse. The live browser service
was left running on its earlier build at the user's request to wrap up; no
post-change dialogue or full benchmark pass is claimed. Restart it and replay
the retained failures before treating the context/intent changes as verified.
