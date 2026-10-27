# Log behavior fix plan

This plan follows the review of `community-lab.log` on 25 October 2026. It
addresses observed behavior while retaining the grounding, provenance and review
requirements in `AGENTS.md`. Semantic decisions must remain contextual model
judgments; the fixes must not add topic-specific word lists or phrase rules.

## 1. Keep unresolved relevance reviews out of accepted findings

**Observed:** extraction can retain an accepted claim when a property review is
malformed, a repair fails, or a repaired claim is rejected. The generic
extraction retry also ran after the property gate, so its replacement could
avoid that gate.

**Change:** run the bounded extraction repair first, then run the property
review/repair gate on the final candidate. If the review is invalid, uncertain,
or rejects the candidate and no same-passage repair passes both semantic and
property review, return no eligible finding. Keep the candidate output and
reason in diagnostics so the failure remains reviewable.

**Check:** cover malformed initial and repair reviews, rejected repairs,
successful same-passage repair, and the generic extraction retry path. Retain
the original log cases as replay inputs where possible.

**Status:** implemented and unit-checked. The bounded extraction retry now runs
before the property gate, and unresolved property reviews remove the finding
while retaining the model output and reason for diagnosis. The controlled
extraction suite and full Rust suite pass. The isolated Trapezium-age replay
produced a relevant same-passage claim that remains `needs_review` pending
source-quality review. The malformed-review and rejected-repair branches still
have only controlled unit coverage; mocked judgments do not establish semantic
reliability. A live bulletin replay without its original title context stalled
in discovery and was stopped after two minutes with no new diagnostics; it is
inconclusive and does not count as validation of that retained case.

## 2. Preserve requested meaning in research queries

**Observed:** several distinct questions about Trapezium-star properties
collapsed to the entity name `Trapezium`, repeatedly retrieving disambiguation
and geometry pages.

**Change:** have contextual model reasoning construct and assess search queries
against the original request, conversation context and named entity. Keep the
requested property or answer type in the query where useful. Do not encode
individual topics or question phrases as runtime rules.

**Check:** exercise paraphrases, ambiguous names, unrelated entities, and
requests with multiple properties. Measure whether returned sources cover the
requested property, not just the entity.

**Status:** implemented and live-checked once. The fallback search prompt now
preserves the requested property and constraints, the contextual lookup
planner has the same requirement, and the redundant subject-only query was
removed so it no longer displaces the full request. A temporary-database run
generated `Trapezium stars approximate age`, then tried the full question;
Wikipedia returned no finding, while SearXNG returned age-specific astronomy
sources and Lyra retained a Chandra-supported median age of about 300,000 years
for review. This is one local-model case, not broad search-quality evidence.

## 3. Stop unproductive research from accumulating review items

**Observed:** related but insufficient sources can create multiple
`needs_review` findings without making progress toward the requested answer.

**Change:** track whether each attempt adds coverage or independent evidence;
avoid treating repeated or shared-origin material as corroboration. When
research stops progressing, reconsider the query or park it with an explicit
unresolved outcome. Review existing affected records through the provenance-
preserving correction workflow.

**Check:** verify that useful partial evidence remains available, unresolved
outcomes remain explicit, and repeated evidence does not count as independent
support.

**Status:** reviewed. The inspected jobs were five distinct requested
properties, not repeated attempts at one unresolved question. Their findings
remain provisional under the existing review gates. No production records were
changed. Query-quality measurements will determine whether progress tracking
needs a code change beyond preserving the property in each query.

## 4. Reduce avoidable thinking and model reload time

**Observed:** a simple greeting used nearly its full thinking budget, and
role-specific adapters caused frequent model cache evictions and reloads.

**Change:** choose bounded thinking effort from task context, and inspect model
load/cache behavior before adjusting cache capacity. Account for the 16 GB
memory limit and maintain independent role adapters.

**Check:** compare simple conversational requests with complex research and
repair tasks. Measure end-to-end latency, load counts, cache misses and memory;
check that response quality and repair gates remain intact.

**Status:** partial measurement completed. The greeting's 1,024 thinking tokens
came from the configured 256-token initial budget being extended to its 4x cap.
The request-intent assessment now judges whether a turn needs extended
reasoning. Simple turns can skip the thinking phase; factual/source-check tasks,
explicit debug requests and reasoning assessment failures retain it. An
isolated INT8 greeting used zero thinking tokens and generated the visible
answer in 347 ms of Interaction inference; the old log recorded 20,833 ms of
inference and 1,024 thinking tokens. This is a single before/after pair with
different sessions, not a controlled latency benchmark.

Two three-greeting CLI runs compared local model-cache capacities one and two.
Capacity one loaded seven role workers; capacity two loaded Curmudgeon and
Interaction once each. Time from request-intent assessment to visible
Interaction completion totalled 4,350 ms at capacity one and 3,203 ms at
capacity two. The first capacity-two answer took longer while it loaded a cold
worker; the next two were faster. Each worker peaked around 2.26 GB; system
memory pressure after both runs reported 59% free. The ignored local `.env` now
sets cache capacity two for this 16 GB machine; the code default remains one
for other installations. This is a small local pilot, not a broad memory or
quality benchmark.

## 5. Diagnose the intermittent web request error

**Observed:** the log contains `Resource temporarily unavailable (os error 35)`
while handling a web request. The current log does not identify the failing
request stage or establish the cause.

**Change:** add stage-specific, privacy-safe diagnostics for accept, read,
parse, response write, timeout and peer disconnect. Use those diagnostics to
reproduce and fix the actual failure.

**Check:** distinguish idle-client timeouts and disconnects from server-side
errors, and verify that one bad connection does not stop later requests.

**Status:** reproduced and handled; the old event's exact cause remains
unproven. An isolated server reproduced `WouldBlock` / OS code 35 while waiting
for the rest of an HTTP header. The new log records `web.request_read_timeout`,
phase `headers`, and the 33 bytes received. The server then returned HTTP 200
for the next valid request. Idle/incomplete clients now close quietly with a
specific timeout event. The old log omitted the stage, so we cannot establish
that this was the cause of that exact request.

## 6. Validate the fixes together

**Change:** replay retained failures alongside unrelated, ambiguous,
paraphrased and adversarial cases. Preserve original failures and run metadata.

**Check:** track irrelevant accepted findings, unsupported answers,
unnecessary abstentions, repair counts, research progress, latency and memory.
Run appropriate Rust checks and live SearXNG/INT8 smoke checks. Keep mocked and
synthetic-source evidence labeled as such. Independent semantic judgments are
required before claiming benchmark acceptance or enabling training.

**Status:** partial. `cargo fmt --check`, `git diff --check` and all 471 Rust
tests pass. Isolated INT8 greeting, repeated-turn cache, live SearXNG/Trapezium-
age and idle-client web tests completed. No independent judgments were
obtained. The grounding benchmark remains unaccepted and training remains
gated.

## Completion criteria

The retained relevance failures no longer produce eligible irrelevant
findings; useful supported partial answers still work; search queries retain
the requested meaning; unresolved research does not loop or inflate review;
latency changes are measured without weakening review gates; and the remaining
web error has a diagnosed cause or retained evidence describing its limits.
