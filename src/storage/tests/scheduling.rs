//! Storage regressions for scheduling.
use super::*;

#[test]
fn scout_cannot_repeat_completed_conversation_question_under_a_new_topic_key() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("frontend", 1, "Who discovered penicillin?")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    store
        .finish_scholar_job_with_evidence(&job.id, &[], None)
        .unwrap();
    drop(store);
    let mut store = LearningStore::open(&path).unwrap();
    let admitted = store
        .try_enqueue_scout_research(
            "expansion",
            "new-source-key",
            "source:42",
            "who discovered penicillin",
            "Unchanged question from another source",
        )
        .unwrap();
    assert!(matches!(
        admitted,
        crate::storage::ScoutAdmission::Blocked(_)
    ));
    assert_eq!(store.scholar_jobs().unwrap().len(), 1);
    // Explicit user repeats and distinct requested relations remain allowed.
    store
        .enqueue_research("frontend", 2, "Who discovered penicillin?")
        .unwrap();
    assert!(matches!(
        store
            .try_enqueue_scout_research(
                "expansion",
                "different-question",
                "source:43",
                "When was penicillin discovered?",
                "Different requested relation"
            )
            .unwrap(),
        crate::storage::ScoutAdmission::Queued(_)
    ));
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn scout_question_cooldown_expires_but_old_pending_questions_still_block_new_keys() {
    for completed in [false, true] {
        let path = temp_path();
        let mut store = LearningStore::open(&path).unwrap();
        store
            .enqueue_research("frontend", 1, "What is a neutron?")
            .unwrap();
        if completed {
            let job = store.claim_scholar_job().unwrap().unwrap();
            store
                .finish_scholar_job_with_evidence(&job.id, &[], None)
                .unwrap();
        }
        store
            .connection
            .execute(
                "UPDATE scholar_jobs SET created_at=strftime('%Y-%m-%dT%H:%M:%fZ','now','-2 days')",
                [],
            )
            .unwrap();
        let result = store
            .try_enqueue_scout_research(
                "expansion",
                "new-key",
                "source:1",
                "What is a neutron?",
                "Follow up after elapsed time",
            )
            .unwrap();
        assert_eq!(
            matches!(result, crate::storage::ScoutAdmission::Queued(_)),
            completed
        );
        drop(store);
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn deferred_research_survives_reopen_and_waits_until_due() {
    let directory = std::env::temp_dir().join(format!(
        "lyra-research-deferral-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("learning.db");
    let mut store = LearningStore::open(&path).unwrap();
    let plan = crate::research::ResearchPlan {
        question: "Where is the Amazon rainforest?".into(),
        queries: vec!["Amazon rainforest location".into()],
    };
    store.enqueue_research_plan("retry-test", 1, &plan).unwrap();
    let job = store
        .claim_conversation_research("retry-test", 1)
        .unwrap()
        .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    store
        .defer_scholar_job(&job.id, "daily budget exhausted", now + 60000)
        .unwrap();
    drop(store);
    let mut reopened = LearningStore::open(&path).unwrap();
    assert!(
        reopened
            .claim_conversation_research("retry-test", 1)
            .unwrap()
            .is_none()
    );
    assert!(!reopened.has_pending_user_research().unwrap());
    reopened
        .connection
        .execute(
            "UPDATE scholar_jobs SET next_attempt_at_ms=0 WHERE id=?1",
            [&job.id],
        )
        .unwrap();
    assert!(reopened.has_pending_user_research().unwrap());
    assert_eq!(
        reopened
            .claim_conversation_research("retry-test", 1)
            .unwrap()
            .unwrap()
            .id,
        job.id
    );
    drop(reopened);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn semantic_lookup_plan_survives_restart_and_foreground_claims_only_its_own_turn() {
    let path = temp_path();
    let store = LearningStore::open(&path).unwrap();
    let plan = crate::research::ResearchPlan {
        question: "Which flavours does Walkers sell?".into(),
        queries: vec!["Walkers crisps".into(), "Walkers (snack foods)".into()],
    };
    store.enqueue_research_plan("frontend", 1, &plan).unwrap();
    store.enqueue_research_plan("frontend", 1, &plan).unwrap();
    store
        .enqueue_research("other-frontend", 2, "What moons does Mars have?")
        .unwrap();
    drop(store);
    let mut store = LearningStore::open(&path).unwrap();
    let job = store
        .claim_conversation_research("frontend", 1)
        .unwrap()
        .unwrap();
    assert_eq!(job.question, plan.question);
    assert_eq!(
        store.scholar_lookup_queries(&job.id).unwrap().unwrap(),
        plan.queries
    );
    assert!(
        store
            .claim_conversation_research("frontend", 1)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store.claim_user_scholar_job().unwrap().unwrap().question,
        "What moons does Mars have?"
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn lookup_plan_schema_upgrade_preserves_existing_jobs() {
    let path = temp_path();
    let store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("existing", 1, "What flavours of crisps exist?")
        .unwrap();
    store
        .connection
        .execute_batch("DROP TABLE scholar_lookup_plans; PRAGMA user_version = 42;")
        .unwrap();
    drop(store);
    let mut store = LearningStore::open(&path).unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    assert_eq!(job.question, "What flavours of crisps exist?");
    assert!(store.scholar_lookup_queries(&job.id).unwrap().is_none());
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn newest_user_research_precedes_background_work_without_losing_older_jobs() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_scout_research_with_reason(
            "model_probe",
            "renewable energy",
            "model_probe:1",
            "How does renewable energy affect food production?",
            "Background exploration",
        )
        .unwrap();
    let (_, finding) = feedback_fixture(&mut store);
    store
        .reject_scholar_finding_with_feedback(
            finding,
            "Need a different source",
            "new_source",
            "Seek independent evidence.",
        )
        .unwrap();
    let retry = store.claim_scholar_feedback("new_source").unwrap().unwrap();
    assert!(!store.has_pending_user_research().unwrap());
    assert!(store.claim_user_scholar_job().unwrap().is_none());
    store
        .enqueue_research("frontend", 1, "What flavours of crisps exist?")
        .unwrap();
    store
        .enqueue_research("frontend", 2, "Who invented potato crisps?")
        .unwrap();
    assert!(store.has_pending_user_research().unwrap());
    let latest = store.claim_user_scholar_job().unwrap().unwrap();
    assert_eq!(latest.question, "Who invented potato crisps?");
    // A second worker cannot claim the same live job.
    let older = store.claim_scholar_job().unwrap().unwrap();
    assert_eq!(older.question, "What flavours of crisps exist?");
    assert!(!store.has_pending_user_research().unwrap());
    let background = store.claim_scholar_job().unwrap().unwrap();
    assert_eq!(
        background.question,
        "How does renewable energy affect food production?"
    );
    assert_eq!(store.claim_scholar_job().unwrap().unwrap().id, retry.job_id);
    assert!(store.claim_scholar_job().unwrap().is_none());
    store
        .connection
        .execute(
            "UPDATE scholar_jobs SET claimed_at = '2000-01-01T00:00:00Z' WHERE id = ?1",
            [&latest.id],
        )
        .unwrap();
    assert!(store.has_pending_user_research().unwrap());
    assert_eq!(
        store.claim_user_scholar_job().unwrap().unwrap().id,
        latest.id
    );
    drop(store);
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
        // Advance both clocks: question cooldowns also cover user-originated
        // Scholar jobs, which have no Scout investigation timestamp.
        store.connection.execute(
            "UPDATE scholar_jobs SET created_at=strftime('%Y-%m-%dT%H:%M:%fZ','now','-2 days') WHERE id=?1",
            [&job.id],
        ).unwrap();
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
fn scout_retries_no_result_after_cooldown() {
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
    store
        .fail_scholar_job(&job.id, "source lookup returned no result")
        .unwrap();
    assert!(store.scout_topic_exists("stale", "claim:1").unwrap());
    store.connection.execute(
        "UPDATE scout_investigations SET created_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-8 days') WHERE scholar_job_id = ?1",
        [&job.id],
    ).unwrap();
    assert!(!store.scout_topic_exists("stale", "claim:1").unwrap());
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
fn recent_model_topics_keep_distinct_history_when_rejections_repeat() {
    let path = temp_path();
    let store = LearningStore::open(&path).unwrap();
    for topic in std::iter::once("Earlier distinct topic")
        .chain(std::iter::repeat_n("Repeated rejected topic", 40))
    {
        store.connection.execute("INSERT INTO scout_model_probes(seed,status,topic,selection_reason,created_at) VALUES('open public knowledge','rejected',?1,'fixture',strftime('%Y-%m-%dT%H:%M:%fZ','now'))", [topic]).unwrap();
    }
    drop(store);
    let reopened = LearningStore::open(&path).unwrap();
    assert_eq!(
        reopened.recent_model_topics().unwrap(),
        vec!["Repeated rejected topic", "Earlier distinct topic"]
    );
    drop(reopened);
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
                proposition: Default::default(),
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
                    proposition: Default::default(),
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
