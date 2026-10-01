# Async Learning Pipeline: Implementation Guide

The original nine-step guide below covers learning from user observations.
The later scholar extension adds a separate `scholar_jobs` queue for public
encyclopedia research. It stores sourced findings in SQLite, promotes directly
supported claims to the same semantic map, and keeps conflicts or unsupported
claims for review. See the README's Scholar research queue section for commands.

The Curmudgeon extension adds a separate assistant-reply ledger. Reply claims
remain untrusted; an audit records either a provenance-linked challenge or an
inconclusive result. SQLite applies the confidence policy after the audit.

Scout reads that ledger and the active map after Curmudgeon's audit. It queues
one public research gap for Scholar at a time, prioritizing live challenges,
uncompared assistant claims, and subjects with one known relation. Its SQLite
record links the trigger to the Scholar job; duplicate topics and rapid repeated
lookups are suppressed. Scout does not change map claims or verify them itself.

Reviewed SQLite claims can also be consolidated into a versioned Qwen3-0.6B
checkpoint with `model approve` followed by `model consolidate`. The trainer
folds an output adapter into a new untied output head in `model.safetensors`
after a bounded recall probe. The original checkpoint remains available for
rollback. This operation is explicit; Scout and Scholar do not train weights.

## Goal

Replace phrase-specific learning with a general path for capturing
user-provided information, curating it in the background, and promoting
supported claims into the semantic map.

Keep one CLI chatbot and one curator worker. The worker is a second role in the
same program, not a second service or a second model download.

## Intended flow

```text
                           +-------------------------+
                           |   Curator worker        |
                           | extract, resolve, merge |
                           +------------+------------+
                                        |
                              promote transaction
                                        |
                                        v
+----------+     +----------------------+----------------------+
| User CLI | --> | Agent 1: chat        | Semantic map          |
+----------+     | reads map and recent | curated, sourced      |
                 | pending observations | claims                |
                 +----------+-----------+----------------------+
                            |
                   save raw observation
                            |
                            v
                 +----------------------+
                 | Staging graph        |
                 | observations, claims,|
                 | evidence, status     |
                 +----------------------+
```

Agent 1 writes each user message to staging before generating its reply. It can
use that observation immediately, clearly treated as user-reported information.
The worker processes staged observations asynchronously. Agent 1 can answer
from the curated map and relevant pending observations.

## Storage model

Use one local SQLite database at `state/learning.db` for staging, curation
state, the semantic map, and provenance. A single transactional store avoids
lost updates when chat and the worker write concurrently. Enable SQLite WAL
mode and use short transactions. Keep the database local; this does not add a
server or network dependency.

Start with these records:

### Observation

- Stable ID
- Conversation ID and turn number
- Speaker
- Exact raw message
- Creation time
- Processing status: `pending`, `processing`, `promoted`,
  `needs_review`, or `rejected`
- Retry count and last error, when applicable

### Candidate claim

- Stable ID and observation ID
- Subject, relation, and object
- Evidence span from the original observation or earlier conversation
- Confidence and extraction notes
- Status: `proposed`, `promoted`, `needs_review`, or `rejected`

### Semantic claim

- Canonical subject, relation, and object
- Status: active or superseded
- Source observation IDs
- Creation time and superseded claim ID, if applicable

Treat observations as immutable evidence. A worker can add candidate claims or
change processing status, but it does not rewrite the user's original message.

## Invariants

1. **Capture before reply.** Persist the user message before asking the model to
   answer. If the process exits, the observation remains available to the
   worker.
2. **Keep evidence.** Every promoted claim links to the message and evidence
   span that support it.
3. **Do not teach from assistant output by default.** The assistant can be
   wrong. Curate user messages and explicitly trusted sources first.
4. **Make promotion idempotent.** Retrying a job must not create duplicate
   semantic claims.
5. **Commit promotion and status together.** Insert or revise semantic claims
   and mark the candidate promoted in one database transaction.
6. **Keep uncertainty visible.** Ambiguous references, weak evidence, and
   unresolved conflicts go to `needs_review`; they are not silently accepted
   or discarded.
7. **Drain staging logically.** Mark successful records promoted. Retain their
   evidence and status for audit and correction. Later archival can compact old
   records without losing provenance.
8. **Recent corrections outrank old assistant replies.** When a user corrects
   the agent, answer using the new sourced claim even if an earlier assistant
   turn disagrees.

## Step-by-step implementation

### 1. Define the claim and evidence contract

Use this contract for the first implementation. It separates what was actually
said from the worker's interpretation and from the canonical fact the map will
use.

