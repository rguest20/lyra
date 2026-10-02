# Agentic Semantic Map Maintenance

**Status:** implementation checklist, active

This document records the current learning and research workflow and the next
ideas for keeping the semantic map useful, well sourced, and open to correction.
The checklist at the end is the implementation order; completed items should be
checked off with their verification noted.

## Where the project is now

The project has two stores that serve different purposes:

- `state/knowledge.json` is the editable map used by the `learn`, `revise`, and
  `retract` commands.
- `state/learning.db` stores user observations, curated semantic claims, Scholar
  findings, Curmudgeon audits, Scout investigations, review decisions, and
  model-consolidation records.

Lyra retrieves relevant active claims from both stores when forming a reply.
SQLite claims retain their source, and Scholar findings retain a source URL and
the exact supporting excerpt. User observations remain attributed to the user.

The current roles are:

| Role | Current responsibility | Current boundary |
| --- | --- | --- |
| Curator | Extracts candidate claims from user observations and promotes or holds them according to evidence and review rules. | Does not rewrite the original observation. |
| Curmudgeon | Audits factual claims in Lyra's replies against comparable active map claims. | Can flag a conflict or leave it inconclusive; it does not research or correct the claim. |
| Scout | Selects a research gap, prioritizing challenged claims, claims without a comparable map entry, and subjects with one known relation. | Queues a question for Scholar; it does not edit claims. |
| Scholar | Searches English Wikipedia, extracts evidence, and adds supported findings to SQLite or holds them for review. | Its sources and provider are currently narrow; personal facts are excluded from web research. |

Scholar can currently promote a directly supported, nonconflicting extraction
when its extraction confidence meets the configured 0.95 threshold. That score
says how confident the extractor is in its interpretation; it does not measure
the source's reliability or independently establish that a claim is true.
Curmudgeon's 0.50 and 0.10 values are experimental weights for assistant claims,
not calibrated probabilities. These are important places to strengthen the
evidence policy.

The CLI chat worker handles a bounded batch of observations, then checks
Curmudgeon, Scout, and Scholar in sequence using the shared local model. It
wakes on new conversation activity and otherwise checks again every 20 seconds
by default. The browser server also runs a bounded `curate-once` subprocess
after chat activity and on idle ticks, so the model is released between passes.
`COMMUNITY_MAINTENANCE_INTERVAL_SECS` tunes the interval from 1 to 3600 seconds.
Scout now queues challenged claims and claims whose explicit review date has
passed. Curmudgeon still audits queued assistant replies, not every stored map
claim.

SQLite schema version 19 stores claim lifecycle, temporal metadata,
role-proposal/policy-decision history, and Scholar source-quality/freshness
metadata alongside first-class evidence links for Scholar sources and
user observations. The
`maintenance` CLI lists active SQLite claims, their review state, due status,
linked evidence, queue health, and the latest recorded maintenance pass. It can
record a review state or set volatility and the next review date. Age alone
does not change a claim's status.

Examples:

```sh
cargo run --release -- maintenance
cargo run --release -- maintenance schedule 42 fast_changing 2026-10-02T00:00:00Z
cargo run --release -- maintenance review 42 uncertain
cargo run --release -- maintenance scope 42 2020-01-01 2024-12-31
cargo run --release -- maintenance history 20
```

For conflict checks, an unscoped Scholar finding is treated as a current-day
assertion. Two explicitly scoped claims conflict only when their inclusive
validity ranges overlap. Curmudgeon compares current replies only against map
claims valid today; historical and future claims remain in the map and can be
used as dated context.

Reviewed SQLite claims can be explicitly approved for model consolidation. The
consolidation command trains against an approved claim, checks a short recall
probe, and writes a new Qwen3-0.6B checkpoint with the learned output weights in
`model.safetensors`. The old checkpoint remains available. This is a manual,
one-claim-at-a-time step; Scout and Scholar do not train model weights.

## The maintenance scheduler

A periodic tick every roughly 20 seconds sounds useful operationally. The
interval is a scheduling choice; the important behavior is event-driven urgent
work plus periodic checks for due maintenance. When work exists, the scheduler
processes a small, bounded amount. With no useful work, it stays idle instead
of having every role reread the map or call the model.

```text
new event ────────────────┐
                          v
                  maintenance scheduler <── periodic due-work check
                          |
                 bounded, leased work
                          v
                role proposal + evidence
                          |
                    policy decision
                          |
                    map transition
```

Conversation activity can still wake the worker immediately. The timer then
catches up with due maintenance between conversations. Queue leases and a work
budget would keep two worker passes from handling the same item and prevent one
busy map area from monopolizing a cycle. Work selection can consider urgency,
importance, uncertainty, staleness, and expected information gain. These can
start as ordered policy rules; a numeric priority formula is not required.

## How map maintenance could work

### 1. Separate claim state from review timing

The map will need a little more lifecycle information than subject, relation,
object, and source. Useful fields could include:

- epistemic/lifecycle state: `active`, `uncertain`, `challenged`,
  `superseded`, or `retracted`;
