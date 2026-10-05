# Scout and Scholar Research Expansion

**Status:** implementation in progress; Scout and Scholar run unattended with
bounded probes, durable provider queues, policy-checked retrieval, provenance
assessment, and conservative promotion gates.

## Implementation progress

Scout has a separate three-per-rolling-day model-probe budget with five-minute
spacing. It asks model-derived public questions only after map gaps are
exhausted, filters duplicate and private leads, and records the result and link
to any Scholar job. A model response alone never creates a claim.

Scholar retains Wikipedia and also uses persisted SerpAPI and Tavily discovery
queues. Search results are leads only: a separate bounded fetch checks robots
and site policy, records exact passages and source assessments, and leaves
unsupported or uncorroborated findings out of the semantic map. `scholar show`,
`scholar queue`, `scholar sites`, `scholar blocked`, and `scout` expose the
operational records. The provider interface is implemented. An optional, local-only SearXNG
adapter can provide discovery leads when configured; Wikipedia remains the
no-credential baseline.

SQLite migration v21 records a source–claim assessment for every new Scholar
finding and backfills older findings as `unassessed`. `scholar show` displays
the assessment. Automatic promotion requires two directly supporting findings
from distinct source origins, each meeting the configurable extraction-
confidence threshold (default `0.90`), with no unresolved provenance warning
and no active map conflict. Direct findings that have not passed this gate
remain available to Lyra as attributed, uncertain evidence. A confidence
score describes extraction confidence; it is not a probability that a claim is
true. Conflicting direct evidence challenges an active claim.

Further source-identity automation and broader scheduler verification remain
open tasks. SearXNG must be configured locally before it can provide leads;
Wikipedia continues to work without it.

SQLite migration v38 now leases HTTP request slots across processes, limits
work to two concurrent requests globally and one per host, caches `robots.txt`,
`llms.txt`, and `agent-manifest.txt` responses for six hours, and persists
host backoff after HTTP 429 or 503 responses. Curl still does not follow
redirects; a redirect is recorded as unavailable until Scholar can validate
the new destination against DNS and site policy.

Scholar now extracts claims from policy-checked external page passages with the
same exact-passage grounding checks used for Wikipedia. One job can record a
Wikipedia finding and separate external findings, each with its own source URL,
passage, assessment, and review decision. External pages enter with unknown
source quality and unverified independence until those properties are assessed;
they cannot gain automatic trust merely by appearing in search results or by
agreeing with Wikipedia. A storage integration test follows an external
finding from staging through explicit review to a map claim and its source
passage. Fetched HTML now retains self-declared canonical URL, publisher,
author, publication date, and cited URLs for inspection. The corroboration
gate flags a substantial ordered passage match, matching canonical URL, direct
citation between the pages, or a shared declared citation as a possible shared
source, even when two different underlying source IDs were supplied. A short
shared fact does not count as copying. These metadata fields remain unverified
and do not grant expertise or independence. Canonical and citation links queue
a bounded Curmudgeon review; it records whether the pair looks like shared
origin, unclear, or possibly distinct reporting. A possibly-distinct verdict
resolves that provenance warning only; the confidence, direct-support,
two-origin, and map-conflict gates still apply. Secondary source origin can now be
reviewed explicitly with a stable publication ID, evidence passage, and reason;
the same reviewed ID counts as one origin, while distinct IDs can corroborate
only after review. Scholar now also extracts declared DOI and PMID values
and stable publisher URLs from structured data. Matching publication IDs flag a
shared work; matching publisher IDs flag a possible shared publisher for
review. Author and publisher names alone do not establish common ownership or
independence.
Source suitability and independence are now assessed automatically for direct,
non-personal findings. Two findings must have confidence at or above
`SCHOLAR_AUTO_PROMOTION_CONFIDENCE` (default `0.90`), directly support the same
claim, answer the research question directly, and come from distinct HTTPS
hosts. Matching or copied passage signals block promotion unless Curmudgeon
resolves the pair as possibly distinct reporting. An active conflicting map
claim blocks promotion until it is retracted or superseded. Source assessment
and origin records identify whether their review method was automatic or
human. Staged direct findings remain available to Lyra as attributed,
uncertain evidence; they do not enter the semantic map until these conditions
are met. Personal facts about the user cannot be established from web pages.

An isolated live chicken soup run returned 32 SerpAPI leads and five Tavily
leads. The first candidate selection hit a robots denial and a redirect; the
bounded fallback then fetched one relevant recipe page. Its excerpt was stored,
but the extractor did not produce a grounded claim from that page, so no
external finding was invented. Scholar checks a bounded shortlist of up to 12
distinct hosts and tries at most three page fetches per provider/query.
Explicit robots denials are cached in SQLite for 30 days,
with a path-specific entry for denied pages and a host entry when the robots
endpoint itself refuses access. `scholar blocked [limit]` shows active entries.
Migration v29 imports robots denials already recorded on search candidates.

