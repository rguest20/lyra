# Research Baskets And Quotas

## Request Control

Wikipedia requests use the existing persisted `host_request_log` reservations,
including cross-process spacing and in-flight limits. The additional rolling
budgets count all Wikipedia API requests: search, section lists, and page text.
Reservations and failed attempts count; cache hits do not.

```dotenv
SCHOLAR_WIKIPEDIA_INTERVAL_SECONDS=10
SCHOLAR_WIKIPEDIA_REQUESTS_PER_HOUR=60
SCHOLAR_WIKIPEDIA_REQUESTS_PER_DAY=200
```

These are conservative local operating budgets, not Wikimedia-published
entitlements or a guarantee against shared-IP/provider limits. Intervals cannot
go below ten seconds. Zero, invalid, or out-of-range settings fall back to safe
defaults rather than disabling the limiter. There is no shared daily cap on
Wikipedia/external page-content fetches. Wikipedia search, section-list, and
page-content calls still count toward the all-call Wikipedia budgets. External
pages retain host spacing, in-flight limits, source policies, and server backoff.
`scholar usage` retains the combined fetch counter and reports
`shared_page_fetch_limit: null` to indicate that no shared cap is enforced.
It also reports the effective Scout investigation interval, daily budget,
recent investigation count, deferred model-lead count, and current queue blocker.
Pending jobs deferred solely by the removed shared cap become eligible again;
Wikipedia budgets and server backoff can still defer them independently.

Scout queue scheduling limits are not judgments that a question is bad. A valid
model lead blocked by the interval or daily budget remains in its existing probe
record with a `deferred:` reason. Later passes retry admission without another
model call or probe reservation. Deferred probes keep the existing `attempting`
database status; `scout.model_probe_decision` reports `decision: deferred`.
Unchanged deferral reasons are not logged repeatedly. Topic duplicate/cooldown
and exhausted-attempt rules still block redundant investigations. Scout checks
global queue admission before generating another model probe.

HTTP 429/503 and MediaWiki `ratelimited`, `maxlag`, `readonly`, and transient
database errors create persisted cooldowns. JSON API errors returned as HTTP
200 remain HTTP 200 in the request ledger; the cooldown uses a throttle category.
Without `Retry-After`, consecutive throttles wait 1, 2, 4, 8, 16, 32, then 64
minutes. Successful responses reset that sequence. Valid `Retry-After` delays,
including delays longer than a day, are not shortened. Requests include
`maxlag=5`.

Quota-blocked questions remain `pending` with `next_attempt_at_ms`, rather than
becoming `no_result`. The scheduler waits until the applicable rolling budgets
and persisted cooldowns allow another attempt. Conversation-originated research
retains priority when due. Existing terminal `no_result` jobs are not rewritten;
use `scholar ask` to resubmit a historical question explicitly.

Successful Wikipedia search responses cache for one hour; sections and page
content cache for one day, with at most 200 entries and 2 MB per entry. External
successful page responses cache for one hour, with at most 64 entries and 1 MB
per entry. Robots, manifest, safe-address, and source-access checks still run
before external page-cache use. Cache reuse never invokes an MCP endpoint or
bypasses source restrictions.

## Durable Intake

Fetched source text is retained independently of answer extraction. Wikipedia
search leads and selected sections, and external fetched plain text, enter the
source store even if the original finding is rejected, incomplete, or irrelevant
to the immediate question. Each source is bounded to 32,000 characters and
deduplicated by URL and content hash. This is bounded retention, not a complete
website archive. Existing source-policy and raw HTTP response limits remain.

Background Scholar work leases a source and processes at most 2,200 characters
per pass. A durable cursor advances only after successful basket storage. Failed
or malformed extraction gets one format-repair attempt, then keeps the text and
schedules another attempt after five minutes. Each passage is asked for all
distinct grounded assertions and follow-up questions; there is no fixed
per-passage item count. Output remains bounded by the model's available context
window. User research, repair work, and corroboration retain their existing
priority.

Claims are concrete, fact-shaped assertions, not verified facts. For example,
an explicitly quoted assertion that the Amazon rainforest is made of chocolate
can be retained with its source and qualifications without entering the semantic
map. Questions, fictional premises, and attributed statements must not be
converted into endorsed assertions. Negation, dates, uncertainty, and attribution
are preserved in the quotation and qualifications. Extraction confidence measures
faithful extraction, not truth.

