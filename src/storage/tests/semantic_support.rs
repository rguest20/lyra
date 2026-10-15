//! Storage regressions for semantic support.
use super::*;

#[test]
fn semantic_paraphrase_survives_storage_restart_and_ai_promotion() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("semantic-round", 1, "What shape is the world?")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    store
        .finish_scholar_job(
            &job.id,
            "World",
            "https://evidence.example/world",
            "The world is spherical.",
            Some(semantic_round_finding()),
            None,
        )
        .unwrap();
    let id = store.scholar_findings(&job.id).unwrap()[0].id;
    assert_eq!(
        store
            .scholar_source_assessment(id)
            .unwrap()
            .unwrap()
            .direct_support,
        Some(true)
    );
    drop(store);
    let mut store = LearningStore::open(&path).unwrap();
    let context = store.scholar_review_context(id).unwrap().unwrap();
    assert_eq!(context["object"], "round");
    assert_eq!(context["evidence_text"], "The world is spherical.");
    assert_eq!(context["support_method"], "ai_semantic_entailment");
    assert_eq!(context["semantic_support"]["verdict"], "entailed");
    assert!(store.active_semantic_claims().unwrap().is_empty());
    store
        .review_scholar_source_expertise_ai(id, "primary", true, None, "Verified for this topic.")
        .unwrap();
    store
        .review_scholar_finding_ai(id, true, "The source supports this paraphrase.")
        .unwrap();
    assert_eq!(
        store.active_semantic_claims().unwrap()[0].claim.object,
        "round"
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn semantic_assessment_cannot_approve_a_changed_finding() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("semantic-mutation", 1, "What shape is the world?")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    let mut finding = semantic_round_finding();
    finding.object = "flat".into();
    assert!(
        store
            .finish_scholar_job(
                &job.id,
                "World",
                "https://evidence.example/world",
                "The world is spherical.",
                Some(finding),
                None
            )
            .is_err()
    );
    assert!(store.scholar_findings(&job.id).unwrap().is_empty());
    assert!(store.active_semantic_claims().unwrap().is_empty());
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn pending_lexical_failure_can_gain_semantic_support_without_replacing_claim() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("semantic-legacy", 1, "What shape is the world?")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    let mut finding = semantic_round_finding();
    let support = finding.semantic_support.take().unwrap();
    finding.evidence_text = "The world is round.".into();
    store
        .finish_scholar_job(
            &job.id,
            "World",
            "https://evidence.example/world",
            "The world is spherical.",
            Some(finding),
            None,
        )
        .unwrap();
    let id = store.scholar_findings(&job.id).unwrap()[0].id;
    assert_eq!(
        store
            .scholar_source_assessment(id)
            .unwrap()
            .unwrap()
            .direct_support,
        Some(false)
    );
    store.record_scholar_semantic_support(id, &support).unwrap();
    assert_eq!(
        store
            .scholar_source_assessment(id)
            .unwrap()
            .unwrap()
            .direct_support,
        Some(true)
    );
    assert_eq!(store.scholar_findings(&job.id).unwrap()[0].object, "round");
    assert_eq!(
        store.scholar_findings(&job.id).unwrap()[0].evidence_text,
        "The world is spherical."
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn semantic_schema_upgrade_preserves_findings_and_does_not_invent_assessments() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let (job, id) = feedback_fixture(&mut store);
    store
        .connection
        .execute_batch("DROP TABLE scholar_semantic_support; PRAGMA user_version = 41;")
        .unwrap();
    drop(store);
    let store = LearningStore::open(&path).unwrap();
    assert_eq!(store.scholar_findings(&job).unwrap()[0].id, id);
    assert_eq!(
        store
            .connection
            .query_row("SELECT COUNT(*) FROM scholar_semantic_support", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn ai_promotion_rechecks_saved_semantic_assessment_binding() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("semantic-stored-mutation", 1, "What shape is the world?")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    store
        .finish_scholar_job(
            &job.id,
            "World",
            "https://evidence.example/world",
            "The world is spherical.",
            Some(semantic_round_finding()),
            None,
        )
        .unwrap();
    let id = store.scholar_findings(&job.id).unwrap()[0].id;
    store
        .connection
        .execute(
            "UPDATE scholar_findings SET object = 'flat' WHERE id = ?1",
            [id],
        )
        .unwrap();
    let error = store
        .review_scholar_finding_ai(id, true, "Accept revised wording.")
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("saved semantic assessment no longer supports")
    );
    assert!(store.active_semantic_claims().unwrap().is_empty());
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn semantic_disproof_blocks_promotion_despite_matching_candidate_words() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("semantic-disproof", 1, "Is Earth flat?")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    let passage = "Flat Earth is a scientifically disproven conception of the Earth's shape as a plane or disk.";
    let finding = crate::semantic::candidate_from_context(&serde_json::json!({"subject":"Flat Earth","relation":"is","object":"a true conception of the Earth as a plane or disk","evidence_text":passage})).unwrap();
    let mut review_pass = false;
    let support = crate::semantic::assess_with("Is Earth flat?","Flat Earth",passage,&finding, |_| {
        review_pass = !review_pass;
        Ok(if review_pass {
            serde_json::json!({"verdict":"entailed","reason":"It mentions the conception.","evidence_text":passage}).to_string()
        } else {
            serde_json::json!({"verdict":"contradicted","reason":"The source calls the conception scientifically disproven.","evidence_text":null}).to_string()
        })
    }).unwrap();
    assert_eq!(support.verdict(), "contradicted");
    store
        .finish_scholar_job(
            &job.id,
            "Flat Earth",
            "https://evidence.example/flat-earth",
            passage,
            Some(finding),
            None,
        )
        .unwrap();
    let id = store.scholar_findings(&job.id).unwrap()[0].id;
    store.record_scholar_semantic_support(id, &support).unwrap();
    assert_eq!(
        store
            .scholar_source_assessment(id)
            .unwrap()
            .unwrap()
            .direct_support,
        Some(false)
    );
    assert!(
        store
            .review_scholar_finding_ai(id, true, "Accept the mentioned notion.")
            .is_err()
    );
    assert!(store.active_semantic_claims().unwrap().is_empty());
    drop(store);
    let _ = std::fs::remove_file(path);
}
