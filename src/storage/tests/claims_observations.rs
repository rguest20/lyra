//! Storage regressions for claims observations.
use super::*;

#[test]
fn advisory_reply_review_preserves_text_and_uses_only_its_own_past_context() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .stage_observation("dialogue", 1, "How should I prepare?")
        .unwrap();
    store
        .stage_assistant_reply("dialogue", 1, "Prepare examples of your experience.")
        .unwrap();
    store
        .stage_observation("dialogue", 2, "Could you explain that?")
        .unwrap();
    let answer = "Choose situations where you solved a problem and explain your contribution.";
    store.stage_assistant_reply("dialogue", 2, answer).unwrap();
    store
        .stage_observation("dialogue", 3, "This future turn must not affect the audit.")
        .unwrap();
    store
        .stage_assistant_reply("dialogue", 3, "Future reply.")
        .unwrap();
    store
        .stage_observation("another-dialogue", 1, "Unrelated private context.")
        .unwrap();

    let first = store.claim_assistant_reply().unwrap().unwrap();
    store.finish_assistant_reply(&first.id, &[]).unwrap();
    let job = store.claim_assistant_reply().unwrap().unwrap();
    assert_eq!(job.reply_text, answer);
    let context = store.assistant_reply_context(&job.id).unwrap();
    assert_eq!(context["latest_user_message"], "Could you explain that?");
    assert_eq!(context["recent_conversation"].as_array().unwrap().len(), 2);
    assert_eq!(
        context["recent_conversation"][1]["content"],
        "Prepare examples of your experience."
    );
    assert!(!context.to_string().contains("future"));
    assert!(!context.to_string().contains("Unrelated"));

    store
        .record_conversation_review(
            &job.id,
            "uncertain",
            "Cannot confidently judge the advice.",
            "test-model",
            &context,
        )
        .unwrap();
    store.finish_assistant_reply(&job.id, &[]).unwrap();
    drop(store);
    let store = LearningStore::open(&path).unwrap();
    let (text, payload, outcome): (String, String, String) = store
        .connection
        .query_row(
            "SELECT r.reply_text,p.payload,d.outcome FROM assistant_replies r
         JOIN role_proposals p ON p.source_id=r.id AND p.source_type='assistant_reply_quality'
         JOIN policy_decisions d ON d.proposal_id=p.id WHERE r.id=?1",
            [&job.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(text, answer);
    assert_eq!(payload["verdict"], "uncertain");
    assert_eq!(payload["model_id"], "test-model");
    assert_eq!(payload["advisory_only"], true);
    assert_eq!(outcome, "no_change");
    assert!(store.active_semantic_claims().unwrap().is_empty());
    drop(store);
    let _ = std::fs::remove_file(path);
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
fn failed_ai_review_retries_after_restart_without_blocking_other_findings() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let mut ids = Vec::new();
    for turn in 1..=2 {
        store
            .enqueue_research("retry-test", turn, "What is Mars?")
            .unwrap();
        let job = store.claim_scholar_job().unwrap().unwrap();
        store
            .finish_scholar_job(
                &job.id,
                "Mars",
                &format!("https://source{turn}.example/mars"),
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
                    source_quality: "unknown".into(),
                    source_updated_at: None,
                    proposition: Default::default(),
                }),
                None,
            )
            .unwrap();
        ids.push(store.scholar_findings(&job.id).unwrap()[0].id);
    }
    assert_eq!(store.scholar_ai_review_queue().unwrap().len(), 2);
    store
        .defer_scholar_ai_review(ids[0], "source_kind was invalid")
        .unwrap();
    drop(store);
    let reopened = LearningStore::open(&path).unwrap();
    let ready = reopened.scholar_ai_review_queue().unwrap();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].0, ids[1]);
    assert!(reopened.active_semantic_claims().unwrap().is_empty());
    reopened
        .connection
        .execute(
            "UPDATE scholar_ai_review_retries SET retry_after = '2000-01-01T00:00:00.000Z'",
            [],
        )
        .unwrap();
    assert_eq!(reopened.scholar_ai_review_queue().unwrap().len(), 2);
    drop(reopened);
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
        semantic_support: None,
        subject: "Mars".into(),
        relation: "is a".into(),
        object: "planet".into(),
        evidence_text: "Mars is a planet.".into(),
        confidence: 0.99,
        valid_from: None,
        valid_until: None,
        source_quality: quality.into(),
        source_updated_at: None,
        proposition: PropositionMetadata {
            attribution: Some("The source".into()),
            geographic_scope: None,
            context_scope: None,
            polarity: ClaimPolarity::Asserted,
            modality: ClaimModality::Factual,
        },
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
    assert_eq!(findings[0].attribution.as_deref(), Some("The source"));
    assert_eq!(findings[0].polarity, "asserted");
    assert_eq!(findings[0].modality, "factual");
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
    assert_eq!(
        super::super::parse_automatic_promotion_confidence(None),
        0.90
    );
    assert_eq!(
        super::super::parse_automatic_promotion_confidence(Some("0.75")),
        0.75
    );
    for invalid in ["nope", "-0.1", "1.1", "NaN", "inf"] {
        assert_eq!(
            super::super::parse_automatic_promotion_confidence(Some(invalid)),
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
fn ai_review_resolves_a_held_candidate_and_records_no_human_hold() {
    let path = temp_path();
    let _ = std::fs::remove_file(&path);
    let mut store = LearningStore::open(&path).unwrap();
    let observation_id = store
        .stage_observation("conversation-ai-review", 1, "I live in Bristol.")
        .unwrap();
    store.claim_next_observation(600).unwrap().unwrap();
    let claim = CandidateClaim {
        subject: "the user".into(),
        relation: "lives in".into(),
        object: "Bristol".into(),
        evidence_text: "I live in Bristol.".into(),
        context_message_ids: Vec::new(),
        resolution: None,
        confidence: 0.92,
        status: "proposed".into(),
        decision_reason: None,
        valid_from: None,
        valid_until: None,
    };
    store
        .finish_extraction(&observation_id, &[claim], None)
        .unwrap();
    let candidate_id = store
        .observation_detail(&observation_id)
        .unwrap()
        .unwrap()
        .candidates[0]
        .id
        .clone();

    store
        .resolve_candidate_ai(
            &candidate_id,
            false,
            "AI review rejected the low confidence claim.",
        )
        .unwrap();

    let detail = store.observation_detail(&observation_id).unwrap().unwrap();
    assert_eq!(detail.observation.status, "rejected");
    assert_eq!(detail.candidates[0].status, "rejected");
    let outcome: String = store
        .connection
        .query_row(
            "SELECT outcome FROM policy_decisions ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(outcome, "ai_rejected");
    assert_eq!(store.inbox_summary().unwrap().needs_review, 0);

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
    let decision_method: String = store
        .connection
        .query_row(
            "SELECT decision_method FROM policy_decisions ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(decision_method, "deterministic_policy");
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