- `last_reviewed_at` and optional `next_review_at`; a claim is due when its
  review time passes, while its epistemic state remains independent;
- a volatility class such as `stable`, `slow-changing`, or `fast-changing`,
  used to choose the next review time;
- temporal scope for the proposition, kept separate from when Lyra learned or
  checked it.

The word `active` describes whether a claim is currently eligible for use; it
does not mean `verified`. Avoid a numeric usable-confidence value until it is
calibrated. Policy can first choose behavior such as state normally, qualify,
ask for confirmation, or abstain based on evidence state and conflicts.

Keep three questions separate:

1. **Extraction confidence:** did the role interpret its source correctly?
2. **Evidence strength:** how authoritative, direct, current, and corroborated
   is the evidence?
3. **Use policy:** should Lyra state the claim normally, qualify it, or withhold
   it in this context?

A well-extracted claim can still have weak or outdated evidence.

### 2. Give evidence its own identity

SQLite currently keeps Scholar findings and source details, but evidence is not
yet a general object linked to a claim by an explicit relationship. A useful
future shape is:

```text
Claim #42: Foo currently uses Bar
  Evidence #91: encyclopedia excerpt       supports
  Evidence #103: official documentation    contradicts
  Evidence #117: user observation          supports
```

Evidence should retain its source, excerpt, publication or observation date,
retrieval/check date, and whether it supports, contradicts, or qualifies the
claim. Curmudgeon can then flag disagreement without deciding which source is
right. Policy records the proposal, decision, and resulting state transition.

### 3. Give claims temporal identity

Claims often describe a proposition over a period of time. `PHP current stable
version = 8.5.1` can become `8.5.2` as the current value changes. `Ryan works at
Youwe` and `Ryan worked at CityFibre` can both be true if their periods do not
overlap. The temporal scope of each claim helps determine whether two claims
actually conflict.

Fields such as `valid_from`, `valid_until`, `observed_at`, and `checked_at` can
represent world time separately from knowledge time. They need not all be
present on every claim. A stable historical claim may have no review schedule;
a fast-changing claim can have one. This should be addressed before broad map
auditing so Curmudgeon does not treat different time periods as contradictions.

### 4. Preserve history through policy-controlled changes

New evidence should attach to an existing proposition or create a review
proposal; it should not erase the claim's history. Each role should return a
structured proposal identifying the claim, evidence, provenance, suggested
action, and reason. A deterministic policy layer then decides whether to attach
evidence, queue research, challenge a claim, change its use policy, or request
human review. The useful audit chain is:

```text
claim → evidence → role proposal → policy decision → map transition
```

Models may propose epistemic changes; deterministic policy controls state
transitions. Role outputs should not directly delete or silently rewrite active
claims.

### 5. Treat age as a reason to review, not a reason to delete

Claims have different lifetimes. A current office-holder, price, population
estimate, or software version may need frequent checks. A stable historical
event may remain useful indefinitely. The scheduler can mark volatile claims
`review_due` based on their review date. Scout can prioritize them, and Scholar
can seek a current source.

If evidence confirms the claim, record the new source and checked date. If
evidence conflicts, mark the claim challenged and retain both the old and new
evidence. If research finds nothing useful, leave the claim due or uncertain.
Retraction or supersession should preserve the old record and explain why it
left active use. Age alone should never remove a fact.

## Implementation checklist

Work in this order so the scheduler moves well-defined claims and evidence
through a policy rather than inventing its own state changes.

### A. Claim and evidence foundation

- [x] **A1. Define durable claim lifecycle and temporal scope.** Keep epistemic
  state separate from maintenance timing. Add optional `valid_from`,
  `valid_until`, `observed_at`, `checked_at`, and `next_review_at` fields where
  the source provides them. `review_due` is derived from time, not stored as a
  competing claim status.
  - Done when old databases migrate without losing IDs, sources, or retractions;
    temporal fields are optional; active retrieval excludes superseded and
    retracted claims. Verified with a fresh database migration and 61 passing
    release tests.
- [x] **A2. Make evidence first-class.** Store evidence identity, source kind,
  source URI or observation reference, exact excerpt, and relevant dates.
  Link evidence to claims as `supports`, `contradicts`, or `qualifies`.
  - Done when existing Scholar findings and user-observation provenance are
    represented by evidence links and can be listed with their claims.
    Verified with a fresh database migration and 61 passing release tests.
- [x] **A3. Separate confidence concepts.** Keep extraction confidence, evidence
  characterization, and answer-use policy distinct. Avoid treating a model's
  extraction score as truth probability. Scholar extraction confidence,
  source quality, and freshness are persisted independently and shown in the
  answer prompt with their meanings. Claim status supplies the separate answer
  policy: active is eligible with attribution, uncertain must be qualified,
  and challenged must not be presented as established. Missing assessments are
  labeled unknown rather than assigned invented numeric scores.
  - Done when persisted fields and prompts state which concept each value or
    state represents; answer behavior can be selected without fake precision.
    Verified with prompt-context assertions and the Scholar policy tests.

### B. Policy and role contracts

