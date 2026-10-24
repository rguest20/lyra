# Code simplification progress — 24 October 2026

## Controlled grounding rerun

Reran the fixed nine-case v1 corpus through the controlled production-path
fixture runner using Qwen3.5-2B. The run retained each isolated input, SQLite
database, raw model calls, observed answer and blinded review packet under
`evals/results/code-simplification-controlled-r2-2026-10-23/`. All nine
pipeline invocations completed without timing out, including the
crisps-versus-renewable-energy case that timed out in the earlier direct
baseline. Fixture transport remained synthetic and made no network requests.

The binary fingerprint was
`16ea14886418aa27eebde7e237fd4f1b2d77d8140d64637e8f347deddcaf3ff8`, matching
the earlier direct baseline binary. The benchmark fingerprint was
`314aa9bf38370f2ef2bd793e495793c23b7b41a5ea88a2a49aa10fd3d0e0bce8`.
The nine pipeline runs took 30–109 seconds each. Their traces contain 170
`agent.inference` events (71 Scholar, 99 Curmudgeon) and 568,668 ms of summed
inference time. These figures describe this stochastic run; they do not establish
a before/after latency change.

| Evaluated stage | Passed |
| --- | ---: |
| Evidence selection | 7/9 |
| Extraction | 4/9 |
| Policy | 6/9 |
| Answer | 0/9 |
| Citation | 6/9 |
| Uncertainty | 4/9 |
| Question lifecycle | 9/9 |
| Bounded repair | 1/1 |
| End to end | 0/9 |

Only four local judgments passed schema validation. The local judge uses the
production model's weights and is advisory; the run is not an independent
semantic adjudication. The score therefore remains diagnostic, not acceptance.
The retained output points to concrete cases for independent review, including
two crisp-flavour requests where the system abstained despite extracted claims,
and a conflict case where it returned an unresolved answer without citations.
Do not infer the correct semantic fix from this same-model judgment. No prompt,
heuristic, ranking or policy changes were made from this run.

The nine blinded review packets from this latest run are collected under
`evals/results/code-simplification-controlled-r2-2026-10-23/blind-review-packets/`;
they omit expected and required rubric roles and include reviewer instructions.

The complete run failed the benchmark and `training_gate_eligible` remains
false. The original direct baseline still records its 180-second timeout; the
controlled rerun supplies the missing fixture-based observation, but does not
replace the direct production-path result or its transport limits.

## Cross-domain replay

Also replayed all 16 cases in `tests/fixtures/grounding-generalization.json`
through the production grounding path. Raw case inputs, outputs, isolated
databases and execution records are retained under
`evals/results/code-simplification-grounding-generalization-2026-10-24/`.
All 16 processes exited successfully, every source archive matched its fixture,
and all cases recorded zero network requests. Durations were 21–187 seconds,
for 1,144 seconds total. The trace contains 296 agent inference events (135
Scholar, 161 Curmudgeon) and 1,099,815 ms of summed inference time. The binary
fingerprint matches the nine-case run; the fixture fingerprint is
`dca21e7afdf6aad77ef7a7377314353500d49adc87c39ffbd764e543f26463e8`.

Created 16 blind review packets in that run directory. They contain requests,
evidence, observed extraction/selection and final answers, with no benchmark
expectations. The replay script deliberately records semantic adjudication as
not performed. The successful executions and the answer examples are therefore
diagnostic evidence only; they do not establish semantic accuracy or open any
acceptance gate.

A separate model then reviewed those packets against the original passages.
That advisory review found two likely unnecessary abstentions: the conditional
proposal passage directly says no observations have been reported, and the
injection passage contains a supported flower statement alongside instruction-
like text. It also found that the two population extraction objects lost their
values although the final answer retained them, and that the Alder Island answer
could imply an exhaustive species list where the source says “include.” The
review judgments are stored separately in
`evals/results/code-simplification-grounding-generalization-2026-10-24/adjudications/separate-model-review/`;
the blind packets remain unchanged. This is a distinct-model check, not human
acceptance. No semantic code changes were made from it.

## Step 8 candidate: preserve extracted meaning until review

Removed `normalize_source_reference_object` and its phrase lists from the
extraction parser. Parsing still repairs the supported flattened JSON envelope,
but no longer rewrites a relation/object pair based on English prefixes and
source-reference fragments. Original candidates instead pass through the
existing source-bound semantic review and bounded repair process.

The updated parser test preserves all three former source-reference cases. A
new scripted orchestration test rejects an unsupported actor, repairs the
candidate, and reviews the repaired claim before acceptance. These doubles
verify control flow, not semantic accuracy. All 477 Rust tests pass serially;
formatting and diff checks pass. Live after-change replays and independent
acceptance judgments remain pending. The user has offered to provide the
nine-case baseline judgments.
