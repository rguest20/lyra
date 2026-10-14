# Grounding basics: 13 October 2026

This records the earlier bounded pass. The subsequent
[generalization progress report](grounding-generalization-progress.md) contains
the broader repeated runs, further fixes and current remaining work.

The latest two local model replays answer the previously failing museum and
laboratory questions appropriately. This is a bounded improvement, not a passing
full benchmark. Training remains closed.

## Changes

- Evidence selection checks the requested property and the entity in the actual
  source quotation. Shared keywords alone no longer admit unrelated facts.
- Current questions can retain useful historical evidence. A bounded, cached
  request decomposition extracts literal property and constraint spans; it does
  not manufacture a new question or erase geography/purpose constraints.
- Simplified the small model's property decision to a boolean and short reason.
  More elaborate response schemas repeatedly caused malformed or contradictory
  decisions in real runs. Existing detailed assessments still receive strict
  validation when supplied.
- Added narrow source-literal answers for historical-only evidence and verified
  shared-origin reports. These preserve quotations, dates and citations while
  stating exactly what the available evidence cannot establish. Known conflicts,
  mixed current/historical evidence and omitted material qualifiers retain the
  full answer review path.
- Copied reports can both be retained without becoming independent corroboration.
  Companion selection requires verified origin, direct support, matching subject
  and scope, and an exact source excerpt; it does not merge or promote claims.
- Grounding rejects unsupported extensions of source entity names, even when the
  model reviewer approves them. This catches changes such as Helios Laboratory
  becoming Helios Institute. It is not a complete invented-name or relationship
  detector.
- Fixed a temporal classifier bug: `known` and `unknown` contained `now`, so
  ordinary stable claims could incorrectly acquire a freshness requirement.
  The short temporal keywords now match whole words.

These changes use source structure, scope and request properties rather than
topic-specific equivalence lists. The normalization still has English-language
assumptions and model-dependent intent extraction.

## Verification and retained failures

Final validation: **362 Rust tests and 12 Python evaluation tests passed**.
Regression tests cover failures observed before the corresponding fixes,
including wrong-property overlap, invented entity extensions, misleading
currentness claims and the temporal substring bug.

The final production-path fixture replay used local Qwen3.5-2B models, synthetic
passages and explicitly declared fixture lineage:

| Case | Final observed behavior |
| --- | --- |
| Current museum instruments, with only a 2012 source | Quotes the astrolabe/telescope evidence and says currentness has not been established. |
| Laboratory claim repeated by two copies of one report | Retains both citations, quotes the report once and says the supplied evidence does not establish independent corroboration. |

Artifacts are retained locally in
`evals/results/learning-quality-grounding-hour-final-2026-10-13/`, including
inputs, execution metadata, model calls, observed replies, isolated databases and
an audit. The executable SHA-256 matches the recorded run fingerprint.
All three supplied source passages were archived across the two databases;
neither database recorded an HTTP request. Production data was not changed.

Earlier runs are also retained. Three repeated museum cases failed in both the
property and intermediate contract runs: the model rejected historical examples
or failed the more complicated response contract. Other runs exposed fabricated
current exclusions, inappropriate temporal disclaimers and weak attribution.
The final two-case replay is encouraging, but has not yet been repeated or run
against the full nine-case benchmark. Local model judging remains advisory, and
these fixtures are not eligible to open the training gate.

## Rust file size

Moved coherent responsibilities into modules while preserving existing APIs:

| Existing file | Before | After |
| --- | ---: | ---: |
| `src/agent.rs` | 2,905 | 1,776 |
| `src/research.rs` | 841 | 745 |
| `src/storage/research.rs` | 3,335 | 2,417 |

New modules hold reply grounding (571 lines), source-limited rendering (170),
evidence selection (474), storage review operations (923) and reply tests (729).
The selection changes also add substantial regression coverage.

The next useful boundaries are Scholar orchestration versus extraction
(`src/scholar.rs`, 2,692 lines), remaining storage research responsibilities,
and storage integration tests grouped by behavior (`src/storage/tests.rs`,
5,985 lines). Split around responsibilities rather than imposing a line limit.

## Next work

- [x] Repeat the final museum case and expand the final candidate across new
  domains and the unchanged nine-case benchmark; retain every result.
- [ ] Improve attribution and useful partial-answer narration in the ordinary
  generated-answer path; narrow literal contracts do not solve this generally.
- [ ] Exercise differing dates and qualified claims against conflict metadata,
  keeping provisional findings separate by scope.
- [ ] Audit other broad freshness/high-stakes keyword stems without weakening
  medical, legal or financial review requirements.
- [x] Continue responsibility-based Rust module splits, preserving regression
  coverage. Keep training and scaling behind the grounding benchmark.