#### Observation contract

An observation is an immutable record of one input from an allowed source.
Initially, only user messages are allowed. Store:

| Field | Meaning |
| --- | --- |
| `id` | Stable unique identifier, retained through retries |
| `conversation_id` | Conversation containing the input |
| `turn_number` | Position in that conversation |
| `speaker` | `user` for the initial implementation |
| `raw_text` | Exact message text as received; never normalized in place |
| `created_at` | UTC timestamp when captured |
| `status` | `pending`, `processing`, `promoted`, `needs_review`, or `rejected` |
| `retry_count` | Number of worker attempts |
| `last_error` | Optional worker failure detail |

The raw text is the evidence of what the user said, not proof that every claim
in it is objectively true. Preserve who reported it and, where available, the
message's position in the conversation.

#### Candidate claim contract

A candidate is the worker's structured interpretation of an observation. Each
candidate has:

| Field | Meaning |
| --- | --- |
| `id` | Stable unique identifier for this candidate |
| `observation_id` | Observation from which the candidate was extracted |
| `subject` | Entity the claim describes, such as `Lyra` |
| `relation` | Relation expressed, such as `name_was_given_by` |
| `object` | Entity or value related to the subject, such as `the user` |
| `evidence_text` | Exact supporting span copied from an allowed source message |
| `context_message_ids` | Earlier messages used only to resolve references, if any |
| `resolution` | Optional explanation of a resolved phrase, such as `it` → `the name Lyra` |
| `confidence` | Extractor confidence in the interpretation, from `0.0` to `1.0` |
| `status` | `proposed`, `promoted`, `needs_review`, or `rejected` |
| `decision_reason` | Optional reason for review, rejection, or promotion |

The subject, relation, and object are the candidate's meaning. They must never
replace or modify `raw_text` or `evidence_text`. Relations are open text in
this first version; the implementation must not need a Rust code change for a
new relation label.

#### Promotion threshold

Promote a claim automatically only when all of these conditions hold:

1. The source is an allowed input and is attributed to its speaker.
2. The evidence span appears exactly in that source message.
3. The evidence states the claim directly, or its reference has one clear
   resolution in the supplied conversation context.
4. The claim does not conflict with an active claim in a way the correction
   policy cannot resolve.
5. The extractor's confidence is at least `0.80`.

The numeric confidence is a gate, not a substitute for evidence. If any other
condition fails, set `needs_review`, even if the model reports high
confidence. Do not auto-promote guesses, implications, or facts copied from an
assistant response. A direct user correction can supersede an older claim
when it clearly refers to that claim; retain both sources and the supersession
link. If it is unclear whether the user is correcting or adding a separate
fact, hold it for review.

#### Worked example

Conversation context:

```text
User: Could you tell me your name?
Assistant: I'm Lyra, your conversational agent.
User: I gave it to you.
```

The observation contains only the last user message:

```text
raw_text: "I gave it to you."
speaker: user
```

The candidate retains that evidence and makes the context-dependent reference
visible:

```text
subject: Lyra
relation: name_was_given_by
object: the user
evidence_text: "I gave it to you."
context_message_ids: [message containing "I'm Lyra, your conversational agent."]
resolution: "it" refers to the name Lyra; "you" refers to the assistant
confidence: 0.91
status: proposed
```

If the conversation contains multiple possible names or recipients, the
candidate goes to `needs_review` instead of choosing one. On promotion, the
semantic claim stores the canonical relation and value plus a link to the
candidate and source observation; it does not discard the original sentence.

**Done when:** the observation, candidate, and promoted claim can be represented
with these fields, and a reviewer can trace every promoted meaning to the
exact user message and any context used to interpret it.

### 2. Add the SQLite store and migrate current data

Add a small SQLite dependency and a storage module with schema creation and
versioned migrations. The initial schema includes observations, candidate
claims, and semantic claims. Import the existing `state/knowledge.json` claims
into the semantic-claim table, preserving IDs, sources, supersession, and
retraction state. Keep a backup of the JSON file until the imported database
has been checked.

Use database transactions for all writes. Give the chat side and worker side
their own database connections; both point to the same local file.

**Done when:** current map claims can be listed from SQLite and match the JSON
map before the old JSON path is retired.

**Implemented:** `cargo run --release -- migrate-knowledge` creates the local
database and imports the current map. `COMMUNITY_DB` overrides its default
path, `state/learning.db`. The migration is safe to rerun and stops on an ID
collision with different content. Direct `learn`, `revise`, and `retract`
commands still edit the JSON map; worker-curated claims live in SQLite and are
read by chat from there. The `knowledge` command displays both stores.

