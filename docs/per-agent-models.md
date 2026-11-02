# Per-Agent Local Models

## Architecture

Unquantized Qwen3 uses Candle; quantized Qwen3 and Qwen3.5 use MLX. A
`ModelRuntime` gives each agent a lightweight `LocalModel` handle. Handles route
inference through a lazy cache rather than loading a checkpoint per handle.
Foreground chat and background upkeep can share a pool or use separate caches.
Matching canonical model directory, adapter path, and device reuse one backend
within a pool,
even if configured model labels differ. Different adapters intentionally select
different instances. Model identifiers label local files; they do not download
or select a hosted model.

The existing Curator is the Archivist for configuration purposes. Its recorded
SQLite maintenance role remains `curator`. Maintenance still runs Archivist,
Curmudgeon, Scout, and Scholar in the existing order with the existing budgets.
Semantic lookup and source-grounded entity aliases are described in the README.
An unset assignment inherits the global model and adapter. Invalid assignments
fail explicitly; unavailable checkpoints never fall back to a smaller model.

The cache defaults to one resident backend per pool. With
`COMMUNITY_MODEL_SHARED_LANES=on`, foreground and background handles share the
same pool and process request lock. This avoids duplicate resident weights in
one Lyra process when every assignment and adapter matches. Generation is
serialized: a foreground call waits for active background inference to finish.
Background calls still yield while a foreground turn is active and for two
minutes afterward; the quiet-window check is repeated after acquiring the pool.

Shared lanes are opt-in. With them disabled, chat and upkeep use separate caches
and locks (`.chat.requests` and `.requests` derived from `COMMUNITY_MODEL_LOCK`).
This permits concurrent foreground/background inference and can retain two
copies of a checkpoint. Network requests and database work do not hold either
inference mutex. Separate Lyra processes still have separate model workers.
Raw training and checkpoint commands retain their existing loader; stop agents
before manual training or consolidation.

Dropping the last runtime or agent handle drops its cached backends and releases
their leases. Existing MLX cleanup kills and reaps the Python worker. Terminal
chat shares its runtime with background maintenance, retaining models over idle
ticks. Shutdown still waits for an already-running maintenance pass.

The browser server keeps its model runtime for its full lifetime. Browser replies
reuse the warm chat pool; maintenance uses that same pool with shared lanes
enabled, or its own pool otherwise. The server warms Interaction before accepting requests and
waits for an active maintenance pass when shutting down. Standalone `say` and
`curate-once` commands remain short-lived processes.

## Configuration

Keep using `COMMUNITY_MODEL_ID`, `COMMUNITY_MODEL_DIR`, `COMMUNITY_ADAPTER`,
`COMMUNITY_DEVICE`, and `COMMUNITY_CONTEXT_TOKENS` for defaults. For each of
`SCOUT`, `SCHOLAR`, `CURMUDGEON`, `ARCHIVIST`, and `INTERACTION`, optional
overrides are:

```dotenv
COMMUNITY_<AGENT>_MODEL_DIR=models/local-checkpoint
COMMUNITY_<AGENT>_MODEL_ID=label-for-that-checkpoint
COMMUNITY_<AGENT>_ADAPTER=state/model-specific-adapter.safetensors
```

A different model ID requires its model directory. A directory override without
an ID uses the path as its label. A different directory defaults its adapter to
`adapter.safetensors` inside that directory; set an explicit adapter to keep it
in `state`. Agents using the same weights and adapter share a backend. Do not
reuse an adapter belonging to a different architecture or model size.

`COMMUNITY_MODEL_CACHE_SIZE` accepts 1-8, default 1. It is an instance limit,
not a RAM guarantee. Context and device settings are currently global. Existing
Metal acceleration is preserved; setting `COMMUNITY_DEVICE=cpu` is supported
only by Candle. The MLX backend requires Apple Silicon. This routing can be
configured differently on a desktop, but it does not add ROCm or CUDA support.

## MacBook Profile — 28 October 2026

