//! Runs one bounded, recorded maintenance pass in either long-lived mode.

use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::curator;
use crate::curmudgeon;
use crate::inference::assignments::{AgentRole, ModelRuntime};
use crate::scholar;
use crate::scout;
use crate::storage::LearningStore;

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct PassSummary {
    pub curated: usize,
    pub audited: usize,
    pub scouted: usize,
    pub researched: usize,
    pub errors: usize,
}

impl PassSummary {
    pub fn did_work(&self) -> bool {
        self.curated + self.audited + self.scouted + self.researched > 0
    }
}

pub fn interval() -> Duration {
    let seconds = std::env::var("COMMUNITY_MAINTENANCE_INTERVAL_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| (1..=3600).contains(value))
        .unwrap_or(20);
    Duration::from_secs(seconds)
}

fn max_pass_duration() -> Duration {
    let milliseconds = std::env::var("COMMUNITY_MAINTENANCE_MAX_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| (1_000..=600_000).contains(value))
        .unwrap_or(120_000);
    Duration::from_millis(milliseconds)
}

fn inference_budget() -> usize {
    std::env::var("COMMUNITY_MAINTENANCE_INFERENCE_BUDGET")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| (1..=16).contains(value))
        .unwrap_or(8)
}

fn autonomous_research_enabled() -> bool {
    !std::env::var("COMMUNITY_AUTONOMOUS_RESEARCH").is_ok_and(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        )
    })
}

struct RoleResult {
    processed: usize,
    error: bool,
}

struct PassConfig {
    max_duration: Duration,
    inference_budget: usize,
    research_enabled: bool,
}

trait Roles {
    fn curate(&mut self, limit: usize) -> io::Result<usize>;
    fn audit(&mut self) -> io::Result<usize>;
    fn scout(&mut self) -> io::Result<usize>;
    fn research(&mut self) -> io::Result<usize>;
}

struct ModelRoles<'a> {
    database_path: &'a Path,
    knowledge_path: &'a Path,
    models: &'a ModelRuntime,
}

impl Roles for ModelRoles<'_> {
    fn curate(&mut self, limit: usize) -> io::Result<usize> {
        curator::process_once_limited(
            &self.database_path.to_string_lossy(),
            &mut self.models.model(AgentRole::Archivist),
            limit,
        )
    }

    fn audit(&mut self) -> io::Result<usize> {
        curmudgeon::process_once(
            self.database_path,
            self.knowledge_path,
            &mut self.models.model(AgentRole::Curmudgeon),
        )
    }

    fn scout(&mut self) -> io::Result<usize> {
        scout::process_once(
            self.database_path,
            self.knowledge_path,
            &mut self.models.model(AgentRole::Scout),
        )
    }

    fn research(&mut self) -> io::Result<usize> {
        scholar::process_once(
            self.database_path,
            &mut self.models.model(AgentRole::Scholar),
        )
    }
}

struct RoleOutcome {
    status: &'static str,
    processed: usize,
    detail: Option<String>,
    error: Option<String>,
}

fn role_outcome(result: io::Result<usize>, review_delta: bool) -> RoleOutcome {
    match result {
        Ok(processed) => RoleOutcome {
            status: if processed == 0 { "idle" } else { "work" },
            processed,
            detail: (processed > 0 && review_delta)
                .then(|| "items queued for AI review or corroboration".to_owned()),
            error: None,
        },
        Err(error) => RoleOutcome {
            status: "error",
            processed: 0,
            detail: None,
            error: Some(error.to_string()),
        },
    }
}

fn run_role(
    store: &LearningStore,
    pass_id: i64,
    role: &str,
    action: impl FnOnce() -> io::Result<usize>,
) -> io::Result<RoleResult> {
    let before = store.maintenance_review_counts()?;
    let started = Instant::now();
    let result = action();
    let after = store.maintenance_review_counts()?;
    let review_delta = match role {
        "curator" => after.0 > before.0,
        "scholar" => after.1 > before.1,
        _ => false,
    };
    let RoleOutcome {
        status,
        processed,
        detail,
        error,
    } = role_outcome(result, review_delta);
    crate::diagnostics::event(
        "maintenance.role_finished",
        serde_json::json!({
            "pass_id": pass_id,
            "role": role,
            "status": status,
            "processed": processed,
            "elapsed_ms": started.elapsed().as_millis(),
            "review_counts_before": {"curator": before.0, "scholar": before.1},
            "review_counts_after": {"curator": after.0, "scholar": after.1},
            "detail": detail,
            "error": error,
        }),
    );
    store.record_maintenance_role(
        pass_id,
        role,
        status,
        processed,
        error.as_deref().or(detail.as_deref()),
        started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
    )?;
    if let Some(error) = &error {
        eprintln!("{role} maintenance failed: {error}");
    }
    Ok(RoleResult {
        processed,
        error: status == "error",
    })
}

