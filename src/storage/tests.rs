use super::{
    AssistantClaim, CandidateClaim, ClaimJudgment, ConsolidatedClaimRevision, ImportSummary,
    LearningStore, ModelCheckpointManifest, ScholarFinding, evidence_supports_claim,
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
            .enqueue_scout_research(
                "challenged",
                "hagia sophia|height",
                "assistant_claim:42",
                "What is the height of Hagia Sophia?",
            )
            .unwrap()
    );
    assert!(
        !store
            .enqueue_scout_research(
                "challenged",
                "hagia sophia|height",
                "assistant_claim:43",
                "What is the height of Hagia Sophia?",
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
