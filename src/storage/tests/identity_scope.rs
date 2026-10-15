//! Storage regressions for identity scope.
use super::*;

#[test]
fn scope_mismatch_qualifies_an_overlapping_claim_without_challenging_it() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store.connection.execute(
        "INSERT INTO semantic_claims
            (subject, relation, object, source, created_at, attribution, polarity, modality)
         VALUES ('Mars', 'has moons', 'one', 'old source', '2026-01-01T00:00:00Z', NULL, 'asserted', 'factual')",
        [],
    ).unwrap();
    let claim_id = store.connection.last_insert_rowid();
    store
        .connection
        .execute(
            "INSERT INTO semantic_claims
            (subject, relation, object, source, created_at)
         VALUES ('Mars', 'has atmosphere', 'thin', 'another source', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    let different_property_id = store.connection.last_insert_rowid();
    store
        .enqueue_research("scope-check", 1, "How many moons does Mars have?")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    let passage = "Mars has two moons.";
    store
        .finish_scholar_job_with_profile_and_relevance(
            &job.id,
            "Mars moons",
            "https://example.org/mars-moons",
            passage,
            Some(ScholarFinding {
                semantic_support: None,
                subject: "Mars".into(),
                relation: "has moons".into(),
                object: "two".into(),
                evidence_text: passage.into(),
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
    let finding = store.scholar_findings(&job.id).unwrap().remove(0);
    let relation: String = store
        .connection
        .query_row(
            "SELECT relation FROM scholar_finding_claim_links WHERE finding_id=?1 AND claim_id=?2",
            rusqlite::params![finding.id, claim_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(relation, "qualifies");
    let unresolved_relation: String = store
        .connection
        .query_row(
            "SELECT relation FROM scholar_finding_claim_links WHERE finding_id=?1 AND claim_id=?2",
            rusqlite::params![finding.id, different_property_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(unresolved_relation, "unresolved");
    let answer_candidate = store
        .scholar_evidence_candidates(false)
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.id == finding.id)
        .unwrap();
    assert!(answer_candidate.conflicting_claims.is_empty());
    let status: String = store
        .connection
        .query_row(
            "SELECT status FROM semantic_claims WHERE id=?1",
            [claim_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "active");
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
                semantic_support: None,
                subject: "Alex".into(),
                relation: "served as mayor in".into(),
                object: "Town B".into(),
                evidence_text: excerpt.into(),
                confidence: 0.99,
                valid_from: Some("2015-01-01".into()),
                valid_until: Some("2020-01-01".into()),
                source_quality: "reference".into(),
                source_updated_at: None,
                proposition: Default::default(),
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
                semantic_support: None,
                subject: "Alex".into(),
                relation: "served as mayor in".into(),
                object: "Town C".into(),
                evidence_text: excerpt.into(),
                confidence: 0.99,
                valid_from: Some("2019-01-01".into()),
                valid_until: Some("2025-01-01".into()),
                source_quality: "reference".into(),
                source_updated_at: None,
                proposition: Default::default(),
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
fn reviewed_entity_aliases_share_claim_sources_and_detect_conflicts_after_restart() {
    let path = temp_path();
    let store = LearningStore::open(&path).unwrap();
    let passage = "Walkers Snack Foods Limited, known as Walkers, is a British company.";
    let mut calls = 0;
    let aliases=crate::entities::resolve_with("Walkers","Walkers",passage,|_| {
        calls+=1;
        Ok(if calls==1 {serde_json::json!({"aliases":[{"name":"Walkers","alias":"Walkers Snack Foods Limited","evidence_text":passage}]}).to_string()}
            else {serde_json::json!({"verdict":"entailed","reason":"Both names explicitly identify the same company.","evidence_text":passage}).to_string()})
    }).unwrap();
    store
        .record_entity_alias(None, "https://example.org/walkers", passage, &aliases[0])
        .unwrap();
    assert!(
        store
            .record_entity_alias(
                None,
                "https://example.org/walkers",
                "An unrelated passage.",
                &aliases[0]
            )
            .is_err()
    );
    drop(store);
    let mut store = LearningStore::open(&path).unwrap();
    let names = store.entity_aliases().unwrap();
    assert!(names.same("Walkers", "Walkers Snack Foods Limited"));
    assert!(names.same("Walkers Snack Foods Limited", "Walkers"));
    assert!(!names.same("Walkers", "Walkers Shortbread"));
    assert!(!names.same("Walkers", "PepsiCo"));
    assert!(!crate::entities::matches_claim(
        &["Walkers".into()],
        "Ruffles",
        "crisps in various flavours",
        &names
    ));
    assert!(crate::entities::matches_claim(
        &["Walkers".into()],
        "Walkers Snack Foods Limited",
        "crisps",
        &names
    ));
    assert!(crate::entities::matches_claim(
        &["Walkers".into()],
        "A crisp brand",
        "Walkers Snack Foods Limited",
        &names
    ));
    for (turn, subject, object) in [
        (1, "Walkers", "Leicester"),
        (2, "Walkers Snack Foods Limited", "Leicester"),
        (3, "Walkers Snack Foods Limited", "London"),
    ] {
        let text = format!("{subject} is based in {object}.");
        let id = store.stage_observation("aliases", turn, &text).unwrap();
        store.claim_next_observation(600).unwrap().unwrap();
        store
            .finish_extraction(
                &id,
                &[CandidateClaim {
                    subject: subject.into(),
                    relation: "is based in".into(),
                    object: object.into(),
                    evidence_text: text,
                    context_message_ids: Vec::new(),
                    resolution: None,
                    confidence: 0.98,
                    status: "proposed".into(),
                    decision_reason: None,
                    valid_from: None,
                    valid_until: None,
                }],
                None,
            )
            .unwrap();
    }
    assert_eq!(store.claim_count().unwrap(), 1);
    let sources: i64 = store
        .connection
        .query_row("SELECT COUNT(*) FROM semantic_claim_sources", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(sources, 2);
    let conflicts: i64 = store
        .connection
        .query_row(
            "SELECT COUNT(*) FROM candidate_claims WHERE status='needs_review'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(conflicts, 1);
}

#[test]
fn non_overlapping_claim_periods_are_not_linked_as_contradictions() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store.connection.execute(
        "INSERT INTO semantic_claims
            (subject, relation, object, source, created_at, valid_from, valid_until)
         VALUES ('Mars', 'has moons', 'one', 'old source', '2020-01-01T00:00:00Z', '2010-01-01', '2015-12-31')",
        [],
    ).unwrap();
    let old_claim_id = store.connection.last_insert_rowid();
    store
        .enqueue_research(
            "time-scope-check",
            1,
            "How many moons did Mars have in 2022?",
        )
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    let passage = "Mars had two moons from 2022-01-01 through 2022-12-31.";
    store
        .finish_scholar_job_with_profile_and_relevance(
            &job.id,
            "Mars moons in 2022",
            "https://example.org/mars-2022",
            passage,
            Some(ScholarFinding {
                semantic_support: None,
                subject: "Mars".into(),
                relation: "has moons".into(),
                object: "two".into(),
                evidence_text: passage.into(),
                confidence: 0.99,
                valid_from: Some("2022-01-01".into()),
                valid_until: Some("2022-12-31".into()),
                source_quality: "reference".into(),
                source_updated_at: None,
                proposition: Default::default(),
            }),
            None,
            &ScholarSourceProfile::default(),
            "direct",
        )
        .unwrap();
    let finding_id = store.scholar_findings(&job.id).unwrap()[0].id;
    let contradiction_count: i64 = store
        .connection
        .query_row(
            "SELECT COUNT(*) FROM scholar_finding_claim_links
         WHERE finding_id=?1 AND claim_id=?2 AND relation='contradicts'",
            rusqlite::params![finding_id, old_claim_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(contradiction_count, 0);
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn geographic_and_context_scope_mismatches_are_recorded_as_unresolved() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let cases = [
        (
            "Walkers",
            "sells",
            "Cheese and Onion",
            Some("England"),
            None,
            "Walkers sells Cheese and Onion crisps in Wales.",
            Some("Wales"),
            None,
        ),
        (
            "Clark Kent",
            "is",
            "Superman",
            None,
            Some("Earth-One universe"),
            "In the Prime Earth universe, Clark Kent is Superman.",
            None,
            Some("Prime Earth universe"),
        ),
    ];
    for (index, (subject, relation, object, old_geo, old_context, passage, geo, context)) in
        cases.into_iter().enumerate()
    {
        store
            .connection
            .execute(
                "INSERT INTO semantic_claims
                (subject, relation, object, source, created_at, geographic_scope, context_scope)
             VALUES (?1, ?2, ?3, 'old source', '2026-01-01T00:00:00Z', ?4, ?5)",
                rusqlite::params![subject, relation, object, old_geo, old_context],
            )
            .unwrap();
        let old_claim_id = store.connection.last_insert_rowid();
        let job_name = format!("scope-check-{index}");
        store
            .enqueue_research(&job_name, 1, "Check this scoped claim")
            .unwrap();
        let job = store.claim_scholar_job().unwrap().unwrap();
        store
            .finish_scholar_job_with_profile_and_relevance(
                &job.id,
                "Scoped claim",
                &format!("https://example.org/scoped-{index}"),
                passage,
                Some(ScholarFinding {
                    semantic_support: None,
                    subject: subject.into(),
                    relation: relation.into(),
                    object: object.into(),
                    evidence_text: passage.into(),
                    confidence: 0.99,
                    valid_from: None,
                    valid_until: None,
                    source_quality: "reference".into(),
                    source_updated_at: None,
                    proposition: PropositionMetadata {
                        geographic_scope: geo.map(str::to_owned),
                        context_scope: context.map(str::to_owned),
                        ..PropositionMetadata::default()
                    },
                }),
                None,
                &ScholarSourceProfile::default(),
                "direct",
            )
            .unwrap();
        let finding_id = store.scholar_findings(&job.id).unwrap()[0].id;
        let stored_finding = store.scholar_findings(&job.id).unwrap().remove(0);
        assert_eq!(stored_finding.geographic_scope.as_deref(), geo);
        assert_eq!(stored_finding.context_scope.as_deref(), context);
        let links: Vec<(String, String)> = store
            .connection
            .prepare(
                "SELECT relation, rationale FROM scholar_finding_claim_links
             WHERE finding_id=?1 AND claim_id=?2",
            )
            .unwrap()
            .query_map(rusqlite::params![finding_id, old_claim_id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].0, "unresolved");
        assert!(links[0].1.contains("scope"));
        let gate_status = store
            .scholar_source_assessment(finding_id)
            .unwrap()
            .unwrap()
            .gate_status;
        assert_ne!(gate_status, "conflict");
        let answer_candidate = store
            .scholar_evidence_candidates(false)
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.id == finding_id)
            .unwrap();
        assert!(answer_candidate.conflicting_claims.is_empty());
    }
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn scoped_identity_links_match_claims_only_within_their_exact_context() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let identity_passage =
        "In the Earth-One universe, Clark Kent is the civilian identity of Superman.";
    let mut calls = 0;
    let aliases = crate::entities::resolve_with("Clark Kent", "Identity", identity_passage, |_| {
        calls += 1;
        Ok(if calls == 1 {
            serde_json::json!({"aliases":[{"name":"Clark Kent","alias":"Superman","evidence_text":identity_passage,"identity_scope":"Earth-One universe"}]}).to_string()
        } else {
            serde_json::json!({"verdict":"entailed","reason":"Identity is explicitly limited to Earth-One.","evidence_text":identity_passage}).to_string()
        })
    })
    .unwrap();
    store
        .record_entity_alias(
            None,
            "https://example.org/earth-one-identity",
            identity_passage,
            &aliases[0],
        )
        .unwrap();
    store.connection.execute(
        "INSERT INTO semantic_claims
            (subject, relation, object, source, created_at, context_scope)
         VALUES ('Clark Kent', 'was born on', 'Krypton', 'old source', '2026-01-01T00:00:00Z', 'Earth-One universe')",
        [],
    ).unwrap();
    let claim_id = store.connection.last_insert_rowid();
    let passage = "In the Earth-One universe, Superman was born on Krypton.";
    store
        .enqueue_research("scoped-alias", 1, "Where was Superman born in Earth-One?")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    store
        .finish_scholar_job_with_profile_and_relevance(
            &job.id,
            "Superman",
            "https://example.org/earth-one-birthplace",
            passage,
            Some(ScholarFinding {
                semantic_support: None,
                subject: "Superman".into(),
                relation: "was born on".into(),
                object: "Krypton".into(),
                evidence_text: passage.into(),
                confidence: 0.99,
                valid_from: None,
                valid_until: None,
                source_quality: "reference".into(),
                source_updated_at: None,
                proposition: PropositionMetadata {
                    context_scope: Some("Earth-One universe".into()),
                    ..PropositionMetadata::default()
                },
            }),
            None,
            &ScholarSourceProfile::default(),
            "direct",
        )
        .unwrap();
    let finding_id = store.scholar_findings(&job.id).unwrap()[0].id;
    let relation: String = store
        .connection
        .query_row(
            "SELECT relation FROM scholar_finding_claim_links WHERE finding_id=?1 AND claim_id=?2",
            rusqlite::params![finding_id, claim_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(relation, "supports");
    assert!(
        !store
            .entity_aliases()
            .unwrap()
            .same("Clark Kent", "Superman")
    );
    assert!(store.entity_aliases().unwrap().same_in_scope(
        "Clark Kent",
        "Superman",
        "Earth-One universe"
    ));
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn alias_ambiguity_and_rejected_identity_sources_do_not_merge_entities() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    for (turn, long_name) in [
        (1, "Walkers Snack Foods Limited"),
        (2, "Walkers Shortbread"),
    ] {
        let passage = format!("{long_name}, also known as Walkers, is a company.");
        let mut calls = 0;
        let aliases=crate::entities::resolve_with("Walkers","Identity",&passage,|_| {
            calls+=1;
            Ok(if calls==1 {serde_json::json!({"aliases":[{"name":"Walkers","alias":long_name,"evidence_text":passage}]}).to_string()}
                else {serde_json::json!({"verdict":"entailed","reason":"The source explicitly states an alternative name.","evidence_text":passage}).to_string()})
        }).unwrap();
        let url = format!("https://example.org/entity-{turn}");
        store
            .enqueue_research("identities", turn, "What is Walkers?")
            .unwrap();
        let job = store.claim_scholar_job().unwrap().unwrap();
        store
            .finish_scholar_job(
                &job.id,
                "Identity",
                &url,
                &passage,
                Some(aliases[0].finding.clone()),
                None,
            )
            .unwrap();
        let finding = store.scholar_findings(&job.id).unwrap()[0].id;
        store
            .record_entity_alias(Some(finding), &url, &passage, &aliases[0])
            .unwrap();
        let aliases = store.entity_aliases().unwrap();
        if turn == 1 {
            assert!(aliases.same("Walkers", long_name));
        } else {
            assert!(!aliases.same("Walkers", "Walkers Snack Foods Limited"));
            assert!(!aliases.same("Walkers", "Walkers Shortbread"));
            assert!(!aliases.same("Walkers Snack Foods Limited", "Walkers Shortbread"));
            store
                .connection
                .execute(
                    "UPDATE scholar_findings SET status='rejected' WHERE id=?1",
                    [finding],
                )
                .unwrap();
            let aliases = store.entity_aliases().unwrap();
            assert!(aliases.same("Walkers", "Walkers Snack Foods Limited"));
            assert!(!aliases.same("Walkers", "Walkers Shortbread"));
        }
    }
}

#[test]
fn contextual_identity_is_retained_but_never_becomes_a_global_alias() {
    let path = temp_path();
    let store = LearningStore::open(&path).unwrap();
    let passage = "In the Earth-One universe, Clark Kent is the civilian identity of Superman.";
    let mut calls = 0;
    let aliases = crate::entities::resolve_with("Clark Kent", "Identity", passage, |_| {
        calls += 1;
        Ok(if calls == 1 {
            serde_json::json!({"aliases":[{"name":"Clark Kent","alias":"Superman","evidence_text":passage,"identity_scope":"Earth-One universe"}]}).to_string()
        } else {
            serde_json::json!({"verdict":"entailed","reason":"The passage supports the identity in this universe.","evidence_text":passage}).to_string()
        })
    }).unwrap();
    assert_eq!(aliases.len(), 1);
    store
        .record_entity_alias(None, "https://example.org/earth-one", passage, &aliases[0])
        .unwrap();
    let loaded = store.entity_aliases().unwrap();
    assert!(!loaded.same("Clark Kent", "Superman"));
    assert!(loaded.same_in_scope("Clark Kent", "Superman", "Earth-One universe"));
    assert!(!loaded.same_in_scope("Clark Kent", "Superman", "Prime Earth universe"));
    drop(store);
    let _ = std::fs::remove_file(path);
}
