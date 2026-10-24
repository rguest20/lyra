use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::LocalModel;

#[derive(Clone)]
pub struct ModelRuntime {
    pool: Arc<Mutex<ModelPool>>,
    chat_pool: Arc<Mutex<ModelPool>>,
    chat_activity: Arc<ChatActivity>,
    chat_turn: Option<Arc<ChatTurn>>,
}

const CHAT_QUIET_WINDOW: Duration = Duration::from_secs(120);
const CHAT_ACTIVITY_POLL: Duration = Duration::from_millis(100);

#[derive(Default)]
struct ChatActivity {
    last_activity: Mutex<Option<Instant>>,
    active_turns: AtomicUsize,
    stop_background: std::sync::atomic::AtomicBool,
}

struct ChatTurn(Arc<ChatActivity>);

impl Drop for ChatTurn {
    fn drop(&mut self) {
        // A slow answer must not consume the user's entire quiet window.
        self.0.record();
        self.0.active_turns.fetch_sub(1, Ordering::SeqCst);
    }
}

impl ChatActivity {
    fn record(&self) {
        self.record_at(Instant::now());
    }

    fn record_at(&self, now: Instant) {
        *self
            .last_activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(now);
    }

    fn remaining(&self, now: Instant, quiet_window: Duration) -> Option<Duration> {
        if self.active_turns.load(Ordering::SeqCst) > 0 {
            return Some(quiet_window);
        }
        self.last_activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .and_then(|last_message| {
                let remaining =
                    quiet_window.saturating_sub(now.saturating_duration_since(last_message));
                (remaining > Duration::ZERO).then_some(remaining)
            })
    }
}

struct ModelPool {
    assignments: Assignments,
    cache: ModelCache<LocalModel>,
    inference_lock: PathBuf,
    loads: usize,
}

pub struct RoutedModel {
    runtime: ModelRuntime,
    role: AgentRole,
    pub(super) last_thinking: Option<String>,
}

pub struct ReplyModels {
    pub interaction: LocalModel,
    pub research: LocalModel,
    pub scholar: LocalModel,
}

impl ModelRuntime {
    pub fn new(default: ModelSpec) -> io::Result<Self> {
        let assignments = Assignments::from_environment(default)?;
        Ok(Self::with_assignments(assignments))
    }

    fn with_assignments(assignments: Assignments) -> Self {
        let mut background_lock = super::model_lease_path().into_os_string();
        background_lock.push(".requests");
        let mut chat_lock = super::model_lease_path().into_os_string();
        chat_lock.push(".chat.requests");
        Self {
            pool: Arc::new(Mutex::new(ModelPool::new(
                assignments.clone(),
                background_lock.into(),
            ))),
            chat_pool: Arc::new(Mutex::new(ModelPool::new(assignments, chat_lock.into()))),
            chat_activity: Arc::new(ChatActivity::default()),
            chat_turn: None,
        }
    }

    /// Keep every model needed by this chat turn in the foreground. Upkeep
    /// yields until the turn completes, then for a further two idle minutes.
    pub fn for_chat(&self) -> Self {
        self.chat_activity.record();
        self.chat_activity
            .active_turns
            .fetch_add(1, Ordering::SeqCst);
        Self {
            pool: self.chat_pool.clone(),
            chat_turn: Some(Arc::new(ChatTurn(self.chat_activity.clone()))),
            ..self.clone()
        }
    }

    fn chat_pause_remaining(&self, now: Instant) -> Option<Duration> {
        self.chat_turn
            .is_none()
            .then(|| self.chat_activity.remaining(now, CHAT_QUIET_WINDOW))
            .flatten()
    }

