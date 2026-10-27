# Quantized thinking and adapter-training experiment

**Status:** first local speed pilot complete. The local profile assigns every
role to INT8 with role-specific adapter paths; the global/default remains BF16.
No trained adapter is active. The 4-bit checkpoint was removed locally because
of quality-loss concerns; its benchmark records remain historical.

## Questions

1. Does an 8-bit checkpoint improve Lyra's real Interaction latency and memory
   use over the installed BF16 and 4-bit Qwen3.5 2B checkpoints?
2. Does any speed or memory gain preserve answer quality on the same prompts?
3. Can MLX train LoRA adapters in transformer layers while keeping a quantized
   base frozen, and can Lyra load those adapters safely?
4. If an adapter is accepted, should Lyra retain it as a separate versioned
   file, or materialize it into a new quantized checkpoint? The source
   checkpoint must remain available for rollback either way.

## Method

- Use the same local base model family, prompt, context, thinking settings,
  sampler and output-token limits across BF16, INT8 and 4-bit runs.
- Repeat runs; report load/warm-up separately from generation, and record
  prompt processing, thought/visible token counts, end-to-end latency, tokens per
  second, peak MLX memory, device, macOS, MLX and MLX-LM versions.
- Preserve every raw answer. Compare unrelated topics, factual questions,
  corrections, follow-ups and research-backed replies before changing the
  Interaction assignment.
- Keep training experiments isolated from live state and the active model.
  Training cannot be enabled for normal operation until the blind-adjudicated
  grounding acceptance benchmark passes.

## Results

The pilot used an Apple M5 MacBook Pro with 16 GB unified memory, macOS
25.6.0, MLX 0.32.3, MLX-LM 0.32.0, and the current local Qwen3.5 2B base. The
INT8 checkpoint was converted locally from the BF16 checkpoint with MLX-LM,
affine 8-bit quantization and group size 64 (8.502 effective bits per weight).
The existing 4-bit checkpoint uses affine 4-bit quantization and group size 64.

All nine runs used the same prompt, Lyra release binary, 2,048-token context,
and Lyra thinking policy. Each run reached the 1,024-token thinking cap. The
Interaction role used the tested checkpoint; Curmudgeon's other model calls
stayed BF16. Each run had a fresh database, conversation, and zero-initialized
adapter. The order was BF16, INT8, 4-bit, repeated three times. Metrics below
are medians across the three runs; latency and token-rate ranges show the
observed spread.

| Interaction checkpoint | Inference latency | Generated tokens/s | Peak generation memory |
| --- | ---: | ---: | ---: |
| BF16 | 35.5 s (35.3–39.1) | 30.26 (29.98–30.37) | 3.96 GB |
| INT8 | 20.5 s (20.5–29.9) | 51.58 (51.40–51.59) | 2.24 GB |
| 4-bit | 15.7 s (12.4–21.0) | 84.71 (50.53–87.24) | 1.30 GB |

These are Lyra's reported generation-memory peaks for the Interaction worker.
The complete `say` command medians were 42.3 s for BF16, 29.9 s for INT8, and
24.8 s for 4-bit; those include startup, model loading, and the other local
review calls. Lyra does not yet report load and prompt-prefill durations
separately, so the pilot cannot attribute those portions of command time.

All outputs are retained under
[`evals/results/quantized-thinking-2026-10-24/`](../evals/results/quantized-thinking-2026-10-24/),
along with per-run JSON diagnostics and the aggregate summary. The three
precisions produced coherent, useful interview-preparation advice in this
single prompt. The INT8 first run added an odd instruction about not assuming
the interviewer had provided information; it also needed a no-thinking repair
attempt. This is too little and too narrow to judge grounding or generalization.

The MLX-LM quantized LoRA conversion path was also checked without training or
saving weights: the local INT8 Qwen3.5 model loaded, and its final transformer
block converted to LoRA layers. Lyra's own worker still implements only an
output-layer adapter, so full-layer adapter training and runtime loading remain
unimplemented. No model weights were trained or activated.

## All-role speed check

A follow-up used the same interview prompt and release binary for three fresh
BF16-all-role runs and three fresh INT8-all-role runs. Each run had isolated
state, database and role-specific adapters; all five assignments were set to
the measured precision. This prompt invoked Curmudgeon and Interaction; it did
not call Scout, Scholar or Archivist. The measured metrics are medians:

| All-role assignment | End-to-end `say` time | Interaction inference | Interaction peak generation memory |
| --- | ---: | ---: | ---: |
| BF16 | 45.3 s (45.2–48.3) | 35.9 s | 3.96 GB |
| INT8 | 26.3 s (26.3–27.1) | 20.6 s | 2.24 GB |

