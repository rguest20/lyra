# Community Lab

One local multi-agent chatbot in Rust, supporting original **BF16 Qwen3 and
Qwen3.5** checkpoints on Apple Metal. A persistent local MLX worker runs Qwen3.5's hybrid attention model;
Rust handles conversations, research, grounding, AI review, logging, and storage.
No hosted inference service is required. The legacy Qwen3 backend uses Candle.

## Set up the model

Weights and the Python environment are ignored by Git. On Apple Silicon:

```sh
python3 -m venv --system-site-packages .venv-qwen35
.venv-qwen35/bin/pip install -r scripts/requirements-mlx.txt
hf download Qwen/Qwen3.5-2B config.json tokenizer.json tokenizer_config.json model.safetensors.index.json model.safetensors-00001-of-00001.safetensors --local-dir models/qwen3.5-2b
```

Set these in local `.env`:

```dotenv
COMMUNITY_MODEL_ID=Qwen/Qwen3.5-2B
COMMUNITY_MODEL_DIR=models/qwen3.5-2b
COMMUNITY_MLX_PYTHON=.venv-qwen35/bin/python
COMMUNITY_ADAPTER=state/adapter-qwen3.5-2b.safetensors
COMMUNITY_STATE=state/agent-qwen3.5-2b.json
COMMUNITY_CONTEXT_TOKENS=8192
COMMUNITY_MODEL_LOCK=state/model-inference.lock
```

Then run `cargo run --release -- chat` or `cargo run --release -- serve`.
User-facing answers enable thinking; structured extraction disables it. The debug
setting controls whether the reasoning trace is displayed. Only text inputs are supported.
The MLX backend requires Apple Silicon Metal; `COMMUNITY_DEVICE=cpu` is available
for the legacy Candle Qwen3 backend. MLX can load BF16 or quantized Qwen3.5
checkpoints. The current configuration remains BF16. Quantized models keep their
output adapter separate; the runtime will not merge an adapter into quantized
weights.

The switch checklist in [docs/qwen35-2b-switch.md](docs/qwen35-2b-switch.md)
records conversation migration and measured results. The knowledge map and
SQLite research database remain shared. Context exhaustion returns an error;
it does not silently discard conversation history.

`model.loaded` logs backend, precision, shard count, context cap, template,
device, and adapter selection. Agents use lazy model handles. Background upkeep
and foreground chat have separate warm model caches and request locks, so an
upkeep generation already in progress cannot block a chat turn. Each lane is
serialized internally; the background lock is derived from `COMMUNITY_MODEL_LOCK`
(default `state/model-inference.lock`) with `.requests` appended, while chat uses
`.chat.requests`. A waiting process emits `model.waiting_for_lease`, and
`model.request_wait` records quiet-window, local-pool, and process-lock waiting.
`agent.inference` records agent/model identity, duration, and visible token usage
where available, without prompt contents. Models remain cached up to
`COMMUNITY_MODEL_CACHE_SIZE` (default 1) per lane. The browser server and its
background curator keep separate long-lived runtimes, so the same checkpoint
may be resident twice. Agents with matching checkpoint, adapter, and device
assignments share a backend within a lane. Restart running processes to pick up
adapter changes.

For per-agent configuration, the 0.6B/2B MacBook profile, measured memory
constraints, and cache limitations, see [Per-Agent Local Models](docs/per-agent-models.md).

The MLX backend supports a fresh output adapter and teacher-forced adapter
training. That adapter can be merged into an unquantized BF16 output head, while
it remains separate when the base is quantized. Adapter files are
specific to this model's hidden and vocabulary sizes. Consolidation and rebuild
preserve original checkpoint tensors and create an untied output head;
they require additional memory and disk. Lineage hashing covers every source
shard. Do not reuse an adapter from a different model.

Fresh MLX adapters are saved automatically when the model first loads. They
start with a zero output delta, so the base model is unchanged. Keep a distinct
adapter path when changing the base precision. `model.loaded`
reports `adapter_state` (`initialized_untrained`, `loaded_untrained`, or
`loaded_trained`), whether the file exists, and its training-step count. Untrained
adapters skip their unused output computation during generation.

