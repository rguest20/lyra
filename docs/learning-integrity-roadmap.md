# Lyra Learning Integrity Roadmap

This is the detailed policy and architecture record, not a second active task
list. `nextSteps.txt` is the single source for current priorities; unchecked
roadmap items below remain proposals until they are promoted there.

## Objective

Let Lyra retain a broad, useful record of what it reads while preventing
unverified, irrelevant, stale, copied, or misinterpreted material from becoming
settled knowledge or silently shaping an answer.

The core rule is: **keep evidence; qualify claims; control use.** Raw source
material, extracted claims, reviewed findings, and answerable knowledge are
different things and must remain distinguishable throughout the system.

## Operating principles

- Preserve source material and provenance. Never discard a distinct claim only
  because it resembles another claim.
- Store evidence separately from propositions. A source may support, contradict,
  qualify, or be irrelevant to a claim; none of those relationships should be
  inferred from topic similarity alone.
- Keep extraction confidence, evidence strength, source quality, freshness,
  corroboration, and answer-use policy as separate fields. None is a probability
  that a proposition is true.
- Treat model and reviewer outputs as proposals. Deterministic policy decides
  which state transitions are allowed; review history remains auditable.
- Use evidence state when answering. Unreviewed findings may help with a
  clearly provisional answer, but must not be presented as settled knowledge.
- Preserve uncertainty, time scope, attribution, negation, entity identity, and
  list completeness from source to answer.
- Never train model weights from the raw source archive or unapproved claims.

## Ordered work list

### 1. Preserve evidence state through answering

- [x] Carry finding status, gate status/reason, research relevance, and
  extraction confidence through selection, answer generation, and answer review.
  Pending findings are explicitly treated as provisional, and extraction
  confidence is explicitly not truth confidence.
- [x] Also carry source kind, freshness/currency, corroboration lineage,
  direct support, publication date, topic class, and comparable active claims
  with the same subject/relation but a different value through the same path.
- [x] Define an initial answer policy: promoted eligible findings can be stated
  normally to the extent the passage supports them; needs-review and unresolved
  gate states must be attributed and qualified.
- [x] Make the answer reviewer verify passage support and evidence-state
  wording. Citations identify evidence; URLs by themselves are never evidence.
- [x] Add answer and review policy for known disagreement and stale/unknown
  evidence on current or recent questions. Conflicts remain unresolved, stale
  evidence can only be framed historically, and only reviewer-verified
  lineage establishes independence.
- [x] Keep useful partial answers. If a source establishes examples but not a
  full/current list, say so. If claims may disagree, state the unresolved
  scope rather than silently choosing a side.
- [x] Add regression coverage for direct single-source evidence, awaiting
  corroboration, comparable active claims, stale current-value evidence,
  unrelated findings, and sources that support only part of the requested
  scope. The crisp-range regression exercises stale status and unresolved
  scope through answer review; the storage conflict regression verifies that
  active alternate values reach answer candidates with their validity periods.

**Acceptance:** A provisional finding can still help answer a user, but neither
the selection model nor reply model can mistake its review state for approved
knowledge. Tests assert the status and wording survive the entire path from the
database query to the final answer review.

### 2. Make the source archive complete and auditable

- [x] Record source scope, input and retained character counts, and explicit
  truncation status. Hash the full input before retention limits are applied,
  so different source versions with the same retained prefix remain distinct.
- [x] Store provider/query, canonical source URL, retrieval time, title, and
  publication/revision time when available. Legacy imports and generic
  excerpts are labeled as such rather than presented as full sources.
- [x] Link harvested and Scholar findings to an exact source archive version
  and byte span; existing exact quotes remain available with that provenance.
- [x] Keep harvested chunks and raw retained source text separate, so
  re-extraction can improve without overwriting source text or prior decisions.
- [x] Add tests for Unicode counts and offsets, chunk boundaries, truncation
  metadata, distinct versions after the truncation boundary, and exact source
  links for harvested claims and Scholar findings across restart.