    fn wait_for_chat_quiet(&self, role: AgentRole) -> io::Result<Duration> {
        let started = Instant::now();
        let mut reported_wait = false;
        while !self.chat_activity.stop_background.load(Ordering::SeqCst)
            && let Some(remaining) = self.chat_pause_remaining(Instant::now())
        {
            if !reported_wait {
                crate::diagnostics::event(
                    "model.background_waiting_for_chat",
                    serde_json::json!({
                        "agent": role.name(),
                        "quiet_window_ms": remaining.as_millis(),
                    }),
                );
                reported_wait = true;
            }
            thread::sleep(remaining.min(CHAT_ACTIVITY_POLL));
        }
        if reported_wait {
            crate::diagnostics::event(
                "model.background_chat_wait_ended",
                serde_json::json!({
                    "agent": role.name(),
                    "reason": if self.chat_activity.stop_background.load(Ordering::SeqCst) {
                        "shutdown"
                    } else {
                        "quiet_window_elapsed"
                    },
                }),
            );
        }
        if self.chat_turn.is_none() && self.chat_activity.stop_background.load(Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "background inference stopped while waiting for chat",
            ));
        }
        Ok(started.elapsed())
    }

    pub fn model(&self, role: AgentRole) -> LocalModel {
        LocalModel::Shared(RoutedModel {
            runtime: self.clone(),
            role,
            last_thinking: None,
        })
    }

    pub fn reply_models(&self) -> ReplyModels {
        ReplyModels {
            interaction: self.model(AgentRole::Interaction),
            research: self.model(AgentRole::Curmudgeon),
            scholar: self.model(AgentRole::Scholar),
        }
    }

    pub fn model_id(&self, role: AgentRole) -> io::Result<String> {
        let pool = self
            .pool
            .lock()
            .map_err(|_| io::Error::other("model registry lock was poisoned"))?;
        Ok(pool.assignments.model(role).id.clone())
    }

    /// Load an agent's assigned backend now and retain it in this runtime's
    /// cache. This is useful for long-lived frontends that should pay model
    /// startup cost once, before serving the first request.
    pub fn warm(&self, role: AgentRole) -> io::Result<()> {
        self.model(role).is_consolidated().map(|_| ())
    }

    pub(crate) fn stop_background_waits(&self) {
        self.chat_activity
            .stop_background
            .store(true, Ordering::SeqCst);
    }
}

impl RoutedModel {
    pub(super) fn model_id(&self) -> io::Result<String> {
        self.runtime.model_id(self.role)
    }
    pub(super) fn with_model<Result>(
        &self,
        action: impl FnOnce(&mut LocalModel) -> io::Result<Result>,
    ) -> io::Result<Result> {
        let mut quiet_wait = Duration::ZERO;
        let mut pool_wait = Duration::ZERO;
        let mut lease_wait = Duration::ZERO;
        loop {
            quiet_wait += self.runtime.wait_for_chat_quiet(self.role)?;
            let pool_wait_started = Instant::now();
            let mut pool = self
                .runtime
                .pool
                .lock()
                .map_err(|_| io::Error::other("model registry lock was poisoned"))?;
            pool_wait += pool_wait_started.elapsed();
            // A chat turn can start while this upkeep caller waits for its
            // worker. Recheck the quiet window before occupying the lane.
            if self.runtime.chat_pause_remaining(Instant::now()).is_some() {
                drop(pool);
                continue;
            }
            let lease_wait_started = Instant::now();
            let _request_lease = super::acquire_model_lease(&pool.inference_lock)?;
            lease_wait += lease_wait_started.elapsed();
            crate::diagnostics::event(
                "model.request_wait",
                serde_json::json!({
                    "agent": self.role.name(),
                    "lane": if self.runtime.chat_turn.is_some() { "chat" } else { "background" },
                    "quiet_wait_ms": quiet_wait.as_millis(),
                    "pool_wait_ms": pool_wait.as_millis(),
                    "process_lease_wait_ms": lease_wait.as_millis(),
                }),
            );
            let spec = pool.assignments.model(self.role).clone();
            pool.loads += 1;
            let lease = pool.inference_lock.with_extension(format!(
                "resident-{}-{}",
                std::process::id(),
                pool.loads
            ));
            let model = pool.cache.get_or_load(&spec, || {
                let result = (|| {
                    let checkpoint = super::checkpoint_path(&spec.directory)?;
                    LocalModel::load_checkpoint_with_lease(
                        &spec.directory,
                        &checkpoint,
                        &spec.adapter,
                        spec.cpu,
                        &lease,
                    )
                })();
                let model = result.map_err(|error: io::Error| {
                    io::Error::new(
                        error.kind(),
                        format!(
                            "{} model {} ({}): {error}",
                            self.role.name(),
                            spec.id,
                            spec.directory.display()
                        ),
                    )
                })?;
                crate::diagnostics::event(
                    "agent.model_loaded",
                    serde_json::json!({
                        "agent": self.role.name(),
                        "model_id": spec.id,
                        "model_dir": spec.directory
                    }),
                );
                Ok(model)
            })?;
            return action(model);
        }
    }