Migration v30 records each fetched page's extraction attempts with the exact
passage, model output, status, and rejection reason. External pages now supply
up to two contiguous, ranked sections to the local extractor. If neither
produces a grounded claim, a generic heading-plus-bullets reader can stage a
claim about that specific listed subject at lower confidence. This reader does
not depend on recipes or particular list topics. A repeat of the chicken soup
case produced a directly supported list from the recipe page and held it for
review. The run also exposed that `chicken stock` was classified as financial
`stock`; the topic check now uses more specific financial phrases.
Migration v31 upgrades an early attempts table so the list fallback can retain
its own decision without losing either model attempt.

Wikipedia retrieval has since been deepened as Scholar's baseline source.
Scholar ranks up to three search results using the question and each page's
introductory extract, requests section headings for the best article, and reads
up to two matching sections instead of downloading an unbounded full article.
Each request is recorded in SQLite. Migration v23 persists host request
reservations, enforces ten-second spacing across restarts, and caps section
content fetches at 100 per rolling day. Search extracts stay within the
MediaWiki API's documented 1,200-character limit. Extracted findings retain
their direct/related/tangential relevance so related context receives lower
weight during retrieval.

Scholar now discards extracted claims unless their exact evidence occurs in
the fetched excerpt and the claim's subject and object have meaningful lexical
support there. It no longer invents a generic summary/detail finding when the
extractor cannot produce a grounded claim; the job remains `no_result` instead.
Parsed Wikipedia HTML now drops reference superscripts, reference lists, and
edit controls while preserving headings and list structure. The extraction
prompt explicitly handles lists, and a bounded fallback can convert an
explicitly introduced list into one claim whose exact evidence block contains
the heading, introducer, and every item. Storage validates each semicolon-
separated item against that passage and permits its normal line breaks. A live
chicken soup query now records the listed ingredients as directly supported;
the source gate correctly leaves the finding awaiting independent
corroboration.

**Current assessment:** Wikipedia is sufficient as Scholar's initial research
source; no further Wikipedia-specific cleanup is currently planned. Retrieval
uses the best-ranked article and up to two relevant sections, but it does not
independently verify a finding against another source. A single Wikipedia
article remains one secondary source and cannot satisfy corroboration by
itself. The corroboration queue now directs SerpAPI and Tavily to seek a
second source for eligible staged claims. Claim-specific assessment and
independence checks still decide whether that source can support promotion.

The discovery schedule now favors weak Wikipedia findings. SerpAPI has a
persisted 30-minute minimum between calls and Tavily has a persisted one-hour
minimum. When Wikipedia produces a directly relevant finding, a provider is
used only once twice its base interval has elapsed: one hour for SerpAPI or
two hours for Tavily by default. The first eligible job can use a provider
whose history is empty. Search attempts retain `wikipedia_insufficient` or
`exploration_elapsed` as their trigger. The base intervals can be tuned with
`SCHOLAR_SERP_INTERVAL_SECONDS` and `SCHOLAR_TAVILY_INTERVAL_SECONDS`.

SQLite migration v37 adds one corroboration queue item per direct public claim
awaiting independent evidence and a history of provider attempts. Each
provider takes an eligible item when its own persisted interval and quota
permit, with at most one active attempt per claim. Scholar searches for the
specific staged claim, skips already cited hosts, and accepts only a matching
claim from a fetched passage. The normal source assessment, provenance,
conflict, and promotion gates still decide whether evidence can change the
map. A failed or empty search leaves the item queued until its next eligible
slot; three attempts per provider exhaust that provider's work on the claim.
Worker leases recover interrupted attempts, and `scholar queue` shows the
queue state.

## Proposal

### Why

Before this change, Scout found gaps in SQLite and the editable JSON map. It could ask the
local Qwen3-0.6B model for a focused question about an encyclopedia claim, but
it could not deliberately investigate a topic suggested only by the model's
pretrained weights. Scholar searches English Wikipedia, considers at most three
results, and reads short introductory extracts. This is useful for simple
encyclopedia facts but gives the agent little source diversity or basis for
judging a source's expertise on a particular subject.

The maintenance loop already wakes promptly after terminal conversation
activity. Browser maintenance runs after requests and on an idle timer. We
want durable work to wake the relevant role promptly in both modes while
retaining periodic checks for missed signals, expired leases, and due reviews.

### What changes

1. Scout may turn model-suggested topics into bounded, specific research
   questions. A model suggestion is a lead, never evidence or a map fact.
2. Scholar can use a configured open-web provider when Wikipedia is unhelpful
   and occasionally sample it even when Wikipedia supplies an answer. It
   retrieves and compares result pages with bounded curl requests, records source
   provenance, and evaluates each source's suitability for the particular
   claim. It promotes only claims supported by suitable evidence; unclear or
   consequential claims remain available for review.
3. Durable queue writes notify the maintenance scheduler. A timer remains as
   a recovery path. Bounded synchronous fetch workers handle the initial
   network load; the observable requirement is prompt, coalesced event handling.

### Boundaries

- The safetensors checkpoint is not a browseable fact store. Scout can only
  probe it through model inference. A fluent answer from the model does not
  establish truth.
- Autonomous work still does not train or replace model weights. Model
  consolidation retains its existing approval and rollback process.
- Public web research remains excluded for personal facts about the user.
- SQLite remains the durable record of jobs, evidence, decisions, and claims.
  In-memory notifications are hints to check that record, not the record itself.

