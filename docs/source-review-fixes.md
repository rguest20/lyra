# Preserve useful sources when extraction fails

## Problem

The logged Norah Magero passage describes a solar-powered refrigerator for remote healthcare, but Scholar extracted the co-founding of Drop Access. Curmudgeon rejected the page because that claim did not answer the question. A poor extraction should not become a verdict against the source.

## Fix checklist

- [x] Assess passage usefulness independently of whether the proposed claim answers the question. Use explicit suitable, irrelevant, unreliable, or unknown source verdicts.
- [x] Let Curmudgeon identify exact useful evidence and send targeted guidance back to Scholar. Verify that any suggested evidence occurs in the supplied passage.
- [x] Default incomplete or ambiguous reviews to repair without excluding the source.
- [x] Exclude a source only after an explicit source verdict of irrelevant or unreliable. Searching elsewhere after extraction failure must not create an exclusion.
- [x] Scope exclusions to the specific page within the original question's retry lineage. Do not exclude an entire domain.
- [x] Keep the existing three attempts total. Record exhaustion separately from source suitability, preserving the passage and rejected findings.
- [x] Log source verdict, suggested evidence, selected route, exclusion reason, and failed extraction outcome separately.
- [x] Add regression coverage for the logged healthcare example, missing review fields, page exclusions, failed repair, restart persistence, and shared retry budget.
- [x] Run tests and compile the release binary; record results here.

## Compatibility

Existing ambiguous exclusions must become non-excluding. No historical rejection establishes unreliability by itself. Existing pending tasks retain their budget and question; a migration clears the exclusion effect of those older tasks without deleting their audit history.

## Verification

- `cargo test --locked`: 187 tests passed, zero failures.
- `cargo build --release --locked`: succeeded.
- `git diff --check`: passed.
- Regression tests exercise the logged VacciBox scenario with fixed model responses; live model behavior still needs observation after restarting the server.

## Activation and logs

Restart the running process to load the release binary. Database schema 41 migrates automatically on opening the store.

```sh
cargo run --release -- serve 2>>community-lab.log
```

`scholar.ai_source_review` now records `source_verdict`, `useful_evidence`, `selected_feedback_route`, and `exclusion_reason`. `scholar.guided_repair_started` includes `suggested_evidence`. `scholar.feedback_attempt_finished` distinguishes `extraction_failed` from a proposed finding, and reports source verdict as unchanged. Budget exhaustion is logged separately as `attempt_budget_exhausted`.

## Evidence whitespace and exclusion scope follow-up

- [x] Recover an exact contiguous source span when model evidence differs only
  in whitespace (for example, a sentence separator changed from newline to space).
  Store the original source text; keep all claim support and relevance checks.
- [x] Reject changed words, punctuation, case, omitted negation, stitched spans,
  and invented or truncated endings.
- [x] Log evidence match method (`exact`, `whitespace_only`, or
  `no_contiguous_match`), proposed evidence, original span, and byte offset.
  Log the complete selected passage up to its 4,500-character limit, its length,
  and whether the diagnostic excerpt was truncated.
- [x] Verify exclusions stay within one rejected finding's retry chain, including
  after restart. The same source remains eligible for independent claims, even
  when their research question is identical. Add scope and the effective excluded
  URL list to feedback logs.
- [x] Regression suite: 204 passing tests; release build succeeds.

The logged tikka evidence differs by a newline between the sentences. Resolving
that formatting issue does not establish that the extracted claim is fully
supported or relevant to chicken soup: its named subject is absent from the
quote, its wording drops the attribution/uncertainty, and the dish is off-topic.

### Semantic support design

Claim meaning and citation provenance require separate checks. An AI semantic
support assessment should judge whether the exact source span entails a proposed
paraphrase, preserving identity, negation, attribution, uncertainty, dates, and
qualifications. For example, spherical supports round in this shape context;
round alone does not establish a perfect sphere. Approximate or contradicted
claims must receive a qualification or rejection rather than an equivalence
verdict. The source quotation should continue to retain the original wording.

The later [semantic support change](semantic-support.md) now implements an AI support assessment across extraction, intake, review, and promotion. The description below records the limitation before that change.

Full semantic support was not implemented by this whitespace follow-up. Existing
lexical claim support checks can still reject valid paraphrases. Replacing those
checks needs a stored AI support assessment used consistently by extraction,
intake, review, and promotion; semantic similarity alone is insufficient.