Thinking initially receives 256 tokens by default. Set
`COMMUNITY_THINKING_TOKENS` to an integer from 1 to 2048 to change that initial budget.
When exhausted, MLX continues the existing reasoning with up to four times the
initial budget, capped at 2048 tokens and the available context. At that cap it
closes the reasoning phase and generates the answer while preserving the reasoning
in context. Malformed output still gets one recovery attempt with thinking disabled
and a labelled discarded trace. `model.generation` records token
counts, stop reason, and MLX active/peak/cache memory; `model.reply_recovery`
records the retry. The browser forwards reply diagnostics to server stderr on
both success and failure, while error bubbles show the actual error message.

Scholar checks claim meaning in a separate AI entailment pass. Faithful synonyms
and paraphrases can pass without matching the source's words. The reviewer
recovers an original source quotation and returns `entailed`, `contradicted`,
`qualified`, or `insufficient`. Negation, uncertainty, attribution, and dates
must be preserved. Credibility and corroboration requirements still apply.

`scholar.semantic_support` logs the verdict, model verdict, any stance override,
reason, candidate, and original evidence. Saved assessments appear in
Curmudgeon's context and are checked again before promotion. Pending findings
without a semantic assessment receive one during AI review. Invalid assessments
follow automated repair/retry handling. Each candidate adds one model call,
including another check if extraction repair produces a new candidate.

The implementation and probe results are recorded in
[docs/semantic-support.md](docs/semantic-support.md).

Isolated probes use the actual Scholar extraction and Curmudgeon review prompts:

```sh
cargo run --release -- model probe-scholar "What is Mars?" "Mars" "Mars is a planet."
cargo run --release -- model probe-support '{"question":"What shape is the world?","source_title":"World","source_excerpt":"The world is spherical.","subject":"The world","relation":"is","object":"round"}'
cargo run --release -- model probe-review '{"question":"What is Mars?","source_excerpt":"Mars is a planet.","subject":"Mars","relation":"is a","object":"planet"}'
```

Use temporary `COMMUNITY_STATE` and `COMMUNITY_DB` paths for probes when
experimenting with a different model. Probes do not queue searches or promote
claims.


Type `/quit` to leave. The conversation is saved to
the configured `COMMUNITY_STATE` path. You can also use `cargo run --release -- say
"Hello"` for one reply, or `cargo run --release -- status` to see the paths.
Chat loads the model once and is faster over several turns. The full saved
conversation enters each prompt, including older turns. The latest user
message remains the question to answer. If a draft exactly repeats an older
answer to a different question, the agent tries once more with the entire
history presented as reference text. The full transcript remains on disk.

## Use the browser frontend

Run the local web server and open `http://127.0.0.1:8080`:

```sh
cargo run --release -- serve
```

The server accepts connections only on a loopback address. It loads Interaction
at startup and keeps the shared model runtime alive between turns. Background
maintenance runs in a thread using that same runtime and cache. Each browser tab
gets a separate conversation
history under `state/web-sessions/`. Reloading a tab keeps its session; a new
tab or **Start a new conversation** begins a fresh history. Learned knowledge
is shared across sessions. Topic changes do not clear the session history;
Lyra keeps earlier turns available for later clarifications and corrections.
Set `COMMUNITY_WEB_ADDR` to another loopback address if port 8080 is in
use; non-loopback addresses are rejected.

Starting a new browser conversation keeps the previous session file in place.
Archives created by older automatic topic resets also remain intact.
Background inference yields throughout each browser reply and for two idle
minutes afterward; startup provides the same grace period. An inference already
in progress must finish before chat can use the shared backend.

Reply diagnostics include `conversation.context_selected` with the retrieved
claim and finding IDs. Ordinary replies are generated by Lyra; conversational
quality audits run afterward during upkeep and record advisory verdicts in the
proposal ledger. They cannot rewrite delivered text or verify factual claims.
Source-backed answers retain their AI grounding review, and factual
knowledge-promotion gates remain enforced. Retained failures and current
validation limits are in [the progress report](docs/grounding-generalization-progress.md).

## Scholar research queue

