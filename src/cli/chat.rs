use super::{Cli, ensure_no_arguments};
use crate::{agent::ReplyServices, knowledge::KnowledgeMap, maintenance, storage, web};
use std::{
    env,
    io::{self, Write},
    path::Path,
};

fn open_learning_store(database_path: &Path) -> Result<storage::LearningStore, String> {
    storage::LearningStore::open(database_path).map_err(|error| {
        format!(
            "could not open learning database {}: {error}",
            database_path.display()
        )
    })
}

impl Cli {
    pub(super) fn chat(&mut self, args: Vec<String>) -> Result<(), String> {
        self.ensure_chat("chat")?;
        let database_path = &self.database_path;
        let knowledge_path = &self.knowledge_path;
        let state_path = &self.state_path;
        let agent = self.agent.as_mut().expect("chat agent initialized");
        let map = self.map.as_mut().expect("knowledge map initialized");
        let runtime = self
            .model_runtime
            .as_ref()
            .expect("model runtime initialized");
        let chat_runtime = runtime.for_chat();
        ensure_no_arguments(&args, "chat takes no arguments")?;
        let store = open_learning_store(database_path)?;
        let mut models = chat_runtime.reply_models();
        let maintenance = maintenance::MaintenanceHandle::spawn(
            database_path.to_string_lossy().into_owned(),
            knowledge_path.to_string_lossy().into_owned(),
            runtime.clone(),
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
            let (interaction, services) = ReplyServices::split_models(
                &mut models,
                &store,
                database_path,
                Some(&maintenance),
                state_path,
            );
            let reply = agent
                .reply(
                    input,
                    interaction,
                    map,
                    services,
                    env::var_os("COMMUNITY_DEBUG_THINKING").is_some_and(|value| value == "1"),
                )
                .map_err(|e| e.to_string())?;
            println!("Community: {}", reply.text);
            if reply.research_queued {
                println!("[Scholar research requested for this question]");
            }
            if let Some(thinking) = reply.thinking {
                println!("[model thinking]\n{thinking}");
            }
        }
        drop(models);
        drop(chat_runtime);
        maintenance.stop();
        Ok(())
    }

    pub(super) fn serve(&mut self, args: Vec<String>) -> Result<(), String> {
        self.ensure_runtime("serve")?;
        let database_path = &self.database_path;
        ensure_no_arguments(
            &args,
            "serve takes no arguments; set COMMUNITY_WEB_ADDR to change the address",
        )?;
        let address = env::var("COMMUNITY_WEB_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:8080".into())
            .parse()
            .map_err(|error| format!("invalid COMMUNITY_WEB_ADDR: {error}"))?;
        web::serve(
            address,
            &database_path.to_string_lossy(),
            &self.knowledge_path.to_string_lossy(),
            self.model_runtime
                .as_ref()
                .expect("model runtime initialized")
                .clone(),
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub(super) fn say(&mut self, args: Vec<String>) -> Result<(), String> {
        self.ensure_chat("say")?;
        let database_path = &self.database_path;
        let state_path = &self.state_path;
        let agent = self.agent.as_mut().expect("chat agent initialized");
        let map = self.map.as_mut().expect("knowledge map initialized");
        let input = args.join(" ");
        if input.trim().is_empty() {
            return Err("say needs a message".into());
        }
        let store = open_learning_store(database_path)?;
        let chat_runtime = self
            .model_runtime
            .as_ref()
            .expect("model runtime initialized")
            .for_chat();
        let mut models = chat_runtime.reply_models();
        let (interaction, services) =
            ReplyServices::split_models(&mut models, &store, database_path, None, state_path);
        let reply = agent
            .reply(
                &input,
                interaction,
                map,
                services,
                env::var_os("COMMUNITY_DEBUG_THINKING").is_some_and(|value| value == "1"),
            )
            .map_err(|e| e.to_string())?;
        if env::var_os("COMMUNITY_WEB_REPLY").is_some() {
            println!(
                "{}",
                serde_json::json!({"reply": reply.text, "thinking": reply.thinking, "new_conversation": false, "research_queued": reply.research_queued})
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
