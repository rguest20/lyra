mod agent;
mod curator;
mod inference;
mod knowledge;
mod scholar;
mod storage;
mod web;

use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent::{Agent, ReplyServices};
use inference::LocalModel;
use knowledge::KnowledgeMap;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "chat".into());
    if matches!(command.as_str(), "help" | "--help" | "-h") {
        usage();
        return Ok(());
    }
    let model = "Qwen/Qwen3-0.6B";
    let model_dir = env::var_os("COMMUNITY_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("models/qwen3-0.6b"));
    let adapter_path = env::var_os("COMMUNITY_ADAPTER")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("state/adapter.safetensors"));
    let cpu = env::var("COMMUNITY_DEVICE").is_ok_and(|value| value == "cpu");
    let state_path = env::var_os("COMMUNITY_STATE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("state/agent-qwen3-0.6b.json"));
    let knowledge_path = env::var_os("COMMUNITY_KNOWLEDGE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("state/knowledge.json"));
    let database_path = env::var_os("COMMUNITY_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("state/learning.db"));
    let mut agent = Agent::load(&state_path, model).map_err(|e| e.to_string())?;
    let mut map = KnowledgeMap::load(&knowledge_path).map_err(|e| e.to_string())?;
    match command.as_str() {
        "chat" => {
            if args.next().is_some() {
                return Err("chat takes no arguments".into());
            }
            let store = storage::LearningStore::open(&database_path).map_err(|e| {
                format!(
                    "could not open learning database {}: {e}",
                    database_path.display()
                )
            })?;
            let backend = Arc::new(Mutex::new(
                LocalModel::load(&model_dir, &adapter_path, cpu)
                    .map_err(|e| format!("could not load local model: {e}"))?,
            ));
            let curator = curator::CuratorHandle::spawn(
                database_path.to_string_lossy().into_owned(),
                backend.clone(),
            );
            println!(
                "Community is ready. Type /quit to leave. Conversation: {}",
                state_path.display()
            );
            loop {
                print!("You: ");
                io::stdout().flush().map_err(|e| e.to_string())?;
                let mut input = String::new();
                if io::stdin()
                    .read_line(&mut input)
                    .map_err(|e| e.to_string())?
                    == 0
                {
                    break;
                }
                let input = input.trim_end_matches(['\r', '\n']);
                let command_input = input.trim();
                if command_input == "/quit" {
                    break;
                }
                if command_input.is_empty() {
                    continue;
                }
                map = KnowledgeMap::load(&knowledge_path).map_err(|e| e.to_string())?;
                let mut model_guard = backend
                    .lock()
                    .map_err(|_| "local model lock was poisoned".to_owned())?;
                let reply = agent
                    .reply(
                        input,
                        &mut model_guard,
                        &map,
                        ReplyServices {
                            store: &store,
                            curator: Some(&curator),
                            state_path: &state_path,
                        },
                        env::var_os("COMMUNITY_DEBUG_THINKING").is_some(),
                    )
                    .map_err(|e| e.to_string())?;
                if reply.new_conversation {
                    println!("[Started a fresh conversation after a topic change]");
                }
                println!("Community: {}", reply.text);
                if reply.research_queued {
                    println!("[Scholar queued an encyclopedia lookup]");
                }
                if let Some(thinking) = reply.thinking {
                    println!("[model thinking]\n{thinking}");
                }
            }
            curator.stop();
        }
        "serve" => {
            if args.next().is_some() {
                return Err(
                    "serve takes no arguments; set COMMUNITY_WEB_ADDR to change the address".into(),
                );
            }
            let address = env::var("COMMUNITY_WEB_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:8080".into())
                .parse()
                .map_err(|error| format!("invalid COMMUNITY_WEB_ADDR: {error}"))?;
            web::serve(address, &database_path.to_string_lossy()).map_err(|e| e.to_string())?;
        }
        "curate-once" => {
            if args.next().is_some() {
                return Err("curate-once takes no arguments".into());
            }
            let mut backend = LocalModel::load(&model_dir, &adapter_path, cpu)
                .map_err(|e| format!("could not load local model for curation: {e}"))?;
            let processed = curator::process_once(&database_path.to_string_lossy(), &mut backend)
                .map_err(|e| format!("curation failed: {e}"))?;
            let researched = scholar::process_once(&database_path, &mut backend)
                .map_err(|e| format!("scholar failed: {e}"))?;
            println!("Curated {processed} observation(s); processed {researched} scholar job(s)");
        }
        "scholar-once" => {
            if args.next().is_some() {
                return Err("scholar-once takes no arguments".into());
            }
            let mut backend = LocalModel::load(&model_dir, &adapter_path, cpu)
                .map_err(|e| format!("could not load local model for scholar: {e}"))?;
            let processed = scholar::process_once(&database_path, &mut backend)
                .map_err(|e| format!("scholar failed: {e}"))?;
            println!("Processed {processed} scholar job(s)");
        }
        "say" => {
            let input = args.collect::<Vec<_>>().join(" ");
            if input.trim().is_empty() {
                return Err("say needs a message".into());
            }
            let store = storage::LearningStore::open(&database_path).map_err(|e| {
                format!(
                    "could not open learning database {}: {e}",
                    database_path.display()
                )
            })?;
            let mut backend = LocalModel::load(&model_dir, &adapter_path, cpu)
                .map_err(|e| format!("could not load local model: {e}"))?;
            let reply = agent
                .reply(
                    &input,
                    &mut backend,
                    &map,
                    ReplyServices {
                        store: &store,
                        curator: None,
                        state_path: &state_path,
                    },
                    env::var_os("COMMUNITY_DEBUG_THINKING").is_some_and(|value| value == "1"),
                )
                .map_err(|e| e.to_string())?;
            if env::var_os("COMMUNITY_WEB_REPLY").is_some() {
                println!(
                    "{}",
                    serde_json::json!({"reply": reply.text, "thinking": reply.thinking, "new_conversation": reply.new_conversation, "research_queued": reply.research_queued})
                );
            } else {
                println!("{}", reply.text);
                if let Some(thinking) = reply.thinking {
                    println!("[model thinking]\n{thinking}");
                }
            }
        }
        "learn" => {
            let values: Vec<_> = args.collect();
            if values.len() != 4 {
                return Err("learn needs SUBJECT RELATION OBJECT SOURCE".into());
            }
            let id = map
                .add(&values[0], &values[1], &values[2], &values[3])
                .map_err(|e| e.to_string())?;
            map.save(&knowledge_path).map_err(|e| e.to_string())?;
            println!("Added claim #{id}");
        }
        "train" => {
            let values: Vec<_> = args.collect();
            if values.len() != 2 {
                return Err("train needs PROMPT ANSWER".into());
            }
            let mut backend = LocalModel::load(&model_dir, &adapter_path, cpu)
                .map_err(|e| format!("could not load local model: {e}"))?;
            let (tokens, loss) = backend
                .train(&values[0], &values[1])
                .map_err(|e| format!("training failed: {e}"))?;
            println!("Updated adapter: {tokens} target tokens, mean loss {loss:.3}");
            println!("Saved: {}", adapter_path.display());
        }
        "revise" => {
            let values: Vec<_> = args.collect();
            if values.len() != 5 {
                return Err("revise needs ID SUBJECT RELATION OBJECT SOURCE".into());
            }
            let id: u64 = values[0].parse().map_err(|_| "ID must be an integer")?;
            let replacement = map
                .revise(id, &values[1], &values[2], &values[3], &values[4])
                .map_err(|e| e.to_string())?;
            map.save(&knowledge_path).map_err(|e| e.to_string())?;
            println!("Revised claim #{id} as #{replacement}");
        }
        "retract" => {
            let values: Vec<_> = args.collect();
            if values.len() != 2 {
                return Err("retract needs ID SOURCE".into());
            }
            let id: u64 = values[0].parse().map_err(|_| "ID must be an integer")?;
            map.retract(id, &values[1]).map_err(|e| e.to_string())?;
            map.save(&knowledge_path).map_err(|e| e.to_string())?;
            println!("Retracted claim #{id}");
        }
        "knowledge" => {
            if args.next().is_some() {
                return Err("knowledge takes no arguments".into());
            }
            let store = storage::LearningStore::open(&database_path).map_err(|e| e.to_string())?;
            for entry in map
                .entries()
                .iter()
                .filter(|entry| entry.retracted_by.is_none())
            {
                println!(
                    "#{}: {} — {} — {} [source: {}]",
                    entry.id, entry.subject, entry.relation, entry.object, entry.source
                );
            }
            let curated = store.active_semantic_claims().map_err(|e| e.to_string())?;
            let mut curated_count = 0;
            for detail in curated {
                let claim = detail.claim;
                if claim.source_observation_id.is_none() && !claim.source.starts_with("https://") {
                    continue;
                }
                curated_count += 1;
                println!(
                    "db#{}: {} — {} — {} [source: {}]",
                    claim.id, claim.subject, claim.relation, claim.object, claim.source
                );
                for source in detail.sources {
                    println!("  provenance: {source}");
                }
            }
            println!(
                "{} active JSON map claims; {curated_count} curated SQLite claims",
                map.active_count()
            );
        }
        "inbox" => {
            let values = args.collect::<Vec<_>>();
            let store = storage::LearningStore::open(&database_path).map_err(|e| e.to_string())?;
            match values.as_slice() {
                [] => {
                    let summary = store.inbox_summary().map_err(|e| e.to_string())?;
                    println!(
                        "Pending: {}; processing: {}; needs review: {}",
                        summary.pending, summary.processing, summary.needs_review
                    );
                    for observation in summary.observations {
                        println!(
                            "{} [{}] turn {}: {}",
                            observation.id,
                            observation.status,
                            observation.turn_number,
                            observation.raw_text
                        );
                    }
                }
                [subcommand, id] if subcommand == "show" => {
                    let detail = store
                        .observation_detail(id)
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| format!("no observation with ID {id}"))?;
                    let observation = detail.observation;
                    println!(
                        "Observation {} [{}] turn {}",
                        observation.id, observation.status, observation.turn_number
                    );
                    println!("Conversation: {}", observation.conversation_id);
                    println!("Source: {}", observation.raw_text);
                    if let Some(reason) = detail.last_error {
                        println!("Review reason: {reason}");
                    }
                    if detail.candidates.is_empty() {
                        println!("No candidate claims");
                    }
                    for candidate in detail.candidates {
                        println!(
                            "Candidate {} [{}; confidence {:.2}]: {} — {} — {}",
                            candidate.id,
                            candidate.status,
                            candidate.confidence,
                            candidate.subject,
                            candidate.relation,
                            candidate.object
                        );
                        println!("  Evidence: {}", candidate.evidence_text);
                        if let Some(reason) = candidate.decision_reason {
                            println!("  Decision: {reason}");
                        }
                        if let Some(semantic_id) = candidate.semantic_claim_id {
                            println!("  Semantic claim: db#{semantic_id}");
                        }
                    }
                }
                _ => return Err("usage: inbox [show OBSERVATION_ID]".into()),
            }
        }
        "scholar" => {
            let values = args.collect::<Vec<_>>();
            let mut store =
                storage::LearningStore::open(&database_path).map_err(|e| e.to_string())?;
            match values.as_slice() {
                [subcommand, query @ ..] if subcommand == "ask" && !query.is_empty() => {
                    let question = query.join(" ");
                    let nonce = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_err(|e| e.to_string())?
                        .as_nanos();
                    let turn =
                        usize::try_from(nonce).map_err(|_| "scholar job timestamp is too large")?;
                    store
                        .enqueue_research("manual-scholar", turn, &question)
                        .map_err(|e| e.to_string())?;
                    println!("Queued scholar research: {question}");
                }
                [] => {
                    for (job, note) in store.scholar_jobs().map_err(|e| e.to_string())? {
                        println!("{} [{}] {}", job.id, job.status, job.question);
                        if let Some(note) = note {
                            println!("  {note}");
                        }
                    }
                }
                [subcommand, job_id] if subcommand == "show" => {
                    let findings = store.scholar_findings(job_id).map_err(|e| e.to_string())?;
                    if findings.is_empty() {
                        println!("No findings for scholar job {job_id}");
                    }
                    for finding in findings {
                        println!(
                            "#{} [{}; confidence {:.2}]: {} — {} — {}",
                            finding.id,
                            finding.status,
                            finding.confidence,
                            finding.subject,
                            finding.relation,
                            finding.object
                        );
                        println!("  Source: {}", finding.source_url);
                        println!("  Evidence: {}", finding.evidence_text);
                        if let Some(reason) = finding.decision_reason {
                            println!("  Decision: {reason}");
                        }
                    }
                }
                [subcommand, id, decision, reason @ ..]
                    if subcommand == "review"
                        && !reason.is_empty()
                        && matches!(decision.as_str(), "accept" | "reject") =>
                {
                    let id: i64 = id.parse().map_err(|_| "finding ID must be an integer")?;
                    let reason = reason.join(" ");
                    store
                        .review_scholar_finding(id, decision == "accept", &reason)
                        .map_err(|e| e.to_string())?;
                    println!("Scholar finding #{id} {decision}ed: {reason}");
                }
                _ => {
                    return Err(
                        "usage: scholar [ask QUESTION | show JOB_ID | review FINDING_ID accept|reject REASON]"
                            .into(),
                    );
                }
            }
        }
        "review" => {
            let values = args.collect::<Vec<_>>();
            if values.len() < 3 || !matches!(values[1].as_str(), "accept" | "reject") {
                return Err("usage: review CANDIDATE_ID accept|reject REASON".into());
            }
            let id = &values[0];
            let accept = values[1] == "accept";
            let reason = values[2..].join(" ");
            let mut store =
                storage::LearningStore::open(&database_path).map_err(|e| e.to_string())?;
            store
                .resolve_candidate(id, accept, &reason)
                .map_err(|e| e.to_string())?;
            println!(
                "Candidate {id} {}: {}",
                if accept { "accepted" } else { "rejected" },
                reason
            );
        }
        "migrate-knowledge" => {
            if args.next().is_some() {
                return Err("migrate-knowledge takes no arguments".into());
            }
            let source = KnowledgeMap::load(&knowledge_path).map_err(|e| {
                format!(
                    "could not read source knowledge map {}: {e}",
                    knowledge_path.display()
                )
            })?;
            let mut store = storage::LearningStore::open(&database_path).map_err(|e| {
                format!(
                    "could not open learning database {}: {e}",
                    database_path.display()
                )
            })?;
            let summary = store.import_knowledge(&source).map_err(|e| e.to_string())?;
            println!(
                "Imported {} claims ({} already present) into {}. Source JSON remains unchanged.",
                summary.inserted,
                summary.already_present,
                database_path.display()
            );
            println!(
                "Database contains {} semantic claims.",
                store.claim_count().map_err(|e| e.to_string())?
            );
        }
        "status" => {
            if args.next().is_some() {
                return Err("status takes no arguments".into());
            }
            println!("Model: {model}");
            println!("Model directory: {}", model_dir.display());
            println!(
                "Adapter: {} ({})",
                adapter_path.display(),
                if adapter_path.exists() {
                    "trained"
                } else {
                    "fresh"
                }
            );
            println!("Conversation turns: {}", agent.turn_count());
            println!("State: {}", state_path.display());
            println!(
                "Knowledge: {} active claims ({})",
                map.active_count(),
                knowledge_path.display()
            );
        }
        _ => {
            usage();
            return Err(format!("unknown command: {command}"));
        }
    }
    Ok(())
}

fn usage() {
    println!(
        "Usage: cargo run -- [chat | serve | say MESSAGE | train PROMPT ANSWER | learn SUBJECT RELATION OBJECT SOURCE | revise ID SUBJECT RELATION OBJECT SOURCE | retract ID SOURCE | knowledge | inbox [show OBSERVATION_ID] | scholar [ask QUESTION | show JOB_ID | review FINDING_ID accept|reject REASON] | scholar-once | review CANDIDATE_ID accept|reject REASON | migrate-knowledge | status]\n\
        Environment: COMMUNITY_MODEL_DIR, COMMUNITY_ADAPTER, COMMUNITY_DEVICE, COMMUNITY_STATE, COMMUNITY_KNOWLEDGE, COMMUNITY_DB"
    );
}