Fetched passages now also enter separate **unverified claims** and **follow-up
questions** baskets, independently of answer extraction. Source text survives
failed reviews; background harvesting, Scout scheduling, and Curmudgeon evidence
relationships turn it into reusable research. Wikipedia budgets and persisted
retry times prevent temporary quotas from becoming permanent `no_result` jobs.
Inspect `scholar usage` and `scholar baskets`.
`scholar usage` also explains Scout queue cooldowns and budgets. Valid model
leads wait for admission instead of being rejected by scheduling limits. Empty
source baskets are bootstrapped from saved excerpts on a background Scholar pass;
claim/question extraction then proceeds under the normal maintenance budgets.
See
[Research Baskets And Quotas](docs/research-baskets-and-quotas.md) for controls,
confidence limitations, migration notes, and all inspection commands.

When Lyra's reply leaves a public factual question unanswered, a local model
check can queue a short search phrase for the scholar. The scholar has its own
SQLite queue and researches an English Wikipedia article through the MediaWiki
API. Configured SerpAPI and Tavily searches provide additional leads; Scholar
checks site policies before fetching a small number of pages. It extracts
claims only from exact source passages and records each source and assessment.
`scholar show JOB_ID` displays the selected passages and why each extraction
attempt succeeded or failed, plus page-declared author, publisher, publication
date, canonical URL, and citations when present. These declarations are clues,
not proof of independent reporting. Scholar flags substantial repeated passage
wording across two findings and does not count it as independent corroboration;
matching declared canonical URLs, a page citing the other, or both citing the
same source also trigger a shared-origin review. Short shared facts such as
chicken being an ingredient in chicken soup do not trigger a copy warning.
Curmudgeon reviews provenance-linked pairs and records an advisory explanation;
its assessment never clears the independence gate by itself.
Invalid source-review responses log the precise invalid field and a bounded
model-output excerpt. The AI gets one repair attempt; repeated failures are
scheduled for automatic retry after five minutes, allowing other reviews and
assistant audits to continue. The retry schedule survives a restart.
Headings followed by bullets can yield a source-specific list for the Scholar
AI reviewer when the extractor cannot return a grounded claim. The reviewer
assesses source suitability and direct support; suitable secondary findings
continue through automatic independent-source corroboration. Unsupported,
unsuitable, personal, conflicting, or below-threshold findings are rejected or
kept out of promotion with the rationale recorded.

Before replying, Lyra uses semantic assessment to identify missing public facts.
A subjective question can need a factual prerequisite: “best Walkers flavour”
becomes a lookup for available flavours, rather than a search for an objective
winner. Planning uses user questions and selected source passages, excluding
unsourced assistant drafts. The model supplies a neutral question and alternate query phrases,
which persist with the job. Lyra processes its own conversation lookup before
finishing the answer, then selects relevant evidence and checks the enriched
answer against source passages. Selection requires exact useful passage spans
and a relevance rationale; website controls and footer bundles are excluded,
including from previously saved findings. Partial context does not satisfy
missing facts about the requested use or comparison. If a generated answer
cannot be validated, Lyra reports the limitation instead of dumping passages.
Final review checks factual support and whether the reply addresses the actual
request separately; supported facts about a different use are insufficient.
This increases first-answer latency; retrieval and network work remain bounded
by existing request and candidate limits.
Named entities extracted from the request constrain source selection and
retrieval; another brand cannot satisfy a named-brand request. Reviewed aliases
can satisfy that constraint. Existing evidence is shortlisted with model-generated aliases and synonyms,
then selected semantically. Invalid query expansion falls back to the original
question; invalid semantic selection is retried once and does not establish
support. Unrelated recent findings are excluded from the shortlist. If a lookup
finds no relevant evidence, Lyra reports that gap instead of publishing its
unverified draft. Short acknowledgments use recent dialogue without retrieving
new world facts. The shortlist currently examines at most 500 recent eligible findings.

Curmudgeon can learn explicit alternative entity names from suitable reviewed
sources. A second entailment assessment checks each identity against an exact
source quotation. Reviewed names participate in claim deduplication, source
attachment, conflict checks, corroboration and assistant claim review. Original
names and source evidence remain intact. Similar spelling, common ownership,
and parent/subsidiary relationships do not establish identity. Links require explicit naming statements such as “trading as” or “also known
as”. They are direct, without transitive merging. Multiple known identities
for the same name and context-dependent identities remain unresolved.
Source-linked aliases stop applying when their finding is rejected or its
promoted claim is retracted. SQLite schema 44 retains lookup plans and alias
provenance; existing claims are not rewritten or bulk-merged.

