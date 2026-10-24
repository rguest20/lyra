//! Storage regressions for provenance.
use super::*;

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
                Some(scholar_finding(
                    "Mars",
                    "is a",
                    "planet",
                    passage,
                    0.99,
                    "reference",
                    None,
                )),
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
    let linked_supports: i64 = store
        .connection
        .query_row(
            "SELECT COUNT(*) FROM scholar_finding_claim_links
         WHERE claim_id=?1 AND relation='supports'",
            [claims[0].claim.id],
            |row| row.get(0),
        )
        .unwrap();
    let linked_rows: Vec<(i64, i64)> = {
        let mut statement = store
            .connection
            .prepare(
                "SELECT finding_id,claim_id FROM scholar_finding_claim_links
                 WHERE relation='supports'",
            )
            .unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(
        linked_supports, 2,
        "linked rows: {linked_rows:?}; findings: {findings:?}"
    );
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
                Some(scholar_finding(
                    "Chicken soup",
                    "contains",
                    "chicken",
                    sentence,
                    0.99,
                    "reference",
                    None,
                )),
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
                Some(scholar_finding(
                    "Chicken soup",
                    "contains",
                    "chicken",
                    "Chicken soup contains chicken.",
                    0.99,
                    "reference",
                    None,
                )),
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
fn shared_declared_citation_stays_unverified_after_curmudgeon_suggests_distinct_reporting() {
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
                &scholar_search_candidate(
                    "search_result",
                    "Chicken soup",
                    url,
                    "test candidate",
                    "{}",
                ),
            )
            .unwrap();
        let candidate_id = store.scholar_search_candidates(&job.id).unwrap()[0].id;
        store
            .record_scholar_candidate_fetch_result(
                candidate_id,
                &scholar_candidate_review(
                    "fetched",
                    "allowed",
                    None,
                    "fetched",
                    Some(url),
                    Some(&passage),
                ),
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
                Some(scholar_finding(
                    "Chicken soup",
                    "contains",
                    "chicken",
                    "Chicken soup contains chicken.",
                    0.99,
                    "reference",
                    None,
                )),
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
    assert_eq!(assessment.gate_status, "possible_shared_source");
    assert!(store.active_semantic_claims().unwrap().is_empty());
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
            Some(scholar_finding(
                "GPU",
                "has memory",
                "16 GB",
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
                Some(scholar_finding(
                    "Mars",
                    "is a",
                    "planet",
                    "Mars is a planet.",
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
fn distinct_hosts_do_not_automatically_promote_direct_findings_at_configured_default() {
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
                Some(scholar_finding(
                    "Mars",
                    "is a",
                    "planet",
                    "Mars is a planet.",
                    0.90,
                    "unknown",
                    None,
                )),
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
            assert!(staged[0].source_archive_id.is_some());
            assert!(staged[0].evidence_byte_start.is_some());
            assert!(staged[0].evidence_byte_end.is_some());
            assert!(staged[0].direct_support);
            assert_eq!(staged[0].source_kind, "unknown");
            assert_eq!(staged[0].freshness, "not_time_sensitive");
            assert!(!staged[0].independence_verified);
            assert!(staged[0].conflicting_claims.is_empty());
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
    assert_eq!(store.corroboration_queue(10).unwrap()[0].status, "pending");
    assert!(store.active_semantic_claims().unwrap().is_empty());
    let staged = store.relevant_scholar_evidence("Mars planet", 5).unwrap();
    assert_eq!(staged.len(), 2);
    assert!(staged.iter().all(|finding| {
        finding.gate_status == "pending_review" && !finding.independence_verified
    }));
    for id in [1, 2] {
        assert!(
            store
                .scholar_source_expertise_reviews(id)
                .unwrap()
                .is_empty()
        );
        assert!(store.scholar_source_origin_reviews(id).unwrap().is_empty());
    }
    assert_eq!(
        store
            .recent_maintenance_decisions(10)
            .unwrap()
            .iter()
            .filter(|decision| decision.outcome == "automatic_evidence_acceptance")
            .count(),
        0
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn distinct_hosts_do_not_automatically_verify_independent_reporting() {
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
                Some(scholar_finding(
                    "Mars",
                    "has moons",
                    "two",
                    "Mars has two moons.",
                    0.99,
                    "reference",
                    None,
                )),
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
    let answer_candidates = store.scholar_evidence_candidates(false).unwrap();
    assert_eq!(answer_candidates.len(), 2);
    let (combined, review_candidates) = store.scholar_evidence_candidate_sets().unwrap();
    assert_eq!(combined, store.scholar_evidence_candidates(true).unwrap());
    assert_eq!(review_candidates, answer_candidates);
    assert!(answer_candidates.iter().all(|finding| {
        finding
            .conflicting_claims
            .iter()
            .any(|claim| claim.contains("Mars has moons one") && claim.contains("unknown start"))
    }));
    let linked_conflicts: i64 = store
        .connection
        .query_row(
            "SELECT COUNT(*) FROM scholar_finding_claim_links
         WHERE claim_id=?1 AND relation='contradicts'",
            [conflicting_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(linked_conflicts, 2);

    store
        .resolve_semantic_claim(conflicting_id, "retract", "The source was disproven")
        .unwrap();
    assert_eq!(
        store
            .automatically_review_pending_scholar_evidence()
            .unwrap(),
        0
    );
    assert!(store.active_semantic_claims().unwrap().is_empty());
    let pending: i64 = store
        .connection
        .query_row(
            "SELECT COUNT(*) FROM scholar_findings f
         JOIN scholar_source_assessments a ON a.finding_id=f.id
         WHERE f.subject='Mars' AND f.relation='has moons' AND f.object='two'
           AND f.status='needs_review' AND a.gate_status='awaiting_corroboration'
           AND a.independence_verified=0",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending, 2);
    let automatic_origins: i64 = store
        .connection
        .query_row(
            "SELECT COUNT(*) FROM scholar_source_origin_reviews origin
         JOIN scholar_findings f ON f.id=origin.finding_id
         WHERE f.subject='Mars' AND f.relation='has moons' AND f.object='two'
           AND origin.review_method='automatic'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(automatic_origins, 0);
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
                Some(scholar_finding(
                    "Mars",
                    "is a",
                    "planet",
                    "Mars is a planet.",
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
