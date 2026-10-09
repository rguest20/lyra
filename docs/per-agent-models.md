# Per-Agent Local Models

## Architecture

The existing Candle Qwen3 and MLX Qwen3.5 backends remain unchanged. A
`ModelRuntime` gives each agent a lightweight `LocalModel` handle. Handles route
inference through a lazy, shared cache rather than loading their own checkpoint.
Matching canonical model directory, adapter path, and device reuse one backend
within a runtime, even if configured model labels differ. Different adapters
intentionally select different instances. Model identifiers label local files;
they do not download or select a hosted model.

The existing Curator is the Archivist for configuration purposes. Its recorded
SQLite maintenance role remains `curator`. Maintenance still runs Archivist,
Curmudgeon, Scout, and Scholar in the existing order with the existing budgets.
Semantic lookup and source-grounded entity aliases are described in the README.
An unset assignment inherits the global model and adapter. Invalid assignments
fail explicitly; unavailable checkpoints never fall back to a smaller model.

The cache defaults to one resident backend. It evicts the least recently used
backend before loading another at capacity. With capacity two, the shared small
model and Curmudgeon's larger model remain loaded between calls. Loading and
inference are serialized; network requests and database work do not hold the
inference mutex. A request-scoped process lock derived from
`COMMUNITY_MODEL_LOCK` with `.requests` appended serializes routed inference
across Lyra processes. Raw training and checkpoint commands retain their existing
loader; stop agents before manual training or consolidation.

Dropping the last runtime or agent handle drops its cached backends and releases
their leases. Existing MLX cleanup kills and reaps the Python worker. Terminal
chat shares its runtime with background maintenance, retaining models over idle
ticks. Shutdown still waits for an already-running maintenance pass.

The browser still launches separate `say` and maintenance subprocesses. Each
subprocess has its own cache, which is released on exit. Residency and reuse
therefore do not extend across browser turns or separate worker invocations.
Different processes can hold duplicate resident weights, although routed
inference is serialized. A persistent local inference service or in-process web
runtime would be needed to remove that limitation; neither is introduced here.
Prefer terminal `chat` when measuring shared resident memory and feedback loops.

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

The existing Qwen3-0.6B installation supplies Scout, Archivist, and
Interaction. Install the optional original BF16 2B checkpoint and the existing
MLX requirements in new directories; do not replace the small checkpoint:

```sh
python3 -m venv --system-site-packages .venv-qwen35
.venv-qwen35/bin/pip install -r scripts/requirements-mlx.txt
hf download Qwen/Qwen3.5-2B config.json tokenizer.json tokenizer_config.json model.safetensors.index.json model.safetensors-00001-of-00001.safetensors --local-dir models/qwen3.5-2b
```

Set local environment variables or the Git-ignored `.env`:

```dotenv
COMMUNITY_MODEL_ID=Qwen/Qwen3-0.6B
COMMUNITY_MODEL_DIR=models/qwen3-0.6b
COMMUNITY_ADAPTER=state/adapter-qwen3-0.6b.safetensors
COMMUNITY_CURMUDGEON_MODEL_ID=Qwen/Qwen3.5-2B
COMMUNITY_CURMUDGEON_MODEL_DIR=models/qwen3.5-2b
COMMUNITY_CURMUDGEON_ADAPTER=state/adapter-qwen3.5-2b.safetensors
COMMUNITY_MLX_PYTHON=.venv-qwen35/bin/python
COMMUNITY_CONTEXT_TOKENS=2048
COMMUNITY_MODEL_CACHE_SIZE=1
COMMUNITY_CURMUDGEON_OPENAI=off
COMMUNITY_SCHOLAR_MODEL_ID=Qwen/Qwen3.5-2B
COMMUNITY_SCHOLAR_MODEL_DIR=models/qwen3.5-2b
COMMUNITY_SCHOLAR_ADAPTER=state/adapter-qwen3.5-2b.safetensors
```

Restart Lyra after configuration changes. All unspecified agents inherit 0.6B.
When overriding Interaction, also select an appropriate `COMMUNITY_STATE` path
to avoid mixing different models' conversation state.

The development Mac reports 16 GiB of physical RAM and about 32% free at the
time of inspection. The upstream BF16 weights are 1,503,300,328 bytes for 0.6B
and 4,548,221,488 bytes for 2B: about 5.64 GiB combined, before KV caches,
activations, adapters, Python/Metal allocation overhead, and other applications.
Both weights alone exceed the free share reported at that initial inspection.
The original BF16 2B checkpoint and isolated MLX environment are now installed
locally, and the ignored `.env` assigns Curmudgeon and Scholar to 2B with cache capacity one.
Local review smoke probes accepted a supported crisp-flavours claim and rejected
an off-topic US renewable-electricity claim for a food-production question.
These synthetic fixtures check review decisions and routing, not general model
accuracy or source classification. No dual-model residency benchmark was run;
capacity one remains the conservative starting point.

After closing memory-heavy applications, capacity two is an opt-in experiment:
set `COMMUNITY_MODEL_CACHE_SIZE=2`, start terminal chat, run Scholar/Curmudgeon
work, and monitor Activity Monitor and `memory_pressure`. Lower context limits
reduce memory use. If pressure or swap grows, return to capacity one. Reloads
are then expected when switching between model sizes.

## Diagnostics And Validation

`agent.model_loaded` records the requesting agent, model ID, and local directory
only when a new backend is loaded. `agent.inference` records agent, model ID,
status, total elapsed milliseconds (including waiting/loading), inference
milliseconds, and visible output tokens where available. No prompts or response
contents are added to these events. Candle reports re-tokenized visible output;
it excludes hidden thinking and is not a billing count. That field is null for
MLX; existing MLX generation diagnostics retain its native token counts.

`model probe-scholar` and `model probe-support` use Scholar's assignment,
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
