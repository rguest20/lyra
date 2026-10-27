# Qwen3.5 2B quantized runtime experiment

Updated 15 October 2026. This records the state at that experiment, not the
current local assignments or a grounding acceptance result. At that time, the
configured model and running browser service remained on BF16.

## Runtime support

The MLX worker now loads Qwen3.5 checkpoints through MLX-LM's
quantization-aware loader. It reports the checkpoint precision and quantization
settings at startup. The worker keeps Lyra's rank-8 output adapter separate;
updating the adapter does not rewrite or requantize the base. Merging into a
quantized checkpoint is rejected. In the integrated path, the worker generated
after initializing an adapter. A separate smoke run trained one temporary
adapter step on the quantized output head, producing a finite loss and saving
the adapter.

The downloaded candidate is
[`mlx-community/Qwen3.5-2B-4bit`](https://huggingface.co/mlx-community/Qwen3.5-2B-4bit),
with 4-bit affine quantization and group size 64. Its 1,722,271,785-byte weight
file has SHA-256
`713fe7e5d3c3965f7106b0d0ee17615f7869c23c8d327996df8c1196fbcf07d5`.
The BF16 checkpoint directory is about 4.2 GB; the quantized weights are about
1.6 GB. The candidate packages its prompt template in `chat_template.jinja`;
the runtime now accepts that standard sidecar as well as a template embedded in
`tokenizer_config.json`.

The existing adapter is an output-layer adapter. This does not establish
support for QLoRA updates to transformer layers.

## Measurements

Hardware was an Apple M5 with 16 GB unified memory. The environment reported
MLX 0.32.3 and MLX-LM 0.32.0.

An isolated generation used the same interview-preparation prompt, no thinking,
the same sampler and a 128-token cap:

| Checkpoint | Load time | Generation time | Rate | Peak MLX memory |
| --- | ---: | ---: | ---: | ---: |
| BF16 | 1.22 s | 6.12 s | 20.93 tokens/s | 3.82 GB |
| 4-bit | 0.42 s | 2.46 s | 52.01 tokens/s | 1.14 GB |

Both samples reached the token cap, so the rates are useful for a controlled
speed comparison but the text is truncated and does not establish answer
quality.

A separate full `community-lab say` replay used isolated temporary state,
database and adapter paths. Both models answered “How would you prepare for an
interview?” without research:

- The 4-bit run returned concise advice after 14.4 seconds of Interaction
  inference, including 1,024 thinking tokens. Its answer focused on researching
  the company, practicing behavioral questions and highlighting achievements.
- The BF16 run returned similar advice after 162.7 seconds. It used the same
  1,024 thinking tokens, then exhausted the answer budget and needed an extended
  output attempt before returning a 35-token answer.

This end-to-end difference includes different generated output lengths and a
budget retry, so it does not show that quantization alone caused the entire
speedup. It does show that the quantized candidate completed this ordinary
request much faster in this run. The temporary databases and adapters were
isolated from Lyra's live data.

## Next checks

- Compare full answers across unrelated domains, factual and advice questions,
  corrections, follow-ups and research-backed replies.
- Repeat end-to-end timing and distinguish model loading, thinking, visible
  generation, retries and non-model work.
- Review adapter training and model-lineage behavior before using trained
  adapters with a quantized base.
- The experiment recommended keeping BF16 as the configured default until
  broader checks showed an acceptable quality and latency trade-off. The
  current local profile now uses INT8 for all roles; the quality checks remain
  open.

`cargo test` passed 431 tests, formatting and Python syntax checks passed, and
the release build succeeded. These checks validate the runtime change; they do
not validate the semantic quality of quantized answers.