- [x] **B1. Define claim identity and contradiction rules.** Compare relation
  meaning and overlapping temporal scope before challenging claims. Preserve
  distinct time-bounded claims when both may be true.
  - Done when temporal variants do not trigger false direct-conflict
    challenges, while incompatible overlapping values still do. Scholar stores
    source-stated validity dates, checks inclusive range overlap, and retains
    partial overlaps as challenged instead of discarding the old period.
    User and Scholar claims both use explicit periods for conflict checks;
    Curmudgeon filters claims outside today's validity period. Verified with
    release tests for disjoint periods, overlapping values, partial-overlap
    review, and Curmudgeon's current-date filter.
- [x] **B2. Add explicit proposal and policy-decision records.** Curator,
  Scholar, Curmudgeon, and Scout emit durable evidence-linked proposals.
  Deterministic curation, conflict, queue, and human-review decisions are
  recorded separately with reasons and resulting claim/state. Inspect the
  latest decisions with `maintenance history [LIMIT]`.
  - Done when role proposals and decisions can be queried and linked to their
    source, evidence references, and resulting state. Verified by release tests
    and a fresh-database migration smoke check.
- [x] **B3. Tighten Scholar acceptance.** Require directly supporting evidence;
  represent source quality and freshness separately from extraction confidence.
  Wikipedia is recorded as reference quality; unknown-quality sources are held.
  Time-sensitive claims are held when the page revision is stale or unknown,
  and medical, legal, financial, safety, and similar consequential topics
  require human review. The model's extraction score does not override these
  gates. Review the fields with `scholar show JOB_ID`.
  - Done when a high extraction score alone cannot make weak, stale, or
    consequential evidence appear established. Verified with focused release
    tests for unknown source quality, stale time-sensitive sources, and
    consequential claims.

### C. Bounded maintenance scheduler

- [x] **C1. Add event wakeups plus a periodic due-work check.** Start with an
  interval near 20 seconds; keep the interval configurable.
  - Done when urgent new work wakes promptly and idle maintenance waits for the
    periodic tick. New activity wakes the worker; idle maintenance uses a
    configurable 20-second default interval.
- [x] **C2. Add leases, per-role work budgets, and priority ordering.** Start
  with transparent ordered rules over urgency, importance, uncertainty,
  staleness, and expected information gain.
  - Done when duplicate workers cannot claim the same item and one tick does a
    bounded amount of work. Existing queue leases and Scout's challenge, stale,
    then sparse ordering provide this initial policy; role batches remain
    bounded.
- [x] **C3. Review the map incrementally.** SQLite persists a rotating claim
  cursor and Scout advances through at most eight live claims per pass. It
  checks for sparse evidence or a single known relation and can queue one
  focused Scholar investigation. Challenges and due-review claims remain
  higher priority, and the cursor advances even on those passes so background
  coverage does not starve.
  - Done when all active claims can be reached over successive ticks without a
    full-map model pass on every tick. Verified cursor persistence, bounded
    batches, and wraparound with release tests.

### D. Maintenance operations and model history

- [x] **D1. Complete the map-maintenance view.** `maintenance` shows the
  claim's lifecycle state, temporal/review schedule, evidence assessment,
  evidence links, unresolved role proposals, open-work counts, and concrete
  review commands. Due claims also show the schedule-review action.
- [x] **D2. Make stale claims reviewable and reversible.** `maintenance resolve`
  can confirm, challenge, qualify, or retract a claim with a reason recorded as
  evidence and a separate policy decision. `maintenance supersede` links a
  replacement claim and retains the old claim and evidence. Age alone never
  removes a claim.
- [x] **D3. Track checkpoint lineage.** Each new consolidation records a
  SHA-256 of the root base checkpoint, an immutable snapshot of the initial
  adapter when present, the exact approved claim revisions, the training
  prompt, and recall/training limits in SQLite and a manifest beside the
  checkpoint. `model rebuild RUN_ID` verifies those base hashes and retrains
  only the manifest claims that remain active and approved, so withdrawn
  claims are omitted by rebuilding rather than surgically unlearned.
  - Done when a checkpoint's included claims and base are auditable and its
    rebuild command excludes withdrawn claims. Verified with manifest
    round-trip tests and release tests. Model-weight replay still requires the
    recorded base files and an available local inference device.

### E. Steward role decision

- [ ] **E1. Reassess whether a Steward role is needed after A–D.** The scheduler
  and deterministic policy own lifecycle coordination initially. Add a
  Steward only if a recurring reasoning task has no clear owner.

## Should there be a removal role?

There is a real need to challenge stale knowledge. A separate model whose main
job is deleting claims does not seem necessary yet. A scheduler rule can find
claims due for review, Scout can choose which gaps matter, Curmudgeon can flag
inconsistencies, and Scholar can look for current evidence. If this grows into
many kinds of lifecycle decisions, a Steward role could coordinate reviews and
recommend archival or retraction. It should preserve provenance and leave the
final state change to policy or an explicit review.

The goal is a map that can expand while keeping the path from claim to evidence
visible, challenges unresolved where necessary, and old knowledge correctable
without losing the history of how it changed.