The INT8 runs were about 42% faster end to end in this small paired test. The
earlier pilot, where only Interaction used INT8 and Curmudgeon remained BF16,
had a 29.9 s median end-to-end time for the same prompt. The new INT8 median is
about 12% lower, but the comparison is too small to attribute that difference
to changing the other roles; run order, adapter initialization and ordinary
runtime variation may contribute. These runs measure speed only, not answer
quality. Raw outputs and diagnostics are retained in
[`evals/results/all-role-precision-2026-10-24/`](../evals/results/all-role-precision-2026-10-24/).

## Batched inference probe

The standalone probe in
[`scripts/benchmark_mlx_batching.py`](../scripts/benchmark_mlx_batching.py)
compared four independent prompts generated serially against one MLX-LM
`BatchGenerator` batch on the same Qwen3.5 2B INT8 checkpoint. It used Lyra's
no-thinking prompt ending, temperature 0.1, top-p 0.8, top-k 20, and a
64-token cap. The role adapters were inactive in the running Lyra instance;
this benchmark loaded the base checkpoint directly. Three measured repetitions
after one warm-up gave median group times of 8.4 s serially and 2.3 s batched,
about 72% less time for the four prompts together. Median time to complete the
first prompt in input order was nearly unchanged: 2.28 s serially and 2.30 s
batched. Batch generation therefore improved group throughput in this probe,
not single-prompt latency. The outputs are retained in
[`evals/results/mlx-batching-2026-10-25/results.json`](../evals/results/mlx-batching-2026-10-25/results.json).

This was a speed-only probe. Most outputs hit the 64-token cap, paired outputs
were not token-identical under sampling, and no independent answer-quality
review was run. It does not establish a production latency improvement.

Lyra now exposes a shared batch API for independent conversations on any agent
role. MLX handles up to four requests together using that role's checkpoint and
adapter. Each retains its own prompt, token cap, sampler, stop tokens and
thinking budget. Rust applies the existing answer validation, thinking
continuations and bounded retries to each result separately. A failed batch
transport falls back to serial generation; Candle uses serial generation.
Batch memory figures describe the whole group, rather than individual items.
`agent.batch_inference` records request count, successful results and elapsed
time. As with consecutive single calls, `take_thinking()` retains only the last
item's trace; batch callers needing all traces need a richer return type.

Curator now extracts up to four independently leased observations together,
then performs each observation's existing validation, AI review and persistence.
Other stages can use the same API when their inputs are independent; dependent
reviews, repairs and conversational steps continue sequentially. Agent adapters
are never mixed in one batch. The frontend still handles HTTP requests
synchronously, so concurrent browser tabs are not automatically batched.

Validation includes the full Rust suite and a live INT8 worker smoke test with
mixed completion lengths, a one-token thinking budget, stop tokens and a
subsequent serial request. These checks validate orchestration and model
compatibility, not broad semantic accuracy or the production speed benefit.

## Candle backend check

The exact INT8 Qwen3.5 profile was attempted with `COMMUNITY_DEVICE=cpu` to
select Candle. Lyra rejected it before inference: Qwen3.5 routes to the MLX
worker and requires Apple Silicon Metal. Candle currently loads the older
`qwen3` architecture, while this checkpoint declares `qwen3_5`; this test
produced no Candle speed measurement. A fair Candle comparison requires
implementing Qwen3.5's architecture and quantized checkpoint support in the
Rust. A native Swift MLX worker was also prototyped against Apple's
[MLX Swift LM](https://github.com/ml-explore/mlx-swift-lm), which includes a
[Qwen3.5 implementation](https://github.com/ml-explore/mlx-swift-lm/blob/main/Libraries/MLXLLM/Models/Qwen35.swift).
The executable compiled, but could not load the model because MLX could not
find its default Metal library in this environment. It never reached inference,
so there is no Swift speed measurement or adapter-compatibility result. The
existing Python MLX-LM worker remains the runtime; removing Python is deferred.

## Limits and decision

The initial pilot supported INT8 for Interaction, and the all-role check
supports the current local assignment: median end-to-end time fell about 42%
versus BF16 in this one-prompt test. Interaction generation peak memory also
fell from 3.96 GB to 2.24 GB. The user chose to exclude 4-bit because of
quality-loss concerns, despite its higher speed in the initial narrow pilot.
These six runs on one prompt do not establish broad quality or grounding;
additional unrelated quality review and independent judgments are still needed
before drawing general conclusions about INT8.

MLX-LM's QLoRA-style path keeps the quantized base frozen and saves trainable
adapter weights separately. Lyra's training and scaling gate remains closed
until the grounding acceptance benchmark passes. A later merge/requantization
would create a new versioned checkpoint; it must keep its source checkpoint
for rollback and pass the same grounding checks before activation. SQLite
remains the auditable knowledge authority.
