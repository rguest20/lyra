use super::{
    AssistantClaim, CandidateClaim, ClaimJudgment, ConsolidatedClaimRevision, ImportSummary,
    LearningStore, ModelCheckpointManifest, ScholarEvidence, ScholarFinding, ScholarSourceProfile,
    evidence_supports_claim,
};
use crate::knowledge::KnowledgeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_DB: AtomicUsize = AtomicUsize::new(0);

fn temp_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "community-learning-{}-{}.db",
        std::process::id(),
        NEXT_DB.fetch_add(1, Ordering::Relaxed)
    ))
}

#[test]
fn assistant_challenge_is_durable_and_policy_reduces_usable_confidence() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .stage_assistant_reply("conversation", 1, "A cricket bat is 400m long.")
        .unwrap();
    store
        .stage_assistant_reply("conversation", 1, "A cricket bat is 400m long.")
        .unwrap();
    let job = store.claim_assistant_reply().unwrap().unwrap();
    assert!(store.claim_assistant_reply().unwrap().is_none());
    store
        .finish_assistant_reply(
            &job.id,
            &[ClaimJudgment {
                claim: AssistantClaim {
                    subject: "cricket bat".into(),
                    relation: "length".into(),
                    object: "400m".into(),
                    evidence_text: "A cricket bat is 400m long.".into(),
                    extraction_confidence: 0.8,
                },
                verdict: "challenged".into(),
                challenge_type: Some("magnitude_inconsistency".into()),
                reason: "Compared with sourced bat length".into(),
                evidence_refs: vec!["db:7".into()],
            }],
        )
        .unwrap();
    let stored = store.assistant_claims(10).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].confidence_before, 0.5);
    assert_eq!(stored[0].confidence_after, 0.1);
    assert_eq!(stored[0].evidence_refs, ["db:7"]);
    assert_eq!(
        store
            .relevant_assistant_challenges("cricket bat length", 5)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store.latest_reply_challenges("conversation").unwrap().len(),
        1
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn scout_investigation_and_scholar_job_commit_together_without_duplicates() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    assert!(
        store
            .enqueue_scout_research_with_reason(
                "challenged",
                "hagia sophia|height",
                "assistant_claim:42",
                "What is the height of Hagia Sophia?",
                "Challenged claim",
            )
            .unwrap()
    );
    assert!(
        !store
            .enqueue_scout_research_with_reason(
                "challenged",
                "hagia sophia|height",
                "assistant_claim:43",
                "What is the height of Hagia Sophia?",
                "Challenged claim",
            )
            .unwrap()
    );
    let records = store.scout_investigations().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].trigger_ref, "assistant_claim:42");
    let job = store.claim_scholar_job().unwrap().unwrap();
    assert_eq!(job.id, records[0].scholar_job_id);
    assert_eq!(job.question, records[0].question);
    assert!(store.claim_scholar_job().unwrap().is_none());
    drop(store);
    let reopened = LearningStore::open(&path).unwrap();
    assert!(
        reopened
            .scout_topic_exists("challenged", "hagia sophia|height")
            .unwrap()
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn scout_retries_no_result_after_cooldown_and_stops_after_three_attempts() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    for attempt in 0..3 {
        assert!(
            store
                .enqueue_scout_research_with_reason(
                    "sparse",
                    "mars",
                    "db:1",
                    "Find evidence about Mars",
                    "Sparse map coverage",
                )
                .unwrap()
        );
        let job = store.claim_scholar_job().unwrap().unwrap();
        assert!(store.scout_topic_exists("sparse", "mars").unwrap());
        store
            .finish_scholar_job(&job.id, "", "", "", None, Some("no useful result"))
            .unwrap();
        assert!(store.scout_topic_exists("sparse", "mars").unwrap());
        store
            .connection
            .execute(
                "UPDATE scout_investigations SET created_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-2 days')
                 WHERE scholar_job_id = ?1",
                [&job.id],
            )
            .unwrap();
        assert_eq!(
            store.scout_topic_exists("sparse", "mars").unwrap(),
            attempt == 2
        );
    }
    assert!(
        !store
            .enqueue_scout_research_with_reason(
                "sparse",
                "mars",
                "db:1",
                "Find evidence about Mars",
                "Sparse map coverage",
            )
            .unwrap()
    );
    assert_eq!(store.scout_investigations().unwrap().len(), 3);
    let _ = std::fs::remove_file(path);
}

#[test]
fn scout_holds_reviewed_source_for_a_week_before_retry() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_scout_research_with_reason(
            "stale",
            "claim:1",
            "db:1",
            "Check current evidence",
            "Review date passed",
        )
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    store.hold_scholar_job(&job.id, "source was stale").unwrap();
    assert!(store.scout_topic_exists("stale", "claim:1").unwrap());
    store.connection.execute(
        "UPDATE scout_investigations SET created_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-8 days') WHERE scholar_job_id = ?1",
        [&job.id],
    ).unwrap();
    assert!(!store.scout_topic_exists("stale", "claim:1").unwrap());
    let _ = std::fs::remove_file(path);
}

#[test]
fn maintenance_history_survives_restart_and_marks_stale_runs_interrupted() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let first = store.start_maintenance_pass("chat").unwrap();
    store
        .record_maintenance_role(first, "curator", "idle", 0, None, 1)
        .unwrap();
    store
        .record_maintenance_role(first, "curmudgeon", "error", 0, Some("test failure"), 2)
        .unwrap();
    store.finish_maintenance_pass(first).unwrap();
    let crashed = store.start_maintenance_pass("serve").unwrap();
    store
        .connection
        .execute(
            "UPDATE maintenance_passes SET started_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-11 minutes') WHERE id = ?1",
            [crashed],
        )
        .unwrap();
    drop(store);
    let mut reopened = LearningStore::open(&path).unwrap();
    assert_eq!(
        reopened
            .maintenance_status()
            .unwrap()
            .last_pass
            .unwrap()
            .status,
        "interrupted"
    );
    let next = reopened.start_maintenance_pass("chat").unwrap();
    reopened.finish_maintenance_pass(next).unwrap();
    let history = reopened.maintenance_recent_passes(3).unwrap();
    assert_eq!(
        history
            .iter()
            .map(|run| run.status.as_str())
            .collect::<Vec<_>>(),
        ["completed", "interrupted", "partial_error"]
    );
    assert_eq!(history[2].roles[1].detail.as_deref(), Some("test failure"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn maintenance_pass_history_is_bounded() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    for _ in 0..205 {
        let pass = store.start_maintenance_pass("once").unwrap();
        store.finish_maintenance_pass(pass).unwrap();
    }
    assert_eq!(store.maintenance_recent_passes(300).unwrap().len(), 200);
    let count: i64 = store
        .connection
        .query_row("SELECT COUNT(*) FROM maintenance_passes", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 200);
    let _ = std::fs::remove_file(path);
}

#[test]
fn failed_worker_process_marks_its_running_pass_interrupted() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let pass = store.start_maintenance_pass("serve").unwrap();
    store
        .mark_maintenance_worker_interrupted(std::process::id())
        .unwrap();
    assert_eq!(
        store
            .maintenance_status()
            .unwrap()
            .last_pass
            .unwrap()
            .status,
        "interrupted"
    );
    assert!(store.finish_maintenance_pass(pass).is_err());
    let _ = std::fs::remove_file(path);
}

#[test]
fn version_17_migration_keeps_scout_jobs_and_adds_operational_history() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_scout_research_with_reason(
            "sparse",
            "mars",
            "db:1",
            "What is Mars?",
            "Sparse map coverage",
        )
        .unwrap();
    store
        .connection
        .execute_batch(
            "DROP TABLE maintenance_role_runs;
         DROP TABLE maintenance_passes;
         ALTER TABLE scout_investigations RENAME TO scout_investigations_new;
         CREATE TABLE scout_investigations (
             trigger_kind TEXT NOT NULL,
             topic_key TEXT NOT NULL,
             trigger_ref TEXT NOT NULL,
             question TEXT NOT NULL,
             scholar_job_id TEXT NOT NULL REFERENCES scholar_jobs(id),
             created_at TEXT NOT NULL,
             PRIMARY KEY (trigger_kind, topic_key)
         );
         INSERT INTO scout_investigations
             SELECT trigger_kind, topic_key, trigger_ref, question, scholar_job_id, created_at
             FROM scout_investigations_new;
         DROP TABLE scout_investigations_new;
         ALTER TABLE scholar_source_assessments DROP COLUMN research_relevance;
         PRAGMA user_version = 17;",
        )
        .unwrap();
    drop(store);
    let mut upgraded = LearningStore::open(&path).unwrap();
    let investigations = upgraded.scout_investigations().unwrap();
    assert_eq!(investigations.len(), 1);
    assert_eq!(investigations[0].topic_key, "mars");
    assert!(investigations[0].selection_reason.contains("Migrated"));
    let job = upgraded.claim_scholar_job().unwrap().unwrap();
    assert_eq!(job.id, investigations[0].scholar_job_id);
    let pass = upgraded.start_maintenance_pass("once").unwrap();
    upgraded.finish_maintenance_pass(pass).unwrap();
    assert_eq!(
        upgraded
            .maintenance_status()
            .unwrap()
            .last_pass
            .unwrap()
            .status,
        "completed"
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn model_probe_attempts_survive_restart_and_are_capped_when_failed_or_rejected() {
    let path = temp_path();
    for attempt in 0..3 {
        let mut store = LearningStore::open(&path).unwrap();
        let id = store
            .begin_model_probe("open public knowledge", "test")
            .unwrap();
        assert!(id.is_some());
        store
            .complete_model_probe(
                id.unwrap(),
                if attempt == 1 { "failed" } else { "rejected" },
                if attempt == 1 {
                    "inference failed"
                } else {
                    "invalid response"
                },
                None,
                None,
                None,
            )
            .unwrap();
        // Move the attempt outside the cooldown but keep it inside the day.
        store.connection.execute(
            "UPDATE scout_model_probes SET created_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?1)
             WHERE id = ?2",
            rusqlite::params![format!("-{} minutes", 20 - attempt * 5), id.unwrap()],
        ).unwrap();
    }
    let mut reopened = LearningStore::open(&path).unwrap();
    assert!(
        reopened
            .begin_model_probe("open public knowledge", "test")
            .unwrap()
            .is_none()
    );
    assert_eq!(reopened.model_probes().unwrap().len(), 3);
    let _ = std::fs::remove_file(path);
}

#[test]
fn wikipedia_search_attempt_keeps_query_and_outcome_across_restart() {
    let path = temp_path();
    let store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("test", 1, "What are the moons of Mars?")
        .unwrap();
    drop(store);
    let mut store = LearningStore::open(&path).unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    store
        .record_scholar_search_attempt(&job.id, "Mars moons", "no_result", Some(0), None)
        .unwrap();
    drop(store);
    let store = LearningStore::open(&path).unwrap();
    let attempts = store.scholar_search_attempts(&job.id).unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].provider, "wikipedia");
    assert_eq!(attempts[0].trigger_reason, "primary");
    assert_eq!(attempts[0].query, "Mars moons");
    assert_eq!(attempts[0].status, "no_result");
    let _ = std::fs::remove_file(path);
}