Unanswered public questions such as "What crisp flavours do you know?" trigger
research even when phrased conversationally. The newest queued conversation
question is selected before older user questions, Scout investigations, repair
retries, and corroboration. Maintenance gives that lookup the first slot in its
pass, including when autonomous research is disabled. Work already in progress
finishes its current task; background queues remain available afterwards.

```sh
cargo run --release -- scholar
cargo run --release -- scholar ask "What is photosynthesis?"
cargo run --release -- scholar-once
cargo run --release -- scholar show JOB_ID
cargo run --release -- scholar sites
cargo run --release -- scholar blocked
cargo run --release -- scholar queue
cargo run --release -- scholar source-review FINDING_ID primary suitable - "Publisher is a specialist source for this subject"
cargo run --release -- scholar origin-review FINDING_ID report:original-study-123 "The article identifies this report as its underlying source"
cargo run --release -- scholar review FINDING_ID accept "Verified against the cited page"
cargo run --release -- knowledge
```

`scholar blocked` lists robots-denied paths and hosts that Scholar skips for
30 days. Lyra does not use web results to establish personal facts about the
user. Research questions are sent to configured search providers; conversation
history stays local. Internet access and `curl` are required for the worker.
When local SearXNG is configured, Scholar tries it before quota-based providers
and only falls back to those providers if SearXNG returns no candidates. The
bundled local setup is available with `scripts/start-searxng.sh`; it creates a
local secret and starts the container bound only to `127.0.0.1:8888`, enables
JSON, and caps Google and Brave
connection pools. Lyra also enforces a persisted ten-second minimum between
requests to the SearXNG host. The SearXNG limiter is disabled because this
instance is private and local; it protects the instance from incoming clients,
not the upstream search engines. To configure Lyra, copy
`config.example.toml` to the ignored `config.toml` and set
`[search.searxng].url = "http://localhost:8888/search"`, or set `SEARXNG_URL`.
Scholar can use directly supported findings as tentative evidence before they
reach the semantic map. Lyra should attribute them and qualify the answer
naturally, for example, “I'm not completely sure, but these sources say…”.
Scholar uses SerpAPI after weak Wikipedia evidence when its persisted
30-minute interval has elapsed, and Tavily after its persisted one-hour
interval when those providers are needed. When Wikipedia yields a direct
finding, it searches SerpAPI only after an hour and Tavily only after two hours
since each provider's last search. Set `SCHOLAR_SERP_INTERVAL_SECONDS` or
`SCHOLAR_TAVILY_INTERVAL_SECONDS` to change the base intervals; the
strong-Wikipedia interval remains twice the configured value.
Direct, well-grounded public findings that still need an independent source
enter a durable corroboration queue. At each maintenance pass, an available
SerpAPI or Tavily slot takes the next eligible claim. The worker skips hosts
already cited for that claim and stages only a matching claim from a new
source. `scholar queue` shows pending, active, completed, and exhausted items.
Each provider gets at most three attempts per claim; unavailable providers
leave their work pending. Promotion still uses the existing evidence and
conflict checks.
Automatic promotion requires the configured extraction-confidence threshold,
two directly supporting sources from distinct hosts, and no unresolved source
provenance warning or active map conflict. The default threshold is 0.90; set
`SCHOLAR_AUTO_PROMOTION_CONFIDENCE` to a value from 0 to 1 to tune it. A
Curmudgeon provenance warning must be resolved as possibly distinct reporting
before the sources count. Personal facts about the user are never established
from web evidence.

The source-review, origin-review, and finding-review commands remain available
as explicit operator overrides. Normal maintenance uses AI review and automatic
evidence gates; it does not require a human review step.

## Curmudgeon consistency audit