**Acceptance:** Given an answer claim, diagnostics can recover exactly which
source version and span supported it, what was omitted, and which extraction
and review decisions occurred.

### 3. Build an explicit claim/evidence ledger

- [x] Represent extracted claims with subject, relation, object, attribution,
  polarity, modality, and temporal validity. Preserve observation/check times on
  semantic claims. Unknown attribution, polarity, and modality remain unknown;
  extracted scope reaches semantic review and answer evidence.
- [x] Preserve the observed subject label and bind identity assertions to their
  exact source passage, source URL, Scholar finding, and semantic assessment.
  Keep unresolved names separate; stable entity grouping is handled in Part 4.
  Preserve correction/revision history through claim creation/review times,
  validity periods, retraction/supersession records, and versioned source hashes.
- [x] Represent evidence as a separate record linked to one or more claims with
  a typed assessment: supports, contradicts, qualifies, or does not address.
  Scholar now records deterministic `qualifies` links when overlapping claims
  differ in attribution, polarity, modality, or validity period; only aligned
  differences in value become contradictions. Distinct relations for the same
  established subject receive `does_not_address` links. Curmudgeon's separate
  harvested-claim assessments retain typed relations and reviewer model IDs.
- [x] For Scholar findings, persist deterministic support and contradiction
  links to semantic claims with the rule and rationale that created the link.
  Harvested-claim relations already carry supports/contradicts/qualifies/
  unrelated/insufficient labels.
- [x] Track source lineage and copied/syndicated material so many URLs repeating
  one source count as one reporting origin. Do not use majority vote as truth.
  Scholar requires reviewed publication origins for automatic corroboration
  and records likely copy/provenance reviews separately from fact support.
- [x] Preserve distinct claims and disagreements. Relationship edges can help
  retrieval but never imply support, corroboration, or equivalence.
- [x] Store proposal, reviewer assessment, deterministic policy decision, and
  resulting state transition separately, with reason and model/version data.
  Policy decisions now record whether the decision was deterministic, human,
  or AI; AI model ID/version remain null unless explicitly configured.
- [x] Define retention and correction rules that preserve history and allow
  retraction, supersession, or a later source revision without deleting the
  earlier record.

**Acceptance:** A reviewer can distinguish what a source said, what Scholar
extracted, what Curmudgeon assessed, what policy allowed, and what Lyra used.

### 4. Make identity and conflict resolution conservative

- [x] Treat entity resolution as evidence-backed identity assertions, not fuzzy
  string replacement. Keep aliases scoped to their source and context until
  identity is established. Context-limited identity evidence is now retained
  with its exact scope and cannot enter global equivalence matching.
- [x] Preserve ambiguity for names that may refer to multiple entities. Do not
  merge claims merely because subjects are textually similar.
- [x] Compare claims only after subject identity, relation meaning, polarity,
  time, geography, and scope are aligned.
  Comparisons require exact or reviewed identity (including a scoped alias in
  the same exact context), an exact normalized relation, compatible
  attribution/polarity/modality, overlapping validity, and matching geographic
  and contextual scope. A scope mismatch is kept unresolved.
- [x] Classify comparisons as same proposition, contradiction, qualification,
  related-but-distinct, or unresolved. Only the first three affect evidence
  state; distinct relations are recorded as `does_not_address`, while a scope
  mismatch is recorded as `unresolved`. Neither affects evidence state.
- [x] Add tests for aliases (including Clark Kent/Superman-style identity),
  parent/subsidiary relationships, same-name entities, negation, differing
  dates, and claims that are related but not equivalent. Coverage includes
  scoped fictional identity, ownership rejection, ambiguous Walkers names,
  non-overlapping dates, and distinct relations.

**Acceptance:** Entity links improve retrieval and claim comparison without
merging distinct people, companies, propositions, or time periods.

### 5. Make research goal-directed and finite

