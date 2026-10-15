# Replace Qwen3-4B with Qwen3.5-2B

- [x] Download the official Qwen/Qwen3.5-2B BF16 checkpoint.
- [x] Add an MLX backend for the hybrid Qwen3.5 architecture, with one loaded Python worker per Rust model instance.
- [x] Preserve context limits, inference serialization, logging, grounding, and AI review.
- [x] Keep thinking disabled for extraction and ordinary replies.
- [x] Preserve adapter training and explain checkpoint consolidation support.
- [x] Migrate CLI and browser conversations without losing history or conversation IDs.
- [x] Select 2B in local configuration and remove the 4B weights, adapter, and active setup instructions.
- [x] Test the backend protocol, run the regression suite, build the release binary, and measure live extraction/review latency.
- [x] Record activation commands and measured results.

Qwen3.5 uses hybrid linear/full attention and is not interchangeable with Qwen3. This switch uses MLX-LM's Qwen3.5 implementation for text; the application does not expose vision inputs. Weights remain unquantized BF16.

The original setup above selected BF16. The MLX worker now also accepts
quantized Qwen3.5 checkpoints through MLX-LM's quantization-aware loader. A local
4-bit 2B checkpoint is being evaluated separately; it is not the active
configuration. Output adapters stay separate for quantized bases, and merge
requests are refused to avoid writing an invalid checkpoint.

## Completed switch

- Active configuration: `Qwen/Qwen3.5-2B`, `models/qwen3.5-2b`, a fresh
  `state/adapter-qwen3.5-2b.safetensors`, and `state/agent-qwen3.5-2b.json`.
- Installed isolated `.venv-qwen35` with `mlx-lm==0.32.0`; the Python worker
  is embedded in the Rust binary and lives only while its model instance lives.
- Downloaded official BF16 weights; the initial runtime required BF16.
  Text inference uses MLX's Qwen3.5 hybrid attention implementation.
- Preserved all 94 CLI messages (47 turns), plus nine browser conversations:
  168 messages in total. Verified full history equality and unchanged IDs.
- Preserved the research database and knowledge map. Existing research budgets,
  retry limits, AI review, and search/extraction logs remain configured.
- Removed the active 4B weights (8,056,450,781 bytes), state selection, and setup
  document. Historical state/config backups remain available for audit.
- Adapter training saved successfully and reloaded for generation. A separately
  merged BF16 output head also reloaded and generated successfully. This checks
  the backend merge path, rather than the entire claim-approval workflow.
- Added protocol regression tests for request preservation, backend errors,
  invalid JSON, early worker exit, and worker cleanup: **198 tests pass**.
  The release build succeeds.

## Live probes

Elapsed times include process startup and model loading; these are individual
local probes, not a broad benchmark.

| Probe | Result | Time |
| --- | --- | --- |
| Remote healthcare / solar refrigerator extraction | Grounded fact, first attempt | 11.62 s |
| Flat Earth disproof extraction | Preserved scientifically disproven wording | 6.27 s |
| Off-topic co-founding claim review, final prompt | Reject candidate; repair; no source exclusion | 9.02 s |
| Six-token adapter training | Finite loss 2.794, saved adapter | 4.69 s |
| Reload trained adapter and greet | Generated reply | 2.51 s |

The initial review probe returned invalid enum values on both attempts. Those
responses were logged and rejected. The review prompt now repeats the valid
choices explicitly and distinguishes a reject decision from a repair action.
The final probe chose reject/repair but still incorrectly labelled the useful
source irrelevant. The existing exact-useful-evidence routing guard preserved
that source and selected repair with no exclusion. Model reliability therefore
still depends on grounding, validation, and repair; this switch does not prove
that every research review will succeed.

## Run

```sh
cargo run --release -- serve 2>>community-lab.log
```

Open `http://127.0.0.1:8080`. For terminal chat, use
`cargo run --release -- chat 2>>community-lab.log`.
`cargo run --release -- status` confirms model and state paths. Diagnostic JSON
is written to stderr; the redirect appends it to `community-lab.log`.
Restart any previously launched server so it reads the new environment.

## References

- [Official Qwen3.5-2B checkpoint](https://huggingface.co/Qwen/Qwen3.5-2B)
- [MLX-LM implementation](https://github.com/ml-explore/mlx-lm)

Migration backups and probe logs: `state/model-switch-backups/20261005-204028`.
