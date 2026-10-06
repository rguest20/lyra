# Semantic support for researched claims

- [x] Add an independent AI assessment of whether the passage entails the candidate claim, including paraphrases.
- [x] Recover and retain the original source quotation when the extractor paraphrases its evidence.
- [x] Preserve negation, attribution, uncertainty, qualifications, entities, and dates. Similar topics alone are insufficient.
- [x] Bind assessments to the assessed claim and quote, persist them, and expose them to Curmudgeon.
- [x] Let validated semantic support satisfy intake and promotion support requirements without word overlap.
- [x] Log each semantic verdict, reason, source span, and invalid response. Failures remain automated retries.
- [x] Add regression coverage, compile the release binary, and run live 2B probes.

This changes claim support, not source credibility or corroboration requirements.
Citation provenance still uses the original contiguous source text. A similarity
score alone cannot establish that a passage supports a claim.

## Implementation

Semantic checking is enabled for model-extracted candidates. The independent AI
pass judges entailment of the actual subject/relation/object against the passage,
allowing faithful paraphrases, synonyms, and resolved pronouns. It also recovers
the original quotation when the extractor's evidence was paraphrased. Only an
`entailed` assessment with a validated original quote can supply direct support.

The same check runs for pending findings without an assessment during Curmudgeon
review, including legacy findings that failed lexical support. Schema 42 adds
`scholar_semantic_support` without deleting existing data. Saved assessments are
bound to subject, relation, object, dates, and quotation; changing the claim
invalidates approval. Promotion rechecks the saved binding. Source quality,
relevance, freshness, personal-fact restrictions, and corroboration remain separate.

Invalid model responses and failed support judgments follow automated extraction
repair and the existing feedback budget. No human review step is added. Small
models can still misjudge meaning. The initial live probes wrongly approved a
scientifically disproven idea and dropped 'has been suggested'; conservative stance
checks now override these observed errors. These checks only restrict approval,
and can request qualification of a valid partial claim. They do not certify every
semantic assessment. Overlong audit explanations are bounded to 500 characters;
the raw model answer remains in diagnostic logs.

This implements passage-to-claim support. Search ranking, source independence,
claim deduplication, and stored claim conflict matching still have lexical rules;
it is not a replacement of every textual comparison in the application.

## Logs

`scholar.semantic_support` records:

- `candidate`: the claim being judged, including dates.
- `verdict`: `entailed`, `contradicted`, `qualified`, or `insufficient`.
- `model_verdict` and `stance_override`: distinguish AI decisions from guard overrides.
- `reason`, `original_evidence`, and bounded `model_output_excerpt`.
- `invalid` or `model_error` verdicts when assessment could not be completed.

`scholar.finding_disposition` reports the support method and saved assessment.
`scholar.semantic_support_recorded` identifies existing findings reassessed by AI.
Extraction and repair events also include their semantic assessment. To follow:

```sh
tail -f community-lab.log | rg 'semantic_support|extraction_decision|extraction_repair|finding_disposition'
```

## Validation

**218 tests pass**, including paraphrases, original citation recovery, qualification
and negation rejection, source attribution, persisted assessment binding, pending
finding upgrades, AI promotion, schema migration, and the two observed live model
stance failures. The release binary builds successfully.

Live probes use the production 2B semantic reviewer with isolated state and DB:

| Source / candidate | Final result |
| --- | --- |
| spherical / round | Entailed, accepted; original spherical quote retained |
| not exactly spherical / spherical | Qualified, not accepted |
| scientifically disproven / true conception | Contradicted, not accepted; guard also covers the earlier false approval |
| has been suggested / unqualified derivation | Qualified by stance guard, not accepted |
| has been suggested / may be derived from | Entailed, accepted |
| tikka masala served with rice / chicken soup originated in Punjab | Insufficient, not accepted |

Successful support probes take approximately 4–6 seconds including loading on
this machine. Full extraction plus semantic checking passed in 7.62 seconds for
the short shape example. Each model-extracted candidate adds one semantic model
call; a repaired candidate receives another assessment.

## Activate

Restart the local server to load the rebuilt binary and schema migration:

```sh
cargo run --release -- serve 2>>community-lab.log
```

Direct probe (does not store or promote claims):

```sh
cargo run --release -- model probe-support '{"question":"What shape is the world?","source_title":"World","source_excerpt":"The world is spherical.","subject":"The world","relation":"is","object":"round","evidence_text":"The world is round."}'
```

Probe and validation archive: `state/semantic-support-checks/20261005-210734`.
