mod chat;
mod evaluation;
mod knowledge;
mod model_ops;
mod workers;

use crate::inference::assignments::{AgentRole, ModelRuntime, ModelSpec};
use crate::{agent::Agent, knowledge::KnowledgeMap};
use std::{env, path::PathBuf};

fn configured_path(variable: &str, default: impl FnOnce() -> PathBuf) -> PathBuf {
    env::var_os(variable).map_or_else(default, PathBuf::from)
}

fn ensure_no_arguments(args: &[String], error: &str) -> Result<(), String> {
    if !args.is_empty() {
        Err(error.into())
    } else {
        Ok(())
    }
}

struct Cli {
    model: String,
    model_dir: PathBuf,
    adapter_path: PathBuf,
    cpu: bool,
    state_path: PathBuf,
    knowledge_path: PathBuf,
    database_path: PathBuf,
    model_runtime: Option<ModelRuntime>,
    agent: Option<Agent>,
    map: Option<KnowledgeMap>,
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
    let model_dir = configured_path("COMMUNITY_MODEL_DIR", || PathBuf::from("models/qwen3-0.6b"));
    let adapter_path = configured_path("COMMUNITY_ADAPTER", || {
        if model_dir == std::path::Path::new("models/qwen3-0.6b") {
            PathBuf::from("state/adapter.safetensors")
        } else {
            model_dir.join("adapter.safetensors")
        }
    });
    let cpu = env::var("COMMUNITY_DEVICE").is_ok_and(|value| value == "cpu");
    let state_path = configured_path("COMMUNITY_STATE", || {
        PathBuf::from("state/agent-qwen3-0.6b.json")
    });
    let knowledge_path = configured_path("COMMUNITY_KNOWLEDGE", || {
        PathBuf::from("state/knowledge.json")
    });
    let database_path = configured_path("COMMUNITY_DB", || PathBuf::from("state/learning.db"));
    let mut cli = Cli {
        model,
        model_dir,
        adapter_path,
        cpu,
        state_path,
        knowledge_path,
        database_path,
        model_runtime: None,
        agent: None,
        map: None,
    };
    cli.dispatch(&command, args.collect())
}

impl Cli {
    fn ensure_runtime(&mut self, command: &str) -> Result<(), String> {
        if self.model_runtime.is_some() {
            return Ok(());
        }
        let runtime = ModelRuntime::new(ModelSpec {
            id: self.model.clone(),
            directory: self.model_dir.clone(),
            adapter: self.adapter_path.clone(),
            cpu: self.cpu,
        })
        .map_err(|error| {
            let reason = format!("invalid agent model configuration: {error}");
            if command == "curate-once" {
                let mode = if env::var("COMMUNITY_MAINTENANCE_MODE").as_deref() == Ok("serve") {
                    "serve"
                } else {
                    "once"
                };
                if let Err(record_error) =
                    crate::maintenance::record_startup_failure(&self.database_path, mode, &reason)
                {
                    return format!("{reason}; could not record failed pass: {record_error}");
                }
            }
            reason
        })?;
        self.model_runtime = Some(runtime);
        Ok(())
    }

    fn ensure_map(&mut self) -> Result<(), String> {
        if self.map.is_none() {
            self.map =
                Some(KnowledgeMap::load(&self.knowledge_path).map_err(|error| error.to_string())?);
        }
        Ok(())
    }

    fn ensure_chat(&mut self, command: &str) -> Result<(), String> {
        self.ensure_runtime(command)?;
        if self.agent.is_none() {
            let runtime = self.model_runtime.as_ref().expect("runtime initialized");
            let interaction_model = runtime
                .model_id(AgentRole::Interaction)
                .map_err(|error| error.to_string())?;
            self.agent = Some(
                Agent::load_for_model_change(&self.state_path, &interaction_model)
                    .map_err(|error| error.to_string())?,
            );
        }
        self.ensure_map()
    }

    fn dispatch(&mut self, command: &str, args: Vec<String>) -> Result<(), String> {
        match command {
            "eval-fixture" => self.eval_fixture(args),
            "eval-judge" => self.eval_judge(args),
            "eval-lifecycle" => self.eval_lifecycle(args),
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
            "train" => Err("unrestricted prompt/answer training is disabled; approve a provenance-complete stable claim with `model approve`, then use `model consolidate`".into()),
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
        "Usage: cargo run -- [chat | serve | say MESSAGE | model approve CLAIM_ID REASON | model consolidate CLAIM_ID | model history | model rebuild RUN_ID | model probe PROMPT | model probe-chat PROMPT | model probe-scholar QUESTION TITLE PASSAGE | model probe-baskets TITLE PASSAGE | model probe-review CONTEXT_JSON | model probe-support CONTEXT_JSON | learn SUBJECT RELATION OBJECT SOURCE | revise ID SUBJECT RELATION OBJECT SOURCE | retract ID SOURCE | knowledge | maintenance [runs [LIMIT] | history [LIMIT] | resolve CLAIM_ID confirm|challenge|qualify|retract REASON | supersede OLD_CLAIM_ID NEW_CLAIM_ID REASON | review CLAIM_ID active|uncertain|challenged [CHECKED_AT] [NEXT_REVIEW_AT] | schedule CLAIM_ID stable|slow_changing|fast_changing [NEXT_REVIEW_AT] | scope CLAIM_ID VALID_FROM|- VALID_UNTIL|-] | inbox [show OBSERVATION_ID] | scholar [ask QUESTION | queue | show JOB_ID | sites [LIMIT] | blocked [LIMIT] | review FINDING_ID accept|reject REASON] | scholar-once | curmudgeon [audit REPLY_TEXT] | curmudgeon-once | scout | scout-once | review CANDIDATE_ID accept|reject REASON | migrate-knowledge | status]\n\
        Environment: COMMUNITY_MLX_PYTHON, COMMUNITY_MODEL_ID, COMMUNITY_CONTEXT_TOKENS, COMMUNITY_MODEL_LOCK, COMMUNITY_CHAT_MODEL_LOCK, COMMUNITY_MODEL_CACHE_SIZE, COMMUNITY_<AGENT>_MODEL_DIR, COMMUNITY_<AGENT>_MODEL_ID, COMMUNITY_<AGENT>_ADAPTER (SCOUT, SCHOLAR, CURMUDGEON, ARCHIVIST, INTERACTION), COMMUNITY_MODEL_DIR, COMMUNITY_ADAPTER, COMMUNITY_DEVICE, COMMUNITY_STATE, COMMUNITY_KNOWLEDGE, COMMUNITY_DB, COMMUNITY_MAINTENANCE_INTERVAL_SECS, COMMUNITY_MAINTENANCE_MAX_MS, COMMUNITY_MAINTENANCE_INFERENCE_BUDGET, COMMUNITY_AUTONOMOUS_RESEARCH, COMMUNITY_MAX_RESEARCH_PER_DAY, COMMUNITY_SCOUT_MODEL_PROBES_PER_DAY, COMMUNITY_SCOUT_MIN_INTERVAL_SECONDS, COMMUNITY_MODEL_TRAINING_READINESS (requires learning-quality-v1-passed)"
    );
    println!(
        "Research: scholar usage | scholar baskets [claims | questions | sources | claim ID | source ID | retry SOURCE_ID | import]"
    );
    println!(
        "Offline evaluation: eval-fixture INPUT_JSON NEW_OUTPUT_DIRECTORY | eval-judge BLINDED_REVIEW_PACKET_JSON | eval-lifecycle CONTROLLED_FIXTURE_PIPELINE_DIRECTORY"
    );
}
