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

## Validation and limits

**388 Rust tests and 15 Python tests pass.** New regressions were observed failing
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
