# Rust File Guide

A brief map of the Rust source files in this repository. Each entry says what
that file is for and, in short, how it does the work. The list covers the 94
`.rs` files under `src/`.

## Application and conversation

| File | What it does and how |
| --- | --- |
| `src/main.rs` | Starts the program. It declares the application modules, builds the CLI configuration, and dispatches the requested command. |
| `src/agent.rs` | Runs Lyra’s conversation turn. It assembles the system prompt, conversation history, relevant map evidence, generates a reply, and records durable work that other roles can process. |
| `src/inference.rs` | Loads and runs the Candle and MLX Qwen backends, manages adapters, and handles model checkpoint updates. See [Model runtime](../README.md#model-runtime) for backend routing, caches, and request lanes. |
| `src/knowledge.rs` | Represents the editable semantic map. It validates, reads, writes, revises, and retracts sourced claims in the map file. |
| `src/errors.rs` | Provides the shared `InvalidData` error constructor used by parsing and validation boundaries. |
| `src/test_support.rs` | Provides unique temporary paths and shared answer-evidence fixtures for tests, keeping repeated setup consistent. |
| `src/web.rs` | Serves the browser chat UI on a loopback address. It handles HTTP requests and starts bounded maintenance work when chats, timers, or completed passes call for it. |

## Command-line interface

| File | What it does and how |
| --- | --- |
| `src/cli/mod.rs` | Defines CLI settings and command dispatch. It reads environment options and routes commands to the chat, knowledge, model, or worker modules. |
| `src/cli/chat.rs` | Implements interactive chat, single-message replies, and the long-running web server command. It loads the model and wires conversation services together. |
| `src/cli/knowledge.rs` | Implements map editing and inspection commands, maintenance/status views, and review actions by calling the knowledge-map and SQLite storage layers. |
| `src/cli/model_ops.rs` | Implements model inspection, training, and consolidation commands. It validates inputs, updates model artifacts, and records checkpoint lineage. |
| `src/cli/workers.rs` | Implements one-shot and continuous worker commands for Curator, Scout, Scholar, and Curmudgeon, plus queue/history display commands. |

## Learning and maintenance roles

| File | What it does and how |
| --- | --- |
| `src/curator.rs` | Turns stored chat observations into candidate claims. It asks the model for structured extractions, validates references and evidence, then stages suitable candidates for review. |
| `src/curmudgeon.rs` | Challenges questionable claims and assistant statements. It compares them with relevant map evidence and records bounded, evidence-linked review judgments. |
| `src/scout.rs` | Finds public knowledge gaps for Scholar to investigate. It ranks sparse or challenged map topics, creates focused questions, and limits model-suggested probes. |
| `src/scholar.rs` | Coordinates public research jobs. It queries Wikipedia and discovery providers, checks fetched material, extracts grounded findings, and hands evidence to storage for assessment and promotion. |
| `src/maintenance.rs` | Runs one recorded maintenance pass. It gives the available work to the appropriate roles under configured budgets and reports what the pass accomplished. |

## Scholar modules

| File | What it does and how |
| --- | --- |
| `src/scholar/claims.rs` | Checks whether proposed findings answer the research question and are directly supported by source text; it also provides deterministic list extraction. |
| `src/scholar/config.rs` | Loads optional search-provider settings from environment variables and local TOML configuration while keeping credentials out of error messages. |
| `src/scholar/discovery.rs` | Queries the configured local SearXNG instance and stores its results as leads rather than verified facts. Wikipedia search is implemented separately in `wikipedia.rs`. |
| `src/scholar/passages.rs` | Selects a small number of useful, contiguous page sections. It preserves headings and lists so extracted claims can be checked against exact passages. |
| `src/scholar/provenance.rs` | Collects declared publication, citation, and publisher clues from pages. These are stored as review signals, not treated as proof of source independence. |
| `src/scholar/site_policy.rs` | Applies robots.txt, llms.txt, and supported agent-manifest rules before fetching pages. It also bounds page requests and records policy and provenance outcomes. |
| `src/scholar/wikipedia.rs` | Queries the Wikipedia API and fetches selected article sections. It uses persisted request limits and converts page responses into material Scholar can assess. |

## SQLite storage

| File | What it does and how |
| --- | --- |
| `src/storage.rs` | Defines the storage records and opens the SQLite database. It also provides shared validation and database-error helpers used by the storage modules. |
| `src/storage/assistant_audit.rs` | Stores assistant replies for later factual review. It leases audit jobs, records judgments and challenges, and retrieves relevant challenge evidence. |
| `src/storage/candidate_review.rs` | Applies accept or reject decisions to staged claims, preserving the review reason and updating the semantic claim state transactionally. |
| `src/storage/evidence_policy.rs` | Compares source suitability and provenance for a claim. It looks for shared publication IDs, citations, publishers, and copied passages before corroboration counts. |
| `src/storage/maintenance.rs` | Tracks claim review schedules and state changes. It supplies due claims, records decisions, and handles resolution, temporal scope, and supersession. |
| `src/storage/model_lineage.rs` | Controls which reviewed claims may enter model training and records the claims and files associated with each model checkpoint. |
| `src/storage/observations.rs` | Stores chat observations and extracted candidates. It leases observations to workers and commits extraction results or holds them for review. |
| `src/storage/operations.rs` | Records maintenance passes and per-role outcomes. It exposes recent run history, current status, and interruption recovery. |
| `src/storage/queries.rs` | Provides read paths for the semantic map, Scholar evidence, review inbox, and observation details, including relevance ranking. |
| `src/storage/research.rs` | Stores Scholar and Scout research operations. It manages provider attempts, search leads, rate-limit reservations, site-policy caches, provenance reviews, and model-probe history. |
| `src/storage/research/corroboration.rs` | Queues claims that need another source. It leases an eligible SearXNG search and records whether new evidence resolves it. |
| `src/storage/research/intake.rs` | Accepts findings from research jobs. It checks passage support, freshness, and source assessments before storing evidence or proposing map changes. |
| `src/storage/research/promotion.rs` | Decides when staged evidence can become active semantic knowledge. It requires sufficient confidence, independent support, resolved provenance warnings, and no active conflict. |
| `src/storage/schema.rs` | Runs database migrations in order, selecting the migration group needed to bring an older database to the current schema. |
| `src/storage/schema/early.rs` | Creates and upgrades the foundational database tables used by the earliest versions of the application. |
| `src/storage/schema/middle.rs` | Applies the middle series of schema upgrades as learning, research, and review features were added. |
| `src/storage/schema/recent.rs` | Applies newer schema upgrades for request safety, maintenance, and current research workflows. It preserves retired search-provider records under legacy table names. |
| `src/storage/tests.rs` | Exercises storage behavior against temporary SQLite databases, including migrations, leases, evidence decisions, corroboration, and promotion. |

## Tests and how the pieces fit

The tests for individual modules usually sit beside the code under `#[cfg(test)]`;
`src/storage/tests.rs` holds the larger SQLite-focused cases. A typical research
path is: `agent.rs` queues work, `scout.rs` may identify a gap, `scholar.rs` and
its modules gather and assess evidence, `storage/research/` records and promotes
it, and `knowledge.rs` exposes the active map to later replies.
