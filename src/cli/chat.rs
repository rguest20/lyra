use super::Cli;
use crate::{
    agent::ReplyServices, curator, inference::LocalModel, knowledge::KnowledgeMap, storage, web,
};
use std::{
    env,
    io::{self, Write},
    sync::{Arc, Mutex},
};

impl Cli {
    pub(super) fn chat(&mut self, args: Vec<String>) -> Result<(), String> {
        let mut args = args.into_iter();
        let model_dir = &self.model_dir;
        let adapter_path = &self.adapter_path;
        let cpu = self.cpu;
        let database_path = &self.database_path;
        let knowledge_path = &self.knowledge_path;
        let state_path = &self.state_path;
        let agent = &mut self.agent;
        let map = &mut self.map;
        if args.next().is_some() {
            return Err("chat takes no arguments".into());
        }
        let store = storage::LearningStore::open(database_path).map_err(|e| {
            format!(
                "could not open learning database {}: {e}",
                database_path.display()
            )
        })?;
        let backend = Arc::new(Mutex::new(
            LocalModel::load(model_dir, adapter_path, cpu)
                .map_err(|e| format!("could not load local model: {e}"))?,
        ));
        let curator = curator::CuratorHandle::spawn(
            database_path.to_string_lossy().into_owned(),
            knowledge_path.to_string_lossy().into_owned(),
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
            *map = KnowledgeMap::load(knowledge_path).map_err(|e| e.to_string())?;
            let mut model_guard = backend
                .lock()
                .map_err(|_| "local model lock was poisoned".to_owned())?;
            let reply = agent
                .reply(
                    input,
                    &mut model_guard,
                    map,
                    ReplyServices {
                        store: &store,
                        curator: Some(&curator),
                        state_path,
                    },
                    env::var_os("COMMUNITY_DEBUG_THINKING").is_some_and(|value| value == "1"),
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
        Ok(())
    }

    pub(super) fn serve(&mut self, args: Vec<String>) -> Result<(), String> {
        let mut args = args.into_iter();
        let database_path = &self.database_path;
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
        Ok(())
    }

    pub(super) fn say(&mut self, args: Vec<String>) -> Result<(), String> {
        let args = args.into_iter();
        let model_dir = &self.model_dir;
        let adapter_path = &self.adapter_path;
        let cpu = self.cpu;
        let database_path = &self.database_path;
        let state_path = &self.state_path;
        let agent = &mut self.agent;
        let map = &mut self.map;
        let input = args.collect::<Vec<_>>().join(" ");
        if input.trim().is_empty() {
            return Err("say needs a message".into());
        }
        let store = storage::LearningStore::open(database_path).map_err(|e| {
            format!(
                "could not open learning database {}: {e}",
                database_path.display()
            )
        })?;
        let mut backend = LocalModel::load(model_dir, adapter_path, cpu)
            .map_err(|e| format!("could not load local model: {e}"))?;
        let reply = agent
            .reply(
                &input,
                &mut backend,
                map,
                ReplyServices {
                    store: &store,
                    curator: None,
                    state_path,
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
        Ok(())
    }
}
