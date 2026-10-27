# Per-Agent Local Models

## Architecture

The existing Candle Qwen3 and MLX Qwen3.5 backends remain unchanged. A
`ModelRuntime` gives each agent a lightweight `LocalModel` handle. Handles route
inference through a lazy cache rather than loading a checkpoint per handle.
Foreground chat and background upkeep have separate caches; matching canonical
model directory, adapter path, and device reuse one backend within each lane,
even if configured model labels differ. Different adapters intentionally select
different instances. Model identifiers label local files; they do not download
or select a hosted model.

The existing Curator is the Archivist for configuration purposes. Its recorded
SQLite maintenance role remains `curator`. Maintenance still runs Archivist,
Curmudgeon, Scout, and Scholar in the existing order with the existing budgets.
Semantic lookup and source-grounded entity aliases are described in the README.
An unset assignment inherits the global model and adapter. Invalid assignments
fail explicitly; unavailable checkpoints never fall back to a smaller model.

The background cache defaults to one resident backend. A separate chat cache
keeps its own assigned backend warm, so a background generation already in
progress cannot hold up a browser or terminal chat turn. Each lane is serialized
internally and has its own process request lock: background uses
`COMMUNITY_MODEL_LOCK` with `.requests` appended, and chat uses `.chat.requests`.
This can keep two copies of the same model resident and generating at once; check
memory pressure on smaller machines. Network requests and database work do not
hold either inference mutex. Raw training and checkpoint commands retain their
existing loader; stop agents before manual training or consolidation.

Dropping the last runtime or agent handle drops its cached backends and releases
their leases. Existing MLX cleanup kills and reaps the Python worker. Terminal
chat shares its runtime with background maintenance, retaining models over idle
ticks. Shutdown still waits for an already-running maintenance pass.

The browser server keeps its model runtime for its full lifetime. Browser replies
reuse the warm chat lane while the maintenance thread uses the separate
background lane. The server warms Interaction before accepting requests and
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

## MacBook Profile

The global/default assignment remains the original BF16 2B checkpoint. The
local development profile assigns all five roles to the 8-bit quantized
checkpoint, with a separate adapter path for each role. The 0.6B files remain
available for rollback. Keep the BF16 base checkpoint installed:

```sh
python3 -m venv --system-site-packages .venv-qwen35
.venv-qwen35/bin/pip install -r scripts/requirements-mlx.txt
hf download Qwen/Qwen3.5-2B config.json tokenizer.json tokenizer_config.json model.safetensors.index.json model.safetensors-00001-of-00001.safetensors --local-dir models/qwen3.5-2b
```

Set local environment variables or the Git-ignored `.env`:

```dotenv
COMMUNITY_MODEL_ID=Qwen/Qwen3.5-2B
COMMUNITY_MODEL_DIR=models/qwen3.5-2b
COMMUNITY_ADAPTER=state/adapter-qwen3.5-2b.safetensors
COMMUNITY_MLX_PYTHON=.venv-qwen35/bin/python
COMMUNITY_CONTEXT_TOKENS=2048
COMMUNITY_CURMUDGEON_OPENAI=off
COMMUNITY_MODEL_CACHE_SIZE=1
```

The local `.env` overrides every role:

```dotenv
COMMUNITY_SCOUT_MODEL_ID=Qwen/Qwen3.5-2B
COMMUNITY_SCOUT_MODEL_DIR=models/qwen3.5-2b-8bit
COMMUNITY_SCOUT_ADAPTER=state/adapter-qwen3.5-2b-8bit-scout.safetensors
COMMUNITY_SCHOLAR_MODEL_ID=Qwen/Qwen3.5-2B
COMMUNITY_SCHOLAR_MODEL_DIR=models/qwen3.5-2b-8bit
COMMUNITY_SCHOLAR_ADAPTER=state/adapter-qwen3.5-2b-8bit-scholar.safetensors
COMMUNITY_CURMUDGEON_MODEL_ID=Qwen/Qwen3.5-2B
COMMUNITY_CURMUDGEON_MODEL_DIR=models/qwen3.5-2b-8bit
COMMUNITY_CURMUDGEON_ADAPTER=state/adapter-qwen3.5-2b-8bit-curmudgeon.safetensors
COMMUNITY_ARCHIVIST_MODEL_ID=Qwen/Qwen3.5-2B
COMMUNITY_ARCHIVIST_MODEL_DIR=models/qwen3.5-2b-8bit
COMMUNITY_ARCHIVIST_ADAPTER=state/adapter-qwen3.5-2b-8bit-archivist.safetensors
COMMUNITY_INTERACTION_MODEL_ID=Qwen/Qwen3.5-2B
COMMUNITY_INTERACTION_MODEL_DIR=models/qwen3.5-2b-8bit
COMMUNITY_INTERACTION_ADAPTER=state/adapter-qwen3.5-2b-8bit-interaction.safetensors
```

The 4-bit checkpoint was removed locally at the user's request; its benchmark
outputs remain as historical records. The INT8 assignments are inference
choices only and do not enable training or adapter merging.

Restart Lyra after configuration changes. Roles without overrides inherit the
global 2B assignment. Changing Interaction's model preserves its saved
conversation history and updates the model label when the next turn is saved.

The development Mac has 16 GiB of physical RAM. The upstream BF16 weights are
1,503,300,328 bytes for 0.6B and 4,548,221,488 bytes for 2B. Separate chat and
upkeep lanes can each hold a copy, so two 2B BF16 backends require about 9.1 GB
for weights alone, before KV caches, activations, adapters, Python/Metal
allocation overhead, and other applications. Two simultaneous 4-bit MLX workers
each reported about 1.07 GB active memory in a local probe. Monitor Activity
Monitor and `memory_pressure` during normal use, especially while maintenance
runs. Lower context limits reduce cache and attention memory if pressure or
swap grows.
The original BF16 2B checkpoint, experimental 8-bit checkpoint, and isolated
MLX environment are installed locally. The ignored `.env` assigns every role
to INT8. The 4-bit checkpoint was removed; only historical comparison data
remains.
Local review smoke probes accepted a supported crisp-flavours claim and rejected
an off-topic US renewable-electricity claim for a food-production question.
These synthetic fixtures check review decisions and routing, not general model
accuracy or source classification. Lower context limits reduce cache and
attention memory if pressure or swap grows.

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

Scholar benefits from 2B for section selection, faithful extraction and semantic
entailment. In live crisp research, 0.6B rejected an explicitly named company
as unrelated and returned generic definitions. Curmudgeon also supplies the
semantic lookup planner, evidence selector and grounded answer generation.
Scholar and Curmudgeon share weights when their directory and adapter match.
