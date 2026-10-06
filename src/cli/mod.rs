mod chat;
mod knowledge;
mod model_ops;
mod workers;

use crate::{agent::Agent, knowledge::KnowledgeMap};
use std::{env, path::PathBuf};

struct Cli {
    model: String,
    model_dir: PathBuf,
    adapter_path: PathBuf,
    cpu: bool,
    state_path: PathBuf,
    knowledge_path: PathBuf,
    database_path: PathBuf,
    agent: Agent,
    map: KnowledgeMap,
}

pub fn run() -> Result<(), String> {
    let _ = dotenvy::dotenv();
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "chat".into());
    if matches!(command.as_str(), "help" | "--help" | "-h") {
        usage();
        return Ok(());
    }
    let model = env::var("COMMUNITY_MODEL_ID").unwrap_or_else(|_| "Qwen/Qwen3-0.6B".into());
    let model_dir = env::var_os("COMMUNITY_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("models/qwen3-0.6b"));
    let adapter_path = env::var_os("COMMUNITY_ADAPTER")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            if model_dir == std::path::Path::new("models/qwen3-0.6b") {
                PathBuf::from("state/adapter.safetensors")
            } else {
                model_dir.join("adapter.safetensors")
            }
        });
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
    let agent = Agent::load(&state_path, &model).map_err(|e| e.to_string())?;
    let map = KnowledgeMap::load(&knowledge_path).map_err(|e| e.to_string())?;
    let mut cli = Cli {
        model,
        model_dir,
        adapter_path,
        cpu,
        state_path,
        knowledge_path,
        database_path,
        agent,
        map,
    };
    cli.dispatch(&command, args.collect())
}

impl Cli {
    fn dispatch(&mut self, command: &str, args: Vec<String>) -> Result<(), String> {
        match command {
            "chat" => self.chat(args),
            "serve" => self.serve(args),
            "curate-once" => self.curate_once(args),
            "scout-once" => self.scout_once(args),
            "scout" => self.scout(args),
            "scholar-once" => self.scholar_once(args),
            "curmudgeon-once" => self.curmudgeon_once(args),
            "curmudgeon" => self.curmudgeon(args),
            "say" => self.say(args),
            "learn" => self.learn(args),
            "train" => self.train(args),
            "model" => self.model(args),
            "revise" => self.revise(args),
            "retract" => self.retract(args),
            "maintenance" => self.maintenance(args),
            "knowledge" => self.knowledge(args),
            "inbox" => self.inbox(args),
            "scholar" => self.scholar(args),
            "review" => self.review(args),
            "migrate-knowledge" => self.migrate_knowledge(args),
            "status" => self.status(args),
            _ => {
                usage();
                Err(format!("unknown command: {command}"))
            }
        }
    }
}

fn usage() {
    println!(
        "Usage: cargo run -- [chat | serve | say MESSAGE | train PROMPT ANSWER | model approve CLAIM_ID REASON | model consolidate CLAIM_ID | model history | model rebuild RUN_ID | model probe PROMPT | model probe-chat PROMPT | model probe-scholar QUESTION TITLE PASSAGE | model probe-review CONTEXT_JSON | model probe-support CONTEXT_JSON | learn SUBJECT RELATION OBJECT SOURCE | revise ID SUBJECT RELATION OBJECT SOURCE | retract ID SOURCE | knowledge | maintenance [runs [LIMIT] | history [LIMIT] | resolve CLAIM_ID confirm|challenge|qualify|retract REASON | supersede OLD_CLAIM_ID NEW_CLAIM_ID REASON | review CLAIM_ID active|uncertain|challenged [CHECKED_AT] [NEXT_REVIEW_AT] | schedule CLAIM_ID stable|slow_changing|fast_changing [NEXT_REVIEW_AT] | scope CLAIM_ID VALID_FROM|- VALID_UNTIL|-] | inbox [show OBSERVATION_ID] | scholar [ask QUESTION | queue | show JOB_ID | sites [LIMIT] | blocked [LIMIT] | review FINDING_ID accept|reject REASON] | scholar-once | curmudgeon [audit REPLY_TEXT] | curmudgeon-once | scout | scout-once | review CANDIDATE_ID accept|reject REASON | migrate-knowledge | status]\n\
        Environment: COMMUNITY_MLX_PYTHON, COMMUNITY_MODEL_ID, COMMUNITY_CONTEXT_TOKENS, COMMUNITY_MODEL_LOCK, COMMUNITY_MODEL_DIR, COMMUNITY_ADAPTER, COMMUNITY_DEVICE, COMMUNITY_STATE, COMMUNITY_KNOWLEDGE, COMMUNITY_DB, COMMUNITY_MAINTENANCE_INTERVAL_SECS, COMMUNITY_MAINTENANCE_MAX_MS, COMMUNITY_MAINTENANCE_INFERENCE_BUDGET, COMMUNITY_AUTONOMOUS_RESEARCH, COMMUNITY_MAX_RESEARCH_PER_DAY, COMMUNITY_SCOUT_MODEL_PROBES_PER_DAY, COMMUNITY_SCOUT_MIN_INTERVAL_SECONDS"
    );
}
