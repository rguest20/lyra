# Reply budget and missing adapter fixes

- [x] Distinguish unfinished thinking from memory allocation failure.
- [x] Log generation stop reasons, prompt/reply token counts, and MLX memory usage.
- [x] Bound debug thinking and automatically retry once without thinking when no visible answer is returned.
- [x] Preserve the original conversation during recovery and retain a partial debug trace.
- [x] Persist fresh adapters, include training metadata, and describe adapter state explicitly in startup logs.
- [x] Skip zero-effect untrained adapter computation and reduce prefill chunks from 256 to 128 tokens.
- [x] Forward browser reply diagnostics to the server log, including successful replies.
- [x] Keep normal startup JSON out of browser error messages.
- [x] Regression tests, live soup recovery, adapter initialization/reload, and temporary adapter training/reload.

## Diagnosis

The screenshot reports an unfinished thinking section. The old parser used the
message 'model used its reply budget before answering' whenever `</think>` was
absent, without knowing whether the limit was actually reached. No separate
out-of-memory event was found in the inspected log. An actual MLX allocation
error now appears as `model.backend_error`; process failure records exit code
and, on Unix, termination signal. A signal alone does not establish an OOM cause.

The adapter path was a configured destination: previously a fresh adapter only
existed in memory until training saved it. The file now exists at
`state/adapter-qwen3.5-2b.safetensors` (8,012,016 bytes), with zero training steps
and a zero output delta. It is initialized, not trained. Subsequent loads report
`loaded_untrained`. No synthetic training example was applied to the real adapter;
training verification used a separate temporary adapter.

## Recovery and logs

`COMMUNITY_THINKING_TOKENS` defaults to 256 (allowed range 1–2048). Normal chat
and extraction still disable thinking. Generation stops a thinking-only stream
at that budget. Missing closing tags, unexpected thinking-only output, and empty
answers receive one fresh generation with thinking disabled. Recovery keeps the
same conversation context. If that retry also fails, a clear error is logged.

`model.generation` includes `prompt_tokens`, `reply_limit`, `generated_tokens`,
`thinking_enabled`, `thinking_tokens`, `finish_reason`, and MLX memory in bytes.
`model.reply_recovery` includes the reason, action, and a bounded output excerpt.
`model.loaded` distinguishes file existence, creation, activation, and training
steps. A newly created adapter has `adapter_loaded: false` because it was just
initialized rather than read from disk; `adapter_exists: true` and
`adapter_created: true` make that explicit.

Browser reply stderr was previously captured and omitted from the server log
on success, then shown wholesale in an error bubble on failure. It is now
forwarded to server stderr in both cases. Only the failure message is shown in
the browser; startup configuration remains in the diagnostic log.

## Validation

**221 tests pass**, and the release build succeeds. New regression coverage checks
missing/empty thinking output, a complete visible reply, one automatic recovery
with a partial debug trace, and clean browser error messages.

Live Qwen3.5-2B probes verified:

- Normal soup reply with the newly initialized real adapter.
- Debug soup reply with `COMMUNITY_THINKING_TOKENS=1`: first generation stops
  after one thinking token; the retry disables thinking and returns an answer.
- Real adapter reload with zero training steps and no active delta.
- Temporary adapter training, persisted step count 1, and a trained reload.

The soup recovery probe reported approximately 3.7 GiB peak MLX allocation.
That measurement applies to this short prompt, not arbitrary context lengths.

Restart the server to use the new binary and logging:

```sh
cargo run --release -- serve 2>>community-lab.log
```

Probe and validation archive: `state/reply-adapter-checks/20261005-213038`.