The 16 GiB M5 development profile now assigns every local role to
`mlx-community/Qwen3-14B-4bit`, revision
`a4d9b2df59d2c150bef02fcbe0d91046b7ca33a4`. The checkpoint is installed under
`models/qwen3-14b-4bit`. All roles inherit the global assignment and the fresh
`state/adapter-qwen3-14b-4bit-shared.safetensors` output adapter. Old per-role
2B overrides were removed from the ignored `.env`. The role adapters were
untrained; they were not migrated or combined. Sharing this new adapter does
not enable training, create independent reviewers, or change the roles' prompts.

The profile enables shared lanes, sets cache size 1 and caps context at 8,192
tokens. Exact operator settings are in README.md. Conversation, database and
knowledge paths were preserved. A private pre-switch `.env` copy is retained
under `state/config-backups/`; the 2B INT8 checkpoint and its adapters remain
available for rollback. Restart existing processes after configuration changes.

The first live load reported 8,312,829,968 bytes of active MLX allocations;
a short JSON generation peaked at 8,440,576,332 bytes. These are MLX allocation
figures, not total application or system RAM. Longer prompts, KV caches, multiple
items in a batch, Docker and other apps require additional memory. A first
coding replay completed planning and acceptance generation, then its worker
exited during acceptance audit before the user-requested shutdown for Teams.
That interrupted run remains unresolved. It does not establish a reliable 14B
coding workflow or stable memory use under video-call workloads.

A fresh replay completed after three rounds (about 11.5 minutes), with one
model load, no worker restart and about 9.67 GB peak MLX allocation. Both final
eight-case suites and additional Codex-authored finite-domain checks passed.
An isolated ordinary conversation with reasoning enabled also used one worker
across Curmudgeon and Interaction and delivered a reply. Acceptance scope errors
and failed schema repairs remain in the coding record; this is one successful
pilot, not general reviewer or grounding acceptance. Detailed evidence and
limitations are in docs/grounding-generalization-progress.md.

Earlier 0.6B/2B profiles and BF16/INT8/4-bit comparisons are historical evidence
in docs/qlora-speed-experiment.md and the archived quantized-runtime notes.
The 14B 4-bit choice is a new inference configuration; it does not establish
that 4-bit quantization preserves this model's reviewer accuracy.

## Diagnostics And Validation

`agent.model_loaded` records the requesting agent, model ID, and local directory
only when a new backend is loaded. `agent.inference` records agent, model ID,
status, total elapsed milliseconds (including waiting/loading), inference
milliseconds, and visible output tokens where available. No prompts or response
contents are added to these events. Candle reports re-tokenized visible output;
it excludes hidden thinking and is not a billing count. That field is null for
MLX; existing MLX generation diagnostics retain its native token counts.

Thinking is selected by the operation, independently of the assigned role:
user-facing answers enable it, while JSON extraction and review disable it.
Hiding the browser's debug trace does not disable answer reasoning. MLX extends
an exhausted thinking budget before closing that phase, retaining the generated
reasoning as context for the visible answer. The extension and final phase emit
`model.thinking_budget_extended` and `model.thinking_completed_at_budget`.

`model probe-scholar`, `model probe-baskets`, and `model probe-support` use Scholar's assignment,
`model probe-review`, `model probe-lookup`, and `model probe-aliases` use Curmudgeon's, and `model probe-chat` uses Interaction's.
The plain `model probe`, training, consolidation, and rebuild commands continue
to use the global checkpoint.
`status` lists each agent's resolved model ID without loading the checkpoints.

Mock-backed tests cover default and overridden assignments, invalid settings,
missing checkpoints without fallback, model/adapter cache identity, two-model
reuse, LRU eviction, lazy handles, and shutdown. Existing maintenance, research,
review, storage, and web tests remain the workflow regression checks. No learning
database schema or semantic map format was changed.

Historical small-model research pilots found that 0.6B rejected an explicitly
named company as unrelated and returned generic definitions. Those narrow
comparisons do not establish the new 14B model's grounding accuracy. Curmudgeon
also supplies the semantic lookup planner, evidence selector and grounded
answer generation.
Scholar and Curmudgeon share weights when their directory and adapter match.
