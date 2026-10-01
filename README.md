# Community Lab

One local chatbot in Rust. Candle runs the **BF16 Qwen3-0.6B** conversational
model on Apple Metal, with no Ollama process or API. The model uses its own
tokenizer from `models/qwen3-0.6b/`. The chatbot also keeps its conversation
and consults an editable, sourced knowledge map.

## Set up the model

The three model files are ignored by Git. In a fresh checkout:

```sh
mkdir -p models/qwen3-0.6b
hf download Qwen/Qwen3-0.6B config.json model.safetensors tokenizer.json --local-dir models/qwen3-0.6b
cargo run --release -- chat
```

Type `/quit` to leave. The conversation is saved to
`state/agent-qwen3-0.6b.json`. You can also use `cargo run --release -- say
"Hello"` for one reply, or `cargo run --release -- status` to see the paths.
Chat loads the model once and is faster over several turns. The full saved
conversation enters each prompt, including older turns. The latest user
message remains the question to answer. If a draft exactly repeats an older
answer to a different question, the agent tries once more with the entire
history presented as reference text. The full transcript remains on disk.

## Use the browser frontend

Run the local web server and open `http://127.0.0.1:8080`:

```sh
cargo run --release -- serve
```

The server accepts connections only on a loopback address. Each prompt launches
the existing `say` command as a short-lived process, so its model is unloaded
after the reply. The server then runs one bounded curation pass before handling
the next prompt. This lets the curator update SQLite between turns without
keeping the reply agent loaded. Each browser tab gets a separate conversation
history under `state/web-sessions/`. Reloading a tab keeps its session; a new
tab or **Start a new conversation** begins a fresh history. Learned knowledge
is shared across sessions. The agent also starts a fresh history when you
explicitly ask it to ignore the conversation, or when the local model judges
that a new message starts a distinct conversational thread. This semantic
continuity check starts after the first two exchanges and uses recent dialogue;
clear follow-up references stay in the current conversation, and unclear
judgments keep it. The check adds a short local model pass to prompts that need
it. Set `COMMUNITY_WEB_ADDR` to another loopback address if port 8080 is in
use; non-loopback addresses are rejected.

## Scholar research queue

When Lyra's reply leaves a public factual question unanswered, a local model
check can queue a short search phrase for the scholar. The scholar has its own
SQLite queue and looks up an English Wikipedia article through the MediaWiki
API. It extracts one claim relevant to the question or keeps the article's
first sentence as a sourced summary when the small model cannot extract a
reliable triple. It stores the article URL and exact supporting excerpt, then
adds high confidence, nonconflicting claims to the SQLite semantic map.
Unsupported, low confidence, and conflicting claims stay in the scholar queue
for review. A web reply shows when research was
queued; the lookup runs after the reply and may inform later prompts. In the
CLI, the background worker checks the queue between turns.

```sh
cargo run --release -- scholar
cargo run --release -- scholar ask "What is photosynthesis?"
cargo run --release -- scholar-once
cargo run --release -- scholar show JOB_ID
cargo run --release -- scholar review FINDING_ID accept "Verified against the cited page"
cargo run --release -- knowledge
```

This first source provider covers encyclopedia topics. It does not search the
whole web, and Lyra does not use web results to establish personal facts about
the user. The search phrase is sent to Wikipedia; conversation history stays
local. Internet access and `curl` are required for the scholar worker.

## Curmudgeon consistency audit

Lyra's replies are now staged separately as **untrusted assistant claims**.
After a reply, Curmudgeon extracts factual assertions and compares each with
active sourced claims in both the JSON and SQLite maps. A different value for
the same subject and relation can raise a cited challenge. For measurements,
the first check normalizes common length units and challenges gaps of 3× or
more. Without a directly comparable claim, the result is **inconclusive**, not
verified. Curmudgeon never promotes its own claims or invents a correction.

The SQLite policy assigns an unverified assistant claim a usable confidence of
0.50; a severe challenge reduces that to 0.10. These values are experimental
usage weights, not probabilities of truth. The challenge and comparison claim
IDs are retained, and relevant challenges are supplied to Lyra on later turns.
Challenges whose comparison claims have since been retracted are omitted from
later prompts.
The current reply is delivered before its background audit finishes.

```sh
cargo run --release -- curmudgeon
cargo run --release -- curmudgeon audit "A cricket bat is 400m long."
cargo run --release -- curmudgeon-once
```

The `audit` command is for trying deliberately false or unusual statements
without waiting for Lyra to say them. Curmudgeon can only challenge against
facts already in the map. The local model may also miss a claim during
extraction; simple length and height sentences have a deterministic fallback.

## Scout knowledge gaps

Scout runs after Curmudgeon and chooses one public topic for Scholar: first a
challenged claim, then an assistant claim with no comparable map evidence, then
a map subject with only one known relation. It asks a neutral question rather
than treating Lyra's disputed value as a fact. Each investigation records the
triggering assistant claim or map claim and the resulting Scholar job ID in
SQLite. The same topic is queued once per trigger type, and Scout spaces new
jobs by at least five minutes to avoid continuous background research. Personal
facts such as the user's name or address are excluded from autonomous web
research.

```sh
cargo run --release -- scout
cargo run --release -- scout-once
cargo run --release -- scholar
```

`scout-once` selects and queues a question without loading the model or using
the network. Scholar performs the later lookup. A sparse map is a lead for
investigation, not evidence that its existing claim is false. Scholar's normal
source checks and review rules still decide whether a finding enters the map.
For the current architecture and maintenance ideas, see
[Agentic Semantic Map Maintenance](docs/agentic-map-maintenance.md).

## Update the model