Lyra's replies are now staged separately as **untrusted assistant claims**.
After a reply, Curmudgeon extracts factual assertions and compares each with
active sourced claims in both the JSON and SQLite maps. A different value for
the same subject and relation can raise a cited challenge. For measurements,
the first check normalizes common length units and challenges gaps of 3× or
more. Without a directly comparable claim, the result is **inconclusive**, not
verified. Curmudgeon never promotes its own claims or invents a correction.

The SQLite policy assigns an unverified assistant claim a usable confidence of
0.50; a severe challenge reduces that to 0.10. These values are experimental
usage weights, not probabilities of truth. The challenge and comparison claim
IDs are retained, and relevant challenges are supplied to Lyra on later turns.
Challenges whose comparison claims have since been retracted are omitted from
later prompts.
The current reply is delivered before its background audit finishes.

```sh
cargo run --release -- curmudgeon
cargo run --release -- curmudgeon audit "A cricket bat is 400m long."
cargo run --release -- curmudgeon-once
```

The `audit` command is for trying deliberately false or unusual statements
without waiting for Lyra to say them. Curmudgeon can only challenge against
facts already in the map. The local model may also miss a claim during
extraction; simple length and height sentences have a deterministic fallback.

## Scout knowledge gaps

Scout runs after Curmudgeon and chooses one public topic for Scholar: first a
challenged or due claim, then a specific unanswered detail in an encyclopedia
claim, then a sparse map claim or unsupported assistant claim. It asks a
neutral question rather than treating Lyra's disputed value as a fact. Each investigation records the
triggering assistant claim or map claim and the resulting Scholar job ID in
SQLite. Scout records why it selected each gap and spaces new jobs by at least
five minutes. A topic with no result can be retried after one day; an
AI-reviewed finding awaiting independent corroboration can be retried after
seven days. Three attempts is the limit
for one question, and Scout asks at most five expansion questions per subject.
Personal facts such as the user's name or address are excluded from autonomous web
research.

After those gaps are exhausted, broad model-derived exploration rotates through
12 subject areas with three named starting subjects per area. The persisted
probe ID advances the rotation after rejected suggestions and restarts too,
rotating the starting subject on each circuit. Scout chooses that concrete
subject and asks the model for a focused factual question instead of asking the
small model to invent both a topic and question from a long rejection list. Recent-topic
history keeps distinct titles, so repeated rejections cannot crowd out older
ones. Shared substantive topic phrases also block close rewordings from being
queued as fresh background leads. This diversity guard does not merge claims or
entities. Diagnostics and probe records include the selected subject-area seed.

```sh
cargo run --release -- scout
cargo run --release -- scout-once
cargo run --release -- scholar
```

`scout-once` loads the local model to formulate a focused question from known
encyclopedia claims. It does not use the network. Scholar performs the later
lookup. When a precise fact cannot be extracted but Scholar can locate a
different, relevant sentence in the source, it keeps that sentence as a sourced
encyclopedia detail. A sparse map is a lead for
investigation, not evidence that its existing claim is false. Scholar's normal
source checks and review rules still decide whether a finding enters the map.
For the current architecture and maintenance ideas, see
[Agentic Semantic Map Maintenance](docs/agentic-map-maintenance.md).

## Autonomous maintenance

Keep `chat` or `serve` running to let Curator, Curmudgeon, Scout, and Scholar
check for work without another prompt. The default idle interval is 20 seconds.
Both modes write the last 200 maintenance passes, including each role's work,
idle, AI-review-pending, skipped, or error outcome, to SQLite. Inspect them with:

```sh
cargo run --release -- maintenance
cargo run --release -- maintenance runs 10
cargo run --release -- scout
cargo run --release -- scholar
```

The browser header shows a short upkeep summary. `GET /status` (also
`/api/status`) returns the latest pass, queue depths and oldest ages, Scout's
scan cursor, and its priority order as JSON. A pass with an error is recorded
as `partial_error`; the other roles still run when their budgets allow it.
The terminal logs role errors to stderr, and `serve` reports background worker
exits there. Ctrl-C or a termination signal lets `serve` wait for an active
maintenance subprocess.