## Requirements

### R1. Weight-derived research leads

- Scout **SHALL** be able to propose a bounded number of public research topics
  or questions using the local model, even when the topic is not yet an active
  SQLite or JSON claim.
- Model-only probes **SHALL** run only after eligible map-derived gaps have
  been exhausted. They **SHALL** have a separate default cap of three probes
  per rolling day, including probes that do not produce a Scholar job.
- Each lead **SHALL** record its origin (`model_probe`), prompt or seed topic,
  proposed question, creation time, and selection reason.
- Scout **SHALL** turn a lead into a specific externally researchable question
  before Scholar receives it. Repeated or near-equivalent questions, private
  topics, and topics already covered by suitable active evidence **SHALL NOT**
  create another job.
- Model output **SHALL NOT** be promoted as a semantic claim without external
  evidence passing Scholar's normal policy.

**Scenario:** with no Byzantine claim in the map, a permitted model probe
suggests the Byzantine Empire as a topic. Scout queues a specific question such
as “Which sources describe the Byzantine Empire's administrative system?” The
queue records `model_probe` as its origin. No Byzantine fact appears in the
semantic map until Scholar obtains supporting evidence.

**Scenario:** the model repeats a topic or supplies an unsourced historical
assertion. Scout records or rejects the lead according to the retry policy;
neither output becomes a claim solely because the model said it.

**Scenario:** the map has an unresolved challenged claim. Scout researches that
gap before spending a model-only probe. Once map-derived work is exhausted,
no more than three model-only probes run in a rolling day.

### R2. Broader, auditable research

- Scholar **SHALL** support a `SearchProvider` interface with Wikipedia and
  a configurable broader discovery provider. Tavily **SHALL** be the initial
  production provider; a configured self-hosted SearXNG instance **SHALL** be
  the local fallback or the default when Tavily credentials are absent.
  Broader discovery **SHALL** run when
  configured and Wikipedia has no directly relevant evidence, and on a
  an elapsed-time exploration check when Wikipedia is useful. SerpAPI is
  eligible after twice its persisted 30-minute interval and Tavily after twice
  its persisted one-hour interval. The first job may search when no earlier
  provider call exists.
- A Google-backed adapter **SHALL** use SerpAPI's authorized search endpoint.
  Search results are discovery leads only; `curl` must fetch a permitted
  source page before any passage can support a finding. If a provider is
  unavailable, Scholar **SHALL** record that outcome and continue with sources
  already retrieved.
- SerpAPI calls **SHALL** be spaced by a persisted configurable interval,
  defaulting to one call every 30 minutes. `SERP_API_KEY` supplies the key;
  `SCHOLAR_SERP_INTERVAL_SECONDS` configures the interval and
  `SCHOLAR_SERP_MAX_RESULTS` bounds the candidate list (1–10, default 5).
  Each returned domain **SHALL** be stored with the exact search query and
  result count; candidate URLs, titles, snippets, and fetch status **SHALL** be
  retained with the Scholar job. This search-to-domain link is provenance about
  discovery, not evidence that a site's content is correct. Search rank and
  frequency **SHALL NOT** raise source trust. A source's trust may rise only
  after a checked passage supports a claim that is independently corroborated
  by evidence with verified separate provenance. Repeated copies of the same
  underlying source do not count as independent corroboration.
- When SerpAPI returns typed rich result groups, Scholar **SHALL** preserve
  each linked result's returned type and bounded structured fields next to its
  query and source URL. This applies across categories such as recipes,
  shopping, and local results rather than assuming one result type. The fields
  help classify and prioritize a source for follow-up; they remain
  search-provider data, not verified claims.
- Tavily credentials currently come from `TAVILY_API_KEY`; local
  `config.toml` support is still planned. Missing or unreadable credentials **SHALL** record
  `provider_unavailable` in SQLite without stopping the application.
- Tavily **SHALL** have a persisted cap of 25 search calls per rolling 24
  hours and 750 search credits per provider billing month. Standard calls
  **SHALL** use `search_depth = "basic"`; an `advanced`
  retry is allowed only when basic yields no usable candidates, and it counts
  as another call. Search snippets **SHALL** remain discovery hints until
  Scholar fetches and checks their source pages.
- Scholar **SHALL** inspect a bounded set of results, fetch relevant pages,
  and keep the exact passage used to support each proposed claim.
- Before fetching a page, Scholar **SHALL** check the host's `robots.txt` for
  its declared `CommunityLabScholar` user agent, then inspect `/llms.txt`,
  then `/agent-manifest.txt` when present and permitted. It **SHALL** honor
  understood restrictions, prefer eligible Markdown pages listed by
  `llms.txt`, and record explicit capability endpoints without invoking
  arbitrary tools. An unparseable or ambiguous restriction **SHALL** skip the
  host with `policy_ambiguous`. Absence of an optional file is neither
  permission nor prohibition.
- The initial manifest parser **SHALL** recognize only the project profile
  defined below: `version`, `governance_policies.disallow_agents`,
  `rate_limit_req_per_min`, `require_auth`, and `endpoints.llms_txt` or
  `endpoints.mcp_server`. An explicit disallow skips the host as
  `manifest_disallowed`; an unknown key or ambiguous clause skips it as
  `policy_ambiguous`. A declared rate may tighten, never relax, host spacing.