- [x] Preserve the priority order: direct conversation questions first,
  source-derived questions second, Scout-generated questions third.
- [x] Give each research task an explicit target: original question, entity and
  scope constraints, known evidence gap, acceptable evidence, sources tried,
  and completion/stop reason. Scholar persists these fields per job, records
  request-derived entity constraints, and links the complete search-attempt
  history rather than maintaining a lossy duplicate source list.
- [x] Let Scholar retain useful unrelated source material in the archive while
  preventing it from satisfying the current question. Relevance is task-specific
  and must not filter the archive itself. Scholar retains task relevance on
  each finding, while the answer selector and reviewer reject unrelated or
  tangential evidence as an answer to the active question.
- [x] Track question lifecycle and semantically similar prior attempts across
  conversation, source-derived, and Scout paths. Re-ask only when new evidence,
  changed time scope, or a user request justifies it. Scout now receives recent
  direct conversation questions alongside harvested and Scout questions when
  proposing a follow-up, then applies its existing similarity and cooldown
  checks. Direct user questions remain eligible on explicit request.
- [x] Distinguish no source found, source found but insufficient, extraction
  failure, reviewer rejection, and complete answer. Route each outcome
  differently and bound retries. Scholar persists those stop reasons per task;
  Scout and feedback attempts retain their existing finite retry budgets.
- [x] Keep Curmudgeon as an independent critique and repair router, not a truth
  oracle. A rejection may guide a better extraction or source; it must not
  become evidence that the claim is false. Its assessments are stored as typed
  evidence relationships and route repair or new-source feedback separately.

**Acceptance:** A task ends with grounded evidence, a useful partial result, a
clear unresolved state, or a bounded stop reason. It does not loop over the same
question/source without a changed plan.

### 6. Establish a learning-quality evaluation suite

- [x] Turn the observed failures into a versioned benchmark: crisps and
  compound flavour names, Walkers/Smith's entity mismatch, unrelated renewable
  energy evidence, cookie/navigation text, false completeness, nearby-question
  extraction, conflicting dates, copied sources, and failed/malformed repairs.
  Version 1 lives under `evals/learning-quality/v1`.
- [x] Evaluate evidence selection, extraction, policy disposition, answer
  generation, and citation/uncertainty separately, as well as end-to-end, with
  a structured adjudicated-run evaluator.
- [x] Track useful partial-answer rate alongside unsupported-answer-review
  failures so abstaining on everything cannot look like success.
- [x] Record expected evidence IDs/spans, entity and scope constraints,
  acceptable answer properties, and forbidden claims for each scenario.
- [ ] Run and retain a blind-adjudicated benchmark result against the current
  local models before using it as a prompt/model-change gate. A blind direct
  model probe is retained at
  `evals/results/learning-quality-v1-blind-2026-10-13.md` with the after-run
  beside it. A production-path blind smoke now covers all nine questions at
  `evals/results/learning-quality-v1-production-blind-2026-10-13.md`, with raw
  traces alongside it. This smoke used copies of the real DB rather than
  scenario-isolated fixture evidence, so fixture-level selection and policy
  scores are invalid; malformed repair was not injected and answers were not
  independently adjudicated. It exposed long unbounded structured outputs,
  which are now capped at 1,024 tokens, plus continued abstention and evidence
  selection issues. The evaluator and live database/queue smoke checklist are
  ready in `evals/learning-quality`. Controlled isolated fixtures now run through
  production extraction, bounded repair, persistence, selection, answer review
  and unchanged-question Scout admission. The latest record is
  `evals/results/learning-quality-v1-controlled-2026-10-13.md`: selection passes
  7/9 cases, normalized policy 8/9, duplicate admission 9/9, and injected repair
  1/1. The separate local judge still returns invalid or unreliable verdicts;
  independent authoritative adjudication and a passing full run remain required.
  A subsequent failure-repair report is retained at
  `evals/results/learning-quality-v1-fixes-2026-10-13.md`. It adds a bounded
  semantic relevance fallback, separates generation from internal review fields,
  and revises partial-answer review. The final six-case smoke preserves compound
  names, copied-source limits and both date accounts, but still exposes source
  attribution errors and irrelevant commentary. Intermediate full runs remain
  unstable; this does not complete the acceptance task or enable training.
  The cross-topic audit at
  `evals/results/learning-quality-agnostic-2026-10-13.md` removes residual
  food-focused prompt cues and tests researchers, books, museum collections and
  laboratory provenance. Its retained failures require preserving useful dated
  context during relevance review and rejecting invented entities/relationships
  in otherwise qualified answers.
  The subsequent [grounding basics progress report](archive/grounding-basics-progress.md)
  records property matching, historical/provenance answer contracts, entity-name
  safeguards, file splits and retained failed runs. Its final two-case replay is
  encouraging. The [generalization follow-up](grounding-generalization-progress.md)
  records repeated non-food fixtures, source/proposition boundary fixes, scoped
  approval binding, source-literal safeguards and module splits. The original
  scenarios have been replayed, but independent benchmark acceptance and fuller
  dialogue/search/harvest coverage remain outstanding. Training stays closed.
