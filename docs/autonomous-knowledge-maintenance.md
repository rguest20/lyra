# Autonomous Knowledge Maintenance

**Status:** core loop is implemented; reliability and visibility work remains

This document describes what happens after Community starts, what the current
autonomous loop can change, and the remaining work to make unattended
knowledge-map maintenance dependable and easy to inspect.

## Current behavior

The periodic maintenance loop already runs in both supported long-lived modes:

- `cargo run --release -- chat` starts the semantic curator thread. It handles
  queued observations, then runs Curmudgeon, Scout, and Scholar. It wakes on new
  activity and otherwise repeats after the configured interval.
- `cargo run --release -- serve` runs the same bounded role sequence in a short
  subprocess after chat activity and on its periodic tick.

The default interval is 20 seconds. Set
`COMMUNITY_MAINTENANCE_INTERVAL_SECS` to a value from 1 to 3600 to change it.
The loop stops when the process exits. One-shot CLI commands such as
`scout-once` and `scholar-once` run only when explicitly invoked.

In a maintenance pass, Scout scans a bounded, rotating portion of the map and
can queue a focused research question. Scholar can process a queued question
and add supported claims with source evidence to SQLite. Curmudgeon audits
queued assistant claims for conflicts. Curator handles user observations.
Queue leases and bounded batches limit duplicate or monopolizing work.

Automatic maintenance does **not** mean automatic model-weight training. New
knowledge remains in SQLite and is retrieved at reply time. Model consolidation
still requires explicit approval and a separate command. Existing Scholar
policy also holds unsupported or consequential claims for review rather than
promoting them automatically.

## What is still needed

The roles and scheduler exist, so the next work should make the loop observable,
recoverable, configurable, and demonstrably safe. It should not add another
agent merely to coordinate the existing ones.

### 1. Make maintenance status visible

- [ ] Show the last pass time, duration, and per-role outcome (work completed,
  no work, held for review, or error).
- [ ] Show queue depth and oldest-item age for Curator, Scout, and Scholar.
- [ ] Surface this status through `maintenance` and a small `/status` response
  in the web UI.
- [ ] Keep a bounded run history in SQLite so a restart does not erase the
  operational record.

**Acceptance:** after an idle tick, status distinguishes “checked and found no
work” from “did not run”; a failed role includes a useful error and does not
hide successful work by other roles.

### 2. Make the scheduler resilient

- [ ] Record a pass before starting work and its completion state afterward.
- [ ] Ensure a failed role does not prevent later roles from running.
- [ ] Verify expired leases are reclaimed after a process crash and that retry
  behavior does not create duplicate claims or Scholar jobs.
- [ ] Add a configurable per-pass time or inference budget as well as the
  existing item-count bounds.
- [ ] Avoid launching overlapping maintenance subprocesses in `serve`; make
  shutdown wait for or safely stop an active pass.

**Acceptance:** killing a worker mid-pass leaves queued work recoverable, and
repeating a pass cannot duplicate its durable effects.

### 3. Make autonomous exploration deliberate

- [ ] Keep Scout's rotating bounded scan and priority order visible in status.
- [ ] Record why each gap was selected and avoid repeatedly researching the
  same unresolved gap when previous searches found no usable evidence.
- [ ] Add a cooldown or retry policy for gaps with no result, stale sources, or
  already-open Scholar work.
- [ ] Define practical limits for research volume, network access, and source
  domains; preserve a way to disable external research while leaving local
  maintenance enabled.

**Acceptance:** a seeded sparse map eventually produces a focused Scholar job;
an unanswered or repeatedly unsupported gap does not cause a request loop.

### 4. Verify the whole loop end to end

- [ ] Add an integration test that seeds a sparse or challenged claim, runs a
  maintenance pass, and verifies Scout's job and Scholar's resulting evidence
  or held finding.
- [ ] Test that unsupported, stale, conflicting, and consequential findings
  remain held or challenged according to policy.
- [ ] Test both `chat` and `serve` scheduling paths with a short test interval
  and no user message after startup.
- [ ] Document the expected startup mode, environment settings, logs, and
  commands for inspecting and reviewing the resulting claims.

**Acceptance:** the test suite proves that unattended ticks can discover and
research a map gap, while existing evidence gates still control promotion.

## Suggested order

1. Add maintenance run status and persisted pass history.
2. Harden retries, lease recovery, and subprocess shutdown.
3. Add Scout cooldowns and configurable research boundaries.
4. Add end-to-end unattended-tick tests and update the operator guide.

## How to run the current loop

For the terminal chatbot:

```sh
cargo run --release -- chat
```

For the browser UI:

```sh
cargo run --release -- serve
```

Keep the selected process running. To inspect current work and claim state, use
`cargo run --release -- maintenance`; to inspect queued investigations, use
`cargo run --release -- scout`. Use `maintenance resolve` or
`maintenance supersede` for explicit claim decisions. The scheduler does not
approve model-weight consolidation on its own.