Curmudgeon rejection feeds back into research automatically. A fixable extraction
returns to Scholar with the original question, exact source passage, and review
guidance. An unsuitable source returns to Scout to queue a search for better
evidence. Sources are excluded only after an explicit irrelevant or unreliable
verdict, and only for that page within the question's retry loop. Extraction
failures never establish source unsuitability; useful passages return to Scholar
with exact suggested evidence. A request for a new source is honored even when
the source verdict is missing or unknown, without excluding the old page. Other
missing source verdicts default to repair. A finding gets **three
attempts total** (the original plus two retries), shared across both routes.
After the third unsuccessful attempt the loop stops and records its reason.
Personal or sensitive findings stop without retries. The budget, guidance,
and rejected findings survive restarts in SQLite. Corrected findings still
receive AI review before acceptance.

Structured stderr events `scholar.research_feedback`,
`scholar.guided_repair_started`, `scout.research_feedback`, and
`scholar.feedback_attempt_finished` include the original finding ID, attempt
number, and guidance or outcome. `scholar.research_feedback` records
`status: exhausted` when the retry budget is spent. Existing `internet.*`
events show the search queries and pages reviewed during retries.

Curator sends flagged extractions to an independent AI reviewer. Curmudgeon
also resolves any candidate that remains flagged after deterministic conflict
checks. Scholar findings receive an AI source and passage assessment; suitable
secondary findings continue through independent-source corroboration. Review
decisions and rationales are emitted as structured JSON lines to stderr and
retained in SQLite. Scholar extraction events include the research question,
the selected passage excerpt shown to the model, the model's candidate, and the
specific grounding check that rejected it. Internet searches emit `internet.search` events with the
provider, trigger, query, outcome, and result count, plus `internet.search_result`
and `internet.page_review` events showing discovered URLs and whether pages were
allowed, skipped, or fetched. URL credentials, query parameters, and fragments
are omitted. Capture the terminal log with:

```sh
cargo run --release -- serve 2>>community-lab.log
```

The database at `state/learning.db` retains structured maintenance history and
`maintenance runs 10`.

Configuration:

| Environment variable | Default | Effect |
| --- | --- | --- |
| `COMMUNITY_MAINTENANCE_INTERVAL_SECS` | `20` | Idle interval, 1–3600 seconds. |
| `COMMUNITY_MAINTENANCE_MAX_MS` | `120000` | Time budget checked between roles, 1000–600000 ms. It does not interrupt a model call already running. |
| `COMMUNITY_MAINTENANCE_INFERENCE_BUDGET` | `8` | Maximum inference jobs admitted per pass, 1–16. |
| `COMMUNITY_AUTONOMOUS_RESEARCH` | on | Set to `off` or `0` to skip automatic Scout and Scholar work while local curation and audits continue. |
| `COMMUNITY_MAX_RESEARCH_PER_DAY` | `20` | Maximum Scout investigations in a rolling day, 1–200; `0` disables this cap. |
| `COMMUNITY_SCOUT_MODEL_PROBES_PER_DAY` | `3` | Maximum broad, model-suggested Scout leads per rolling day, 1–200; `0` disables this cap. |
| `COMMUNITY_SCOUT_MIN_INTERVAL_SECONDS` | `300` | Minimum spacing between Scout investigations and broad model probes, 0–86400 seconds; `0` disables spacing. Per-topic retry limits still apply. |
| `COMMUNITY_CONFIG` | `config.toml` | Optional local TOML file for search provider settings. |
| `TAVILY_API_KEY` | — | Tavily API key; overrides `[search.tavily].api_key`. |
| `SERP_API_KEY` | — | SerpAPI key. |
| `SCHOLAR_WIKIPEDIA_INTERVAL_SECONDS` | `10` | Minimum Wikipedia request spacing, never below ten seconds. |
| `SCHOLAR_WIKIPEDIA_REQUESTS_PER_HOUR` | `60` | Rolling all-call Wikipedia API budget. |
| `SCHOLAR_WIKIPEDIA_REQUESTS_PER_DAY` | `200` | Rolling all-call Wikipedia API budget; external pages have no shared daily fetch cap. |
| `SEARXNG_URL` | — | Local SearXNG search URL; overrides `[search.searxng].url`. |
| `OPENAI_API_KEY` | unset | When non-empty and OpenAI reviews are explicitly enabled, Curmudgeon uses OpenAI for claim, source, semantic-support, provenance, and assistant-audit reviews instead of the local model. |
| `COMMUNITY_CURMUDGEON_OPENAI` | `off` | Only `on`, `true`, or `1` enables OpenAI reviews. Otherwise the local model is used even with an API key configured. |
| `COMMUNITY_CURMUDGEON_OPENAI_MODEL` | `gpt-4.1-mini` | OpenAI chat-completions model for Curmudgeon; must support JSON-object responses. |