    pub(super) fn generate(
        &mut self,
        messages: &[crate::agent::Message],
        extra_tokens: usize,
        cap: usize,
        temperature: f64,
        thinking: bool,
    ) -> io::Result<String> {
        self.last_thinking = None;
        let started = Instant::now();
        let result = self.with_model(|model| {
            let inference_started = Instant::now();
            let text =
                model.chat_with_reply_cap(messages, extra_tokens, cap, temperature, thinking)?;
            let inference_ms = inference_started.elapsed().as_millis();
            let tokens = match model {
                LocalModel::Candle(model) => model
                    .tokenizer
                    .encode(text.as_str(), false)
                    .ok()
                    .map(|encoding| encoding.len()),
                _ => None,
            };
            Ok((text, model.take_thinking(), tokens, inference_ms))
        });
        crate::diagnostics::event(
            "agent.inference",
            serde_json::json!({
                "agent": self.role.name(),
                "model_id": self.runtime.model_id(self.role)?,
                "elapsed_ms": started.elapsed().as_millis(),
                "inference_ms": result.as_ref().ok().map(|(_, _, _, duration)| *duration),
                "visible_output_tokens": result.as_ref().ok().and_then(|(_, _, tokens, _)| *tokens),
                "status": if result.is_ok() { "completed" } else { "error" },
            }),
        );
        let (text, thinking, _, _) = result?;
        self.last_thinking = thinking;
        Ok(text)
    }
}