The BF16 checkpoint is an ordinary weight checkpoint. This first training path
keeps its pretrained weights fixed and updates a **rank-8 output adapter** with
gradient descent. The adapter changes the model's token logits and is saved to
`state/adapter.safetensors`; subsequent chat runs reload it. This is a real
model weight update, separate from editing the knowledge map. It is deliberately
small enough to experiment with on the Mac. Full base-weight training is not
implemented yet.

```sh
cargo run --release -- train "What is the lab's greeting?" "Hello from the lab."
cargo run --release -- say "What is the lab's greeting?"
```

`train` accepts one reviewed prompt and answer, up to 128 answer tokens,
and performs one teacher-forced pass with AdamW. A single pass may have little
visible effect; repeat with suitable examples to develop behavior. Training is
explicit, not triggered by casual chat. Keep a backup of the adapter before
experiments you may want to undo. Qwen3-0.6B has a 40,960-token context. The
prompt can use up to 40,704 tokens, leaving at least 256 tokens for an answer.
The normal reply cap is 4,096 tokens; it grows by the token count of added
observation context when room remains, and is always bounded by the model's
context window. If the prompt itself exceeds the supported limit, the app
reports an error instead of dropping conversation history. Long prompts take
more memory and time on the Mac.

### Consolidate a reviewed SQLite claim into Qwen3-0.6B

The direct `train` command writes a separate adapter. For a durable model
checkpoint, approve an active SQLite semantic claim and consolidate it:

```sh
cargo run --release -- knowledge
cargo run --release -- model approve 12 "Checked the cited evidence"
cargo run --release -- model consolidate 12
cargo run --release -- model history
cargo run --release -- model rebuild 1
COMMUNITY_MODEL_DIR=models/qwen3-0.6b-claim-12-... cargo run --release -- model probe "What is known about the subject?"
```

Consolidation trains the small output adapter on the approved claim, checks
that a short probe recalls its value, then folds the learned output weights
into `lm_head.weight` in a **new** `model.safetensors`. It writes matching
`config.json` and `tokenizer.json` alongside the new checkpoint. The original
model directory remains the rollback copy. Set `COMMUNITY_MODEL_DIR` to the
printed new directory to use it; unset the variable to return to the original.
The new directory starts with no separate adapter, avoiding a double
application of the old one. Approval and the source and destination checkpoint
paths are recorded in SQLite. Claims that are retracted cannot be consolidated.
Each consolidation writes a manifest with the SHA-256 of its root checkpoint and
the exact approved claim revisions included. `model rebuild RUN_ID` verifies
the recorded base and rebuilds from the claims that remain active and approved.

The recall check covers the training question. It does not establish broad
generalization or guarantee that later model replies will always use the
stored fact. SQLite remains the auditable source of truth. Each consolidation
adds one reviewed claim; a later rebuild can replay the recorded claim set
from its original base checkpoint.

## Edit the knowledge map

```sh
cargo run --release -- learn "willow" "grows beside" "streams" "field note 1"
cargo run --release -- knowledge
cargo run --release -- revise 1 "willow" "grows beside" "rivers" "field note 2"
cargo run --release -- retract 2 "field note 3"
```

The map lives at `state/knowledge.json`. Each relationship has an ID and
source. Revisions keep the old claim as retracted, and retractions record their
source. The agent reloads the map before each reply and includes up to five
active claims that share words with the question. The matching is lexical, not
semantic vector search. Map claims are user-reported information, not verified
facts.

The first learning-store migration creates `state/learning.db` as a local
SQLite file and imports the current map without deleting or changing the JSON
file:

```sh
cargo run --release -- migrate-knowledge
```

The command can be rerun safely. It reports matching claims already present
and stops if an existing claim ID has different content. The JSON map remains
available for direct `learn`, `revise`, and `retract` commands. Curated claims
are stored in SQLite and included in chat context; `knowledge` displays both
sources. Set `COMMUNITY_DB` to use a different database path.

Chat and `say` save every user input as a pending observation in
`state/learning.db` before reply generation. Relevant unreviewed observations
from the same conversation are included as user-reported context; the latest
input remains the final prompt message. Repeating an input for the same
conversation turn after a failure reuses its observation. The `chat` command
starts one asynchronous curator thread that shares the local model and extracts
claims with source evidence. Claims at or above the 0.95 confidence threshold
are curated into SQLite; lower confidence claims and conflicts wait for review.
Equivalent claims retain additional observation provenance without creating
duplicates. Inputs sent with `say` remain pending until a later `chat` run.
Older conversation history is not backfilled into the staging table.

Inspect the worker inbox and review candidates with:

```sh
cargo run --release -- inbox
cargo run --release -- inbox show OBSERVATION_ID
cargo run --release -- knowledge
cargo run --release -- review CANDIDATE_ID accept "The user confirmed this correction"
cargo run --release -- review CANDIDATE_ID reject "This was a hypothetical example"
```

Review actions require a reason and preserve it with the candidate. Accepting a
conflict supersedes the previous active claim while retaining its history and
linking the accepted claim to the source observation. Back up
`state/learning.db` and `state/knowledge.json` after a clean shutdown and
before manual database maintenance.

`COMMUNITY_MODEL_DIR`, `COMMUNITY_ADAPTER`, `COMMUNITY_STATE`,
`COMMUNITY_KNOWLEDGE`, `COMMUNITY_DB`, and `COMMUNITY_DEVICE=cpu` override
the defaults.
A different base model needs a compatible config/checkpoint/tokenizer and a
separate conversation state and adapter path. The same agent and knowledge-map
logic can be used with a larger compatible model on a PC after a CUDA backend
is added.

The old `model.safetensors`, `tokenizer.json`,
`tokenizer_config.json`, and `vocab.txt` at the repository root belong to
an earlier MiniLM embedding model and are unused by the chatbot.