### 3. Replace direct learning with observation capture

Change the chat and single-reply paths so every non-empty user input is
inserted as an observation before reply generation. Remove the phrase-specific
extractor, history-learning cursor, and direct semantic-map writes from
`learning.rs`. The staging store owns capture; this step does not yet extract
claims or add pending observations to the prompt.

Use the observation ID as the durable job ID. Give the observation a unique
conversation and turn key, and make capture idempotent so retrying the same
input after a generation failure returns the original ID. Persist the exact
input text before calling the model. No worker notification is needed yet; the
worker will find pending rows during its startup scan.

**Done when:** a new message is present in staging even if reply generation
fails, retries do not duplicate it, and chat no longer writes phrase-matched
claims directly to the semantic map.

**Implemented:** chat and `say` stage input in `state/learning.db`; a schema
migration adds the unique conversation/turn/input key. The phrase-specific
learning module and `sync-learning` command have been removed. Existing
knowledge-map claims remain intact. The next step adds staged observations to
the prompt.

### 4. Let Agent 1 use current context

Build each chat prompt from:

1. The system instructions.
2. Relevant active semantic claims, with their sources.
3. Relevant pending observations, labeled as unreviewed user statements.
4. The full conversation history, within the model's context limit.
5. The latest user message.

Keep the latest message as the explicit question to answer. Do not present
pending observations as verified facts. Preserve the current direct factual
grounding behavior where a strong stored claim corrects a contradictory draft.

**Done when:** a just-staged correction can inform the reply before the worker
has promoted it, while the reply still identifies it as something the user
said.

**Implemented:** relevant pending, processing, and review observations from the
same conversation are added as a labeled system context. The current input is
left as the final user message and is not duplicated in that context. If these
observations add prompt tokens, the generation cap grows by the same number,
starting from the existing 4,096-token reply cap. Both prompt and reply remain
bounded by the model's configured context window, with at least 256 tokens
reserved for an answer. Observations are not dropped to fit the former fixed
reply reserve.

### 5. Add the durable curator worker

Start one worker thread in the CLI process. At startup, scan for pending or
expired `processing` observations, then wait for notifications and process
jobs in small batches. Claim each job with a short transaction and a lease or
processing timestamp so a restart can recover it.

Do not start a second copy of the language model for the worker in this phase.
Share model access through one serialized interface. The worker can run between
chat generations; if model contention becomes noticeable, measure it before
adding another model instance or process.

On normal shutdown, let the worker finish its current transaction. Leave
unprocessed observations pending so the next run can resume.

**Done when:** a message captured by Agent 1 is processed without a separate
command, and an interrupted worker resumes it after restart.

**Implemented:** `chat` starts one named worker thread with a separate SQLite
connection and the same `LocalModel` behind a mutex. It scans at startup,
wakes after a new observation, and also rescans every two seconds. Job claims
use a ten-minute lease; an expired `processing` row is eligible after a crash.
The worker handles batches of four and finishes its current batch on shutdown.
It extracts candidate claims and applies the Part 7 curation policy, storing
accepted semantic claims and their observation provenance in SQLite.

### 6. Add general claim extraction

Ask the local model to extract zero or more claims into a strict structured
format. Include the observation and only the relevant conversation context
needed to resolve references. Require each claim to cite an evidence span.

Parse and validate the response before writing candidates. Reject malformed
output. Check that evidence occurs in the observation or an allowed context
message. Keep unresolved pronouns, multiple possible referents, and
unsupported inferences in `needs_review`.

Extraction should be relation-agnostic: new relation labels are allowed rather
than added as special cases in Rust. Keep canonicalization separate from
extraction so later extractors can replace the model without changing storage.

**Done when:** the same pipeline can stage and extract different subjects and
relations without adding a new phrase-specific branch.

**Implemented with the worker:** extraction requests strict JSON and requires
an exact evidence substring from the original user message. It supplies earlier
messages with overlapping meaningful terms, plus the most recent exchange when
the observation contains a reference pronoun, with a 12-message maximum.
Ambiguous explanations and invalid candidate fields/evidence are retained as
`needs_review`; valid siblings are preserved. Low-confidence and unresolved
references become `needs_review`; candidates then pass through the promotion
and conflict policy described in Part 7. Extraction uses low-temperature
decoding and examples for general first-person factual statements. A missing
confidence value defaults to zero and remains reviewable rather than losing
the candidate.

### 7. Curate, deduplicate, and resolve conflicts

For each candidate:

- Normalize whitespace and entity labels without changing the evidence.
- Find equivalent active claims and attach the new observation as another
  source instead of inserting a duplicate.
- When evidence contradicts an active claim, retain both sources and create a
  superseding claim or mark the conflict `needs_review`.
- Promote only claims that meet the agreed evidence threshold.
- Record a reason for rejection or review.

The first policy should trust direct user reports about the user's own
preferences and supplied context, while keeping external claims attributed to
their source. Do not turn repeated assistant wording into extra evidence.

**Done when:** duplicate observations do not create duplicate claims, and
conflicts remain traceable.

**Implemented:** SQLite normalizes whitespace for stored candidate fields and
matches active semantic claims case insensitively. Claims with confidence at or
above 0.95 and validated evidence are promoted; lower confidence claims remain
in review. An equivalent active claim is reused and linked to the new
observation through `semantic_claim_sources`. A different object for an active
subject and relation is retained as a review candidate with the conflicting
claim ID in its reason. Curated database claims are retrieved into chat
context, with provenance. Self-referential questions can retrieve user claims
even when their words differ by a simple inflection. If generation says it
doesn't know despite matching sourced user evidence, the reply quotes that
evidence and attributes it to the user. The database schema migrates to version
4.

### 8. Add basic operations to the CLI

Keep the chat command as the main workflow. Add small inspection commands:

- `inbox`: pending, processing, and review counts, with observation IDs
- `inbox show ID`: source message, candidate claims, evidence, and status
- `knowledge`: active semantic claims and provenance
- `review CANDIDATE_ID accept|reject REASON`: resolve a held candidate and
  record why
- Existing revise and retract operations should record why and preserve source

These commands make worker behavior visible without turning normal chat into a
moderation dashboard.

**Done when:** a user can see what was captured, what was promoted, and why an
item needs review.

**Implemented:** `inbox` lists pending, processing, and review observations;
`inbox show OBSERVATION_ID` displays the source and extracted candidates.
`knowledge` lists active JSON map claims and curated SQLite claims with
observation provenance. `review CANDIDATE_ID accept|reject REASON` records the
decision. Accepting a conflict supersedes the prior active claim while keeping
the prior record and linking the accepted claim to its observation. The README
documents the commands and backing up the local stores.

### Browser frontend

`cargo run --release -- serve` serves the chat page from `web/index.html` on
`127.0.0.1:8080`. The HTTP server only binds loopback addresses. Each POST
launches the existing `say` CLI command and returns its response as JSON. After
the reply, it launches one bounded `curate-once` process, then waits for that
pass before handling the next prompt. The web server itself does not load the
model, so the reply and curation processes release model memory when they exit.
Set `COMMUNITY_WEB_ADDR` to select a different loopback address.

### 9. Retire the temporary phrase rules

After the worker passes the acceptance checks, remove the specific patterns
from `src/learning.rs`. Keep direct, high-confidence map grounding in the
chat response path as a general operation over claims rather than a
Lyra-specific rule.

Update the README with the new storage location, worker lifecycle, review
commands, and backup behavior.

**Implemented:** the phrase-specific `src/learning.rs` module and
`sync-learning` command were removed with observation capture in Part 3. Chat
grounding now retrieves relevant active claims generically from the JSON map
and curated SQLite store, labels them as sourced user information, and keeps
the full conversation available as history. There are no name-giving or
personal-fact extraction branches in the reply path. The README documents
SQLite staging, worker behavior, inspection/review commands, and backup paths.

## Acceptance checks

- A user correction is saved to staging before the assistant replies.
- The assistant can use the staged correction in that same turn with clear
  source attribution.
- A general extractor can produce claims with different subjects and relations
  without adding phrase-specific code.
- The worker processes an observation asynchronously and recovers pending work
  after restart.
- A promoted claim has a source link back to the exact observation.
- Reprocessing an observation is safe and does not duplicate claims.
- Contradictions and ambiguous references remain inspectable.
- The staging inbox drains after promotion while provenance remains available.
- A correction in the semantic map takes precedence over an older assistant
  answer.
- The conversation agent remains usable when the curator worker is stopped;
  new observations wait in staging.

## Suggested build order

1. Store schema, migration, and observation tests.
2. Synchronous observation capture and pending-observation context.
3. Worker queue, restart recovery, and job status.
4. Structured general extraction and evidence validation.
5. Semantic promotion, deduplication, and conflict handling.
6. CLI inspection and review commands.
7. Remove temporary rules and update user documentation.

This order gives Agent 1 useful staged context early. Agent 2 can then improve
the semantic map without making chat depend on successful curation.