- [x] Add evaluation counters for repeated questions, irrelevant selected
  evidence, repair success, source diversity, provisional-answer rate, and
  unsupported-answer-review failures.

**Acceptance:** A change that improves one stage cannot silently regress
entity accuracy, uncertainty, useful partial answers, or research diversity.

### 7. Keep learning policy and model training behind the evidence boundary

- [x] Keep raw sources, harvested claims, and review proposals out of model
  consolidation. Only explicitly approved, provenance-complete claim revisions
  may enter a training run. The unrestricted `train PROMPT ANSWER` command is
  disabled. Approval and consolidation require an active, stable, unscoped
  claim with linked supporting web evidence, a source URL and excerpt.
- [x] Before consolidation, check that the claim is current for its stated
  scope, supported by acceptable evidence, not in unresolved conflict, and not
  withdrawn or superseded. Time-scoped or changing claims, claims due for
  review, competing active values, and claims with contradictory or qualifying
  evidence remain retrieval-only.
- [x] Keep checkpoint lineage and rebuildability so a later correction can be
  removed by replaying approved claims from the base model. Manifests now
  snapshot approval reason/time and linked evidence; rebuild rechecks the exact
  claim and evidence revision before replay. Legacy manifests without these
  snapshots cannot be replayed as if they were fully auditable.
- [x] Reassess model training only after retrieval and answer-time evidence
  policy are reliable; retrieval remains the source of truth for changing or
  contested facts. The available blind probes are not a passing production-path
  benchmark, so training remains paused. Consolidation and rebuild require the
  explicit `COMMUNITY_MODEL_TRAINING_READINESS=learning-quality-v1-passed`
  operator acknowledgement after the full blind-adjudicated Part 6 benchmark
  passes. The readiness gate is closed by default.

**Acceptance:** Every learned weight change can be traced to approved claim
revisions and rebuilt without withdrawn information.

## Foundation already present

- Research source text is retained separately from extracted claim and question
  baskets; claims are not deduplicated just for semantic similarity.
- Source-derived questions have a persisted lifecycle and feed into Scholar
  through Scout, with user conversation research prioritized ahead of them.
- Entity aliases require review, and semantic relationships do not count as
  corroboration.
- Scholar checks exact source spans, performs bounded repair, tries alternatives,
  and can use Curmudgeon feedback to repair a useful passage or seek another
  source.
- Answer grounding rejects irrelevant evidence, mismatched brands, unsupported
  completeness, and split compound item names.
- Full Rust tests, focused regression tests, and live crisp lookup have been
  exercised. The live run confirmed relevant-source selection and exposed the
  compound-name issue now covered by a regression test.

These are starting safeguards, not proof that the whole learning pipeline is
correct. Work in order, beginning with preserving evidence state through the
answer path.