fn skip_role(store: &LearningStore, pass_id: i64, role: &str, reason: &str) -> io::Result<()> {
    store.record_maintenance_role(pass_id, role, "skipped", 0, Some(reason), 0)
}

pub fn run_pass(
    database_path: &Path,
    knowledge_path: &Path,
    models: &ModelRuntime,
    mode: &str,
) -> io::Result<PassSummary> {
    let mut roles = ModelRoles {
        database_path,
        knowledge_path,
        models,
    };
    run_pass_with_roles(
        database_path,
        mode,
        &mut roles,
        PassConfig {
            max_duration: max_pass_duration(),
            inference_budget: inference_budget(),
            research_enabled: autonomous_research_enabled(),
        },
    )
}

pub fn record_startup_failure(database_path: &Path, mode: &str, reason: &str) -> io::Result<()> {
    let mut store = LearningStore::open(database_path)?;
    let pass_id = store.start_maintenance_pass(mode)?;
    store.record_maintenance_role(pass_id, "curator", "error", 0, Some(reason), 0)?;
    for role in ["curmudgeon", "scout", "scholar"] {
        skip_role(&store, pass_id, role, "model failed to load")?;
    }
    store.finish_maintenance_pass(pass_id)
}

fn run_pass_with_roles(
    database_path: &Path,
    mode: &str,
    roles: &mut impl Roles,
    config: PassConfig,
) -> io::Result<PassSummary> {
    let mut store = LearningStore::open(database_path)?;
    let pass_id = store.start_maintenance_pass(mode)?;
    let started = Instant::now();
    let mut budget = config.inference_budget;
    let mut summary = PassSummary::default();

    // Run a waiting conversation lookup before background roles can consume
    // the pass budget. Explicit user research also works with autonomy off.
    let user_research_first = store.has_pending_user_research()?;
    if user_research_first {
        let result = run_role(&store, pass_id, "scholar", || roles.research())?;
        budget = budget.saturating_sub(result.processed.max(usize::from(result.error)));
        summary.researched = result.processed;
        summary.errors += usize::from(result.error);
    }

    let curator_limit = budget.min(4);
    if curator_limit == 0 || started.elapsed() >= config.max_duration {
        skip_role(&store, pass_id, "curator", "pass budget exhausted")?;
    } else {
        let result = run_role(&store, pass_id, "curator", || roles.curate(curator_limit))?;
        budget = budget.saturating_sub(if result.error {
            curator_limit
        } else {
            result.processed
        });
        summary.curated = result.processed;
        summary.errors += usize::from(result.error);
    }

    if budget == 0 || started.elapsed() >= config.max_duration {
        skip_role(&store, pass_id, "curmudgeon", "pass budget exhausted")?;
    } else {
        let result = run_role(&store, pass_id, "curmudgeon", || roles.audit())?;
        budget = budget.saturating_sub(result.processed.max(usize::from(result.error)));
        summary.audited = result.processed;
        summary.errors += usize::from(result.error);
    }

    if !config.research_enabled {
        skip_role(&store, pass_id, "scout", "autonomous research disabled")?;
    } else if budget == 0 || started.elapsed() >= config.max_duration {
        skip_role(&store, pass_id, "scout", "pass budget exhausted")?;
    } else {
        let result = run_role(&store, pass_id, "scout", || roles.scout())?;
        budget = budget.saturating_sub(2);
        summary.scouted = result.processed;
        summary.errors += usize::from(result.error);
    }

    if !user_research_first {
        if !config.research_enabled {
            skip_role(&store, pass_id, "scholar", "autonomous research disabled")?;
        } else if budget == 0 || started.elapsed() >= config.max_duration {
            skip_role(&store, pass_id, "scholar", "pass budget exhausted")?;
        } else {
            let result = run_role(&store, pass_id, "scholar", || roles.research())?;
            summary.researched = result.processed;
            summary.errors += usize::from(result.error);
        }
    }

    store.finish_maintenance_pass(pass_id)?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::KnowledgeMap;
    use crate::storage::ScholarFinding;

    #[test]
    fn frontend_lookup_gets_the_first_pass_slot_even_with_autonomy_disabled() {
        struct UserRoles(std::path::PathBuf);
        impl Roles for UserRoles {
            fn curate(&mut self, _: usize) -> io::Result<usize> {
                panic!("user lookup must consume the first slot")
            }
            fn audit(&mut self) -> io::Result<usize> {
                panic!("user lookup must precede auditing")
            }
            fn scout(&mut self) -> io::Result<usize> {
                panic!("autonomy is disabled")
            }
            fn research(&mut self) -> io::Result<usize> {
                let mut store = LearningStore::open(&self.0)?;
                assert_eq!(
                    store.claim_scholar_job()?.unwrap().question,
                    "What crisp flavours exist?"
                );
                Ok(1)
            }
        }
        let path = std::env::temp_dir().join(format!(
            "lyra-user-priority-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = LearningStore::open(&path).unwrap();
        store
            .enqueue_research("frontend", 1, "What crisp flavours exist?")
            .unwrap();
        let mut roles = UserRoles(path.clone());
        let summary = run_pass_with_roles(
            &path,
            "serve",
            &mut roles,
            PassConfig {
                max_duration: Duration::from_secs(10),
                inference_budget: 1,
                research_enabled: false,
            },
        )
        .unwrap();
        assert_eq!(summary.researched, 1);
        assert_eq!(summary.curated + summary.audited + summary.scouted, 0);
        let pass = store.maintenance_status().unwrap().last_pass.unwrap();
        assert_eq!(
            pass.roles
                .iter()
                .find(|role| role.role == "scholar")
                .unwrap()
                .status,
            "work"
        );
        drop(store);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn successful_review_queue_work_has_detail_but_no_error() {
        let result = role_outcome(Ok(1), true);
        assert_eq!(result.status, "work");
        assert_eq!(result.processed, 1);
        assert_eq!(
            result.detail.as_deref(),
            Some("items queued for AI review or corroboration")
        );
        assert!(result.error.is_none());
        let failure = role_outcome(Err(io::Error::other("model unavailable")), true);
        assert_eq!(failure.status, "error");
        assert_eq!(failure.error.as_deref(), Some("model unavailable"));
        assert!(failure.detail.is_none());
    }

    struct FixtureRoles<'a> {
        database_path: &'a Path,
        knowledge_path: &'a Path,
        audit_fails: bool,
        curated: usize,
    }

    impl Roles for FixtureRoles<'_> {
        fn curate(&mut self, limit: usize) -> io::Result<usize> {
            Ok(self.curated.min(limit))
        }

        fn audit(&mut self) -> io::Result<usize> {
            if self.audit_fails {
                Err(io::Error::other("deliberate audit failure"))
            } else {
                Ok(0)
            }
        }

        fn scout(&mut self) -> io::Result<usize> {
            scout::process_once_with(self.database_path, self.knowledge_path, |_, _, _| {
                Ok("QUESTION: What is the atmosphere of Mars?".into())
            })
        }

        fn research(&mut self) -> io::Result<usize> {
            let mut store = LearningStore::open(self.database_path)?;
            let Some(job) = store.claim_scholar_job()? else {
                return Ok(0);
            };
            store.finish_scholar_job(
                &job.id,
                "Mars",
                "https://en.wikipedia.org/wiki/Mars",
                "Mars is a planet.",
                Some(ScholarFinding {
                    semantic_support: None,
                    subject: "Mars".into(),
                    relation: "is a".into(),
                    object: "planet".into(),
                    evidence_text: "Mars is a planet.".into(),
                    confidence: 0.99,
                    valid_from: None,
                    valid_until: None,
                    source_quality: "reference".into(),
                    source_updated_at: None,
                }),
                None,
            )?;
            Ok(1)
        }
    }

    #[test]
    fn idle_tick_finds_a_gap_and_scholar_evidence_reaches_the_map() {
        let directory = std::env::temp_dir().join(format!(
            "community-maintenance-loop-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let database_path = directory.join("learning.db");
        let knowledge_path = directory.join("knowledge.json");
        let mut map = KnowledgeMap::default();
        map.add("Mars", "orbits", "the Sun", "test seed").unwrap();
        map.save(&knowledge_path).unwrap();
        LearningStore::open(&database_path)
            .unwrap()
            .import_knowledge(&map)
            .unwrap();
        let mut roles = FixtureRoles {
            database_path: &database_path,
            knowledge_path: &knowledge_path,
            audit_fails: true,
            curated: 0,
        };
        let config = PassConfig {
            max_duration: Duration::from_secs(10),
            inference_budget: 4,
            research_enabled: true,
        };
        let first = run_pass_with_roles(&database_path, "chat", &mut roles, config).unwrap();
        assert_eq!(first.errors, 1);
        assert_eq!(first.scouted, 1);
        assert_eq!(first.researched, 1);
        let mut store = LearningStore::open(&database_path).unwrap();
        let status = store.maintenance_status().unwrap();
        let pass = status.last_pass.unwrap();
        assert_eq!(pass.status, "partial_error");
        assert_eq!(pass.roles.len(), 4);
        assert_eq!(pass.roles[1].status, "error");
        assert_eq!(pass.roles[2].status, "work");
        assert_eq!(pass.roles[3].status, "work");
        assert_eq!(
            pass.roles[3].detail.as_deref(),
            Some("items queued for AI review or corroboration")
        );
        assert_eq!(store.active_semantic_claims().unwrap().len(), 1);
        let investigation = store.scout_investigations().unwrap().remove(0);
        assert!(investigation.selection_reason.contains("Rotating map scan"));
        assert_eq!(
            store
                .scholar_findings(&investigation.scholar_job_id)
                .unwrap()[0]
                .status,
            "needs_review"
        );
        let finding_id = store
            .scholar_findings(&investigation.scholar_job_id)
            .unwrap()[0]
            .id;
        store
            .review_scholar_finding(finding_id, true, "Reviewed the cited passage")
            .unwrap();
        assert_eq!(store.active_semantic_claims().unwrap().len(), 2);
        drop(store);
        roles.audit_fails = false;
        let second = run_pass_with_roles(
            &database_path,
            "chat",
            &mut roles,
            PassConfig {
                max_duration: Duration::from_secs(10),
                inference_budget: 2,
                research_enabled: true,
            },
        )
        .unwrap();
        assert_eq!(second.scouted, 0);
        assert_eq!(second.researched, 0);
        let store = LearningStore::open(&database_path).unwrap();
        assert_eq!(store.scout_investigations().unwrap().len(), 1);
        assert_eq!(
            store
                .maintenance_status()
                .unwrap()
                .last_pass
                .unwrap()
                .status,
            "completed"
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn failed_model_load_is_visible_as_a_failed_pass() {
        let path = std::env::temp_dir().join(format!(
            "community-maintenance-startup-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        record_startup_failure(&path, "serve", "model unavailable").unwrap();
        let store = LearningStore::open(&path).unwrap();
        let pass = store.maintenance_status().unwrap().last_pass.unwrap();
        assert_eq!(pass.status, "partial_error");
        assert_eq!(pass.roles[0].detail.as_deref(), Some("model unavailable"));
        assert!(pass.roles[1..].iter().all(|role| role.status == "skipped"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn disabled_research_and_inference_budget_skip_their_roles() {
        let directory = std::env::temp_dir().join(format!(
            "community-maintenance-budget-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let database_path = directory.join("learning.db");
        let knowledge_path = directory.join("knowledge.json");
        let mut roles = FixtureRoles {
            database_path: &database_path,
            knowledge_path: &knowledge_path,
            audit_fails: false,
            curated: 1,
        };
        let result = run_pass_with_roles(
            &database_path,
            "chat",
            &mut roles,
            PassConfig {
                max_duration: Duration::from_secs(10),
                inference_budget: 1,
                research_enabled: false,
            },
        )
        .unwrap();
        assert_eq!(result.curated, 1);
        assert_eq!(result.audited, 0);
        assert_eq!(result.scouted, 0);
        assert_eq!(result.researched, 0);
        let store = LearningStore::open(&database_path).unwrap();
        let pass = store.maintenance_status().unwrap().last_pass.unwrap();
        assert_eq!(pass.roles[0].status, "work");
        assert!(pass.roles[1..].iter().all(|role| role.status == "skipped"));
        let _ = std::fs::remove_dir_all(directory);
    }
}
