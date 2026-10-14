//! Storage regressions for migrations.
use super::*;

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
                Some(ScholarFinding {
                    semantic_support: None,
                    subject: "Example Organization".into(),
                    relation: "opened a clinic in".into(),
                    object: "2019".into(),
                    evidence_text: "Example Organization opened a clinic in 2019.".into(),
                    confidence: 0.99,
                    valid_from: None,
                    valid_until: None,
                    source_quality: "reference".into(),
                    source_updated_at: None,
                    proposition: Default::default(),
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
            Some(ScholarFinding {
                semantic_support: None,
                subject: "Walkers".into(),
                relation: "sells".into(),
                object: "crisps in a variety of flavours".into(),
                evidence_text: "## WALKERS CRISPS\nLegendary Ready Salted".into(),
                confidence: 0.98,
                valid_from: None,
                valid_until: None,
                source_quality: "reference".into(),
                source_updated_at: None,
                proposition: Default::default(),
            }),
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