OpenAI reviews are disabled by default. To enable them, set
`COMMUNITY_CURMUDGEON_OPENAI=on` and OpenAI credentials directly in the
Git-ignored `.env`, then restart Lyra.
Chat and the other agents still use the local model. Curmudgeon sends review
evidence to OpenAI, including source excerpts, user observations, assistant
replies, and relevant claim metadata. Requests incur API costs, use a 45-second
timeout and a 1,536-token response cap, and request `store=false`; this is not a
promise of zero provider retention. API errors, refusals, and incomplete
responses leave reviews pending rather than rejecting the evidence or silently
falling back to the smaller model. Set `COMMUNITY_CURMUDGEON_OPENAI=off` or
leave it unset to keep all Curmudgeon inference local without removing your key.
Remove or blank `OPENAI_API_KEY` for the same effect. Restart running processes
after changes.

The review rubric accepts supported paraphrases and does not demand perfect
wording. Materially flawed candidates still receive specific repair guidance;
confidence, exact quoted evidence, privacy, freshness, conflict resolution,
and independent-corroboration gates are unchanged. A stronger reviewer cannot
override those storage policies or automatically reopen old rejected findings.

Copy `config.example.toml` to `config.toml` to configure Tavily or a local
SearXNG instance. `config.toml` is ignored by Git. SearXNG endpoints must be
local (`localhost`, loopback IP, or `.local` hostname) and return JSON results.
If no paid discovery provider can run, Scholar can use configured SearXNG;
Wikipedia remains available in every configuration. Search snippets remain
leads until source pages pass policy checks and are fetched.

Scholar uses its source and review rules before a claim enters the map;
personal facts are excluded. Model checkpoint training and approval remain
manual. See [Autonomous Knowledge Maintenance](docs/autonomous-knowledge-maintenance.md)
for the implementation and verification details.

## Update the model

The BF16 checkpoint is an ordinary weight checkpoint. This first training path
keeps its pretrained weights fixed and updates a **rank-8 output adapter** with
gradient descent. The adapter changes the model's token logits and is saved to
`state/adapter.safetensors`; subsequent chat runs reload it. This is a real
model weight update, separate from editing the knowledge map. It is deliberately
small enough to experiment with on the Mac. Full base-weight training is not
implemented yet.

```sh
cargo run --release -- train "What is the lab's greeting?" "Hello from the lab."
cargo run --release -- say "What is the lab's greeting?"
```

`train` accepts one reviewed prompt and answer, up to 128 answer tokens,
and performs one teacher-forced pass with AdamW. A single pass may have little
visible effect; repeat with suitable examples to develop behavior. Training is
explicit, not triggered by casual chat. Keep a backup of the adapter before
experiments you may want to undo. Qwen3-0.6B has a 40,960-token context. The
prompt can use up to 40,704 tokens, leaving at least 256 tokens for an answer.
The normal reply cap is 4,096 tokens; it grows by the token count of added
observation context when room remains, and is always bounded by the model's
context window. If the prompt itself exceeds the supported limit, the app
reports an error instead of dropping conversation history. Long prompts take
more memory and time on the Mac.

### Consolidate a reviewed SQLite claim into Qwen3-0.6B

The direct `train` command writes a separate adapter. For a durable model
checkpoint, approve an active SQLite semantic claim and consolidate it:

```sh
cargo run --release -- knowledge
cargo run --release -- model approve 12 "Checked the cited evidence"
cargo run --release -- model consolidate 12
cargo run --release -- model history
cargo run --release -- model rebuild 1
COMMUNITY_MODEL_DIR=models/qwen3-0.6b-claim-12-... cargo run --release -- model probe "What is known about the subject?"
```