Questions retain their exact source basis and deduplicate by normalized wording.
If a generated question paraphrases its basis, the original input passage is
attached as context provenance instead. This records where the question came
from, not evidence that any premise in the question is true. Claim quotations
are never repaired by substituting the whole passage.
Repeated source occurrences add provenance rather than another question. Scout
schedules one retained question after urgent challenged/due claims, under its
existing spacing and daily investigation policies. Due user questions take
priority. At most 20 retained questions are queued simultaneously, and derived
question chains stop at depth three. Terminal research is labeled `researched`
or `unanswered`, never automatically `answered` merely because a job ran.

## Evidence Relationships

When existing Curmudgeon review/audit queues are idle, it compares one retained
cross-source claim pair about the same normalized subject. It records
`supports`, `contradicts`, `qualifies`, `unrelated`, or `insufficient`, with a
rationale and reviewer model ID. Scope, time, negation, attribution, and unequal
specificity matter; different object text alone does not prove contradiction.
Each claim gets at most eight new comparisons. Temporary review errors retry
after five minutes without inventing a relationship or rejecting either claim.

Claim inspection aggregates supporting, contradicting, internally disputed, and
qualifying hostname groups. Repeated pages on one host do not count repeatedly.
The provisional support score is:

```text
supporting_groups /
(supporting_groups + contradicting_groups + disputed_groups + qualifying_groups + 2)
```

The original quoted source contributes one supporting group. This is a heuristic
evidence balance, not a calibrated probability of truth. Different hosts do not
establish independent reporting: inspection explicitly reports
`independence_verified=false`. Copied reporting on different hosts can still
inflate this provisional metric. The existing provenance, freshness, confidence,
privacy, and promotion gates remain authoritative; this score is not supplied to
chat as established truth and does not automatically promote or retract claims.

## Operations

Stop running Lyra processes, back up the learning database, then restart with the
new binary. The original additive rollout upgraded schema 44 through 47 and
retained existing findings, learning/audit records, and semantic claims; later
migrations bring the current schema to version 63. The JSON map format remains
unchanged. Do not run old and new binaries against the same database.

If the source basket is empty, the next background Scholar pass automatically
imports previously saved finding/candidate excerpts once material is available.
This seeds sources only: claims and questions appear as background harvesting
processes them. Existing reviews and semantic claims are unchanged. User work,
repair, and corroboration can postpone harvesting; an empty claim basket does
not imply that the saved source text was discarded.

```sh
cargo run --release -- scholar usage
cargo run --release -- scholar baskets
cargo run --release -- scholar baskets sources
cargo run --release -- scholar baskets claims
cargo run --release -- scholar baskets questions
cargo run --release -- scholar baskets source 1
cargo run --release -- scholar baskets claim 1
cargo run --release -- scholar baskets retry 1
cargo run --release -- scholar baskets import
```

`retry` restarts harvesting of an idle retained source; claim/question insertion
remains deduplicated. `import` retains up to 500 previously saved finding and
candidate excerpts per invocation, without fetching pages or changing old review
decisions. New raw assertions and relationships remain separate from the map.

Diagnostics `internet.cache_hit`, `scholar.job_deferred`,
`research.baskets_retained`, `research.harvest_deferred`,
`research.question_queued`, and `research.evidence_link` record IDs, counts,
status, and scheduling metadata rather than adding prompt contents to logs.

### Bounded basket-output repair

Basket extraction starts at 1,024 output tokens. An invalid response gets a fresh
schema-guided attempt at 2,048 tokens, then one final attempt at 4,096 tokens.
These are output budgets; structured extraction continues to disable thinking.
The normal context budget still applies. Other extraction operations keep their
existing default cap.

After three invalid responses, the harvest stops and the source is parked until
an explicit `scholar baskets retry SOURCE_ID`. Its body and harvest cursor are
preserved; invalid partial baskets are not stored. The existing retry timestamp
uses `i64::MAX` for parked sources, so no schema migration is required. Transient
model errors and storage errors retain the existing five-minute retry cooldown.
`research.harvest_abandoned` identifies exhausted schema/syntax retries; the source
is preserved rather than deleted. Parse diagnostics include the
actual schema/syntax error, attempt, output budget and bounded output excerpts.
`research.basket_output_budget_extended` records each increase. A question must
still be an object containing `question` and `evidence_text`; plain strings are
not silently accepted as valid questions.

Use `model probe-baskets TITLE PASSAGE` to replay extraction without inserting
its generated claims or questions into the baskets. The probe follows the same
budget and repair policy as normal harvesting.
