//! Storage regressions for migrations.
use super::*;
use crate::storage::SCHEMA_VERSION;

#[test]
fn assertion_depth_migration_preserves_existing_review_state() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research(
            "assertion-depth-migration",
            1,
            "What did the observatory report?",
        )
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    store
        .finish_scholar_job(
            &job.id,
            "Observatory",
            "https://example.org/observatory",
            "The observatory opened in 1974.",
            None,
            None,
        )
        .unwrap();
    store
        .retain_research_source(
            &job.id,
            "Observatory",
            "https://example.org/observatory",
            "The observatory opened in 1974.",
        )
        .unwrap();
    let source = store.claim_research_source().unwrap().unwrap();
    store
        .finish_research_baskets(
            &source,
            &crate::storage::ResearchBaskets {
                claims: vec![crate::storage::RetainedClaim {
                    subject: "The observatory".into(),
                    relation: "opened in".into(),
                    object: "1974".into(),
                    evidence_text: "The observatory opened in 1974.".into(),
                    qualifications: String::new(),
                    extraction_confidence: 0.95,
                }],
                questions: vec![],
            },
        )
        .unwrap();
    let lead = store.next_research_assertion_lead().unwrap().unwrap();
    store
        .record_research_assertion_selection(
            &lead,
            Some("What evidence independently confirms the opening date?"),
            "The dated public assertion can be checked.",
        )
        .unwrap();
    store
        .connection
        .execute_batch("ALTER TABLE research_assertion_checks DROP COLUMN derivation_depth; PRAGMA user_version=68;")
        .unwrap();
    drop(store);

    let store = LearningStore::open(&path).unwrap();
    assert_eq!(
        store
            .connection
            .query_row(
                "SELECT status,derivation_depth,question FROM research_assertion_checks WHERE claim_id=?1",
                [lead.claim_id],
                |row| Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?,row.get::<_,String>(2)?)),
            )
            .unwrap(),
        ("pending".into(), 0, "What evidence independently confirms the opening date?".into())
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn differing_relation_migration_reopens_only_unreviewed_lexical_decisions() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .connection
        .execute_batch(
            "INSERT INTO semantic_claims(subject,relation,object,source,created_at)
         VALUES ('Example Organization','started operating in','1998','old source','2026-01-01');
         INSERT INTO semantic_claims(subject,relation,object,source,created_at)
         VALUES ('Example Organization','employs','120 staff','old source','2026-01-01');",
        )
        .unwrap();
    store
        .enqueue_research("relation-migration", 1, "When did it begin operating?")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    let passage = "Example Organization began operations in 2000.";
    store
        .finish_scholar_job(
            &job.id,
            "History",
            "https://example.org/history",
            passage,
            Some(scholar_finding(
                "Example Organization",
                "began operations in",
                "2000",
                passage,
                0.99,
                "reference",
                None,
            )),
            None,
        )
        .unwrap();
    let finding = store.scholar_findings(&job.id).unwrap().remove(0);
    let links: i64 = store.connection.query_row(
        "SELECT count(*) FROM scholar_finding_claim_links WHERE finding_id=?1 AND relation='unresolved'",
        [finding.id], |row| row.get(0),
    ).unwrap();
    assert_eq!(
        links, 2,
        "paraphrases and different properties both need semantic assessment"
    );
    store
        .connection
        .execute_batch(
            "UPDATE scholar_finding_claim_links SET relation='does_not_address',
             assessment_method='deterministic_distinct_relation' WHERE claim_id=1;
         UPDATE scholar_finding_claim_links SET relation='does_not_address',
             assessment_method='ai_review' WHERE claim_id=2;
         PRAGMA user_version=62;",
        )
        .unwrap();
    drop(store);
    let store = LearningStore::open(&path).unwrap();
    let links: Vec<(i64, String, String)> = store.connection.prepare(
        "SELECT claim_id,relation,assessment_method FROM scholar_finding_claim_links ORDER BY claim_id",
    ).unwrap().query_map([], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)))
        .unwrap().collect::<Result<_, _>>().unwrap();
    assert_eq!(
        links,
        vec![
            (
                1,
                "unresolved".into(),
                "relation_requires_semantic_review".into()
            ),
            (2, "does_not_address".into(), "ai_review".into()),
        ]
    );
    assert_eq!(store.active_semantic_claims().unwrap().len(), 2);
    assert_eq!(
        store.scholar_findings(&job.id).unwrap()[0].status,
        "needs_review"
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn concurrent_first_open_runs_schema_migrations_once() {
    let path = temp_path();
    let workers = (0..8)
        .map(|_| {
            let path = path.clone();
            std::thread::spawn(move || LearningStore::open(&path).map(drop))
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().unwrap().unwrap();
    }
    let store = LearningStore::open(&path).unwrap();
    assert_eq!(
        store
            .connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn scholar_task_target_migration_backfills_existing_jobs() {
    let path = temp_path();
    let store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("legacy-target", 1, "Who discovered penicillin?")
        .unwrap();
    let job_id: String = store
        .connection
        .query_row("SELECT id FROM scholar_jobs", [], |row| row.get(0))
        .unwrap();
    store
        .connection
        .execute_batch(
            "DROP TRIGGER scholar_job_target_after_insert;
             DROP TABLE scholar_task_targets;
             PRAGMA user_version=59;",
        )
        .unwrap();
    drop(store);

    let upgraded = LearningStore::open(&path).unwrap();
    let target = upgraded.scholar_task_target(&job_id).unwrap().unwrap();
    assert_eq!(target.original_question, "Who discovered penicillin?");
    assert_eq!(target.evidence_gap, target.original_question);
    drop(upgraded);
    let _ = std::fs::remove_file(path);
}

#[test]
fn corroboration_queue_backfills_and_uses_searxng_preserving_legacy_history() {
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
            Some(scholar_finding(
                "Mars",
                "is a",
                "planet",
                "Mars is a planet.",
                0.99,
                "reference",
                None,
            )),
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
        .claim_corroboration_task(true, 1_800_000)
        .unwrap()
        .unwrap();
    assert_eq!(first.finding_id, finding_id);
    assert_eq!(
        store.corroboration_source_urls(&first).unwrap(),
        vec!["https://first.example.org/mars"]
    );
    assert!(
        store
            .claim_corroboration_task(true, 1_800_000)
            .unwrap()
            .is_none()
    );
    store
        .finish_scholar_job_with_evidence(&first.job_id, &[], Some("no match"))
        .unwrap();
    store.finish_corroboration_task(&first, "no_match").unwrap();
    store
        .connection
        .execute(
            "UPDATE scholar_corroboration_queue SET next_searxng_at_ms = 0
             WHERE finding_id = ?1",
            [finding_id],
        )
        .unwrap();
    let second = store
        .claim_corroboration_task(true, 1_800_000)
        .unwrap()
        .unwrap();
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
            .claim_corroboration_task(true, 1_800_000)
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
    assert_eq!(queue[0].searxng_attempts, 2);

    store
        .connection
        .execute(
            "UPDATE scholar_corroboration_queue SET status = 'exhausted',
                legacy_serp_attempts = 2, legacy_tavily_attempts = 1 WHERE finding_id = ?1",
            [finding_id],
        )
        .unwrap();
    store
        .connection
        .execute(
            "INSERT INTO scholar_corroboration_attempts
                (finding_id, provider, job_id, started_at_ms, finished_at_ms, outcome)
             VALUES (?1, 'serpapi', ?2, 100, 200, 'no_match')",
            rusqlite::params![finding_id, first_job.id],
        )
        .unwrap();
    store
        .connection
        .execute_batch(
            "CREATE TABLE scholar_corroboration_queue_v63 (
                finding_id INTEGER PRIMARY KEY REFERENCES scholar_findings(id),
                claim_key TEXT NOT NULL UNIQUE,
                status TEXT NOT NULL DEFAULT 'pending'
                    CHECK (status IN ('pending', 'processing', 'done', 'exhausted')),
                serp_attempts INTEGER NOT NULL DEFAULT 0,
                tavily_attempts INTEGER NOT NULL DEFAULT 0,
                next_serp_at_ms INTEGER NOT NULL DEFAULT 0,
                next_tavily_at_ms INTEGER NOT NULL DEFAULT 0,
                claimed_at_ms INTEGER,
                claimed_provider TEXT CHECK (claimed_provider IN ('serpapi', 'tavily')),
                attempt_job_id TEXT REFERENCES scholar_jobs(id),
                created_at_ms INTEGER NOT NULL,
                last_outcome TEXT
             );
             INSERT INTO scholar_corroboration_queue_v63
                (finding_id, claim_key, status, serp_attempts, tavily_attempts,
                 next_serp_at_ms, next_tavily_at_ms, created_at_ms, last_outcome)
             SELECT finding_id, claim_key, status, legacy_serp_attempts,
                    legacy_tavily_attempts, legacy_next_serp_at_ms,
                    legacy_next_tavily_at_ms, created_at_ms, last_outcome
             FROM scholar_corroboration_queue;
             CREATE TABLE scholar_corroboration_attempts_v63 (
                id INTEGER PRIMARY KEY,
                finding_id INTEGER NOT NULL REFERENCES scholar_corroboration_queue_v63(finding_id),
                provider TEXT NOT NULL CHECK (provider IN ('serpapi', 'tavily')),
                job_id TEXT NOT NULL UNIQUE REFERENCES scholar_jobs(id),
                started_at_ms INTEGER NOT NULL,
                finished_at_ms INTEGER,
                outcome TEXT NOT NULL DEFAULT 'processing'
             );
             INSERT INTO scholar_corroboration_attempts_v63
                (id, finding_id, provider, job_id, started_at_ms, finished_at_ms, outcome)
             SELECT id, finding_id, provider, job_id, started_at_ms, finished_at_ms, outcome
             FROM scholar_corroboration_attempts WHERE provider IN ('serpapi', 'tavily');
             DROP TABLE scholar_corroboration_attempts;
             DROP TABLE scholar_corroboration_queue;
             ALTER TABLE scholar_corroboration_queue_v63 RENAME TO scholar_corroboration_queue;
             ALTER TABLE scholar_corroboration_attempts_v63 RENAME TO scholar_corroboration_attempts;
             ALTER TABLE legacy_serpapi_search_domains RENAME TO serpapi_search_domains;
             ALTER TABLE legacy_serpapi_usage RENAME TO serpapi_usage;
             ALTER TABLE legacy_tavily_usage RENAME TO tavily_usage;
             PRAGMA user_version = 63;",
        )
        .unwrap();
    drop(store);

    let store = LearningStore::open(&path).unwrap();
    let queue = store.corroboration_queue(10).unwrap();
    assert_eq!(queue[0].status, "pending");
    assert_eq!(queue[0].searxng_attempts, 0);
    let legacy_counts: (i64, i64) = store
        .connection
        .query_row(
            "SELECT legacy_serp_attempts, legacy_tavily_attempts FROM scholar_corroboration_queue
             WHERE finding_id = ?1",
            [finding_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(legacy_counts, (2, 1));
    let legacy_attempt: (String, String) = store
        .connection
        .query_row(
            "SELECT provider, outcome FROM scholar_corroboration_attempts WHERE job_id = ?1",
            [&first_job.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(legacy_attempt, ("serpapi".into(), "no_match".into()));
    let legacy_tables: i64 = store
        .connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table'
             AND name IN ('legacy_serpapi_usage', 'legacy_serpapi_search_domains', 'legacy_tavily_usage')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(legacy_tables, 3);
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
            Some(scholar_finding(
                "Mars",
                "is a",
                "planet",
                "Mars is a planet.",
                0.99,
                "reference",
                None,
            )),
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
fn migration_withdraws_host_only_corroboration_from_old_auto_promotions() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let mut finding_ids = Vec::new();
    for (turn, url, origin, passage) in [
        (
            1,
            "https://first.example/claim",
            "first:report",
            "Example Organization opened a clinic in 2019. The first report includes regional survey methods and sampling notes.",
        ),
        (
            2,
            "https://second.example/claim",
            "second:report",
            "Example Organization opened a clinic in 2019. The second report documents a separate inspection and operating schedule.",
        ),
    ] {
        store
            .enqueue_research("old-auto", turn, "What is the claim?")
            .unwrap();
        let job = store.claim_scholar_job().unwrap().unwrap();
        store
            .finish_scholar_job_with_profile(
                &job.id,
                "Example Organization",
                url,
                passage,
                Some(scholar_finding(
                    "Example Organization",
                    "opened a clinic in",
                    "2019",
                    "Example Organization opened a clinic in 2019.",
                    0.99,
                    "reference",
                    None,
                )),
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
                "The publication directly supports this claim.",
            )
            .unwrap();
        store
            .review_scholar_source_origin(
                finding.id,
                origin,
                "Reviewer verified a distinct underlying report.",
            )
            .unwrap();
        finding_ids.push(finding.id);
    }
    let claim_id: i64 = store
        .connection
        .query_row(
            "SELECT semantic_claim_id FROM scholar_findings WHERE id=?1",
            [finding_ids[0]],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(store.active_semantic_claims().unwrap().len(), 1);

    for finding_id in &finding_ids {
        store.connection.execute(
            "UPDATE scholar_source_origin_reviews SET review_method='automatic' WHERE finding_id=?1",
            [finding_id],
        ).unwrap();
        store.connection.execute(
            "UPDATE scholar_findings SET decision_reason='automatically accepted after two directly supporting, high-confidence findings with suitable topics and distinct verified publication origins' WHERE id=?1",
            [finding_id],
        ).unwrap();
    }
    store
        .connection
        .execute("PRAGMA user_version=60", [])
        .unwrap();
    drop(store);

    let store = LearningStore::open(&path).unwrap();
    let status: String = store
        .connection
        .query_row(
            "SELECT status FROM semantic_claims WHERE id=?1",
            [claim_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "challenged");
    let pending: i64 = store
        .connection
        .query_row(
            "SELECT COUNT(*) FROM scholar_findings WHERE id IN (?1,?2) AND status='needs_review'",
            rusqlite::params![finding_ids[0], finding_ids[1]],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending, 2);
    for finding_id in finding_ids {
        let assessment = store
            .scholar_source_assessment(finding_id)
            .unwrap()
            .unwrap();
        assert!(!assessment.independence_verified);
        assert_eq!(assessment.gate_status, "awaiting_corroboration");
    }
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn migration_withdraws_plural_claims_supported_by_only_one_example() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("old-plural-support", 1, "What flavours does Walkers sell?")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    store
        .finish_scholar_job_with_profile(
            &job.id,
            "Product",
            "https://www.walkers.co.uk/products",
            "## WALKERS CRISPS\nLegendary Ready Salted",
            Some(scholar_finding(
                "Walkers",
                "sells",
                "crisps in a variety of flavours",
                "## WALKERS CRISPS\nLegendary Ready Salted",
                0.98,
                "reference",
                None,
            )),
            None,
            &ScholarSourceProfile::default(),
        )
        .unwrap();
    let finding_id = store.scholar_findings(&job.id).unwrap()[0].id;
    // Recreate a legacy assessment made before the plural-evidence check.
    store
        .connection
        .execute(
            "UPDATE scholar_source_assessments SET direct_support=1,gate_status='awaiting_corroboration'
             WHERE finding_id=?1",
            [finding_id],
        )
        .unwrap();
    store
        .connection
        .execute_batch("PRAGMA user_version=61;")
        .unwrap();
    drop(store);

    let store = LearningStore::open(&path).unwrap();
    let assessment = store
        .scholar_source_assessment(finding_id)
        .unwrap()
        .unwrap();
    assert!(!assessment.direct_support.unwrap_or(false));
    assert_eq!(assessment.gate_status, "unsupported");
    assert!(
        assessment
            .reason
            .contains("one example does not establish multiple options")
    );
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
            &scholar_search_candidate(
                "organic_result",
                "Topic",
                "https://example.org/topic",
                "topic",
                "{}",
            ),
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