Consolidation trains the small output adapter on the approved claim, checks
that a short probe recalls its value, then folds the learned output weights
into `lm_head.weight` in a **new** `model.safetensors`. It writes matching
`config.json` and `tokenizer.json` alongside the new checkpoint. The original
model directory remains the rollback copy. Set `COMMUNITY_MODEL_DIR` to the
printed new directory to use it; unset the variable to return to the original.
The new directory starts with no separate adapter, avoiding a double
application of the old one. Approval and the source and destination checkpoint
paths are recorded in SQLite. Claims that are retracted cannot be consolidated.
Each consolidation writes a manifest with the SHA-256 of its root checkpoint and
the exact approved claim revisions included. `model rebuild RUN_ID` verifies
the recorded base and rebuilds from the claims that remain active and approved.

The recall check covers the training question. It does not establish broad
generalization or guarantee that later model replies will always use the
stored fact. SQLite remains the auditable source of truth. Each consolidation
adds one reviewed claim; a later rebuild can replay the recorded claim set
from its original base checkpoint.

## Edit the knowledge map

```sh
cargo run --release -- learn "willow" "grows beside" "streams" "field note 1"
cargo run --release -- knowledge
cargo run --release -- revise 1 "willow" "grows beside" "rivers" "field note 2"
cargo run --release -- retract 2 "field note 3"
```

The map lives at `state/knowledge.json`. Each relationship has an ID and
source. Revisions keep the old claim as retracted, and retractions record their
source. The agent reloads the map before each reply and includes up to five
active claims that share words with the question. The matching is lexical, not
semantic vector search. Map claims are user-reported information, not verified
facts.

The first learning-store migration creates `state/learning.db` as a local
SQLite file and imports the current map without deleting or changing the JSON
file:

```sh
cargo run --release -- migrate-knowledge
```

The command can be rerun safely. It reports matching claims already present
and stops if an existing claim ID has different content. The JSON map remains
available for direct `learn`, `revise`, and `retract` commands. Curated claims
are stored in SQLite and included in chat context; `knowledge` displays both
sources. Set `COMMUNITY_DB` to use a different database path.

Chat and `say` save every user input as a pending observation in
`state/learning.db` before reply generation. Relevant unreviewed observations
from the same conversation are included as user-reported context; the latest
input remains the final prompt message. Repeating an input for the same
conversation turn after a failure reuses its observation. The `chat` command
starts one asynchronous curator thread that shares the model cache and extracts
claims with source evidence. A second AI reviewer assesses flagged extractions;
acceptance still requires the 0.95 confidence threshold, exact evidence, valid
dates, and conflict checks. Curmudgeon resolves any remaining candidate
conflicts through a separate AI review. Failed model reviews are logged and
retried after the observation lease expires.
Equivalent claims retain additional observation provenance without creating
duplicates. Inputs sent with `say` remain pending until a later `chat` run.
Older conversation history is not backfilled into the staging table.

Inspect the worker inbox and review candidates with:

```sh
cargo run --release -- inbox
cargo run --release -- inbox show OBSERVATION_ID
cargo run --release -- knowledge
cargo run --release -- review CANDIDATE_ID accept "The user confirmed this correction"
cargo run --release -- review CANDIDATE_ID reject "This was a hypothetical example"
```

Review actions require a reason and preserve it with the candidate. Accepting a
conflict supersedes the previous active claim while retaining its history and
linking the accepted claim to the source observation. Back up
`state/learning.db` and `state/knowledge.json` after a clean shutdown and
before manual database maintenance.

`COMMUNITY_MODEL_ID`, `COMMUNITY_MODEL_DIR`, `COMMUNITY_CONTEXT_TOKENS`, `COMMUNITY_ADAPTER`, `COMMUNITY_STATE`,
`COMMUNITY_KNOWLEDGE`, `COMMUNITY_DB`, and `COMMUNITY_DEVICE=cpu` override
the defaults.
A different base model needs a compatible config/checkpoint/tokenizer and a
separate conversation state and adapter path. The same agent and knowledge-map
logic can be used with a larger compatible model on a PC after a CUDA backend
is added.

The old `model.safetensors`, `tokenizer.json`,
`tokenizer_config.json`, and `vocab.txt` at the repository root belong to
an earlier MiniLM embedding model and are unused by the chatbot.