- Scholar **SHALL** allow at most two in-flight HTTP requests globally, one
  in flight per host, at least ten seconds between requests to the same host,
  and at most 100 external page fetches per day. Metadata requests count for
  concurrency and host spacing. A SQLite `host_request_log` **SHALL** preserve
  scheduling and backoff across restarts. Timeouts, redirect and response-size
  caps, and `429`/`503` backoff **SHALL** also apply.
- Every source record **SHALL** include its URL, publisher or domain, page
  title, retrieval time, available publication or update time, and the fetched
  passage. Scholar **SHALL** record which search query and provider found it.
- Source assessment **SHALL** be claim-specific: topical expertise,
  directness of support, currency when relevant, independence from other
  sources, and identifiable provenance. A domain's reputation alone **SHALL
  NOT** grant blanket authority across subjects.
- Scholar **SHALL** distinguish independent corroboration from pages that
  merely repeat the same underlying source.
- Automatic promotion **SHALL** require at least the configured extraction-
  confidence threshold (default `0.90`), two directly supporting findings
  from independent source origins, and resolution of all active map conflicts.
  An unresolved provenance warning holds the pair. Extraction confidence is
  not truth probability. Personal facts about the user cannot be established
  from public web sources.
- Failed retrieval, missing provenance, weak topical expertise, and conflict
  with active evidence **SHALL** produce an inspectable review or no-result
  outcome rather than a silently promoted claim.

**Scenario:** a specialist graphics publication directly quotes a GPU
specification and identifies the product. It can help assess the claim, but a
fast-changing specification still needs dated vendor or primary evidence to
auto-promote. The same publication's unsupported list of rulers during China's
Warring States period does not gain equivalent historical authority.

**Scenario:** two sites repeat one press release. Scholar records both URLs but
counts them as one underlying source when applying a corroboration rule.

**Scenario:** Wikipedia's short introduction does not answer a question.
Scholar records `wikipedia_insufficient`, asks the configured open-web provider
for a small set of URLs, and fetches eligible source pages with curl. Search
result snippets are discovery hints, not evidence for promotion.

**Scenario:** Wikipedia directly answers a question. A bounded exploratory
elapsed-time check still tries broader discovery, recording `exploration_elapsed`
as the reason. A second source can corroborate or challenge the first; its mere
appearance in search results does not raise confidence.

**Scenario:** a result site disallows the Scholar user agent in `robots.txt`,
or its manifest contains a restriction Scholar cannot safely interpret.
Scholar skips the host, records which rule or ambiguity caused the skip, and
continues with another result. A `429` response pauses requests to that host
for its `Retry-After` period or a conservative backoff.

**Scenario:** Scholar records the publication date of a current vendor
specification so Lyra can qualify time-sensitive evidence appropriately. A
single source, missing source date, unresolved provenance warning, or active
map conflict cannot by itself satisfy the unattended promotion gate.

### R3. Prompt maintenance without duplicate work

- A committed observation, research job, review decision, or changed claim
  **SHALL** be able to wake the appropriate maintenance role promptly.
- Repeated notifications **SHALL** coalesce; one role's active pass **SHALL
  NOT** overlap with another pass for the same work.
- On startup and periodically, the scheduler **SHALL** inspect SQLite for
  pending and due work. Lost notifications and expired leases **SHALL** remain
  recoverable.
- Role execution **SHALL** retain per-pass budgets, job leases, error history,
  and the existing ability to continue with later roles after one role fails.

**Scenario:** Scout commits a Scholar job. Scholar is woken without waiting for
the next 20-second idle tick. A simultaneous timer tick does not start a
duplicate Scholar job.

**Scenario:** a process exits after writing a job but before notifying the
worker. The next startup scan or periodic recovery tick finds the durable job.

## Design

### Data flow and trust boundary

```text
map gap or model probe
        ↓
Scout lead (origin, question, reason, retry state)
        ↓
durable Scholar job → wake hint
        ↓
search results → fetched passages → source assessments
        ↓
proposed claim + exact evidence + policy decision
        ↓
promote, challenge, hold for review, or no result
```

Model probes should ask for *questions worth checking*, not for facts to
remember. Scout needs a bounded seed mechanism: existing public map subjects,
unresolved questions, user-requested public topics, and a small allowance for
model-suggested adjacent topics. To support a topic present only in weights,
the model must first suggest or be given a seed such as “Byzantine Empire”; no
algorithm can enumerate every concept in a safetensors file. Keep the existing
spacing, daily research cap, cooldowns, and private-topic filter, and add a
separate cap for weight-derived leads.

### Research and source policy

Use a `SearchProvider` trait so discovery can be swapped without changing
page retrieval or evidence policy. Bound result counts, timeouts, and fetch
sizes. Retain Wikipedia as one provider. Use Tavily as the first production
open-web provider. Its adapter returns structured candidate URLs and snippets; Scholar
fetches and inspects permitted pages with curl rather than treating search
snippets or provider-generated summaries as claim evidence. Broader discovery
runs when Wikipedia is insufficient and, after twice each provider's minimum
interval, for useful Wikipedia results. Record the trigger, query, provider, results, and any access
failure. Failure of one provider must not erase earlier results.
Candidate implementations are:

| Provider | Planning decision |
| --- | --- |
| Tavily | Structured open-web discovery through its [documented search API](https://help.tavily.com/articles/4840311948-tavily-search-api). |
| SearXNG | Self-hosted fallback, or default if no Tavily key, using its [JSON search API](https://docs.searxng.org/dev/search_api.html); configure an instance that enables JSON output. |
| SerpAPI | Google-backed discovery through its [Google Search API](https://serpapi.com/search-api), paced to one request per 30 minutes by default. Results remain leads until source pages pass site-policy checks and are fetched. |
| DuckDuckGo HTML | Disabled as a direct fallback. Its [terms](https://duckduckgo.com/terms) and [acceptable-use policy](https://duckduckgo.com/acceptable-use) do not give this project a supported automated general-results API. |
| Google Custom Search JSON API | Legacy option only if an existing project key and endpoint are available. [Google says it is closed to new customers and ends for existing customers on 1 January 2027](https://developers.google.com/custom-search/v1/overview). |

Do not substitute unattended curl requests to Google search-result HTML; see
[Google's automated-access policy](https://developers.google.com/search/docs/essentials/spam-policies).
Keep API keys out of SQLite, logs, and URLs shown in maintenance history.

The current implementation reads `TAVILY_API_KEY`, `SERP_API_KEY`, and
`SEARXNG_URL` at startup, with matching Tavily and SearXNG settings available
in local `config.toml` (see `config.example.toml`). Invalid configuration is
reported without exposing secret values. SearXNG accepts only local endpoints,
normalizes an origin or `/search` URL, and requests JSON results. If no keyed
provider is available or those providers return no leads, configured SearXNG
can supply discovery leads; absent SearXNG, Wikipedia remains available.

Persist a Tavily search-call and credit ledger and reserve capacity before
dispatch: at most 25 calls per rolling 24 hours and 750 credits per provider
billing month, including failed calls and advanced retries. Use
`search_depth = "basic"` for the first call. Only if it returns
zero usable candidates may Scholar retry once with `advanced`, subject to the
same cap. Tavily currently lists [1,000 free API credits monthly, one credit
for basic search, and two for advanced](https://docs.tavily.com/documentation/api-credits).
The separate credit cap preserves a 250-credit buffer on the current free
plan, including in 31-day months and when advanced retries cost two credits.
If the cap or upstream quota is reached, record the reason and continue with
other available discovery sources, including configured local SearXNG, then
Wikipedia.

For each result host, fetch and cache `robots.txt` first and apply its rules
to the declared `CommunityLabScholar` user agent, following the
[Robots Exclusion Protocol](https://www.rfc-editor.org/rfc/rfc9309.html).
An applicable disallow skips the URL immediately. Next, inspect `/llms.txt`
if permitted; prefer relevant listed Markdown endpoints when they also pass
the host policy. The [llms.txt proposal](https://llmstxt.org/) describes a
content map, not an access grant. Finally, inspect `/agent-manifest.txt` if
permitted. Treat it as an optional capability manifest or skip signal: honor
an explicit understood restriction and record a documented REST/MCP endpoint
as a possible read-only source. Do not automatically call arbitrary endpoints
or MCP tools. Only a separately validated, read-only adapter may use an
endpoint for retrieval. Because website `agent-manifest.txt` formats are
evolving, parse the following **Community Lab profile** only. It is not a
claim of conformance with [Agent Format](https://agentformat.org/docs/introduction/),
which defines agent `.agf.yaml` files rather than website access policy:

```yaml
version: "1.0"
governance_policies:
  disallow_agents: ["*"]
  rate_limit_req_per_min: 6
  require_auth: true
endpoints:
  llms_txt: "/llms.txt"
  mcp_server: "https://api.example.com"
```

Accept JSON or a bounded YAML subset with these exact keys and expected
types. A disallow matching `*` or `CommunityLabScholar` skips the host as
`manifest_disallowed`. A declared rate of `r` requests/minute sets host
spacing to `max(10 seconds, 60/r seconds)`, so it can tighten the default
but cannot loosen it. A declared `llms_txt` path is a candidate same-origin
Markdown index after robots and manifest checks; it never overrides access
restrictions. `require_auth: true` means skip gated endpoints without
configured access. Record `mcp_server` as metadata only. Unknown keys,
invalid types, unparseable syntax, or natural-language restrictions yield
`policy_ambiguous` and skip the host. An absent optional file does not
override `robots.txt`.

Use a shared request budget across Scholar processes: at most two in-flight
HTTP requests globally, one per host, and at least ten seconds between any
requests to that host. Limit external page fetches to 100 per rolling 24 hours. Count
`robots.txt`, `llms.txt`, manifests, redirects, and content requests for
concurrency and host spacing; count content page fetches toward the daily
page cap. Persist atomic request reservations, completion state, and retry
deadlines in SQLite `host_request_log` before dispatch, so concurrent Scholar
processes respect the caps and restart does not cause a burst. Expire
abandoned reservations conservatively rather than freeing them at startup.
Cache host policy; recheck policy after a cross-host redirect. Honor
`Retry-After`, use bounded exponential backoff, and stop after a small retry
count. Limit response bytes and redirects, avoid non-public network
destinations, and make every skip or delay visible in job history. Site
guidance can tighten these defaults.

Store source assessments **per source–claim pair**. A practical initial
assessment is a set of reasons and states rather than a single universal
reliability number:

| Dimension | Example question |
| --- | --- |
| Topical expertise | Is this publisher qualified for this kind of claim? |
| Direct support | Does the cited passage actually say the proposed fact? |
| Provenance | Is the author, publisher, or underlying document identifiable? |
| Currency | Is the evidence current enough for a changing fact? |
| Independence | Does it add evidence beyond sources already recorded? |

Automatic promotion requires two directly supporting findings from distinct
HTTPS hosts, each at or above `SCHOLAR_AUTO_PROMOTION_CONFIDENCE` (default
`0.90`). The finding must directly answer the research question, the pair
must have no unresolved provenance warning, and no active map conflict may
remain. Curmudgeon can resolve a flagged pair as possibly distinct reporting;
unclear or likely shared origins remain staged. A confidence score describes
the extractor's mapping confidence and is not a probability that a claim is
true. Different hosts are an operational independence signal, not proof of
different ownership or editorial independence, so known citations and copied
passages override it. Personal user facts retain the no-public-web-research
rule.

### Scheduler and Tokio decision

First add explicit wake notifications after durable writes. The current
terminal worker already uses `std::sync::mpsc`; the browser subprocess
scheduler can receive a coalesced wake signal from its request path. Both keep
the periodic SQLite scan. This meets the promptness requirement without a
runtime migration.

Start with the existing synchronous architecture and a small bounded pool of
blocking curl/HTTP fetch workers or scoped threads. Do not migrate the whole
application to Tokio for this change. Measure research latency and scheduler
complexity first. Only if Scholar later needs many concurrent I/O streams,
consider Tokio around the network and scheduler boundary:
a channel plus timer can wait for a wake or recovery tick without catch-up
bursts. Keep Candle inference on a dedicated model queue or bounded blocking
threads ([Tokio blocking guidance](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html));
keep synchronous SQLite calls off async worker threads. Limit
concurrent fetches and serialize work that uses the shared model. Tokio must
not cause two model passes to run at once or make browser replies wait behind
unbounded research.

### Migration and compatibility

Add SQLite migrations for model-probe leads, search attempts, retrieved source
documents, source–claim assessments, `host_request_log`, a Tavily query ledger, and underlying-source
identity where it can be established. Preserve existing Scholar jobs, findings,
evidence links, and review history. Old Wikipedia findings should remain readable with their
current source-quality state; absence of new assessment fields means
`unassessed`, not retrospectively verified. Keep current CLI commands working
and extend `scout`, `scholar show`, `maintenance`, and `/status` to reveal lead
origin, provider attempts, and assessment reasons.

## Implementation tasks

Recommended execution order: A → C → B → D → E. Scholar's evidence pipeline
should be ready before model-only Scout leads increase research volume.

### A. Durable leads and provenance

- [x] Add schema migration and types for model-probe leads and per-attempt
      search provenance, including discovery trigger and policy checks,
      preserving existing jobs and evidence.
- [x] Add source–claim assessment records with explicit reasons, timestamps,
      and an `unassessed` state for older records.
- [x] Record Scholar question relevance (`direct`, `related`, `tangential`)
      separately from source reliability and extraction confidence; lower
      retrieval priority for adjacent findings without discarding them.
      Use 1.00/0.75/0.40 weights and cap model relevance with lexical question
      coverage so an overconfident label cannot make a tangential claim direct.
- [x] Add `host_request_log` with host, request kind, reserved/request time,
      response status, and retry deadline; preserve daily page counts and host
      spacing through process restarts.
- [x] Persist Tavily search-call and credit reservations and
      `provider_unavailable` or quota outcomes; keep credentials out of SQLite
      and logs.
- [x] Expose leads, search attempts, and assessments through CLI/status views.

### B. Scout weight probes

- [x] Add bounded model prompts that produce research questions from permitted
      seeds and optional model-suggested adjacent topics.
- [x] Filter private, duplicate, already-answered, and unfocused leads before
      enqueueing Scholar work; record rejection reasons.
- [x] Apply separate attempt, time, and daily limits to weight-derived leads.
      Default to three model-only probes per rolling day after map gaps are
      exhausted.

### C. Scholar multi-source research

- [x] Define a `SearchProvider` trait for bounded discovery adapters and add
      an optional local SearXNG fallback for new research jobs when keyed
      providers are unavailable or return no candidate leads. Its URLs pass
      through the existing candidate-fetch policy and evidence pipeline. The
      SerpAPI/Tavily corroboration queue remains limited to those providers.
- [x] Add SerpAPI discovery with a persisted 30-minute default interval,
      configurable candidate cap, and durable candidate records. Search
      snippets remain discovery hints and do not count as evidence.
- [x] Persist each SerpAPI search's returned domains against its query and
      expose the history through `scholar sites [limit]`. Domain appearance is
      discovery provenance only; it does not increase reliability.
- [x] Preserve linked SerpAPI rich-result groups by their returned category,
      with bounded structured fields as query-linked leads.
- [x] Keep source suitability and independence claim-specific, requiring
      verified provenance before a source counts toward corroboration. Do not
      assign global domain trust from search rank, repeated copies, or a single
      claim.
- [x] Add local `[search.tavily].api_key` and `[search.searxng].url`
      configuration, with `TAVILY_API_KEY`, `SERP_API_KEY`, and `SEARXNG_URL`
      environment overrides. Keep credentials out of SQLite and recorded
      errors.
- [x] Enforce 25 Tavily calls per rolling 24 hours and 750 credits per provider
      billing month with `basic` depth by default.
- [x] Add at most one `advanced` retry after a successful basic search returns
      no usable candidates. Count the retry against request, call, and credit
      limits and retain normal per-host request spacing.
- [x] Add `wikipedia_insufficient` and `exploration_elapsed` triggers for
      broader discovery with persisted provider intervals. Configured local
      SearXNG is used as a fallback for new research-job discovery when keyed
      providers are unavailable or return no leads.
- [x] Check up to 12 relevant candidates on distinct hosts, and try at most
      three page fetches per provider/query with bounded HTTPS curl requests
      after checking `robots.txt`, `/llms.txt`, and `/agent-manifest.txt`; prefer
      relevant same-host Markdown links and persist
      policy outcomes plus exact page passages. Fetched passages are evidence
      candidates only; this step does not itself raise source trust or promote
      claims.
- [x] Persist robots-denied URL paths and host-wide access denials for 30 days;
      skip them before DNS/HTTP on later jobs and expose the active list through
      `scholar blocked`.
- [x] Extract one grounded claim from each fetched passage and process it with
      the existing topic, conflict, and review policy. Keep each source's
      passage and assessment separate within the Scholar job.
- [x] Try at most two coherent page sections per fetched source, retain each
      extractor outcome and bounded model output, and use a generic structured
      list fallback when a relevant heading and consecutive bullets directly
      support a source-specific claim.
- [x] Keep Wikipedia retrieval bounded while exploring beyond the lead:
      rank up to three pages, inspect the best page's section headings, fetch
      up to two relevant sections, and persist host spacing and daily fetch
      reservations in SQLite.
- [x] Parse only the Community Lab manifest profile with YAML/JSON type and
      key validation; enforce disallow and tighter host spacing, resolve
      same-origin `llms_txt` paths, and retain the checked manifest response
      without invoking any declared MCP endpoint.
- [x] Add cross-process in-flight caps, policy caching, and persisted
      `429`/`503` backoff. Current fetches run synchronously, persist per-host
      reservations with at least ten seconds between requests, cap page fetches
      at 100 per rolling day, pin public DNS results, reject private addresses,
      do not follow redirects, and bound response size and request time.
- [x] Add explicit per-finding review of source type and claim-specific topic
      suitability, with rationale and the exact evidence passage persisted.
      A reviewer-confirmed publication date is required for recent claims.
- [x] Add explicit and automatic assessment of the underlying publication
      identity for a secondary finding. Matching reviewed IDs count as one
      source; distinct origins can corroborate directly supported findings.
- [x] Compare reliable declared publication and publisher identifiers.
      Recognized DOI/PMID matches flag a shared work, and matching structured
      publisher URLs flag a possible shared publisher for review. Names and
      absence of matching identifiers do not establish independence.
      Exact-passage support, source suitability, reviewed origin IDs, and
      copied-passage warnings remain separate promotion gates.
- [x] Add a bounded Curmudgeon queue for provenance-linked finding pairs. It
      should explain whether recorded canonical, citation, and passage signals
      suggest shared reporting, while leaving independence unverified unless
      source evidence establishes it.
- [x] Require exact excerpt support and subject/object term coverage before
      accepting a model-extracted claim; reject heading-only evidence and do
      not synthesize a generic finding when extraction fails.
- [x] Normalize Wikipedia citations and edit controls while preserving lists;
      handle explicit answer lists as grounded claims and validate each item
      in the exact multiline source passage.
- [x] Apply unattended promotion at the configured confidence threshold,
      after two independent, directly supporting sources, and only when all
      active conflicts are resolved. Keep tentative evidence available to Lyra
      while it waits for promotion.

### D. Event-driven maintenance

- [x] Emit coalesced wake hints after durable observation, assistant-audit,
      and Scholar-job writes in `chat`; `serve` wakes maintenance after each
      completed chat request. Productive maintenance passes trigger up to three
      immediate follow-up passes so newly written review and claim work is
      picked up without waiting for the timer.
- [x] Preserve startup scans, periodic due-work checks, job leases, per-pass
      budgets, and clean shutdown. `serve` accepts requests while its single
      bounded maintenance subprocess is running and coalesces another wake.
- [x] Keep fetches synchronous and bounded by candidate/page limits, request
      timeouts, response caps, and persisted global/per-host slots. No Tokio
      runtime is used; reconsider only if measured workload needs more I/O
      concurrency.

### E. Verification

- [x] Test weight-derived questions without promoting model output as fact.
- [x] Test that map gaps take priority and the three-per-day model-probe cap
      survives restarts and failed probes.
- [x] Test topic-matched versus topic-mismatched sources, direct versus copied
      corroboration, weak evidence, stale pages, and conflicts with map claims
      (`unrelated_search_result_is_skipped`, source-freshness, copied-source,
      direct-support, and active-conflict tests).
- [x] Test missing/malformed provider configuration, durable
      `provider_unavailable` recording, and end-to-end local SearXNG fallback
      dispatch when no discovery candidates exist.
- [x] Test the Tavily rolling call/credit reservation boundary, the
      basic-to-advanced retry condition, local SearXNG URL validation and JSON
      result parsing.
- [x] Test elapsed-time exploration after useful Wikipedia findings and the
      normal interval for weak Wikipedia results.
- [x] Test manifest disallow, malformed/unknown fields, lower declared rates,
      alternate `llms_txt` paths, and auth-gated/MCP endpoint handling; per-host
      spacing and daily counts across restarts; `Retry-After` and redirect-response
      handling; and bounded HTTP requests with local fixtures. MCP declarations are retained
      in the cached manifest response and never invoked.
- [x] Test unattended promotion at the configured confidence threshold,
      after two distinct source origins, and only after active map conflicts
      are resolved. Keep provenance warnings staged until Curmudgeon resolves
      them as possibly distinct reporting.
- [x] Test duplicate wake coalescing and bounded productive-pass follow-up.
- [x] Test durable research-queue recovery after a lost wake and process restart,
      lease expiry, and migration from earlier schemas.
- [x] Run a bounded end-to-end research case and inspect the recorded source,
      passage, assessment, decision, and resulting map claim. A local storage
      integration test follows two automatically assessed secondary findings
      from exact passage support through distinct-host source assessment,
      verifies the claim remains absent after one source, then checks automatic
      acceptance after the second, both retained source passages, and both
      automatic policy decisions. No live search provider is required.

## Acceptance criteria

1. A Byzantine research lead can originate from a model probe, but it cannot
   become a map fact without separately recorded external evidence.
2. When an open-web provider is configured, Scholar uses it after weak
   Wikipedia results when the base interval permits, and after twice that
   interval when Wikipedia supplies a direct finding. Every promoted
   web claim traces to a fetched passage and recorded source assessment.
3. A weak source on an unrelated topic remains staged even if its prose sounds
   plausible; independent corroboration is visible rather than assumed.
4. New durable work wakes maintenance promptly in both long-lived modes;
   repeated or lost wake hints do not lose work or duplicate processing.
5. Existing claims, Scholar findings, review decisions, and model checkpoints
   remain readable after migration.
6. Unattended research honors checked site policies and persisted rate limits:
   at most two global and one per-host concurrent requests, ten seconds between
   host requests, and 100 external page fetches per day. Disallowed or
   ambiguous hosts receive no content page fetch.
7. Promotion requires the configured confidence threshold, independent
   source origins, resolved provenance warnings, and resolved active map
   conflicts.
8. Missing Tavily credentials do not stop Scholar. The absence is recorded as
   `provider_unavailable`, Wikipedia can continue, and no more than 25 Tavily
   calls occur in a rolling 24-hour window or 750 credits in a provider billing
   month. Configured local SearXNG may supply discovery leads; its use remains
   optional.

## Decisions recorded

- Model-only topic proposals run only after map-derived gaps are exhausted,
  with a default maximum of three probes per rolling day.
- Tavily is the first production open-web provider. SearXNG is the self-hosted
  fallback or keyless default. Direct DuckDuckGo HTML discovery is disabled;
  SerpAPI is implemented as a discovery adapter and Google Custom Search is
  legacy-only.
- Tavily keys come from `TAVILY_API_KEY` or `[search.tavily].api_key` in
  `config.toml`; local SearXNG uses `SEARXNG_URL` or
  `[search.searxng].url`. SearXNG is optional and local-only. Tavily has a
  25-call rolling-day and 750-credit monthly cap; basic depth is
  standard and advanced is reserved for one retry after no usable basic
  candidates.
- Exploration runs after twice each provider's base interval. HTTP limits default to two
  global/one per-host in flight, ten-second host spacing, and 100 external
  page fetches per day, persisted in `host_request_log`.
- The site-policy cascade is `robots.txt`, `/llms.txt`, then
  `/agent-manifest.txt`. The initial YAML/JSON parser follows the explicit
  Community Lab profile above. An ambiguous manifest skips the host with
  `policy_ambiguous`; capability links are not automatically executed.
- Topic-class evidence gates determine automatic promotion or the explicit
  review statuses described above.
- Begin with bounded synchronous fetches. Consider scoped Tokio use only
  if many concurrent I/O streams justify it; keep model inference on a
  dedicated or blocking queue.

## Open decisions before implementation

- Identify the self-hosted SearXNG instance for deployment and ensure it
  enables JSON responses. Scholar can use Wikipedia until one is configured.