impl ModelPool {
    fn new(assignments: Assignments, inference_lock: PathBuf) -> Self {
        Self {
            cache: ModelCache::new(assignments.capacity),
            assignments,
            inference_lock,
            loads: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AgentRole {
    Scout,
    Scholar,
    Curmudgeon,
    Archivist,
    Interaction,
}

impl AgentRole {
    pub fn name(self) -> &'static str {
        match self {
            Self::Scout => "scout",
            Self::Scholar => "scholar",
            Self::Curmudgeon => "curmudgeon",
            Self::Archivist => "archivist",
            Self::Interaction => "interaction",
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ModelSpec {
    pub id: String,
    pub directory: PathBuf,
    pub adapter: PathBuf,
    pub cpu: bool,
}

#[derive(Clone)]
pub struct Assignments {
    models: HashMap<AgentRole, ModelSpec>,
    pub capacity: usize,
}

impl Assignments {
    pub fn from_environment(default: ModelSpec) -> io::Result<Self> {
        Self::with_lookup(default, |name| std::env::var(name).ok())
    }

    fn with_lookup(
        default: ModelSpec,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> io::Result<Self> {
        let capacity = match lookup("COMMUNITY_MODEL_CACHE_SIZE") {
            Some(value) => value
                .parse::<usize>()
                .ok()
                .filter(|value| (1..=8).contains(value))
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "COMMUNITY_MODEL_CACHE_SIZE must be 1-8",
                    )
                })?,
            None => 1,
        };
        let mut models = HashMap::new();
        for role in [
            AgentRole::Scout,
            AgentRole::Scholar,
            AgentRole::Curmudgeon,
            AgentRole::Archivist,
            AgentRole::Interaction,
        ] {
            let prefix = format!("COMMUNITY_{}", role.name().to_ascii_uppercase());
            let directory = lookup(&format!("{prefix}_MODEL_DIR"));
            let id = lookup(&format!("{prefix}_MODEL_ID"));
            let adapter = lookup(&format!("{prefix}_ADAPTER"));
            for value in [&directory, &id, &adapter].into_iter().flatten() {
                if value.trim().is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{prefix} model settings must not be empty"),
                    ));
                }
            }
            if directory.is_none() && id.as_ref().is_some_and(|id| id != &default.id) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{prefix}_MODEL_ID requires {prefix}_MODEL_DIR for a different model"),
                ));
            }
            let mut spec = default.clone();
            if let Some(directory) = directory {
                spec.directory = PathBuf::from(directory);
                if spec.directory != default.directory {
                    spec.adapter = spec.directory.join("adapter.safetensors");
                    spec.id = spec.directory.to_string_lossy().into_owned();
                }
            }
            if let Some(id) = id {
                spec.id = id;
            }
            if let Some(adapter) = adapter {
                spec.adapter = PathBuf::from(adapter);
            }
            models.insert(role, spec);
        }
        Ok(Self { models, capacity })
    }

    pub fn model(&self, role: AgentRole) -> &ModelSpec {
        &self.models[&role]
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ModelKey {
    directory: PathBuf,
    adapter: PathBuf,
    cpu: bool,
}

impl From<&ModelSpec> for ModelKey {
    fn from(spec: &ModelSpec) -> Self {
        Self {
            directory: spec
                .directory
                .canonicalize()
                .unwrap_or_else(|_| spec.directory.clone()),
            adapter: spec.adapter.canonicalize().unwrap_or_else(|_| {
                spec.adapter
                    .parent()
                    .and_then(|parent| parent.canonicalize().ok())
                    .map_or_else(
                        || spec.adapter.clone(),
                        |parent| parent.join(spec.adapter.file_name().unwrap_or_default()),
                    )
            }),
            cpu: spec.cpu,
        }
    }
}

pub(super) struct ModelCache<Model> {
    entries: HashMap<ModelKey, Model>,
    recent: Vec<ModelKey>,
    capacity: usize,
}

impl<Model> ModelCache<Model> {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            recent: Vec::new(),
            capacity,
        }
    }

    pub(super) fn get_or_load(
        &mut self,
        spec: &ModelSpec,
        load: impl FnOnce() -> io::Result<Model>,
    ) -> io::Result<&mut Model> {
        let key = ModelKey::from(spec);
        if !self.entries.contains_key(&key) {
            if self.entries.len() >= self.capacity {
                let oldest = self.recent.remove(0);
                self.entries.remove(&oldest);
            }
            self.entries.insert(key.clone(), load()?);
        }
        self.recent.retain(|existing| existing != &key);
        self.recent.push(key.clone());
        Ok(self.entries.get_mut(&key).unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    fn default_model() -> ModelSpec {
        ModelSpec {
            id: "small".into(),
            directory: "models/small".into(),
            // Other tests may create `state` while this fixture is cached.
            // Use an existing parent so canonicalization stays stable.
            adapter: std::env::temp_dir().join(format!(
                "lyra-cache-test-{}.safetensors",
                std::process::id()
            )),
            cpu: false,
        }
    }

    #[test]
    fn defaults_and_agent_overrides() {
        let defaults = Assignments::with_lookup(default_model(), |_| None).unwrap();
        assert_eq!(defaults.model(AgentRole::Interaction), &default_model());
        assert_eq!(defaults.model(AgentRole::Curmudgeon), &default_model());
        assert_eq!(defaults.capacity, 1);
        let configured = Assignments::with_lookup(default_model(), |key| match key {
            "COMMUNITY_CURMUDGEON_MODEL_DIR" => Some("models/large".into()),
            "COMMUNITY_CURMUDGEON_MODEL_ID" => Some("large".into()),
            "COMMUNITY_MODEL_CACHE_SIZE" => Some("2".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(configured.model(AgentRole::Scholar), &default_model());
        assert_eq!(configured.model(AgentRole::Curmudgeon).id, "large");
        assert_eq!(
            configured.model(AgentRole::Curmudgeon).adapter,
            PathBuf::from("models/large/adapter.safetensors")
        );
        assert_eq!(configured.capacity, 2);
    }

    #[test]
    fn invalid_assignments_are_errors() {
        for (key, value) in [
            ("COMMUNITY_MODEL_CACHE_SIZE", "0"),
            ("COMMUNITY_MODEL_CACHE_SIZE", "bad"),
            ("COMMUNITY_SCHOLAR_MODEL_DIR", ""),
            ("COMMUNITY_SCOUT_MODEL_ID", "other"),
        ] {
            assert!(
                Assignments::with_lookup(default_model(), |name| (name == key)
                    .then(|| value.into()))
                .is_err()
            );
        }
    }

    #[test]
    fn identical_models_are_reused_and_missing_models_are_retryable() {
        let mut cache = ModelCache::new(2);
        let spec = default_model();
        assert!(
            cache
                .get_or_load(&spec, || Err::<usize, _>(io::Error::new(
                    io::ErrorKind::NotFound,
                    "missing checkpoint"
                )))
                .is_err()
        );
        assert_eq!(*cache.get_or_load(&spec, || Ok(7)).unwrap(), 7);
        let mut alias = spec.clone();
        alias.id = "another label".into();
        assert_eq!(
            *cache
                .get_or_load(&alias, || panic!("duplicate load"))
                .unwrap(),
            7
        );
        let mut distinct = spec.clone();
        distinct.adapter = "state/different.safetensors".into();
        assert_eq!(*cache.get_or_load(&distinct, || Ok(9)).unwrap(), 9);
    }

    #[test]
    fn eviction_and_shutdown_release_models() {
        struct Model(Rc<Cell<usize>>);
        impl Drop for Model {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let drops = Rc::new(Cell::new(0));
        let mut cache = ModelCache::new(1);
        let spec = default_model();
        cache
            .get_or_load(&spec, || Ok(Model(drops.clone())))
            .unwrap();
        let mut other = spec.clone();
        other.directory = "models/other".into();
        cache
            .get_or_load(&other, || Ok(Model(drops.clone())))
            .unwrap();
        assert_eq!(drops.get(), 1);
        drop(cache);
        assert_eq!(drops.get(), 2);
    }

    #[test]
    fn two_resident_models_survive_repeated_role_switches() {
        let small = default_model();
        let mut large = small.clone();
        large.directory = "models/large".into();
        let mut cache = ModelCache::new(2);
        cache.get_or_load(&small, || Ok(1)).unwrap();
        cache.get_or_load(&large, || Ok(2)).unwrap();
        for _ in 0..3 {
            assert_eq!(
                *cache
                    .get_or_load(&small, || panic!("small reloaded"))
                    .unwrap(),
                1
            );
            assert_eq!(
                *cache
                    .get_or_load(&large, || panic!("large reloaded"))
                    .unwrap(),
                2
            );
        }
        let mut third = small.clone();
        third.directory = "models/third".into();
        cache.get_or_load(&third, || Ok(3)).unwrap();
        assert_eq!(
            *cache
                .get_or_load(&large, || panic!("most recent model evicted"))
                .unwrap(),
            2
        );
    }

    #[test]
    fn shared_handles_are_lazy_and_release_the_registry_on_shutdown() {
        let assignments = Assignments::with_lookup(default_model(), |_| None).unwrap();
        let runtime = ModelRuntime::with_assignments(assignments);
        let weak = Arc::downgrade(&runtime.pool);
        let scout = runtime.model(AgentRole::Scout);
        let scholar = runtime.model(AgentRole::Scholar);
        assert!(runtime.pool.lock().unwrap().cache.entries.is_empty());
        drop(runtime);
        assert!(weak.upgrade().is_some());
        drop(scout);
        drop(scholar);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn each_chat_message_extends_the_background_quiet_window() {
        let activity = ChatActivity::default();
        let started = Instant::now();
        assert_eq!(activity.remaining(started, CHAT_QUIET_WINDOW), None);

        activity.record_at(started);
        assert_eq!(
            activity.remaining(started + Duration::from_secs(119), CHAT_QUIET_WINDOW),
            Some(Duration::from_secs(1))
        );

        let follow_up = started + Duration::from_secs(60);
        activity.record_at(follow_up);
        assert_eq!(
            activity.remaining(started + Duration::from_secs(179), CHAT_QUIET_WINDOW),
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            activity.remaining(started + Duration::from_secs(180), CHAT_QUIET_WINDOW),
            None
        );
    }

    #[test]
    fn chat_review_and_research_bypass_quiet_window_but_upkeep_does_not() {
        let assignments = Assignments::with_lookup(default_model(), |_| None).unwrap();
        let runtime = ModelRuntime::with_assignments(assignments);
        let chat = runtime.for_chat();
        let now = Instant::now();
        runtime.chat_activity.record_at(now);

        assert!(Arc::ptr_eq(&chat.pool, &runtime.chat_pool));
        assert!(!Arc::ptr_eq(&chat.pool, &runtime.pool));
        assert_ne!(
            chat.pool.lock().unwrap().inference_lock,
            runtime.pool.lock().unwrap().inference_lock,
            "foreground and upkeep must not share the process request lock"
        );
        for role in [
            AgentRole::Curmudgeon,
            AgentRole::Scholar,
            AgentRole::Interaction,
        ] {
            let LocalModel::Shared(chat_model) = chat.model(role) else {
                panic!("expected shared handle")
            };
            assert_eq!(
                chat_model.runtime.chat_pause_remaining(now),
                None,
                "{role:?} serving chat must proceed immediately"
            );
            assert_eq!(
                runtime.chat_pause_remaining(now),
                Some(CHAT_QUIET_WINDOW),
                "{role:?} doing upkeep must yield to chat"
            );
        }
    }

    #[test]
    fn foreground_and_upkeep_request_leases_can_be_held_at_once() {
        let runtime = ModelRuntime::with_assignments(
            Assignments::with_lookup(default_model(), |_| None).unwrap(),
        );
        let lease_paths = crate::test_support::unique_temp_path("lyra-request-leases", "");
        let background_path = lease_paths.with_extension("background.lock");
        let chat_path = lease_paths.with_extension("chat.lock");
        runtime.pool.lock().unwrap().inference_lock = background_path.clone();
        runtime.chat_pool.lock().unwrap().inference_lock = chat_path.clone();

        let background_lease = super::super::acquire_model_lease(&background_path).unwrap();
        let chat_lease = super::super::acquire_model_lease(&chat_path)
            .expect("chat must acquire its lease while upkeep is using its own worker");
        drop(chat_lease);
        drop(background_lease);
        let _ = std::fs::remove_file(background_path);
        let _ = std::fs::remove_file(chat_path);
    }

    #[test]
    fn slow_chat_and_its_cloned_models_keep_upkeep_paused_until_completion() {
        let runtime = ModelRuntime::with_assignments(
            Assignments::with_lookup(default_model(), |_| None).unwrap(),
        );
        let chat = runtime.for_chat();
        let model = chat.model(AgentRole::Curmudgeon);
        drop(chat);
        let now = Instant::now();
        runtime.chat_activity.record_at(now - CHAT_QUIET_WINDOW);
        assert_eq!(runtime.chat_pause_remaining(now), Some(CHAT_QUIET_WINDOW));
        drop(model);
        // Completing the last foreground handle starts a fresh idle window.
        assert!(runtime.chat_pause_remaining(Instant::now()).is_some());
        runtime
            .chat_activity
            .record_at(Instant::now() - CHAT_QUIET_WINDOW);
        assert_eq!(runtime.chat_pause_remaining(Instant::now()), None);
    }

    #[test]
    fn shutdown_interrupts_background_waiting_for_the_chat_quiet_window() {
        let runtime = ModelRuntime::with_assignments(
            Assignments::with_lookup(default_model(), |_| None).unwrap(),
        );
        let chat = runtime.for_chat();
        let LocalModel::Shared(background) = runtime.model(AgentRole::Curmudgeon) else {
            panic!("expected shared handle")
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = thread::spawn(move || {
            tx.send(background.with_model(|_| Ok(()))).unwrap();
        });
        thread::sleep(Duration::from_millis(30));
        runtime.stop_background_waits();
        let error = rx
            .recv_timeout(Duration::from_secs(1))
            .expect("shutdown must release a quiet-window wait")
            .unwrap_err();
        thread.join().unwrap();
        drop(chat);
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    }

    #[test]
    fn paused_upkeep_leaves_the_pool_available_for_chat_review_and_research() {
        // Missing checkpoints exercise runtime scheduling without loading a
        // model or competing with a running application for its request lock.
        let mut spec = default_model();
        spec.directory = crate::test_support::unique_temp_path("lyra-chat-priority-absent", "");
        let runtime =
            ModelRuntime::with_assignments(Assignments::with_lookup(spec, |_| None).unwrap());
        let lock = crate::test_support::unique_temp_path("lyra-chat-priority-requests", "lock");
        runtime.pool.lock().unwrap().inference_lock = lock.clone();
        let chat_lock = lock.with_extension("chat.lock");
        runtime.chat_pool.lock().unwrap().inference_lock = chat_lock.clone();
        let chat = runtime.for_chat();
        let LocalModel::Shared(background) = runtime.model(AgentRole::Curmudgeon) else {
            panic!("expected shared handle")
        };
        let (background_tx, background_rx) = std::sync::mpsc::channel();
        let background_thread = thread::spawn(move || {
            let result = background.with_model(|_| Ok(()));
            background_tx.send(result).unwrap();
        });
        let background_paused = matches!(
            background_rx.recv_timeout(Duration::from_millis(100)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        );
        let (chat_tx, chat_rx) = std::sync::mpsc::channel();
        let chat_thread = thread::spawn(move || {
            let results = [AgentRole::Curmudgeon, AgentRole::Scholar].map(|role| {
                let LocalModel::Shared(model) = chat.model(role) else {
                    panic!("expected shared handle")
                };
                model.with_model(|_| Ok(())).unwrap_err().to_string()
            });
            chat_tx.send(results).unwrap();
        });
        let errors = chat_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("chat models were delayed by the quiet window");
        chat_thread.join().unwrap();
        // Expire the idle window after the foreground handles are released.
        runtime
            .chat_activity
            .record_at(Instant::now() - CHAT_QUIET_WINDOW);
        background_thread.join().unwrap();
        let _ = std::fs::remove_file(lock);
        let _ = std::fs::remove_file(chat_lock);
        assert!(background_paused, "upkeep started during the quiet window");
        assert!(errors[0].contains("curmudgeon model"));
        assert!(errors[1].contains("scholar model"));
    }

    #[test]
    fn missing_role_checkpoint_does_not_load_a_fallback_model() {
        let mut spec = default_model();
        spec.directory = crate::test_support::unique_temp_path("lyra-absent-checkpoint", "");
        let assignments = Assignments::with_lookup(spec, |_| None).unwrap();
        let runtime = ModelRuntime::with_assignments(assignments);
        // This missing-file test never performs inference. Give it its own
        // request lock so a live resident model cannot delay the assertion.
        let request_lock =
            crate::test_support::unique_temp_path("lyra-missing-checkpoint-requests", "lock");
        runtime.pool.lock().unwrap().inference_lock = request_lock.clone();
        let LocalModel::Shared(model) = runtime.model(AgentRole::Curmudgeon) else {
            panic!("expected shared handle")
        };
        let error = model.with_model(|_| Ok(())).unwrap_err();
        assert!(error.to_string().contains("curmudgeon model"));
        assert!(runtime.pool.lock().unwrap().cache.entries.is_empty());
        let _ = std::fs::remove_file(request_lock);
    }
}
