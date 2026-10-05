# Autonomous Knowledge Maintenance

**Status:** implemented and verified (SQLite schema 19)

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
can queue a focused research question. Encyclopedia summaries and details are
interrogated for a specific unanswered detail, using the local model and prior
questions about the subject. Scholar can process a queued question
and add supported claims with source evidence to SQLite. Curmudgeon audits
queued assistant claims for conflicts. Curator handles user observations.
Expansion questions have a five-per-subject limit; near-duplicate wording is
rejected. A sourced passage that answers the question can be kept as an exact
encyclopedia detail when the small model cannot extract a reliable triple.
Queue leases and bounded batches limit duplicate or monopolizing work.

Automatic maintenance does **not** mean automatic model-weight training. New
knowledge remains in SQLite and is retrieved at reply time. Model consolidation
still requires explicit approval and a separate command. Flagged user
observations and Scholar findings receive an AI review with a recorded
rationale. Evidence, confidence, provenance, freshness, and conflict gates still
control promotion; rejected and unresolved decisions remain inspectable in
SQLite and the JSON-line stderr diagnostics. Operator review commands remain
available as overrides, but normal maintenance does not wait for a human.

## Implemented controls

The roles share one recorded pass runner. A failed role is recorded and later
roles still run when their budgets allow it. No extra coordinating agent is
needed for these scheduling decisions.

### 1. Make maintenance status visible

- [x] Show the last pass time, duration, and per-role outcome (work completed,
  no work, AI review or corroboration queued, or error).
- [x] Show queue depth and oldest-item age for Curator, Scout, and Scholar.
- [x] Surface this status through `maintenance` and a small `/status` response
  in the web UI.
- [x] Keep a bounded run history in SQLite so a restart does not erase the
  operational record.

**Acceptance:** after an idle tick, status distinguishes “checked and found no
work” from “did not run”; a failed role includes a useful error and does not
hide successful work by other roles.

`maintenance runs [LIMIT]` shows past passes. The browser header summarizes
the latest pass, and `/status` (also `/api/status`) returns its full JSON view.
The last 200 completed or interrupted passes are retained. Scout's queue count
is the number of challenged or due claims available for consideration; its
rotating scan cursor and priority order are shown separately.

### 2. Make the scheduler resilient

- [x] Record a pass before starting work and its completion state afterward.
- [x] Ensure a failed role does not prevent later roles from running.
- [x] Verify expired leases are reclaimed after a process crash and that retry
  behavior does not create duplicate claims or Scholar jobs.
- [x] Add a configurable per-pass time or inference budget as well as the
  existing item-count bounds.
- [x] Avoid launching overlapping maintenance subprocesses in `serve`; make
  shutdown wait for or safely stop an active pass.

**Acceptance:** killing a worker mid-pass leaves queued work recoverable, and
repeating a pass cannot duplicate its durable effects.

`serve` coalesces pending wakeups while one worker runs and waits for that
worker on Ctrl-C or termination. A worker that exits unexpectedly is marked interrupted by
its process ID; stale running passes are shown as interrupted after ten
minutes. SQLite leases reclaim unfinished observation, assistant-audit, and
Scholar jobs. The time budget is checked between roles; a model call already
running is allowed to finish. The default inference budget admits eight jobs
per pass: up to four Curator observations, one audit, up to two Scout question
attempts, and one Scholar job.

### 3. Make autonomous exploration deliberate

- [x] Keep Scout's rotating bounded scan and priority order visible in status.
- [x] Record why each gap was selected and avoid repeatedly researching the
  same unresolved gap when previous searches found no usable evidence.
- [x] Add a cooldown or retry policy for gaps with no result, stale sources, or
  already-open Scholar work.
- [x] Define practical limits for research volume, network access, and source
  domains; preserve a way to disable external research while leaving local
  maintenance enabled.

**Acceptance:** a seeded sparse map eventually produces a focused Scholar job;
an unanswered or repeatedly unsupported gap does not cause a request loop.

Each Scout attempt keeps its own reason and Scholar job. Pending or processing
jobs block another attempt. A `no_result` waits one day; an AI-reviewed finding
awaiting independent corroboration waits seven days. Unsupported or stale
findings receive an AI rejection and can be investigated again through Scout.
A promoted topic is done,
and any topic stops after three attempts. Scout also spaces investigations by
five minutes and accepts at most 20 per rolling day by default. Automatic
Scout and Scholar work can be disabled with `COMMUNITY_AUTONOMOUS_RESEARCH=off`.
The sole network provider remains English Wikipedia, with article URLs checked
against that domain before extraction.

### 4. Verify the whole loop end to end

- [x] Add an integration test that seeds a sparse or challenged claim, runs a
  maintenance pass, and verifies Scout's job and Scholar's resulting evidence
  or staged finding.
- [x] Test that unsupported, stale, conflicting, and consequential findings
  are kept out of promotion by automated policy.
- [x] Test both `chat` and `serve` scheduling paths with a short test interval
  and no user message after startup.
- [x] Document the expected startup mode, environment settings, logs, and
  commands for inspecting and reviewing the resulting claims.

**Acceptance:** the test suite proves that unattended ticks can discover and
research a map gap, while existing evidence gates still control promotion.

The integration test runs the real Scout selector and a local Scholar fixture
without network access. Existing Scholar policy tests cover unsupported,
stale, conflicting, and consequential findings. Live smoke checks observed two
completed `chat` passes without input and a `serve` pass after one idle second;
the latter exposed its error in `/status` when given a missing model and exited
cleanly on Ctrl-C. A live Wikipedia lookup was not part of the smoke test.

## Implementation order followed

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
`cargo run --release -- maintenance`; use `maintenance runs 10` for pass history
and `cargo run --release -- scout` for queued investigations. Use
`maintenance resolve` or
`maintenance supersede` for explicit claim decisions. The scheduler does not
approve model-weight consolidation on its own.

Settings: `COMMUNITY_MAINTENANCE_INTERVAL_SECS` defaults to `20` (1–3600);
`COMMUNITY_MAINTENANCE_MAX_MS` defaults to `120000` (1000–600000);
`COMMUNITY_MAINTENANCE_INFERENCE_BUDGET` defaults to `6` (1–16);
`COMMUNITY_MAX_RESEARCH_PER_DAY` defaults to `20` (0–200). The terminal logs
role errors to stderr; the browser server logs worker exits there and shows
the latest upkeep state in the page header. See the README for the operator
commands and settings table.
