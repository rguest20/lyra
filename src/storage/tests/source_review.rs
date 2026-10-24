//! Storage regressions for source review.
use super::*;

#[test]
fn scholar_task_target_keeps_search_history_and_a_bounded_stop_reason() {
    let path = temp_path();
    let store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("target-test", 1, "Which flavours does Walkers sell?")
        .unwrap();
    drop(store);

    let mut store = LearningStore::open(&path).unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    store
        .set_scholar_task_constraints(&job.id, &["Walkers".into()], &["current range".into()])
        .unwrap();
    let target = store.scholar_task_target(&job.id).unwrap().unwrap();
    assert_eq!(
        target.original_question,
        "Which flavours does Walkers sell?"
    );
    assert_eq!(target.evidence_gap, target.original_question);
    assert_eq!(target.entity_constraints, vec!["Walkers"]);
    assert_eq!(target.scope_constraints, vec!["current range"]);
    store
        .record_scholar_search_attempt(
            &job.id,
            "Walkers crisp flavours",
            "no_result",
            Some(0),
            None,
        )
        .unwrap();
    store
        .fail_scholar_job(&job.id, "source extraction failed")
        .unwrap();
    drop(store);

    let store = LearningStore::open(&path).unwrap();
    let target = store.scholar_task_target(&job.id).unwrap().unwrap();
    assert_eq!(target.sources_tried.len(), 1);
    assert_eq!(target.sources_tried[0].query, "Walkers crisp flavours");
    assert_eq!(target.stop_reason.as_deref(), Some("extraction_failure"));
    let _ = std::fs::remove_file(path);
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
            Some(scholar_finding(
                "Mars",
                "is a",
                "planet",
                "Mars has oceans.",
                0.99,
                "reference",
                None,
            )),
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
            Some(scholar_finding(
                "Example",
                "has",
                "a property",
                "Example has a property.",
                0.99,
                "unknown",
                None,
            )),
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
            Some(scholar_finding(
                "Mars",
                "current population is",
                "100",
                "Mars current population is 100.",
                0.99,
                "reference",
                Some("2001-01-01T00:00:00Z"),
            )),
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
            Some(scholar_finding(
                "Insomnia",
                "treatment is",
                "available",
                "A treatment is available.",
                0.99,
                "reference",
                None,
            )),
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
            Some(scholar_finding(
                "Photosynthesis",
                "converts",
                "light energy",
                evidence,
                0.99,
                "unknown",
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
                &[ScholarEvidence::new(
                    format!("Mars report {turn}"),
                    url,
                    passage,
                    Some(scholar_finding(
                        "Mars", "is a", "planet", evidence, 0.99, "unknown", None,
                    )),
                    "direct",
                )],
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
            &scholar_search_candidate(
                "organic_result",
                "Mars overview",
                "https://example.org/mars",
                "Mars composition",
                "{}",
            ),
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
            Some(scholar_finding(
                "Example GPU",
                "has",
                "24 GB memory",
                "Example GPU has 24 GB memory.",
                0.99,
                "primary",
                None,
            )),
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
            Some(scholar_finding(
                "Photosynthesis",
                "converts",
                "light energy into chemical energy",
                "Photosynthesis converts light energy into chemical energy.",
                0.98,
                "reference",
                None,
            )),
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
            Some(scholar_finding(
                "Mars",
                "is a",
                "planet",
                "Mars has oceans.",
                0.99,
                "reference",
                None,
            )),
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
            Some(scholar_finding(
                "Mars",
                "is a",
                "planet",
                "Mars is a planet.",
                0.8,
                "reference",
                None,
            )),
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