#[test]
fn serpapi_cooldown_and_discovery_candidates_survive_restart() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("test", 1, "Chicken soup ingredients")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    let reservation = store
        .reserve_serpapi_search(1_800_000, &job.id, "Chicken soup ingredients")
        .unwrap();
    store
        .complete_serpapi_search(reservation.id, Some(200), None)
        .unwrap();
    store
        .record_typed_search_candidate(
            &job.id,
            "serpapi",
            "Chicken soup ingredients",
            "recipe",
            "Chicken soup",
            "https://example.org/chicken-soup",
            "Search result snippet is discovery only.",
            r#"{"source":"Example Kitchen","ingredients":["chicken","carrots"],"rating":4.5}"#,
        )
        .unwrap();
    let candidate_id = store.scholar_search_candidates(&job.id).unwrap()[0].id;
    store
        .record_scholar_candidate_fetch_result(
            candidate_id,
            "fetched",
            "allowed",
            Some("robots and site policy allowed the fetch"),
            "fetched",
            Some("https://example.org/chicken-soup"),
            Some("Chicken soup contains chicken and carrots."),
        )
        .unwrap();
    store
        .record_scholar_candidate_provenance(
            candidate_id,
            r#"{"canonical_url":"https://example.org/chicken-soup","author":"A Cook","publisher":null,"published_at":null,"cited_urls":[]}"#,
        )
        .unwrap();
    store
        .record_serpapi_search_domains(
            reservation.id,
            &[
                ("www.Example.org.".to_owned(), 2),
                ("news.example.net".to_owned(), 1),
            ],
        )
        .unwrap();
    drop(store);

    let mut store = LearningStore::open(&path).unwrap();
    assert_eq!(
        store
            .reserve_serpapi_search(1_800_000, &job.id, "second query")
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
    let candidates = store.scholar_search_candidates(&job.id).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].provider, "serpapi");
    assert_eq!(candidates[0].query, "Chicken soup ingredients");
    assert_eq!(candidates[0].result_type, "recipe");
    assert!(
        candidates[0]
            .page_provenance_json
            .as_deref()
            .unwrap()
            .contains("A Cook")
    );
    assert!(
        candidates[0]
            .structured_data_json
            .contains("Example Kitchen")
    );
    assert_eq!(candidates[0].status, "fetched");
    assert_eq!(candidates[0].policy_status, "allowed");
    assert_eq!(
        candidates[0].fetched_excerpt.as_deref(),
        Some("Chicken soup contains chicken and carrots.")
    );
    assert_eq!(candidates[0].url, "https://example.org/chicken-soup");
    let domains = store.serpapi_domain_history(50).unwrap();
    assert_eq!(domains.len(), 2);
    let example_domain = domains
        .iter()
        .find(|domain| domain.domain == "example.org")
        .unwrap();
    assert_eq!(example_domain.query, "Chicken soup ingredients");
    assert_eq!(example_domain.result_count, 2);
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn tavily_daily_reservations_enforce_the_persisted_call_cap() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    for _ in 0..25 {
        store
            .reserve_tavily_search_with_interval("basic", 0)
            .unwrap();
    }
    assert_eq!(
        store
            .reserve_tavily_search_with_interval("basic", 0)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert_eq!(
        store
            .reserve_tavily_search_with_interval("unsupported", 0)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::InvalidData
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn tavily_advanced_retries_reserve_two_monthly_credits() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let historical = now - 25 * 60 * 60 * 1000;
    for _ in 0..374 {
        store
            .connection
            .execute(
                "INSERT INTO tavily_usage (scheduled_at_ms, search_depth, credits, outcome)
                 VALUES (?1, 'advanced', 2, 'succeeded')",
                [historical],
            )
            .unwrap();
    }
    store
        .connection
        .execute(
            "INSERT INTO tavily_usage (scheduled_at_ms, search_depth, credits, outcome)
             VALUES (?1, 'basic', 1, 'succeeded')",
            [historical],
        )
        .unwrap();
    assert_eq!(
        store
            .reserve_tavily_search_with_interval("advanced", 0)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock,
        "a two-credit advanced retry must respect the 750-credit monthly ceiling"
    );
    store
        .reserve_tavily_search_with_interval("basic", 0)
        .unwrap();
    let credits: i64 = store
        .connection
        .query_row("SELECT SUM(credits) FROM tavily_usage", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(credits, 750);
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn tavily_hourly_spacing_survives_restart() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .reserve_tavily_search_with_interval("basic", 3_600_000)
        .unwrap();
    drop(store);

    let mut reopened = LearningStore::open(&path).unwrap();
    assert!(
        reopened
            .last_scholar_provider_search_ms("tavily")
            .unwrap()
            .is_some()
    );
    assert_eq!(
        reopened
            .reserve_tavily_search_with_interval("basic", 3_600_000)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
    drop(reopened);
    let _ = std::fs::remove_file(path);
}

#[test]
fn corroboration_queue_backfills_on_upgrade_and_routes_each_provider() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("queue-test", 1, "What kind of object is Mars?")
        .unwrap();
    let first_job = store.claim_scholar_job().unwrap().unwrap();
    store
        .finish_scholar_job_with_profile_and_relevance(
            &first_job.id,
            "Mars report",
            "https://first.example.org/mars",
            "Mars is a planet. This report describes telescope observations.",
            Some(ScholarFinding {
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
            &ScholarSourceProfile::default(),
            "direct",
        )
        .unwrap();
    let finding_id = store.scholar_findings(&first_job.id).unwrap()[0].id;
    assert_eq!(store.corroboration_queue(10).unwrap().len(), 1);
    store
        .connection
        .execute_batch(
            "DELETE FROM scholar_corroboration_queue;
             PRAGMA user_version = 36;",
        )
        .unwrap();
    drop(store);

    let mut store = LearningStore::open(&path).unwrap();
    assert_eq!(store.corroboration_queue(10).unwrap().len(), 1);
    let first = store
        .claim_corroboration_task(true, true, 1_800_000, 3_600_000)
        .unwrap()
        .unwrap();
    assert_eq!(first.finding_id, finding_id);
    assert_eq!(first.provider, "serpapi");
    assert_eq!(
        store.corroboration_source_urls(&first).unwrap(),
        vec!["https://first.example.org/mars"]
    );
    assert!(
        store
            .claim_corroboration_task(true, true, 1_800_000, 3_600_000)
            .unwrap()
            .is_none()
    );
    store
        .finish_scholar_job_with_evidence(&first.job_id, &[], Some("no match"))
        .unwrap();
    store.finish_corroboration_task(&first, "no_match").unwrap();
    let second = store
        .claim_corroboration_task(false, true, 1_800_000, 3_600_000)
        .unwrap()
        .unwrap();
    assert_eq!(second.provider, "tavily");
    store
        .connection
        .execute(
            "UPDATE scholar_corroboration_queue SET claimed_at_ms = 0
             WHERE finding_id = ?1",
            [finding_id],
        )
        .unwrap();
    assert!(
        store
            .claim_corroboration_task(false, false, 1_800_000, 3_600_000)
            .unwrap()
            .is_none()
    );
    let recovered_job = store
        .scholar_jobs()
        .unwrap()
        .into_iter()
        .find(|(job, _)| job.id == second.job_id)
        .unwrap();
    assert_eq!(recovered_job.0.status, "no_result");
    let queue = store.corroboration_queue(10).unwrap();
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].status, "pending");
    assert_eq!((queue[0].serp_attempts, queue[0].tavily_attempts), (1, 1));
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn source_assessment_migration_marks_old_findings_unassessed() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store.enqueue_research("migration", 1, "Mars").unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    store
        .finish_scholar_job(
            &job.id,
            "Mars",
            "https://en.wikipedia.org/wiki/Mars",
            "Mars is a planet.",
            Some(ScholarFinding {
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
        )
        .unwrap();
    let finding_id = store.scholar_findings(&job.id).unwrap()[0].id;
    store
        .connection
        .execute_batch("DROP TABLE scholar_source_assessments; PRAGMA user_version = 20;")
        .unwrap();
    drop(store);
    let store = LearningStore::open(&path).unwrap();
    let assessment = store
        .scholar_source_assessment(finding_id)
        .unwrap()
        .unwrap();
    assert_eq!(assessment.gate_status, "unassessed");
    assert_eq!(assessment.direct_support, None);
    let _ = std::fs::remove_file(path);
}

#[test]
fn expired_scholar_lease_reclaims_once_and_duplicate_finish_rolls_back() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("lease-test", 1, "Mars planet")
        .unwrap();
    let first = store.claim_scholar_job().unwrap().unwrap();
    store.connection.execute(
        "UPDATE scholar_jobs SET claimed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-11 minutes') WHERE id = ?1",
        [&first.id],
    ).unwrap();
    let reclaimed = store.claim_scholar_job().unwrap().unwrap();
    assert_eq!(first.id, reclaimed.id);
    store
        .finish_scholar_job(
            &first.id,
            "Mars",
            "https://en.wikipedia.org/wiki/Mars",
            "Mars is a planet.",
            Some(ScholarFinding {
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
        )
        .unwrap();
    assert!(
        store
            .finish_scholar_job(
                &first.id,
                "Mars",
                "https://en.wikipedia.org/wiki/Mars",
                "Mars is a planet.",
                Some(ScholarFinding {
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
            )
            .is_err()
    );
    assert_eq!(store.scholar_findings(&first.id).unwrap().len(), 1);
    assert!(store.claim_scholar_job().unwrap().is_none());
    let _ = std::fs::remove_file(path);
}

#[test]
fn queued_research_survives_a_lost_wake_and_process_restart() {
    let path = temp_path();
    {
        let store = LearningStore::open(&path).unwrap();
        store
            .enqueue_research("lost-wake", 1, "What is the composition of Mars?")
            .unwrap();
        // The in-memory wake hint may be lost; the durable queue remains authoritative.
    }
    let mut restarted = LearningStore::open(&path).unwrap();
    let job = restarted.claim_scholar_job().unwrap().unwrap();
    assert_eq!(job.question, "What is the composition of Mars?");
    assert!(restarted.claim_scholar_job().unwrap().is_none());
    drop(restarted);
    let _ = std::fs::remove_file(path);
}

#[test]
fn only_active_explicitly_approved_claims_can_train_the_model() {
    let path = temp_path();
    let mut map = KnowledgeMap::default();
    map.add("Auric fern", "has test colour", "violet", "review source")
        .unwrap();
    let mut store = LearningStore::open(&path).unwrap();
    store.import_knowledge(&map).unwrap();
    assert!(store.approved_model_claim(1).is_err());
    assert!(store.approve_model_claim(2, "reviewed").is_err());
    store
        .approve_model_claim(1, "I checked the source")
        .unwrap();
    assert_eq!(store.approved_model_claim(1).unwrap().object, "violet");
    drop(store);
    let reopened = LearningStore::open(&path).unwrap();
    assert_eq!(
        reopened.approved_model_claim(1).unwrap().subject,
        "Auric fern"
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn scholar_evidence_must_support_the_relation_and_object() {
    let claim = ScholarFinding {
        subject: "Hellenic Police".into(),
        relation: "first editor".into(),
        object: "Dimitrios Mallios".into(),
        evidence_text: "Dimitrios Mallios is Chief of the Hellenic Police.".into(),
        confidence: 0.99,
        valid_from: None,
        valid_until: None,
        source_quality: "reference".into(),
        source_updated_at: None,
    };
    assert!(!evidence_supports_claim(&claim));
}

#[test]
fn scholar_evidence_supports_each_item_in_a_cited_source_list() {
    let evidence_text = "Usually, the main ingredients of chicken soup are:\n* Chicken broth\n* Chicken\n* Noodles\n* Carrots";
    let claim = ScholarFinding {
        subject: "Chicken soup".into(),
        relation: "lists ingredients".into(),
        object: "Chicken broth; Chicken; Noodles; Carrots".into(),
        evidence_text: evidence_text.into(),
        confidence: 0.99,
        valid_from: None,
        valid_until: None,
        source_quality: "reference".into(),
        source_updated_at: None,
    };
    assert!(evidence_supports_claim(&claim));

    let unsupported_item = ScholarFinding {
        object: "Chicken broth; Chicken; Noodles; Peas".into(),
        ..claim
    };
    assert!(!evidence_supports_claim(&unsupported_item));
}

#[test]
fn review_cannot_promote_a_finding_without_direct_passage_support() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store.enqueue_research("review-test", 1, "Mars").unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    store
        .finish_scholar_job(
            &job.id,
            "Mars",
            "https://en.wikipedia.org/wiki/Mars",
            "Mars is a planet.",
            Some(ScholarFinding {
                subject: "Mars".into(),
                relation: "is a".into(),
                object: "planet".into(),
                evidence_text: "Mars has oceans.".into(),
                confidence: 0.99,
                valid_from: None,
                valid_until: None,
                source_quality: "reference".into(),
                source_updated_at: None,
            }),
            None,
        )
        .unwrap();
    let id = store.scholar_findings(&job.id).unwrap()[0].id;
    assert_eq!(
        store
            .scholar_source_assessment(id)
            .unwrap()
            .unwrap()
            .gate_status,
        "unsupported"
    );
    assert!(
        store
            .review_scholar_finding(id, true, "Looks right to me")
            .is_err()
    );
    assert!(store.active_semantic_claims().unwrap().is_empty());
    let _ = std::fs::remove_file(path);
}

#[test]
fn scholar_requires_quality_and_freshness_separate_from_extraction_score() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();

    store
        .enqueue_research("conversation", 1, "a supported claim")
        .unwrap();
    let unknown_source_job = store.claim_scholar_job().unwrap().unwrap();
    store
        .finish_scholar_job(
            &unknown_source_job.id,
            "Example",
            "https://example.invalid/page",
            "Example has a property.",
            Some(ScholarFinding {
                subject: "Example".into(),
                relation: "has".into(),
                object: "a property".into(),
                evidence_text: "Example has a property.".into(),
                confidence: 0.99,
                valid_from: None,
                valid_until: None,
                source_quality: "unknown".into(),
                source_updated_at: None,
            }),
            None,
        )
        .unwrap();
    let finding = store
        .scholar_findings(&unknown_source_job.id)
        .unwrap()
        .remove(0);
    assert_eq!(finding.confidence, 0.99);
    assert_eq!(finding.source_quality, "unknown");
    assert_eq!(finding.status, "needs_review");

    store
        .enqueue_research("conversation", 2, "Mars current population")
        .unwrap();
    let stale_job = store.claim_scholar_job().unwrap().unwrap();
    store
        .finish_scholar_job(
            &stale_job.id,
            "Mars",
            "https://en.wikipedia.org/wiki/Mars",
            "Mars current population is 100.",
            Some(ScholarFinding {
                subject: "Mars".into(),
                relation: "current population is".into(),
                object: "100".into(),
                evidence_text: "Mars current population is 100.".into(),
                confidence: 0.99,
                valid_from: None,
                valid_until: None,
                source_quality: "reference".into(),
                source_updated_at: Some("2001-01-01T00:00:00Z".into()),
            }),
            None,
        )
        .unwrap();
    let finding = store.scholar_findings(&stale_job.id).unwrap().remove(0);
    assert_eq!(finding.freshness, "stale");
    assert_eq!(finding.status, "needs_review");

    store
        .enqueue_research("conversation", 3, "medical treatment")
        .unwrap();
    let consequential_job = store.claim_scholar_job().unwrap().unwrap();
    store
        .finish_scholar_job(
            &consequential_job.id,
            "Insomnia",
            "https://en.wikipedia.org/wiki/Insomnia",
            "A treatment is available.",
            Some(ScholarFinding {
                subject: "Insomnia".into(),
                relation: "treatment is".into(),
                object: "available".into(),
                evidence_text: "A treatment is available.".into(),
                confidence: 0.99,
                valid_from: None,
                valid_until: None,
                source_quality: "reference".into(),
                source_updated_at: None,
            }),
            None,
        )
        .unwrap();
    let finding = store
        .scholar_findings(&consequential_job.id)
        .unwrap()
        .remove(0);
    assert_eq!(finding.status, "needs_review");

    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn two_verified_independent_secondary_sources_can_promote_one_claim() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let passage = "Mars is a planet.";
    let mut findings = Vec::new();
    for (turn, url, source_id) in [
        (
            1,
            "https://archive.example.org/mars",
            "archive:observations",
        ),
        (2, "https://journal.example.net/mars", "journal:survey"),
    ] {
        store
            .enqueue_research("corroboration", turn, "Mars planet")
            .unwrap();
        let job = store.claim_scholar_job().unwrap().unwrap();
        store
            .finish_scholar_job_with_profile_and_relevance(
                &job.id,
                "Mars",
                url,
                passage,
                Some(ScholarFinding {
                    subject: "Mars".into(),
                    relation: "is a".into(),
                    object: "planet".into(),
                    evidence_text: passage.into(),
                    confidence: 0.99,
                    valid_from: None,
                    valid_until: None,
                    source_quality: "reference".into(),
                    source_updated_at: None,
                }),
                None,
                &ScholarSourceProfile {
                    verified_underlying_source_id: Some(source_id.into()),
                    ..Default::default()
                },
                "direct",
            )
            .unwrap();
        let finding = store.scholar_findings(&job.id).unwrap().remove(0);
        findings.push(finding.id);
        let assessment = store
            .scholar_source_assessment(finding.id)
            .unwrap()
            .unwrap();
        assert!(assessment.independence_verified);
        assert_eq!(
            assessment.gate_status,
            if turn == 1 {
                "awaiting_corroboration"
            } else {
                "eligible"
            }
        );
    }
    let claims = store.active_semantic_claims().unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(store.claim_evidence(claims[0].claim.id).unwrap().len(), 2);
    assert_eq!(
        store
            .scholar_source_assessment(findings[0])
            .unwrap()
            .unwrap()
            .gate_status,
        "awaiting_corroboration",
        "the original assessment remains in history"
    );
    let earlier_status: String = store
        .connection
        .query_row(
            "SELECT status FROM scholar_findings WHERE id = ?1",
            [findings[0]],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(earlier_status, "promoted");
    let _ = std::fs::remove_file(path);
}

#[test]
fn copied_secondary_sources_do_not_count_as_independent() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    for (turn, url) in [
        (1, "https://one.example.org/mars"),
        (2, "https://two.example.net/mars"),
    ] {
        store
            .enqueue_research("copied", turn, "Mars planet")
            .unwrap();
        let job = store.claim_scholar_job().unwrap().unwrap();
        store
            .finish_scholar_job_with_profile(
                &job.id,
                "Mars",
                url,
                "Mars is a planet.",
                Some(ScholarFinding {
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
                &ScholarSourceProfile {
                    verified_underlying_source_id: Some("same-original-report".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let assessment = store
            .scholar_source_assessment(store.scholar_findings(&job.id).unwrap()[0].id)
            .unwrap()
            .unwrap();
        assert_eq!(assessment.gate_status, "awaiting_corroboration");
    }
    assert!(store.active_semantic_claims().unwrap().is_empty());
    let _ = std::fs::remove_file(path);
}

#[test]
fn repeated_article_wording_blocks_corroboration_even_with_different_source_ids() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let sentence = "Chicken soup contains chicken.";
    let copied = "Chicken soup contains chicken. First the vegetables are prepared and gently cooked, then the broth is warmed with herbs before all the ingredients are brought together and simmered until the flavors have blended. This method is used for a simple home style soup with a clear broth and tender pieces of chicken.";
    for (turn, url, source_id, passage) in [
        (1, "https://one.example.org/soup", "original:one", copied),
        (2, "https://two.example.net/soup", "original:two", copied),
    ] {
        store
            .enqueue_research("copied-article", turn, "Chicken soup ingredients")
            .unwrap();
        let job = store.claim_scholar_job().unwrap().unwrap();
        store
            .finish_scholar_job_with_profile_and_relevance(
                &job.id,
                "Chicken soup",
                url,
                passage,
                Some(ScholarFinding {
                    subject: "Chicken soup".into(),
                    relation: "contains".into(),
                    object: "chicken".into(),
                    evidence_text: sentence.into(),
                    confidence: 0.99,
                    valid_from: None,
                    valid_until: None,
                    source_quality: "reference".into(),
                    source_updated_at: None,
                }),
                None,
                &ScholarSourceProfile {
                    verified_underlying_source_id: Some(source_id.into()),
                    ..Default::default()
                },
                "direct",
            )
            .unwrap();
        let finding = store.scholar_findings(&job.id).unwrap().remove(0);
        let assessment = store
            .scholar_source_assessment(finding.id)
            .unwrap()
            .unwrap();
        assert_eq!(
            assessment.gate_status,
            if turn == 1 {
                "awaiting_corroboration"
            } else {
                "possible_shared_source"
            }
        );
        if turn == 2 {
            assert!(assessment.reason.contains("substantially overlaps finding"));
        }
    }
    assert!(store.active_semantic_claims().unwrap().is_empty());
    let _ = std::fs::remove_file(path);
}

#[test]
fn shared_short_fact_in_distinct_recipes_is_not_a_copy_signal() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    for (turn, url, source_id, passage) in [
        (
            1,
            "https://cook.example.org/soup",
            "cook:own-recipe",
            "Chicken soup contains chicken. For this version, brown the meat in a pan, add diced carrots and onion, then cover it with water. Let the pot simmer while preparing rice separately. Serve the soup once the chicken is tender and the rice is ready.",
        ),
        (
            2,
            "https://kitchen.example.net/soup",
            "kitchen:own-recipe",
            "Chicken soup contains chicken. Our recipe starts with a prepared stock and shredded roast chicken. Celery and noodles go in near the end so they keep their texture. Finish each bowl with parsley and a squeeze of lemon for a fresh flavor.",
        ),
    ] {
        store
            .enqueue_research("distinct-recipes", turn, "Chicken soup ingredients")
            .unwrap();
        let job = store.claim_scholar_job().unwrap().unwrap();
        store
            .finish_scholar_job_with_profile_and_relevance(
                &job.id,
                "Chicken soup",
                url,
                passage,
                Some(ScholarFinding {
                    subject: "Chicken soup".into(),
                    relation: "contains".into(),
                    object: "chicken".into(),
                    evidence_text: "Chicken soup contains chicken.".into(),
                    confidence: 0.99,
                    valid_from: None,
                    valid_until: None,
                    source_quality: "reference".into(),
                    source_updated_at: None,
                }),
                None,
                &ScholarSourceProfile {
                    verified_underlying_source_id: Some(source_id.into()),
                    ..Default::default()
                },
                "direct",
            )
            .unwrap();
        let finding = store.scholar_findings(&job.id).unwrap().remove(0);
        let assessment = store
            .scholar_source_assessment(finding.id)
            .unwrap()
            .unwrap();
        assert_eq!(
            assessment.gate_status,
            if turn == 1 {
                "awaiting_corroboration"
            } else {
                "eligible"
            }
        );
    }
    assert_eq!(store.active_semantic_claims().unwrap().len(), 1);
    let _ = std::fs::remove_file(path);
}

#[test]
fn shared_declared_citation_holds_until_curmudgeon_resolves_it_as_distinct() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let mut current_job_id = String::new();
    for (turn, url, source_id, note) in [
        (
            1,
            "https://one.example/soup",
            "publisher:one",
            "Alice adds herbs.",
        ),
        (
            2,
            "https://two.example/soup",
            "publisher:two",
            "Bob adds lemon.",
        ),
    ] {
        store
            .enqueue_research("provenance-review", turn, "Chicken soup ingredients")
            .unwrap();
        let job = store.claim_scholar_job().unwrap().unwrap();
        current_job_id = job.id.clone();
        let passage = format!("Chicken soup contains chicken. {note}");
        store
            .record_typed_search_candidate(
                &job.id,
                "test",
                "Chicken soup ingredients",
                "search_result",
                "Chicken soup",
                url,
                "test candidate",
                "{}",
            )
            .unwrap();
        let candidate_id = store.scholar_search_candidates(&job.id).unwrap()[0].id;
        store
            .record_scholar_candidate_fetch_result(
                candidate_id,
                "fetched",
                "allowed",
                None,
                "fetched",
                Some(url),
                Some(&passage),
            )
            .unwrap();
        let metadata = format!(
            r#"{{"canonical_url":"{url}","cited_urls":["https://food-source.example/chicken-soup"]}}"#
        );
        store
            .record_scholar_candidate_provenance(candidate_id, &metadata)
            .unwrap();
        store
            .finish_scholar_job_with_profile_and_relevance(
                &job.id,
                "Chicken soup",
                url,
                &passage,
                Some(ScholarFinding {
                    subject: "Chicken soup".into(),
                    relation: "contains".into(),
                    object: "chicken".into(),
                    evidence_text: "Chicken soup contains chicken.".into(),
                    confidence: 0.99,
                    valid_from: None,
                    valid_until: None,
                    source_quality: "reference".into(),
                    source_updated_at: None,
                }),
                None,
                &ScholarSourceProfile {
                    verified_underlying_source_id: Some(source_id.into()),
                    ..Default::default()
                },
                "direct",
            )
            .unwrap();
    }
    let current = store.scholar_findings(&current_job_id).unwrap().remove(0);
    let assessment = store
        .scholar_source_assessment(current.id)
        .unwrap()
        .unwrap();
    assert_eq!(assessment.gate_status, "possible_shared_source");
    assert!(store.active_semantic_claims().unwrap().is_empty());
    let review = store.claim_scholar_provenance_review().unwrap().unwrap();
    assert_eq!(review.finding_id, current.id);
    assert!(review.signal.contains("same cited source"));
    store
        .finish_scholar_provenance_review(
            review.id,
            "possibly_distinct_reporting",
            "The pages have distinct passages, though they cite a common page.",
        )
        .unwrap();
    let assessment = store
        .scholar_source_assessment(current.id)
        .unwrap()
        .unwrap();
    assert_eq!(assessment.gate_status, "eligible");
    assert_eq!(store.active_semantic_claims().unwrap().len(), 1);
    let _ = std::fs::remove_file(path);
}

#[test]
fn reviewed_primary_source_can_clear_expertise_gate_but_claim_needs_its_own_review() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("source-expertise", 1, "Photosynthesis")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    let evidence = "Photosynthesis converts light energy.";
    store
        .finish_scholar_job_with_profile(
            &job.id,
            "Photosynthesis study",
            "https://journal.example/photosynthesis",
            evidence,
            Some(ScholarFinding {
                subject: "Photosynthesis".into(),
                relation: "converts".into(),
                object: "light energy".into(),
                evidence_text: evidence.into(),
                confidence: 0.99,
                valid_from: None,
                valid_until: None,
                source_quality: "unknown".into(),
                source_updated_at: None,
            }),
            None,
            &ScholarSourceProfile::default(),
        )
        .unwrap();
    let finding = store.scholar_findings(&job.id).unwrap().remove(0);
    store
        .review_scholar_source_expertise(
            finding.id,
            "primary",
            true,
            None,
            "The cited experimental paper directly measures this process.",
        )
        .unwrap();
    let assessment = store
        .scholar_source_assessment(finding.id)
        .unwrap()
        .unwrap();
    assert_eq!(assessment.gate_status, "eligible");
    assert!(assessment.topical_expertise.contains("reviewer_verified"));
    assert!(store.active_semantic_claims().unwrap().is_empty());
    let review = store
        .scholar_source_expertise_reviews(finding.id)
        .unwrap()
        .remove(0);
    assert_eq!(review.evidence_text, evidence);
    assert_eq!(
        review.rationale,
        "The cited experimental paper directly measures this process."
    );

    store
        .review_scholar_finding(finding.id, true, "Claim matches the primary passage")
        .unwrap();
    assert_eq!(store.active_semantic_claims().unwrap().len(), 1);
    let _ = std::fs::remove_file(path);
}

#[test]
fn reviewed_secondary_still_needs_independence_and_recent_claim_needs_confirmed_date() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("source-expertise", 1, "Current GPU specification")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    let evidence = "Current GPU has memory 16 GB.";
    store
        .finish_scholar_job_with_profile(
            &job.id,
            "GPU specification report",
            "https://tech.example/gpu",
            evidence,
            Some(ScholarFinding {
                subject: "GPU".into(),
                relation: "has memory".into(),
                object: "16 GB".into(),
                evidence_text: evidence.into(),
                confidence: 0.99,
                valid_from: None,
                valid_until: None,
                source_quality: "unknown".into(),
                source_updated_at: None,
            }),
            None,
            &ScholarSourceProfile::default(),
        )
        .unwrap();
    let finding = store.scholar_findings(&job.id).unwrap().remove(0);
    store
        .review_scholar_source_expertise(
            finding.id,
            "secondary",
            true,
            None,
            "The publication specializes in graphics hardware.",
        )
        .unwrap();
    let assessment = store
        .scholar_source_assessment(finding.id)
        .unwrap()
        .unwrap();
    assert_eq!(assessment.gate_status, "unverified_temporal");
    store
        .review_scholar_source_expertise(
            finding.id,
            "secondary",
            true,
            Some("2026-10-02"),
            "The publication specializes in graphics hardware and confirms the date.",
        )
        .unwrap();
    let assessment = store
        .scholar_source_assessment(finding.id)
        .unwrap()
        .unwrap();
    assert_eq!(assessment.gate_status, "awaiting_corroboration");
    assert!(!assessment.independence_verified);
    assert_eq!(
        store
            .scholar_source_expertise_reviews(finding.id)
            .unwrap()
            .len(),
        2
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn two_suitable_secondary_sources_need_distinct_reviewer_verified_origins() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let mut findings = Vec::new();
    for (turn, url, note) in [
        (1, "https://first.example/mars", "One source passage."),
        (2, "https://second.example/mars", "Another source passage."),
    ] {
        store
            .enqueue_research("verified-origins", turn, "Mars planet")
            .unwrap();
        let job = store.claim_scholar_job().unwrap().unwrap();
        let passage = "Mars is a planet. A review notes additional details.";
        store
            .finish_scholar_job_with_profile(
                &job.id,
                "Mars",
                url,
                passage,
                Some(ScholarFinding {
                    subject: "Mars".into(),
                    relation: "is a".into(),
                    object: "planet".into(),
                    evidence_text: "Mars is a planet.".into(),
                    confidence: 0.99,
                    valid_from: None,
                    valid_until: None,
                    source_quality: "unknown".into(),
                    source_updated_at: None,
                }),
                None,
                &ScholarSourceProfile::default(),
            )
            .unwrap();
        let finding = store.scholar_findings(&job.id).unwrap().remove(0);
        store
            .review_scholar_source_expertise(finding.id, "secondary", true, None, note)
            .unwrap();
        findings.push(finding.id);
    }
    for (finding_id, origin) in findings.iter().zip(["report:survey-a", "report:survey-b"]) {
        store
            .review_scholar_source_origin(
                *finding_id,
                origin,
                "Distinct publisher report identifiers and separate original reporting.",
            )
            .unwrap();
    }
    for id in findings {
        let assessment = store.scholar_source_assessment(id).unwrap().unwrap();
        assert!(assessment.independence_verified);
        assert_eq!(assessment.gate_status, "eligible");
        assert!(assessment.reason.contains("distinct origins"));
    }
    let claims = store.active_semantic_claims().unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(store.claim_evidence(claims[0].claim.id).unwrap().len(), 2);
    assert_eq!(
        store
            .recent_maintenance_decisions(10)
            .unwrap()
            .iter()
            .filter(|decision| decision.outcome == "automatic_evidence_acceptance")
            .count(),
        2
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn scholar_research_flow_keeps_two_source_evidence_through_review_and_promotion() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let sources = [
        (
            1,
            "https://history.example/articles/mars-a",
            "Mars is a planet. The first report describes observations from its southern hemisphere and explains the telescope calibration used by the research team.",
            "Mars is a planet.",
            "report:independent-mars-study-a",
        ),
        (
            2,
            "https://history.example/research/mars-b",
            "Mars is a planet. A separate observatory's report summarizes orbital measurements and its independent analysis of the planet's atmosphere.",
            "Mars is a planet.",
            "report:independent-mars-study-b",
        ),
    ];
    let mut finding_ids = Vec::new();

    for (turn, url, passage, evidence, _) in sources {
        store
            .enqueue_research("mars-e2e", turn, "What kind of object is Mars?")
            .unwrap();
        let job = store.claim_scholar_job().unwrap().unwrap();
        store
            .finish_scholar_job_with_evidence(
                &job.id,
                &[ScholarEvidence {
                    source_title: format!("Mars report {turn}"),
                    source_url: url.to_owned(),
                    source_excerpt: passage.to_owned(),
                    finding: Some(ScholarFinding {
                        subject: "Mars".into(),
                        relation: "is a".into(),
                        object: "planet".into(),
                        evidence_text: evidence.into(),
                        confidence: 0.99,
                        valid_from: None,
                        valid_until: None,
                        source_quality: "unknown".into(),
                        source_updated_at: None,
                    }),
                    profile: ScholarSourceProfile::default(),
                    research_relevance: "direct".into(),
                }],
                None,
            )
            .unwrap();
        let findings = store.scholar_findings(&job.id).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].evidence_text, evidence);
        assert_eq!(findings[0].source_url, url);
        let assessment = store
            .scholar_source_assessment(findings[0].id)
            .unwrap()
            .unwrap();
        assert_eq!(assessment.direct_support, Some(true));
        assert_eq!(assessment.gate_status, "pending_review");
        finding_ids.push(findings[0].id);
    }

    // Both checked excerpts support the same claim, but nothing reaches the
    // semantic map until reviewers establish topic suitability and origin.
    assert!(store.active_semantic_claims().unwrap().is_empty());
    for ((finding_id, (_, _, _, _, origin)), index) in
        finding_ids.iter().zip(sources.iter()).zip(0..)
    {
        store
            .review_scholar_source_expertise(
                *finding_id,
                "secondary",
                true,
                None,
                "The publication is an astronomy research source and directly supports the passage.",
            )
            .unwrap();
        if index == 0 {
            assert_eq!(
                store
                    .scholar_source_assessment(*finding_id)
                    .unwrap()
                    .unwrap()
                    .gate_status,
                "awaiting_corroboration"
            );
        }
        store
            .review_scholar_source_origin(
                *finding_id,
                origin,
                "Reviewed publication records identify distinct original research reports.",
            )
            .unwrap();
        let origin_reviews = store.scholar_source_origin_reviews(*finding_id).unwrap();
        assert_eq!(origin_reviews.len(), 1);
        assert_eq!(origin_reviews[0].evidence_text, "Mars is a planet.");
    }

    for finding_id in &finding_ids {
        let assessment = store
            .scholar_source_assessment(*finding_id)
            .unwrap()
            .unwrap();
        assert!(assessment.independence_verified);
        assert_eq!(assessment.gate_status, "eligible");
    }
    let claims = store.active_semantic_claims().unwrap();
    assert_eq!(claims.len(), 1);
    let evidence = store.claim_evidence(claims[0].claim.id).unwrap();
    assert_eq!(evidence.len(), 2);
    for (turn, url, excerpt, _, _) in sources {
        assert!(
            evidence.iter().any(|item| {
                item.source_uri.as_deref() == Some(url)
                    && item.excerpt == excerpt
                    && item.source_kind == "web"
            }),
            "missing accepted source evidence for turn {turn}"
        );
    }
    let decisions = store.recent_maintenance_decisions(10).unwrap();
    assert_eq!(
        decisions
            .iter()
            .filter(|decision| decision.outcome == "automatic_evidence_acceptance")
            .count(),
        2
    );

    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn scholar_auto_reviews_direct_findings_from_distinct_hosts_at_configured_default() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let source_passages = [
        (
            1,
            "https://observatory-one.example/reports/mars",
            "Mars is a planet. The observatory's report details its telescope schedule, image calibration, and the team's analysis of observations gathered during a long southern-sky survey.",
        ),
        (
            2,
            "https://astronomy-journal.example/articles/mars",
            "Mars is a planet. Researchers at a separate institute describe their orbital measurements, atmospheric instruments, and analysis of a second dataset collected over several seasons.",
        ),
    ];

    for (turn, url, passage) in source_passages {
        store
            .enqueue_research("automatic-review", turn, "What kind of object is Mars?")
            .unwrap();
        let job = store.claim_scholar_job().unwrap().unwrap();
        store
            .finish_scholar_job_with_profile_and_relevance(
                &job.id,
                "Mars report",
                url,
                passage,
                Some(ScholarFinding {
                    subject: "Mars".into(),
                    relation: "is a".into(),
                    object: "planet".into(),
                    evidence_text: "Mars is a planet.".into(),
                    confidence: 0.90,
                    valid_from: None,
                    valid_until: None,
                    source_quality: "unknown".into(),
                    source_updated_at: None,
                }),
                None,
                &ScholarSourceProfile::default(),
                "direct",
            )
            .unwrap();

        if turn == 1 {
            let staged = store.relevant_scholar_evidence("Mars planet", 5).unwrap();
            assert_eq!(staged.len(), 1);
            assert_eq!(staged[0].source_url, url);
            assert_eq!(staged[0].evidence_text, "Mars is a planet.");
            assert_eq!(staged[0].gate_status, "pending_review");
            assert!(store.active_semantic_claims().unwrap().is_empty());
            assert_eq!(store.corroboration_queue(10).unwrap().len(), 1);
        }
    }
    assert!(
        store
            .claim_corroboration_task(false, false, 1_800_000, 3_600_000)
            .unwrap()
            .is_none()
    );
    assert_eq!(store.corroboration_queue(10).unwrap()[0].status, "done");

    let claims = store.active_semantic_claims().unwrap();
    assert_eq!(claims.len(), 1);
    let evidence = store.claim_evidence(claims[0].claim.id).unwrap();
    assert_eq!(evidence.len(), 2);
    assert!(source_passages.iter().all(|(_, url, _)| {
        evidence
            .iter()
            .any(|item| item.source_uri.as_deref() == Some(*url))
    }));
    assert!(
        store
            .relevant_scholar_evidence("Mars planet", 5)
            .unwrap()
            .is_empty()
    );
    for id in [1, 2] {
        assert_eq!(
            store.scholar_source_expertise_reviews(id).unwrap()[0].review_method,
            "automatic"
        );
        assert_eq!(
            store.scholar_source_origin_reviews(id).unwrap()[0].review_method,
            "automatic"
        );
    }
    assert_eq!(
        store
            .recent_maintenance_decisions(10)
            .unwrap()
            .iter()
            .filter(|decision| decision.outcome == "automatic_evidence_acceptance")
            .count(),
        2
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn scholar_auto_promotion_waits_until_active_map_conflict_is_resolved() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .connection
        .execute(
            "INSERT INTO semantic_claims
                (subject, relation, object, source, created_at)
             VALUES ('Mars', 'has moons', 'one', 'old source', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    let conflicting_id = store.connection.last_insert_rowid();

    for (turn, url, passage) in [
        (
            1,
            "https://observatory-a.example/mars",
            "Mars has two moons. A separate observing report describes long-term images and telescope calibration from a southern survey.",
        ),
        (
            2,
            "https://journal-b.example/mars",
            "Mars has two moons. Another research group analyzed orbital motion and observations collected across multiple seasons.",
        ),
    ] {
        store
            .enqueue_research(
                "conflicting-research",
                turn,
                "How many moons does Mars have?",
            )
            .unwrap();
        let job = store.claim_scholar_job().unwrap().unwrap();
        store
            .finish_scholar_job_with_profile_and_relevance(
                &job.id,
                "Mars moons",
                url,
                passage,
                Some(ScholarFinding {
                    subject: "Mars".into(),
                    relation: "has moons".into(),
                    object: "two".into(),
                    evidence_text: "Mars has two moons.".into(),
                    confidence: 0.94,
                    valid_from: None,
                    valid_until: None,
                    source_quality: "unknown".into(),
                    source_updated_at: None,
                }),
                None,
                &ScholarSourceProfile::default(),
                "direct",
            )
            .unwrap();
    }
    assert_eq!(store.active_semantic_claims().unwrap().len(), 1);
    assert_eq!(
        store
            .scholar_jobs()
            .unwrap()
            .iter()
            .filter(|(job, _)| job.status == "needs_review")
            .count(),
        2
    );

    store
        .resolve_semantic_claim(conflicting_id, "retract", "The source was disproven")
        .unwrap();
    assert_eq!(
        store
            .automatically_review_pending_scholar_evidence()
            .unwrap(),
        1
    );
    let claims = store.active_semantic_claims().unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].claim.object, "two");
    assert_eq!(store.claim_evidence(claims[0].claim.id).unwrap().len(), 2);
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn matching_verified_publication_ids_do_not_satisfy_secondary_corroboration() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let mut findings = Vec::new();
    for (turn, url) in [
        (1, "https://first.example/mars"),
        (2, "https://mirror.example/mars"),
    ] {
        store
            .enqueue_research("same-origin", turn, "Mars planet")
            .unwrap();
        let job = store.claim_scholar_job().unwrap().unwrap();
        let passage = "Mars is a planet. A distinct contextual sentence.";
        store
            .finish_scholar_job_with_profile(
                &job.id,
                "Mars",
                url,
                passage,
                Some(ScholarFinding {
                    subject: "Mars".into(),
                    relation: "is a".into(),
                    object: "planet".into(),
                    evidence_text: "Mars is a planet.".into(),
                    confidence: 0.99,
                    valid_from: None,
                    valid_until: None,
                    source_quality: "unknown".into(),
                    source_updated_at: None,
                }),
                None,
                &ScholarSourceProfile::default(),
            )
            .unwrap();
        let finding = store.scholar_findings(&job.id).unwrap().remove(0);
        store
            .review_scholar_source_expertise(
                finding.id,
                "secondary",
                true,
                None,
                "This is a specialist astronomy reference.",
            )
            .unwrap();
        store
            .review_scholar_source_origin(
                finding.id,
                "report:shared-original",
                "The page attributes this statement to the same original report.",
            )
            .unwrap();
        findings.push(finding.id);
    }
    for id in findings {
        let assessment = store.scholar_source_assessment(id).unwrap().unwrap();
        assert_eq!(assessment.gate_status, "awaiting_corroboration");
    }
    assert!(store.active_semantic_claims().unwrap().is_empty());
    let _ = std::fs::remove_file(path);
}

#[test]
fn one_job_keeps_wikipedia_and_fetched_source_claims_with_separate_assessments() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("multiple-sources", 1, "What is Mars?")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    let make_finding = |quality: &str| ScholarFinding {
        subject: "Mars".into(),
        relation: "is a".into(),
        object: "planet".into(),
        evidence_text: "Mars is a planet.".into(),
        confidence: 0.99,
        valid_from: None,
        valid_until: None,
        source_quality: quality.into(),
        source_updated_at: None,
    };
    store
        .finish_scholar_job_with_evidence(
            &job.id,
            &[
                ScholarEvidence {
                    source_title: "Mars".into(),
                    source_url: "https://en.wikipedia.org/wiki/Mars".into(),
                    source_excerpt: "Mars is a planet.".into(),
                    finding: Some(make_finding("reference")),
                    profile: ScholarSourceProfile::default(),
                    research_relevance: "direct".into(),
                },
                ScholarEvidence {
                    source_title: "Mars overview".into(),
                    source_url: "https://en.wikipedia.org/wiki/Mars#overview".into(),
                    source_excerpt: "Mars is a planet.".into(),
                    finding: Some(make_finding("unknown")),
                    profile: ScholarSourceProfile::default(),
                    research_relevance: "direct".into(),
                },
            ],
            None,
        )
        .unwrap();
    let findings = store.scholar_findings(&job.id).unwrap();
    assert_eq!(findings.len(), 2);
    assert_eq!(findings[0].status, "needs_review");
    assert_eq!(
        findings[1].source_url,
        "https://en.wikipedia.org/wiki/Mars#overview"
    );
    assert_eq!(findings[1].evidence_text, "Mars is a planet.");
    let external_assessment = store
        .scholar_source_assessment(findings[1].id)
        .unwrap()
        .unwrap();
    assert_eq!(external_assessment.gate_status, "awaiting_corroboration");
    assert_eq!(external_assessment.direct_support, Some(true));
    assert!(!external_assessment.independence_verified);
    assert!(store.active_semantic_claims().unwrap().is_empty());
    assert_eq!(store.scholar_jobs().unwrap()[0].0.status, "needs_review");
    store
        .review_scholar_finding(findings[1].id, true, "confirmed against source")
        .unwrap();
    let claims = store.active_semantic_claims().unwrap();
    assert_eq!(claims.len(), 1);
    let evidence = store.claim_evidence(claims[0].claim.id).unwrap();
    assert!(evidence.iter().any(|item| {
        item.source_uri.as_deref() == Some("https://en.wikipedia.org/wiki/Mars#overview")
            && item.excerpt == "Mars is a planet."
    }));
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn robots_denials_skip_only_the_recorded_path_and_expire_across_restarts() {
    let path = temp_path();
    let store = LearningStore::open(&path).unwrap();
    store
        .remember_robots_block(
            "example.org",
            "/private/article?view=full",
            "/private/article",
            "robots.txt disallows this source path",
            false,
        )
        .unwrap();
    assert!(
        store
            .robots_blocked("example.org", "/private/article?view=full")
            .unwrap()
    );
    assert!(
        !store
            .robots_blocked("example.org", "/public/article")
            .unwrap()
    );
    store
        .remember_robots_block(
            "closed.example.org",
            "/first",
            "/first",
            "robots.txt is access denied",
            true,
        )
        .unwrap();
    assert!(
        store
            .robots_blocked("closed.example.org", "/another")
            .unwrap()
    );
    drop(store);
    let store = LearningStore::open(&path).unwrap();
    let blocks = store.scholar_robots_blocks(10).unwrap();
    assert_eq!(blocks.len(), 2);
    assert!(
        blocks
            .iter()
            .any(|block| block.display_path == "/private/article")
    );
    assert!(blocks.iter().any(|block| block.display_path == "/*"));
    store
        .connection
        .execute(
            "UPDATE scholar_robots_blocks SET expires_at = '2000-01-01T00:00:00.000Z'",
            [],
        )
        .unwrap();
    assert!(
        !store
            .robots_blocked("example.org", "/private/article?view=full")
            .unwrap()
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn robots_cache_migration_imports_prior_denied_candidates() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("robots-migration", 1, "topic")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    store
        .record_typed_search_candidate(
            &job.id,
            "serpapi",
            "topic",
            "organic_result",
            "Private article",
            "https://example.org/private?view=full",
            "topic",
            "{}",
        )
        .unwrap();
    let candidate = store.scholar_search_candidates(&job.id).unwrap().remove(0);
    store
        .record_scholar_candidate_fetch_result(
            candidate.id,
            "skipped",
            "disallowed",
            Some("robots.txt disallows the candidate path for CommunityLabScholar"),
            "skipped",
            None,
            None,
        )
        .unwrap();
    store
        .connection
        .execute_batch("PRAGMA user_version = 28;")
        .unwrap();
    drop(store);
    let store = LearningStore::open(&path).unwrap();
    assert!(
        store
            .robots_blocked("example.org", "/private?view=full")
            .unwrap()
    );
    assert_eq!(store.scholar_robots_blocks(10).unwrap().len(), 1);
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn scholar_http_slots_enforce_global_and_per_host_caps_and_recover_expired_leases() {
    let path = temp_path();
    let mut first = LearningStore::open(&path).unwrap();
    let mut second = LearningStore::open(&path).unwrap();
    let a = first
        .reserve_host_request_with_interval("one.example", "tavily_search", 10_000)
        .unwrap();
    let b = second
        .reserve_host_request_with_interval("two.example", "external_robots_txt", 10_000)
        .unwrap();
    let same_host = first
        .reserve_host_request_with_interval("one.example", "external_page_content", 10_000)
        .unwrap_err();
    assert_eq!(same_host.kind(), std::io::ErrorKind::WouldBlock);
    let global = first
        .reserve_host_request_with_interval("three.example", "serpapi_search", 10_000)
        .unwrap_err();
    assert_eq!(global.kind(), std::io::ErrorKind::WouldBlock);

    first
        .connection
        .execute(
            "UPDATE host_request_log SET lease_until_ms = 0 WHERE id = ?1",
            [a.id],
        )
        .unwrap();
    let recovered = first
        .reserve_host_request_with_interval("one.example", "external_page_content", 10_000)
        .unwrap();
    second.complete_host_request(b.id, Some(200), None).unwrap();
    first
        .complete_host_request(recovered.id, Some(200), None)
        .unwrap();
    drop(first);
    drop(second);
    let _ = std::fs::remove_file(path);
}

#[test]
fn scholar_host_retry_after_and_policy_cache_survive_restarts() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let reservation = store
        .reserve_host_request_with_interval("limited.example", "external_robots_txt", 10_000)
        .unwrap();
    store
        .complete_host_request_with_retry_after(
            reservation.id,
            Some(429),
            Some(120_000),
            Some("rate limited"),
        )
        .unwrap();
    assert_eq!(
        store
            .reserve_host_request_with_interval("limited.example", "external_page_content", 10_000)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
    store
        .cache_site_policy(
            "https://limited.example",
            "external_robots_txt",
            200,
            "text/plain",
            "User-agent: *\nDisallow: /private",
            60_000,
        )
        .unwrap();
    drop(store);

    let store = LearningStore::open(&path).unwrap();
    let mut store = store;
    assert_eq!(
        store
            .reserve_host_request_with_interval("limited.example", "external_page_content", 10_000)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock,
        "Retry-After must remain active after a restart"
    );
    let cached = store
        .cached_site_policy("https://limited.example", "external_robots_txt")
        .unwrap()
        .unwrap();
    assert_eq!(cached.0, 200);
    assert_eq!(cached.1, "text/plain");
    assert!(cached.2.contains("Disallow: /private"));
    assert!(
        store
            .cache_site_policy(
                "https://limited.example",
                "external_robots_txt",
                302,
                "text/plain",
                "redirect",
                60_000,
            )
            .is_err()
    );
    store
        .connection
        .execute(
            "UPDATE scholar_site_policy_cache SET expires_at_ms = 0 WHERE origin = ?1",
            ["https://limited.example"],
        )
        .unwrap();
    assert!(
        store
            .cached_site_policy("https://limited.example", "external_robots_txt")
            .unwrap()
            .is_none()
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn schema_v37_migrates_persisted_http_safety_state() {
    let path = temp_path();
    let store = LearningStore::open(&path).unwrap();
    store
        .connection
        .execute_batch(
            "DROP TABLE scholar_site_policy_cache;
             DROP TABLE scholar_host_backoff;
             DROP INDEX host_request_log_by_lease;
             ALTER TABLE host_request_log DROP COLUMN lease_until_ms;
             PRAGMA user_version = 37;",
        )
        .unwrap();
    drop(store);

    let mut migrated = LearningStore::open(&path).unwrap();
    let version: i64 = migrated
        .connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 38);
    migrated
        .reserve_host_request_with_interval("migration.example", "external_robots_txt", 10_000)
        .unwrap();
    assert!(
        migrated
            .cached_site_policy("https://migration.example", "external_robots_txt")
            .unwrap()
            .is_none()
    );
    drop(migrated);
    let _ = std::fs::remove_file(path);
}

#[test]
fn scholar_extraction_attempts_keep_both_passage_decisions_across_restarts() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("extraction-attempts", 1, "Mars composition")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    store
        .record_typed_search_candidate(
            &job.id,
            "serpapi",
            "Mars composition",
            "organic_result",
            "Mars overview",
            "https://example.org/mars",
            "Mars composition",
            "{}",
        )
        .unwrap();
    let candidate_id = store.scholar_search_candidates(&job.id).unwrap()[0].id;
    store
        .record_scholar_extraction_attempt(
            candidate_id,
            1,
            "# Navigation\nMars overview",
            "no_claim",
            "model returned no claim",
            Some("{\"claim\":null}"),
        )
        .unwrap();
    store
        .record_scholar_extraction_attempt(
            candidate_id,
            2,
            "Mars is a planet.",
            "accepted",
            "exact passage and claim term checks passed",
            Some("{\"claim\":{}}"),
        )
        .unwrap();
    assert!(
        store
            .record_scholar_extraction_attempt(candidate_id, 4, "Mars", "no_claim", "invalid", None)
            .is_err()
    );
    drop(store);
    let store = LearningStore::open(&path).unwrap();
    let attempts = store.scholar_extraction_attempts(candidate_id).unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].status, "no_claim");
    assert_eq!(attempts[1].passage, "Mars is a planet.");
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn source_specific_list_is_directly_supported_but_held_for_review() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("list-evidence", 1, "What are ingredients in chicken soup?")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    let passage =
        "## Simple Chicken Soup Ingredients\n* Chicken breasts\n* Chicken stock\n* Carrots";
    store
        .finish_scholar_job_with_evidence(
            &job.id,
            &[ScholarEvidence {
                source_title: "Simple Chicken Soup".into(),
                source_url: "https://example.org/chicken-soup".into(),
                source_excerpt: passage.into(),
                finding: Some(ScholarFinding {
                    subject: "Simple Chicken Soup".into(),
                    relation: "lists ingredients".into(),
                    object: "Chicken breasts; Chicken stock; Carrots".into(),
                    evidence_text: passage.into(),
                    confidence: 0.8,
                    valid_from: None,
                    valid_until: None,
                    source_quality: "unknown".into(),
                    source_updated_at: None,
                }),
                profile: ScholarSourceProfile::default(),
                research_relevance: "related".into(),
            }],
            None,
        )
        .unwrap();
    let finding = store.scholar_findings(&job.id).unwrap().remove(0);
    let assessment = store
        .scholar_source_assessment(finding.id)
        .unwrap()
        .unwrap();
    assert_eq!(assessment.direct_support, Some(true));
    assert_eq!(assessment.topic_class, "historical_scientific");
    assert_eq!(assessment.gate_status, "pending_review");
    assert!(store.active_semantic_claims().unwrap().is_empty());
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn version_30_extraction_attempts_upgrade_for_structured_list_fallback() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("attempt-migration", 1, "topic")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    store
        .record_typed_search_candidate(
            &job.id,
            "serpapi",
            "topic",
            "organic_result",
            "Topic",
            "https://example.org/topic",
            "topic",
            "{}",
        )
        .unwrap();
    let candidate_id = store.scholar_search_candidates(&job.id).unwrap()[0].id;
    store
        .connection
        .execute_batch(
            "DROP TABLE scholar_extraction_attempts;
             CREATE TABLE scholar_extraction_attempts (
                id INTEGER PRIMARY KEY,
                candidate_id INTEGER NOT NULL REFERENCES scholar_search_candidates(id),
                attempt_index INTEGER NOT NULL CHECK(attempt_index BETWEEN 1 AND 2),
                passage TEXT NOT NULL,
                status TEXT NOT NULL CHECK(status IN
                    ('accepted', 'no_claim', 'invalid_json', 'model_error',
                     'low_confidence', 'ungrounded', 'filtered_expansion')),
                reason TEXT NOT NULL,
                model_output TEXT,
                created_at TEXT NOT NULL,
                UNIQUE(candidate_id, attempt_index)
             );
             PRAGMA user_version = 30;",
        )
        .unwrap();
    store
        .record_scholar_extraction_attempt(candidate_id, 1, "Topic text", "no_claim", "none", None)
        .unwrap();
    drop(store);
    let store = LearningStore::open(&path).unwrap();
    store
        .record_scholar_extraction_attempt(
            candidate_id,
            3,
            "## Topic Items\n* A\n* B",
            "structured_list",
            "heading and bullets",
            None,
        )
        .unwrap();
    assert_eq!(
        store
            .scholar_extraction_attempts(candidate_id)
            .unwrap()
            .len(),
        2
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn recent_primary_requires_verified_expertise_and_publication_date() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let today: String = store
        .connection
        .query_row("SELECT date('now')", [], |row| row.get(0))
        .unwrap();
    store
        .enqueue_research("recent", 1, "current GPU specification")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    store
        .finish_scholar_job_with_profile(
            &job.id,
            "Example GPU",
            "https://vendor.example.com/specs",
            "Example GPU has 24 GB memory.",
            Some(ScholarFinding {
                subject: "Example GPU".into(),
                relation: "has".into(),
                object: "24 GB memory".into(),
                evidence_text: "Example GPU has 24 GB memory.".into(),
                confidence: 0.99,
                valid_from: None,
                valid_until: None,
                source_quality: "primary".into(),
                source_updated_at: None,
            }),
            None,
            &ScholarSourceProfile {
                published_at: Some(today.clone()),
                topical_expertise_verified: true,
                ..Default::default()
            },
        )
        .unwrap();
    let finding = store.scholar_findings(&job.id).unwrap().remove(0);
    let assessment = store
        .scholar_source_assessment(finding.id)
        .unwrap()
        .unwrap();
    assert_eq!(assessment.topic_class, "recent");
    assert_eq!(assessment.published_at.as_deref(), Some(today.as_str()));
    assert_eq!(assessment.gate_status, "eligible");
    assert_eq!(store.active_semantic_claims().unwrap().len(), 1);
    let _ = std::fs::remove_file(path);
}

#[test]
fn scholar_conflicts_depend_on_overlapping_claim_periods() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .connection
        .execute(
            "INSERT INTO semantic_claims
                (subject, relation, object, source, created_at, valid_from, valid_until)
             VALUES ('Alex', 'served as mayor', 'Town A', 'archive', '2025-01-01T00:00:00Z',
                     '2000-01-01', '2010-12-31')",
            [],
        )
        .unwrap();

    store
        .enqueue_research("conversation", 1, "Alex's mayoral service")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    let excerpt = "Alex served as mayor in Town B from 2015-01-01 through 2020-01-01.";
    store
        .finish_scholar_job(
            &job.id,
            "Alex",
            "https://en.wikipedia.org/wiki/Alex",
            excerpt,
            Some(ScholarFinding {
                subject: "Alex".into(),
                relation: "served as mayor in".into(),
                object: "Town B".into(),
                evidence_text: excerpt.into(),
                confidence: 0.99,
                valid_from: Some("2015-01-01".into()),
                valid_until: Some("2020-01-01".into()),
                source_quality: "reference".into(),
                source_updated_at: None,
            }),
            None,
        )
        .unwrap();
    let first_finding = store.scholar_findings(&job.id).unwrap()[0].id;
    assert_eq!(
        store
            .scholar_source_assessment(first_finding)
            .unwrap()
            .unwrap()
            .gate_status,
        "awaiting_corroboration"
    );
    store
        .review_scholar_finding(first_finding, true, "Reviewed the dated source passage")
        .unwrap();
    let claims = store.active_semantic_claims().unwrap();
    assert_eq!(claims.len(), 2, "non-overlapping periods should coexist");
    assert_eq!(claims[1].claim.status, "active");
    assert_eq!(claims[1].claim.valid_from.as_deref(), Some("2015-01-01"));

    store
        .enqueue_research("conversation", 2, "Alex's later mayoral service")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    let excerpt = "Alex served as mayor in Town C from 2019-01-01 through 2025-01-01.";
    store
        .finish_scholar_job(
            &job.id,
            "Alex",
            "https://en.wikipedia.org/wiki/Alex#later",
            excerpt,
            Some(ScholarFinding {
                subject: "Alex".into(),
                relation: "served as mayor in".into(),
                object: "Town C".into(),
                evidence_text: excerpt.into(),
                confidence: 0.99,
                valid_from: Some("2019-01-01".into()),
                valid_until: Some("2025-01-01".into()),
                source_quality: "reference".into(),
                source_updated_at: None,
            }),
            None,
        )
        .unwrap();
    let claims = store.active_semantic_claims().unwrap();
    assert_eq!(
        claims.len(),
        2,
        "overlapping alternative values should not auto-promote"
    );
    assert_eq!(claims[1].claim.status, "challenged");
    assert_eq!(
        store.claim_evidence(claims[1].claim.id).unwrap()[1].stance,
        "contradicts"
    );
    let finding_id = store.scholar_findings(&job.id).unwrap()[0].id;
    store
        .review_scholar_finding(finding_id, true, "accepted for its stated period")
        .unwrap();
    let claims = store.active_semantic_claims().unwrap();
    assert_eq!(
        claims.len(),
        3,
        "accepting a partial overlap must retain the old period"
    );
    assert_eq!(claims[1].claim.status, "challenged");
    assert_eq!(claims[1].claim.valid_until.as_deref(), Some("2020-01-01"));
    assert_eq!(claims[2].claim.valid_from.as_deref(), Some("2019-01-01"));
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn scholar_queue_promotes_only_source_supported_claims() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("conversation", 1, "photosynthesis")
        .unwrap();
    store
        .enqueue_research("conversation", 1, "photosynthesis")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    assert_eq!(job.question, "photosynthesis");
    assert!(store.claim_scholar_job().unwrap().is_none());
    store
        .finish_scholar_job(
            &job.id,
            "Photosynthesis",
            "https://en.wikipedia.org/wiki/Photosynthesis",
            "Photosynthesis converts light energy into chemical energy.",
            Some(ScholarFinding {
                subject: "Photosynthesis".into(),
                relation: "converts".into(),
                object: "light energy into chemical energy".into(),
                evidence_text: "Photosynthesis converts light energy into chemical energy.".into(),
                confidence: 0.98,
                valid_from: None,
                valid_until: None,
                source_quality: "reference".into(),
                source_updated_at: None,
            }),
            None,
        )
        .unwrap();
    let first_finding = store.scholar_findings(&job.id).unwrap()[0].id;
    assert_eq!(
        store
            .scholar_source_assessment(first_finding)
            .unwrap()
            .unwrap()
            .gate_status,
        "awaiting_corroboration"
    );
    store
        .review_scholar_finding(first_finding, true, "Reviewed the source passage")
        .unwrap();
    let claims = store.active_semantic_claims().unwrap();
    assert_eq!(claims.len(), 1);
    assert!(claims[0].sources[0].contains("en.wikipedia.org/wiki/Photosynthesis"));
    assert_eq!(
        store
            .relevant_semantic_claims("What is photosynthesis?", 5)
            .unwrap()[0]
            .source,
        "https://en.wikipedia.org/wiki/Photosynthesis"
    );

    store.enqueue_research("conversation", 2, "Mars").unwrap();
    let second = store.claim_scholar_job().unwrap().unwrap();
    store
        .finish_scholar_job(
            &second.id,
            "Mars",
            "https://en.wikipedia.org/wiki/Mars",
            "Mars is a planet.",
            Some(ScholarFinding {
                subject: "Mars".into(),
                relation: "is a".into(),
                object: "planet".into(),
                evidence_text: "Mars has oceans.".into(),
                confidence: 0.99,
                valid_from: None,
                valid_until: None,
                source_quality: "reference".into(),
                source_updated_at: None,
            }),
            None,
        )
        .unwrap();
    assert_eq!(store.active_semantic_claims().unwrap().len(), 1);
    assert_eq!(
        store
            .scholar_jobs()
            .unwrap()
            .into_iter()
            .find(|(job, _)| job.id == second.id)
            .unwrap()
            .0
            .status,
        "needs_review"
    );

    store
        .enqueue_research("conversation", 3, "planet Mars")
        .unwrap();
    let third = store.claim_scholar_job().unwrap().unwrap();
    store
        .finish_scholar_job(
            &third.id,
            "Mars",
            "https://en.wikipedia.org/wiki/Mars",
            "Mars is a planet.",
            Some(ScholarFinding {
                subject: "Mars".into(),
                relation: "is a".into(),
                object: "planet".into(),
                evidence_text: "Mars is a planet.".into(),
                confidence: 0.8,
                valid_from: None,
                valid_until: None,
                source_quality: "reference".into(),
                source_updated_at: None,
            }),
            None,
        )
        .unwrap();
    let finding = store.scholar_findings(&third.id).unwrap().remove(0);
    assert_eq!(finding.status, "needs_review");
    store
        .review_scholar_finding(finding.id, true, "Checked cited excerpt")
        .unwrap();
    assert_eq!(store.active_semantic_claims().unwrap().len(), 2);
    let _ = std::fs::remove_file(path);
}

#[test]
fn imports_claims_and_is_idempotent() {
    let path = temp_path();
    let _ = std::fs::remove_file(&path);
    let mut map = KnowledgeMap::default();
    let id = map
        .add("Lyra", "name was given by", "the user", "user message 26")
        .unwrap();
    map.revise(
        id,
        "Lyra",
        "name was given by",
        "the user",
        "correction from user message 27",
    )
    .unwrap();

    {
        let mut store = LearningStore::open(&path).unwrap();
        assert_eq!(
            store.import_knowledge(&map).unwrap(),
            ImportSummary {
                inserted: 2,
                already_present: 0
            }
        );
        assert_eq!(store.claim_count().unwrap(), 2);
        assert_eq!(
            store.import_knowledge(&map).unwrap(),
            ImportSummary {
                inserted: 0,
                already_present: 2
            }
        );
    }

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}

#[test]
fn automatic_promotion_confidence_is_configurable_and_bounded() {
    assert_eq!(super::parse_automatic_promotion_confidence(None), 0.90);
    assert_eq!(
        super::parse_automatic_promotion_confidence(Some("0.75")),
        0.75
    );
    for invalid in ["nope", "-0.1", "1.1", "NaN", "inf"] {
        assert_eq!(
            super::parse_automatic_promotion_confidence(Some(invalid)),
            0.90
        );
    }
}

#[test]
fn staging_same_turn_is_idempotent_and_preserves_raw_text() {
    let path = temp_path();
    let _ = std::fs::remove_file(&path);
    let store = LearningStore::open(&path).unwrap();
    let raw = "  I gave it to you.  ";
    let first = store.stage_observation("conversation-a", 3, raw).unwrap();
    let retry = store.stage_observation("conversation-a", 3, raw).unwrap();
    assert_eq!(first, retry);
    let count: i64 = store
        .connection
        .query_row("SELECT COUNT(*) FROM observations", [], |row| row.get(0))
        .unwrap();
    let stored: String = store
        .connection
        .query_row(
            "SELECT raw_text FROM observations WHERE id = ?1",
            [&first],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(stored, raw);
    drop(store);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}

#[test]
fn pending_observations_are_retrieved_by_relevance_and_conversation() {
    let path = temp_path();
    let _ = std::fs::remove_file(&path);
    let store = LearningStore::open(&path).unwrap();
    store
        .stage_observation("conversation-a", 1, "I live in Bristol.")
        .unwrap();
    store
        .stage_observation("conversation-a", 2, "I like painting.")
        .unwrap();
    store
        .stage_observation("conversation-b", 1, "I live in London.")
        .unwrap();
    let current = store
        .stage_observation("conversation-a", 3, "Where do I live?")
        .unwrap();

    let relevant = store
        .relevant_observations("conversation-a", "Where do I live?", &current, 8)
        .unwrap();
    assert_eq!(relevant.len(), 1);
    assert_eq!(relevant[0].raw_text, "I live in Bristol.");

    drop(store);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}

#[test]
fn processing_observations_can_be_reclaimed_after_their_lease_expires() {
    let path = temp_path();
    let _ = std::fs::remove_file(&path);
    let mut store = LearningStore::open(&path).unwrap();
    let id = store
        .stage_observation("conversation-a", 1, "I live in Bristol.")
        .unwrap();
    let first = store.claim_next_observation(600).unwrap().unwrap();
    assert_eq!(first.id, id);
    assert_eq!(first.status, "processing");

    std::thread::sleep(std::time::Duration::from_millis(5));
    drop(store);
    let mut store = LearningStore::open(&path).unwrap();
    let recovered = store.claim_next_observation(0).unwrap().unwrap();
    assert_eq!(recovered.id, id);
    let retry_count: i64 = store
        .connection
        .query_row(
            "SELECT retry_count FROM observations WHERE id = ?1",
            [&id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retry_count, 2);

    drop(store);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}

#[test]
fn candidate_insert_and_observation_status_commit_together() {
    let path = temp_path();
    let _ = std::fs::remove_file(&path);
    let mut store = LearningStore::open(&path).unwrap();
    let id = store
        .stage_observation("conversation-a", 1, "I live in Bristol.")
        .unwrap();
    store.claim_next_observation(600).unwrap().unwrap();
    let claim = CandidateClaim {
        subject: "the user".into(),
        relation: "lives in".into(),
        object: "Bristol".into(),
        evidence_text: "I live in Bristol".into(),
        context_message_ids: Vec::new(),
        resolution: None,
        confidence: 0.92,
        status: "proposed".into(),
        decision_reason: None,
        valid_from: None,
        valid_until: None,
    };
    store.finish_extraction(&id, &[claim], None).unwrap();
    let observation_status: String = store
        .connection
        .query_row(
            "SELECT status FROM observations WHERE id = ?1",
            [&id],
            |row| row.get(0),
        )
        .unwrap();
    let (candidate_status, evidence): (String, String) = store
        .connection
        .query_row(
            "SELECT status, evidence_text FROM candidate_claims WHERE observation_id = ?1",
            [&id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(observation_status, "needs_review");
    assert_eq!(candidate_status, "needs_review");
    assert_eq!(evidence, "I live in Bristol");

    drop(store);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}

#[test]
fn high_confidence_claims_promote_and_equivalent_evidence_attaches() {
    let path = temp_path();
    let _ = std::fs::remove_file(&path);
    let mut store = LearningStore::open(&path).unwrap();
    let first_id = store
        .stage_observation("conversation-a", 1, "I live in Bristol.")
        .unwrap();
    store.claim_next_observation(600).unwrap().unwrap();
    let make_claim = || CandidateClaim {
        subject: " the   user ".into(),
        relation: "lives in".into(),
        object: "Bristol".into(),
        evidence_text: "I live in Bristol.".into(),
        context_message_ids: Vec::new(),
        resolution: None,
        confidence: 0.97,
        status: "proposed".into(),
        decision_reason: None,
        valid_from: None,
        valid_until: None,
    };
    store
        .finish_extraction(&first_id, &[make_claim()], None)
        .unwrap();
    let second_id = store
        .stage_observation("conversation-a", 2, "I live in Bristol.")
        .unwrap();
    store.claim_next_observation(600).unwrap().unwrap();
    store
        .finish_extraction(&second_id, &[make_claim()], None)
        .unwrap();
    let semantic_count: i64 = store
        .connection
        .query_row("SELECT COUNT(*) FROM semantic_claims", [], |row| row.get(0))
        .unwrap();
    let source_count: i64 = store
        .connection
        .query_row("SELECT COUNT(*) FROM semantic_claim_sources", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(semantic_count, 1);
    assert_eq!(source_count, 2);
    let proposals: i64 = store.connection.query_row(
            "SELECT COUNT(*) FROM role_proposals WHERE role = 'curator' AND source_type = 'candidate_claim'",
            [], |row| row.get(0),
        ).unwrap();
    let applied: i64 = store
        .connection
        .query_row(
            "SELECT COUNT(*) FROM policy_decisions d JOIN role_proposals p ON p.id = d.proposal_id
             WHERE p.role = 'curator' AND d.outcome = 'applied' AND d.resulting_state = 'active'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(proposals, 2);
    assert_eq!(applied, 2);
    let history = store.recent_maintenance_decisions(1).unwrap();
    assert_eq!(history[0].role, "curator");
    assert_eq!(history[0].resulting_claim_id, Some(1));
    assert_eq!(
        store.relevant_semantic_claims("Bristol", 5).unwrap().len(),
        1
    );
    drop(store);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}

#[test]
fn confirmation_promotes_a_matching_review_candidate_and_keeps_both_sources() {
    let path = temp_path();
    let _ = std::fs::remove_file(&path);
    let mut store = LearningStore::open(&path).unwrap();
    let first_id = store
        .stage_observation("conversation-a", 1, "I live in Shifnal.")
        .unwrap();
    store.claim_next_observation(600).unwrap().unwrap();
    store
        .finish_extraction(
            &first_id,
            &[CandidateClaim {
                subject: "the user".into(),
                relation: "lives in".into(),
                object: "Shifnal".into(),
                evidence_text: "I live in Shifnal.".into(),
                context_message_ids: Vec::new(),
                resolution: None,
                confidence: 0.0,
                status: "needs_review".into(),
                decision_reason: Some("extractor confidence was missing".into()),
                valid_from: None,
                valid_until: None,
            }],
            None,
        )
        .unwrap();

    let confirmation_id = store
        .stage_observation("conversation-a", 2, "Yes, that's right.")
        .unwrap();
    store.claim_next_observation(600).unwrap().unwrap();
    store
        .finish_extraction(
            &confirmation_id,
            &[CandidateClaim {
                subject: "the user".into(),
                relation: "lives in".into(),
                object: "Shifnal".into(),
                evidence_text: "Yes, that's right.".into(),
                context_message_ids: vec!["turn:1:user".into(), "turn:1:assistant".into()],
                resolution: Some("The user confirms the location stated earlier.".into()),
                confidence: 0.99,
                status: "proposed".into(),
                decision_reason: None,
                valid_from: None,
                valid_until: None,
            }],
            None,
        )
        .unwrap();

    let claims = store.active_semantic_claims().unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].claim.object, "Shifnal");
    assert_eq!(claims[0].sources.len(), 2);
    assert!(
        claims[0]
            .sources
            .iter()
            .any(|source| source.contains("I live in Shifnal."))
    );
    assert!(
        claims[0]
            .sources
            .iter()
            .any(|source| source.contains("Yes, that's right."))
    );
    drop(store);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}

#[test]
fn self_referential_name_question_finds_a_curated_user_claim() {
    let path = temp_path();
    let _ = std::fs::remove_file(&path);
    let mut store = LearningStore::open(&path).unwrap();
    let id = store
        .stage_observation("conversation-a", 1, "I am Ryan.")
        .unwrap();
    store.claim_next_observation(600).unwrap().unwrap();
    let claim = CandidateClaim {
        subject: "the user".into(),
        relation: "is named".into(),
        object: "Ryan".into(),
        evidence_text: "I am Ryan.".into(),
        context_message_ids: Vec::new(),
        resolution: None,
        confidence: 0.98,
        status: "proposed".into(),
        decision_reason: None,
        valid_from: None,
        valid_until: None,
    };
    store.finish_extraction(&id, &[claim], None).unwrap();
    let matches = store
        .relevant_semantic_claims("Do you know my name?", 5)
        .unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].object, "Ryan");
    drop(store);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}

#[test]
fn personal_questions_retrieve_unreviewed_candidate_interpretations() {
    let path = temp_path();
    let _ = std::fs::remove_file(&path);
    let mut store = LearningStore::open(&path).unwrap();
    let id = store
        .stage_observation("conversation-a", 1, "I am Ryan.")
        .unwrap();
    store.claim_next_observation(600).unwrap().unwrap();
    let claim = CandidateClaim {
        subject: "the user".into(),
        relation: "is named".into(),
        object: "Ryan".into(),
        evidence_text: "I am Ryan.".into(),
        context_message_ids: Vec::new(),
        resolution: None,
        confidence: 0.0,
        status: "needs_review".into(),
        decision_reason: Some("missing confidence".into()),
        valid_from: None,
        valid_until: None,
    };
    store.finish_extraction(&id, &[claim], None).unwrap();
    let matches = store
        .relevant_review_candidates("conversation-a", "Do you know my name?", 5)
        .unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].object, "Ryan");
    assert_eq!(matches[0].observation_id, id);
    let current = store
        .stage_observation("conversation-a", 2, "Do you know my name?")
        .unwrap();
    let source = store
        .relevant_observations("conversation-a", "Do you know my name?", &current, 5)
        .unwrap();
    assert_eq!(source.len(), 1);
    assert_eq!(source[0].raw_text, "I am Ryan.");
    drop(store);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}

#[test]
fn maintenance_scan_cursor_covers_claims_in_bounded_persistent_batches() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    for index in 1..=5 {
        store.connection.execute(
                "INSERT INTO semantic_claims (subject, relation, object, source, created_at)
                 VALUES (?1, 'has property', ?2, 'test source', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                rusqlite::params![format!("Topic {index}"), format!("value {index}")],
            ).unwrap();
    }
    let first = store.next_claims_for_maintenance(2).unwrap();
    assert_eq!(
        first.iter().map(|claim| claim.id).collect::<Vec<_>>(),
        [1, 2]
    );
    assert!(
        first
            .iter()
            .all(|claim| claim.relation_count == 1 && claim.evidence_count == 0)
    );
    drop(store);

    let mut store = LearningStore::open(&path).unwrap();
    assert_eq!(
        store
            .next_claims_for_maintenance(2)
            .unwrap()
            .iter()
            .map(|claim| claim.id)
            .collect::<Vec<_>>(),
        [3, 4],
    );
    assert_eq!(store.next_claims_for_maintenance(2).unwrap()[0].id, 5);
    assert_eq!(
        store
            .next_claims_for_maintenance(2)
            .unwrap()
            .iter()
            .map(|claim| claim.id)
            .collect::<Vec<_>>(),
        [1, 2],
        "the cursor wraps after reaching the end so new or revised claims are revisited",
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn claim_review_actions_keep_evidence_and_supersession_history() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    for (subject, relation, object) in [
        ("Example", "has value", "old"),
        ("Example", "has value", "new"),
        ("Example", "has status", "unclear"),
        ("Example", "has note", "temporary"),
    ] {
        store
            .connection
            .execute(
                "INSERT INTO semantic_claims (subject, relation, object, source, created_at)
                 VALUES (?1, ?2, ?3, 'test source', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                rusqlite::params![subject, relation, object],
            )
            .unwrap();
    }
    store
        .resolve_semantic_claim(1, "qualify", "Only partially supported")
        .unwrap();
    assert_eq!(
        store
            .connection
            .query_row(
                "SELECT status FROM semantic_claims WHERE id = 1",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "uncertain",
    );
    assert_eq!(store.claim_evidence(1).unwrap()[0].stance, "qualifies");
    store
        .supersede_semantic_claim(1, 2, "New evidence replaced the old value")
        .unwrap();
    let old = store
        .connection
        .query_row(
            "SELECT status, retracted_by FROM semantic_claims WHERE id = 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .unwrap();
    assert_eq!(old.0, "superseded");
    assert!(old.1.unwrap().contains("claim #2"));
    assert_eq!(store.claim_evidence(2).unwrap()[0].stance, "supports");
    assert_eq!(
        store.claim_count().unwrap(),
        4,
        "superseded and retracted versions remain stored"
    );
    store
        .resolve_semantic_claim(3, "challenge", "Contradictory record found")
        .unwrap();
    store
        .resolve_semantic_claim(4, "retract", "The source was withdrawn")
        .unwrap();
    assert_eq!(store.claim_evidence(3).unwrap()[0].stance, "contradicts");
    assert_eq!(
        store
            .connection
            .query_row(
                "SELECT status FROM semantic_claims WHERE id = 3",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "challenged",
    );
    assert_eq!(
        store
            .connection
            .query_row(
                "SELECT status FROM semantic_claims WHERE id = 4",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "retracted",
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn model_checkpoint_lineage_roundtrips_exact_claim_revisions() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store.connection.execute(
            "INSERT INTO semantic_claims (subject, relation, object, source, created_at)
             VALUES ('Photosynthesis', 'converts', 'light to chemical energy', 'reviewed source', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            [],
        ).unwrap();
    store.approve_model_claim(1, "evidence checked").unwrap();
    let manifest = ModelCheckpointManifest {
        version: 1,
        base_model_dir: "/models/base".into(),
        base_checkpoint: "/models/base/model.safetensors".into(),
        base_checkpoint_sha256: "sha256-base".into(),
        base_adapter_snapshot: None,
        base_adapter_sha256: None,
        included_claims: vec![ConsolidatedClaimRevision {
            id: 1,
            subject: "Photosynthesis".into(),
            relation: "converts".into(),
            object: "light to chemical energy".into(),
            source: "reviewed source".into(),
            valid_from: None,
            valid_until: None,
        }],
        system_prompt: "frozen system prompt".into(),
        max_training_passes: 24,
        loss_threshold: 0.6,
        probe_max_chars: 200,
    };
    let json = serde_json::to_string(&manifest).unwrap();
    let run_id = store
        .record_model_consolidation_with_lineage(
            1,
            "/models/base/model.safetensors",
            "/models/output/model.safetensors",
            0.4,
            "/models/output/consolidation-manifest.json",
            &json,
        )
        .unwrap();
    assert_eq!(store.model_manifest_for_run(run_id).unwrap(), manifest);
    let history = store.model_consolidations().unwrap();
    assert_eq!(history[0].included_claim_ids, [1]);
    assert_eq!(
        history[0].base_checkpoint_sha256.as_deref(),
        Some("sha256-base")
    );
    assert!(store.claim_is_active_and_approved(1).unwrap());
    store
        .connection
        .execute(
            "UPDATE semantic_claims SET status = 'challenged' WHERE id = 1",
            [],
        )
        .unwrap();
    assert!(!store.claim_is_active_and_approved(1).unwrap());
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn personal_claim_retrieval_does_not_surface_a_different_user_fact() {
    let path = temp_path();
    let _ = std::fs::remove_file(&path);
    let mut store = LearningStore::open(&path).unwrap();
    let add_review_candidate =
        |store: &mut LearningStore, turn: usize, text: &str, relation: &str, object: &str| {
            let id = store
                .stage_observation("conversation-a", turn, text)
                .unwrap();
            store.claim_next_observation(600).unwrap().unwrap();
            store
                .finish_extraction(
                    &id,
                    &[CandidateClaim {
                        subject: "the user".into(),
                        relation: relation.into(),
                        object: object.into(),
                        evidence_text: text.into(),
                        context_message_ids: Vec::new(),
                        resolution: None,
                        confidence: 0.0,
                        status: "needs_review".into(),
                        decision_reason: Some("missing confidence".into()),
                        valid_from: None,
                        valid_until: None,
                    }],
                    None,
                )
                .unwrap();
        };
    add_review_candidate(&mut store, 1, "I live in Shifnal.", "lives in", "Shifnal");
    add_review_candidate(&mut store, 2, "My name is Ryan.", "is named", "Ryan");

    let matches = store
        .relevant_review_candidates("conversation-a", "What about my name?", 5)
        .unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].object, "Ryan");
    let current = store
        .stage_observation("conversation-a", 3, "What about my name?")
        .unwrap();
    let observations = store
        .relevant_observations("conversation-a", "What about my name?", &current, 5)
        .unwrap();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].raw_text, "My name is Ryan.");
    drop(store);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}

#[test]
fn contradictory_high_confidence_claim_stays_for_review() {
    let path = temp_path();
    let _ = std::fs::remove_file(&path);
    let mut store = LearningStore::open(&path).unwrap();
    let first_id = store
        .stage_observation("conversation-a", 1, "I live in Bristol.")
        .unwrap();
    store.claim_next_observation(600).unwrap().unwrap();
    let claim = |object: &str, evidence: &str| CandidateClaim {
        subject: "the user".into(),
        relation: "lives in".into(),
        object: object.into(),
        evidence_text: evidence.into(),
        context_message_ids: Vec::new(),
        resolution: None,
        confidence: 0.98,
        status: "proposed".into(),
        decision_reason: None,
        valid_from: None,
        valid_until: None,
    };
    store
        .finish_extraction(&first_id, &[claim("Bristol", "I live in Bristol.")], None)
        .unwrap();
    let second_id = store
        .stage_observation("conversation-a", 2, "I live in London.")
        .unwrap();
    store.claim_next_observation(600).unwrap().unwrap();
    store
        .finish_extraction(&second_id, &[claim("London", "I live in London.")], None)
        .unwrap();
    let (status, reason): (String, Option<String>) = store
        .connection
        .query_row(
            "SELECT status, decision_reason FROM candidate_claims WHERE observation_id = ?1",
            [&second_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "needs_review");
    assert!(
        reason
            .unwrap()
            .contains("conflicts with active semantic claim")
    );
    assert_eq!(store.claim_count().unwrap(), 1);
    drop(store);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}

#[test]
fn user_claims_with_disjoint_validity_periods_can_coexist() {
    fn add(
        store: &mut LearningStore,
        turn: usize,
        text: &str,
        employer: &str,
        valid_from: &str,
        valid_until: &str,
    ) {
        let observation_id = store
            .stage_observation("conversation-a", turn, text)
            .unwrap();
        store.claim_next_observation(600).unwrap().unwrap();
        store
            .finish_extraction(
                &observation_id,
                &[CandidateClaim {
                    subject: "the user".into(),
                    relation: "worked at".into(),
                    object: employer.into(),
                    evidence_text: text.into(),
                    context_message_ids: Vec::new(),
                    resolution: None,
                    confidence: 0.99,
                    status: "proposed".into(),
                    decision_reason: None,
                    valid_from: Some(valid_from.into()),
                    valid_until: Some(valid_until.into()),
                }],
                None,
            )
            .unwrap();
    }
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    add(
        &mut store,
        1,
        "I worked at Acme from 2010-01-01 to 2015-01-01.",
        "Acme",
        "2010-01-01",
        "2015-01-01",
    );
    add(
        &mut store,
        2,
        "I worked at Beta from 2016-01-01 to 2020-01-01.",
        "Beta",
        "2016-01-01",
        "2020-01-01",
    );
    assert_eq!(store.active_semantic_claims().unwrap().len(), 2);

    add(
        &mut store,
        3,
        "I worked at Gamma from 2019-01-01 to 2021-01-01.",
        "Gamma",
        "2019-01-01",
        "2021-01-01",
    );
    let details = store.inbox_summary().unwrap();
    let third = store
        .observation_detail(&details.observations[0].id)
        .unwrap()
        .unwrap();
    assert_eq!(third.candidates[0].status, "needs_review");
    assert_eq!(store.active_semantic_claims().unwrap().len(), 2);
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn inbox_details_and_review_acceptance_preserve_conflict_history() {
    let path = temp_path();
    let _ = std::fs::remove_file(&path);
    let mut store = LearningStore::open(&path).unwrap();
    let first_id = store
        .stage_observation("conversation-a", 1, "I live in Bristol.")
        .unwrap();
    store.claim_next_observation(600).unwrap().unwrap();
    let candidate = |object: &str, text: &str, confidence| CandidateClaim {
        subject: "the user".into(),
        relation: "lives in".into(),
        object: object.into(),
        evidence_text: text.into(),
        context_message_ids: Vec::new(),
        resolution: None,
        confidence,
        status: "proposed".into(),
        decision_reason: None,
        valid_from: None,
        valid_until: None,
    };
    store
        .finish_extraction(
            &first_id,
            &[candidate("Bristol", "I live in Bristol.", 0.97)],
            None,
        )
        .unwrap();
    let second_id = store
        .stage_observation("conversation-a", 2, "I live in London.")
        .unwrap();
    store.claim_next_observation(600).unwrap().unwrap();
    store
        .finish_extraction(
            &second_id,
            &[candidate("London", "I live in London.", 0.98)],
            None,
        )
        .unwrap();
    let inbox = store.inbox_summary().unwrap();
    assert_eq!(inbox.needs_review, 1);
    let detail = store.observation_detail(&second_id).unwrap().unwrap();
    assert_eq!(detail.observation.raw_text, "I live in London.");
    let candidate_id = detail.candidates[0].id.clone();
    store
        .resolve_candidate(&candidate_id, true, "The user corrected their location.")
        .unwrap();
    let claims = store.active_semantic_claims().unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].claim.object, "London");
    assert_eq!(
        claims[0].claim.source_observation_id.as_deref(),
        Some(second_id.as_str())
    );
    assert_eq!(claims[0].sources.len(), 1);
    let old_retracted: Option<String> = store
        .connection
        .query_row(
            "SELECT retracted_by FROM semantic_claims WHERE object = 'Bristol'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        old_retracted
            .unwrap()
            .contains("The user corrected their location.")
    );
    assert_eq!(store.inbox_summary().unwrap().needs_review, 0);
    drop(store);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}
