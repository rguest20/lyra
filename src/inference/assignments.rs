use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::LocalModel;

#[derive(Clone)]
pub struct ModelRuntime {
    pool: Arc<Mutex<ModelPool>>,
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

impl ModelRuntime {
    pub fn new(default: ModelSpec) -> io::Result<Self> {
        let assignments = Assignments::from_environment(default)?;
        Ok(Self::with_assignments(assignments))
    }

    fn with_assignments(assignments: Assignments) -> Self {
        let mut lock = super::model_lease_path().into_os_string();
        lock.push(".requests");
        Self {
            pool: Arc::new(Mutex::new(ModelPool {
                cache: ModelCache::new(assignments.capacity),
                assignments,
                inference_lock: lock.into(),
                loads: 0,
            })),
        }
    }

    pub fn model(&self, role: AgentRole) -> LocalModel {
        LocalModel::Shared(RoutedModel {
            runtime: self.clone(),
            role,
            last_thinking: None,
        })
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
}

impl RoutedModel {
    pub(super) fn model_id(&self) -> io::Result<String> {
        self.runtime.model_id(self.role)
    }
    pub(super) fn with_model<Result>(
        &self,
        action: impl FnOnce(&mut LocalModel) -> io::Result<Result>,
    ) -> io::Result<Result> {
        let mut pool = self
            .runtime
            .pool
            .lock()
            .map_err(|_| io::Error::other("model registry lock was poisoned"))?;
        let _request_lease = super::acquire_model_lease(&pool.inference_lock)?;
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
                LocalModel::load_checkpoint_with_lease(&spec.directory, &checkpoint, &spec.adapter, spec.cpu, &lease)
            })();
            let model = result.map_err(|error: io::Error| io::Error::new(error.kind(), format!("{} model {} ({}): {error}", self.role.name(), spec.id, spec.directory.display())))?;
            crate::diagnostics::event("agent.model_loaded", serde_json::json!({"agent": self.role.name(), "model_id": spec.id, "model_dir": spec.directory}));
            Ok(model)
        })?;
        action(model)
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
                    .map(|parent| parent.join(spec.adapter.file_name().unwrap_or_default()))
                    .unwrap_or_else(|| spec.adapter.clone())
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
            adapter: "state/small.safetensors".into(),
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
    fn missing_role_checkpoint_does_not_load_a_fallback_model() {
        let mut spec = default_model();
        spec.directory =
            std::env::temp_dir().join(format!("lyra-absent-checkpoint-{}", std::process::id()));
        let assignments = Assignments::with_lookup(spec, |_| None).unwrap();
        let runtime = ModelRuntime::with_assignments(assignments);
        let LocalModel::Shared(model) = runtime.model(AgentRole::Curmudgeon) else {
            panic!("expected shared handle")
        };
        let error = model.with_model(|_| Ok(())).unwrap_err();
        assert!(error.to_string().contains("curmudgeon model"));
        assert!(runtime.pool.lock().unwrap().cache.entries.is_empty());
    }
}
