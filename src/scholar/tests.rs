//! Scholar orchestration and extraction integration regressions.
use super::claims::claim_is_grounded;
use super::discovery::{SerpApiResponse, linked_serpapi_results};
use super::{
    ExtractionAttempt, ScholarFindingJson, SearchPage, calibrate_research_relevance,
    discover_configured_searxng, discovery_trigger, evaluate_extraction, finding_matches_target,
    html_to_text, parse_extraction, relevant_page, retry_after_delay_ms, should_retry_grounding,
};
use crate::storage::{CorroborationTask, ScholarFinding};

#[test]
fn flattened_extraction_preserves_attribution_and_proposition_scope() {
    let passage = "In the Earth-One universe, Dana Vale said the crystal may not conduct electricity in Europe.";
    let nested = serde_json::json!({"claim":{
        "subject":"the crystal","relation":"may not conduct","object":"electricity",
        "evidence_text":passage,"confidence":0.98,"question_relevance":"direct",
        "attribution":"Dana Vale","geographic_scope":"Europe",
        "context_scope":"Earth-One universe","polarity":"negated","modality":"possible"
    }});
    let mut flattened = nested["claim"].clone();
    flattened.as_object_mut().unwrap().remove("subject");
    flattened["claim"] = serde_json::json!("the crystal");
    let expected = parse_extraction(&nested.to_string())
        .unwrap()
        .claim
        .unwrap();
    let recovered = parse_extraction(&flattened.to_string())
        .unwrap()
        .claim
        .unwrap();
    assert_eq!(recovered.attribution, expected.attribution);
    assert_eq!(recovered.polarity, expected.polarity);
    assert_eq!(recovered.modality, expected.modality);
    assert_eq!(recovered.geographic_scope, expected.geographic_scope);
    assert_eq!(recovered.context_scope, expected.context_scope);
    assert_eq!(
        recovered.proposition(passage),
        expected.proposition(passage)
    );
}

#[test]
fn proposition_scope_is_explicit_and_unknown_fields_stay_unknown() {
    let parsed: ScholarFindingJson = serde_json::from_str(
        r#"{"subject":"Mars","relation":"is","object":"a planet","evidence_text":"The astronomer said Mars is not a planet in Europe.","attribution":"The astronomer said","geographic_scope":"Europe","context_scope":"outer space","polarity":"negated","modality":"possible"}"#,
    )
    .unwrap();
    let metadata = parsed.proposition(&parsed.evidence_text);
    assert_eq!(metadata.attribution.as_deref(), Some("The astronomer said"));
    assert_eq!(metadata.polarity, crate::storage::ClaimPolarity::Negated);
    assert_eq!(metadata.modality, crate::storage::ClaimModality::Possible);
    assert_eq!(metadata.geographic_scope.as_deref(), Some("Europe"));
    assert_eq!(metadata.context_scope, None);

    let unscoped: ScholarFindingJson = serde_json::from_str(
        r#"{"subject":"Mars","relation":"is","object":"a planet","evidence_text":"Mars is a planet."}"#,
    )
    .unwrap();
    let unknown = unscoped.proposition(&unscoped.evidence_text);
    assert_eq!(unknown.attribution, None);
    assert_eq!(unknown.polarity, crate::storage::ClaimPolarity::Unknown);
    assert_eq!(unknown.modality, crate::storage::ClaimModality::Unknown);
}

#[test]
fn vacci_box_feedback_replaces_company_fact_with_relevant_healthcare_fact() {
    let path = std::env::temp_dir().join(format!(
        "community-vaccibox-repair-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut store = crate::storage::LearningStore::open(&path).unwrap();
    let question =
        "How can renewable energy technologies support healthcare access in remote communities?";
    let company_fact =
        "Norah Magero co-founded Drop Access to address energy challenges in rural areas.";
    let useful_fact = "The VacciBox is a 50-litre portable, solar-powered refrigerator designed for healthcare use.";
    let passage = format!(
        "{company_fact}\n{useful_fact}\nIt can be carried on motorbikes, bicycles, or boats, making it ideal for remote regions."
    );
    store
        .enqueue_research("vaccibox-test", 1, question)
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    store
        .finish_scholar_job(
            &job.id,
            "Norah Magero",
            "https://en.wikipedia.org/wiki/Norah_Magero",
            &passage,
            Some(ScholarFinding {
                semantic_support: None,
                subject: "Norah Magero".into(),
                relation: "co-founded".into(),
                object: "Drop Access".into(),
                evidence_text: company_fact.into(),
                confidence: 0.98,
                valid_from: None,
                valid_until: None,
                source_quality: "unknown".into(),
                source_updated_at: None,
                proposition: Default::default(),
            }),
            None,
        )
        .unwrap();
    let root = store.scholar_findings(&job.id).unwrap()[0].id;
    store
        .reject_scholar_finding_with_source_feedback(
            root,
            "Company fact does not answer the healthcare question.",
            "repair",
            "Extract how VacciBox uses solar energy for healthcare.",
            None,
            Some(useful_fact),
        )
        .unwrap();
    let task = store.claim_scholar_feedback("repair").unwrap().unwrap();
    super::process_feedback_task_with(&mut store, &task, |task| {
        assert_eq!(task.question, question);
        assert_eq!(task.suggested_evidence.as_deref(), Some(useful_fact));
        evaluate_extraction(&serde_json::json!({"claim":{
            "subject":"VacciBox", "relation":"is a", "object":"50-litre portable, solar-powered refrigerator designed for healthcare use",
            "evidence_text":useful_fact, "confidence":0.98, "question_relevance":"direct", "valid_from":null, "valid_until":null
        }}).to_string(), &task.source_excerpt, "unknown", None)
    }).unwrap();
    let findings = store.scholar_findings(&task.job_id).unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].subject, "VacciBox");
    assert_eq!(findings[0].status, "needs_review");
    assert_eq!(
        findings[0].source_url,
        "https://en.wikipedia.org/wiki/Norah_Magero"
    );
    assert!(
        store
            .scholar_feedback_excluded_sources(&task.job_id)
            .unwrap()
            .is_empty()
    );
    assert!(store.active_semantic_claims().unwrap().is_empty());
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn feedback_exclusions_only_block_the_rejected_page() {
    let excluded = vec![
        "https://www.bad.example/claim".into(),
        "https://en.wikipedia.org/wiki/Flat_Earth".into(),
    ];
    assert!(super::feedback_source_blocked(
        "https://bad.example/claim?query=x#section",
        &excluded
    ));
    assert!(!super::feedback_source_blocked(
        "https://bad.example/other?query=x",
        &excluded
    ));
    assert!(super::feedback_source_blocked(
        "https://en.wikipedia.org/wiki/Flat_Earth?oldid=1#History",
        &excluded
    ));
    assert!(!super::feedback_source_blocked(
        "https://en.wikipedia.org/wiki/Earth",
        &excluded
    ));
    assert!(!super::feedback_source_blocked(
        "https://good.example/claim",
        &excluded
    ));
}

#[test]
fn guided_repair_preserves_passage_and_stages_corrected_claim_for_ai_review() {
    let path = std::env::temp_dir().join(format!(
        "community-guided-repair-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut store = crate::storage::LearningStore::open(&path).unwrap();
    store
        .enqueue_research("guided-repair", 1, "What is Mars?")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    store
        .finish_scholar_job(
            &job.id,
            "Mars",
            "https://reference.example/mars",
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
    let original = store.scholar_findings(&job.id).unwrap()[0].id;
    store
        .reject_scholar_finding_with_feedback(
            original,
            "Needs precise wording",
            "repair",
            "State the classification from the passage.",
        )
        .unwrap();
    let task = store.claim_scholar_feedback("repair").unwrap().unwrap();
    assert_eq!(super::process_feedback_task_with(&mut store, &task, |task| {
        assert_eq!(task.question, "What is Mars?");
        assert_eq!(task.guidance, "State the classification from the passage.");
        assert_eq!(task.source_excerpt, "Mars is a planet.");
        evaluate_extraction(r#"{"claim":{"subject":"Mars","relation":"is_a","object":"planet","evidence_text":"Mars is a planet.","confidence":0.98,"question_relevance":"direct","valid_from":null,"valid_until":null}}"#, &task.source_excerpt, "unknown", None)
    }).unwrap(), 1);
    let corrected = store.scholar_findings(&task.job_id).unwrap();
    assert_eq!(corrected.len(), 1);
    assert_eq!(corrected[0].relation, "is a");
    assert_eq!(corrected[0].status, "needs_review");
    assert_eq!(
        store.scholar_findings(&job.id).unwrap()[0].status,
        "rejected"
    );
    assert!(store.active_semantic_claims().unwrap().is_empty());
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn configured_searxng_is_dispatched_only_when_no_candidates_exist() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 2048];
        let _ = stream.read(&mut request).unwrap();
        let body = r#"{"results":[{"title":"Chicken soup","url":"https://example.test/chicken-soup","content":"Chicken soup is made with chicken and vegetables."}]}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });

    let path = std::env::temp_dir().join(format!(
        "community-searxng-primary-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut store = crate::storage::LearningStore::open(&path).unwrap();
    store
        .enqueue_research("fallback-test", 1, "What is in chicken soup?")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    assert!(
        discover_configured_searxng(
            &mut store,
            &job.id,
            "chicken soup ingredients",
            Some(&format!("http://127.0.0.1:{}/search", address.port()))
        )
        .unwrap()
    );
    server.join().unwrap();
    let candidates = store.scholar_search_candidates(&job.id).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].provider, "searxng");
    assert_eq!(candidates[0].title, "Chicken soup");

    assert!(
        !discover_configured_searxng(
            &mut store,
            &job.id,
            "chicken soup ingredients",
            Some("https://untrusted.example/search")
        )
        .unwrap()
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn corroboration_requires_the_same_claim_and_validity_period() {
    let task = CorroborationTask {
        finding_id: 1,
        provider: "serpapi".into(),
        job_id: "job".into(),
        question: "Mars is a planet".into(),
        subject: "Mars".into(),
        relation: "is a".into(),
        object: "planet".into(),
        valid_from: None,
        valid_until: None,
    };
    let mut finding = ScholarFinding {
        semantic_support: None,
        subject: "mars".into(),
        relation: "is a".into(),
        object: "planet".into(),
        evidence_text: "Mars is a planet.".into(),
        confidence: 0.99,
        valid_from: None,
        valid_until: None,
        source_quality: "unknown".into(),
        source_updated_at: None,
        proposition: Default::default(),
    };
    assert!(finding_matches_target(&finding, &task));
    finding.object = "star".into();
    assert!(!finding_matches_target(&finding, &task));
    finding.object = "planet".into();
    finding.valid_until = Some("2020-01-01".into());
    assert!(!finding_matches_target(&finding, &task));
}

#[test]
fn weak_wikipedia_uses_normal_spacing_and_strong_wikipedia_waits_twice_as_long() {
    let now = 10_000_000;
    let last = now - 3_600_000;
    assert_eq!(
        discovery_trigger(true, Some(last), now, 1_800),
        Some("wikipedia_insufficient")
    );
    assert_eq!(discovery_trigger(false, Some(last), now, 3_600), None);
    assert_eq!(
        discovery_trigger(false, Some(last), now, 1_800),
        Some("exploration_elapsed")
    );
    assert_eq!(discovery_trigger(true, Some(now - 1_000), now, 1_800), None);
    assert_eq!(
        discovery_trigger(false, Some(now - 1_000), now, 1_800),
        None
    );
    assert_eq!(
        discovery_trigger(false, None, now, 3_600),
        Some("exploration_elapsed")
    );
}

#[test]
fn logged_definition_unpacks_with_copula_alias_but_repeated_name_does_not() {
    let passage = "Antidisestablishmentarianism is a political position that originated in 19th century Britain.";
    let response = |object: &str| {
        serde_json::json!({"claim": {
            "subject": "Antidisestablishmentarianism", "relation": "is_a", "object": object,
            "evidence_text": passage, "confidence": 0.98, "question_relevance": "direct"
        }})
        .to_string()
    };
    let accepted = evaluate_extraction(&response("political position"), passage, "reference", None);
    assert_eq!(accepted.status, "accepted");
    assert_eq!(accepted.finding.unwrap().relation, "is a");
    let collapsed = evaluate_extraction(
        &response("Antidisestablishmentarianism"),
        passage,
        "reference",
        None,
    );
    assert_eq!(collapsed.status, "ungrounded");
    assert_eq!(
        collapsed.reason,
        "object adds no factual detail beyond the subject"
    );
}

#[test]
fn semantic_support_accepts_synonyms_and_replaces_paraphrased_citation() {
    let passage = "The world is spherical.";
    let raw = serde_json::json!({"claim":{"subject":"The world","relation":"is","object":"round","evidence_text":"The world is round.","confidence":0.98,"question_relevance":"direct"}}).to_string();
    assert_eq!(
        evaluate_extraction(&raw, passage, "unknown", None).status,
        "ungrounded"
    );
    let attempt = super::evaluate_extraction_semantically(
        &raw,
        "What shape is the world?",
        "World",
        passage,
        "unknown",
        None,
        |_| {
            Ok(serde_json::json!({"verdict":"entailed","reason":"Spherical supports round.","evidence_text":passage}).to_string())
        },
    );
    assert_eq!(attempt.status, "accepted", "{}", attempt.reason);
    let finding = attempt.finding.unwrap();
    assert_eq!(finding.object, "round");
    assert_eq!(finding.evidence_text, passage);
    assert!(
        finding
            .semantic_support
            .as_ref()
            .unwrap()
            .supports(&finding, passage)
    );
    assert!(!crate::storage::evidence_supports_claim(&finding));
}

#[test]
fn semantic_rejection_overrides_lexical_overlap_and_invalid_review_does_not_fall_back() {
    let passage = "The world is not exactly spherical.";
    let raw = serde_json::json!({"claim":{"subject":"The world","relation":"is","object":"spherical","evidence_text":passage,"confidence":0.98,"question_relevance":"direct"}}).to_string();
    // All claim words occur in this quotation, but it drops an essential caveat.
    assert_eq!(
        evaluate_extraction(&raw, passage, "unknown", None).status,
        "accepted"
    );
    for verdict in ["qualified", "contradicted", "insufficient"] {
        let attempt = super::evaluate_extraction_semantically(
            &raw,
            "What shape is the world?",
            "World",
            passage,
            "unknown",
            None,
            |_| {
                Ok(serde_json::json!({"verdict":verdict,"reason":"The candidate omits the qualification.","evidence_text":passage}).to_string())
            },
        );
        assert!(attempt.finding.is_none());
        assert!(attempt.reason.contains(verdict));
    }
    let invalid = super::evaluate_extraction_semantically(
        &raw,
        "What shape is the world?",
        "World",
        passage,
        "unknown",
        None,
        |_| Ok("invalid json".into()),
    );
    assert!(invalid.finding.is_none());
    assert!(invalid.reason.contains("assessment failed"));
}

#[test]
fn extraction_restores_original_whitespace_before_grounding_and_storage() {
    let passage = "Chicken soup contains chicken stock.\nIt is served warm.";
    let raw = serde_json::json!({"claim": {
        "subject":"Chicken soup", "relation":"contains", "object":"chicken stock",
        "evidence_text":"Chicken soup contains chicken stock. It is served warm.",
        "confidence":0.98, "question_relevance":"direct"
    }})
    .to_string();
    let attempt = evaluate_extraction(&raw, passage, "unknown", None);
    assert_eq!(attempt.status, "accepted", "{}", attempt.reason);
    let finding = attempt.finding.unwrap();
    assert_eq!(finding.evidence_text, passage);
    assert!(passage.contains(&finding.evidence_text));
    assert!(crate::storage::evidence_supports_claim(&finding));
    let diagnostics = super::extraction_evidence_diagnostics(Some(&raw), passage);
    assert_eq!(diagnostics["method"], "whitespace_only");
    assert_eq!(diagnostics["original_passage_span"], passage);
}

#[test]
fn logged_tikka_evidence_matches_with_newline_but_keeps_other_grounding_checks() {
    let evidence = "It has been suggested that the dish is derived from butter chicken, a popular dish in the northern Indian subcontinent. Rahul Verma, a food critic for The Hindu, claimed that the dish has its origins in the Punjab region.";
    let passage = evidence.replace(". Rahul", ".\nRahul");
    let raw = serde_json::json!({"claim": {
        "subject":"chicken tikka masala", "relation":"is derived from",
        "object":"butter chicken in the northern Indian subcontinent",
        "evidence_text":evidence, "confidence":0.98, "question_relevance":"direct"
    }})
    .to_string();
    let attempt = evaluate_extraction(&raw, &passage, "unknown", None);
    assert_ne!(
        attempt.reason,
        "evidence_text is not an exact substring of the selected passage"
    );
    assert_eq!(
        super::extraction_evidence_diagnostics(Some(&raw), &passage)["method"],
        "whitespace_only"
    );
    // Resolving formatting does not waive missing subject/claim support.
    assert_eq!(attempt.status, "ungrounded");
    assert_eq!(
        attempt.reason,
        "subject terms are not sufficiently present in the evidence span"
    );
}

#[test]
fn matching_diagnostics_do_not_hide_invented_or_truncated_evidence() {
    let passage = "Chicken soup is not curry.\nIts origins are debated.";
    let raw = serde_json::json!({"claim": {
        "subject":"Chicken soup", "relation":"is", "object":"curry",
        "evidence_text":"Chicken soup is curry. Its origins are debated.",
        "confidence":0.98, "question_relevance":"direct"
    }})
    .to_string();
    assert_eq!(
        super::extraction_evidence_diagnostics(Some(&raw), passage)["method"],
        "no_contiguous_match"
    );
    assert_eq!(
        evaluate_extraction(&raw, passage, "unknown", None).status,
        "ungrounded"
    );
    let partial = "Chicken soup is not curry.\nIts origins are deb";
    assert!(
        super::evidence::original_span(
            partial,
            "Chicken soup is not curry. Its origins are debated."
        )
        .is_none()
    );
}

#[test]
fn extraction_outcomes_distinguish_supported_claims_from_model_failures() {
    let passage = "Chicken soup contains chicken stock and carrots.";
    let response = |confidence: f64, evidence: &str| {
        serde_json::json!({"claim": {
            "subject": "Chicken soup",
            "relation": "contains",
            "object": "chicken stock and carrots",
            "evidence_text": evidence,
            "confidence": confidence,
            "question_relevance": "direct",
            "valid_from": null,
            "valid_until": null
        }})
        .to_string()
    };
    let accepted = evaluate_extraction(&response(0.99, passage), passage, "unknown", None);
    assert_eq!(accepted.status, "accepted");
    assert_eq!(accepted.finding.unwrap().source_quality, "unknown");
    let low = evaluate_extraction(&response(0.7, passage), passage, "unknown", None);
    assert_eq!(low.status, "low_confidence");
    let unsupported = evaluate_extraction(
        &response(0.99, "Chicken soup contains rocket fuel."),
        passage,
        "unknown",
        None,
    );
    assert_eq!(unsupported.status, "ungrounded");
    assert!(unsupported.reason.contains("exact substring"));
    let plausible_but_not_direct = evaluate_extraction(
        &serde_json::json!({"claim": {
            "subject": "chicken soup", "relation": "contains", "object": "chicken stock",
            "evidence_text": "chicken stock added to a soup pot", "confidence": 0.98,
            "question_relevance": "direct"
        }})
        .to_string(),
        "chicken stock added to a soup pot",
        "unknown",
        None,
    );
    assert_eq!(plausible_but_not_direct.status, "ungrounded");
    let invented_date = serde_json::json!({"claim": {
        "subject": "Chicken soup", "relation": "contains", "object": "chicken stock",
        "evidence_text": passage, "confidence": 0.98,
        "question_relevance": "direct", "valid_from": "2024-01-01"
    }})
    .to_string();
    let dated = evaluate_extraction(&invented_date, passage, "unknown", None);
    assert_eq!(dated.status, "ungrounded");
    assert!(dated.reason.contains("validity dates"));
    assert_eq!(
        evaluate_extraction("{\"claim\":null}", passage, "unknown", None).status,
        "no_claim"
    );
    assert_eq!(
        evaluate_extraction("unparseable", passage, "unknown", None).status,
        "invalid_json"
    );
}

#[test]
fn exact_evidence_does_not_rescue_a_relation_the_source_never_states() {
    let passage = "Clay pot cooking is a process of cooking food in a ceramic pot.";
    let response = serde_json::json!({"claim": {
        "subject": "Clay pot cooking",
        "relation": "is located in",
        "object": "a ceramic pot",
        "evidence_text": passage,
        "confidence": 0.98,
        "question_relevance": "direct",
        "valid_from": null,
        "valid_until": null
    }})
    .to_string();

    let result = evaluate_extraction(&response, passage, "reference", None);

    assert_eq!(result.status, "ungrounded");
    assert!(result.reason.contains("direct support"));
}

#[test]
fn malformed_and_ungrounded_outputs_receive_one_bounded_ai_repair() {
    let passage = "Chicken soup contains carrots.";
    let ungrounded = evaluate_extraction(
        &serde_json::json!({"claim": {
            "subject": "Chicken soup",
            "relation": "contains",
            "object": "rocket fuel",
            "evidence_text": passage,
            "confidence": 0.98,
            "question_relevance": "direct"
        }})
        .to_string(),
        passage,
        "reference",
        None,
    );
    let low_confidence = evaluate_extraction(
        &serde_json::json!({"claim": {
            "subject": "Chicken soup",
            "relation": "contains",
            "object": "carrots",
            "evidence_text": passage,
            "confidence": 0.70,
            "question_relevance": "direct"
        }})
        .to_string(),
        passage,
        "reference",
        None,
    );
    let malformed_json = evaluate_extraction("not JSON", passage, "reference", None);
    let empty = evaluate_extraction(r#"{"claim":null}"#, passage, "reference", None);

    assert!(should_retry_grounding(&ungrounded));
    assert!(should_retry_grounding(&malformed_json));
    assert!(should_retry_grounding(&empty));
    assert!(!should_retry_grounding(&low_confidence));
    let model_error = ExtractionAttempt {
        finding: None,
        relevance: "unknown".into(),
        status: "model_error",
        reason: "model unavailable".into(),
        model_output: None,
    };
    assert!(!should_retry_grounding(&model_error));
}

#[test]
fn serpapi_rich_result_groups_preserve_type_and_structured_context() {
    let response: SerpApiResponse = serde_json::from_str(
        r#"{
            "organic_results": [],
            "recipes_results": [{"title":"Chicken Soup","link":"https://www.example.test/chicken-soup","source":"Example Kitchen","rating":4.8,"reviews":123,"ingredients":["chicken","carrots","celery"],"total_time":"45 minutes"}],
            "shopping_results": [{"title":"Soup Pot","link":"https://shop.example.test/pot","price":"£20","rating":4.2}],
            "local_results": {"places":[{"title":"Soup Kitchen","website":"https://food.example.test","address":"Main Street"}]}
        }"#,
    )
    .unwrap();
    let recipes = linked_serpapi_results(
        "recipes_results",
        response.structured_results.get("recipes_results").unwrap(),
    );
    assert_eq!(recipes.len(), 1);
    assert_eq!(recipes[0].result_type, "recipes_results");
    assert_eq!(recipes[0].title, "Chicken Soup");
    assert!(recipes[0].snippet.contains("carrots"));
    assert!(recipes[0].structured_data_json.contains("45 minutes"));
    let products = linked_serpapi_results(
        "shopping_results",
        response.structured_results.get("shopping_results").unwrap(),
    );
    assert_eq!(products.len(), 1);
    assert_eq!(products[0].result_type, "shopping_results");
    assert!(products[0].structured_data_json.contains("£20"));
    let places = linked_serpapi_results(
        "local_results",
        response.structured_results.get("local_results").unwrap(),
    );
    assert_eq!(places.len(), 1);
    assert_eq!(places[0].title, "Soup Kitchen");
    assert_eq!(places[0].url, "https://food.example.test/");
}

#[test]
fn parses_supported_claim_or_no_result() {
    let parsed = parse_extraction(r#"{"claim":{"subject":"Mars","relation":"is a","object":"planet","evidence_text":"Mars is a planet","confidence":0.98}}"#).unwrap();
    assert_eq!(parsed.claim.unwrap().subject, "Mars");
    assert!(
        parse_extraction(r#"{"claim":null}"#)
            .unwrap()
            .claim
            .is_none()
    );
    let flattened = parse_extraction(
        r#"{"claim":"Gold","relation":"chemical symbol","object":"Au","evidence_text":"Gold is a chemical element; its chemical symbol is Au.","confidence":0.98,"question_relevance":"direct"}"#,
    )
    .unwrap()
    .claim
    .unwrap();
    assert_eq!(flattened.subject, "Gold");
    assert_eq!(flattened.relation, "chemical symbol");
    assert_eq!(flattened.object, "Au");
}

#[test]
fn retry_after_accepts_seconds_and_http_dates_without_shortening_server_backoff() {
    assert_eq!(retry_after_delay_ms("45", 1_000), Some(45_000));
    assert_eq!(
        retry_after_delay_ms("Thu, 01 Jan 1970 00:02:00 GMT", 0),
        Some(120_000)
    );
    assert_eq!(retry_after_delay_ms("not a date", 0), None);
    assert_eq!(retry_after_delay_ms("999999", 0), Some(999_999_000));
    assert_eq!(retry_after_delay_ms("-1", 0), None);
}

#[test]
fn unrelated_search_result_is_skipped() {
    let page = SearchPage {
        index: 2,
        pageid: 1,
        title: "Hellenic Police".into(),
        extract: Some("The Hellenic Police is Greece's national police service.".into()),
        fullurl: Some("https://en.wikipedia.org/wiki/Hellenic_Police".into()),
        revisions: None,
    };
    assert!(!relevant_page(
        "Who was the first editor of The Journal of Hellenic Studies?",
        &page
    ));
    assert!(relevant_page("What is the Hellenic Police?", &page));
}

#[test]
fn specific_direct_claim_is_grounded_by_an_exact_passage() {
    let excerpt = "Usually, the main ingredients of this chicken soup are:\n* Chicken broth\n* Chicken\n* Noodles\n* Carrots\n* Salt\n* Celery.";
    assert!(claim_is_grounded(
        "Chicken soup",
        "commonly includes",
        "chicken broth, chicken, noodles, carrots, salt, and celery",
        excerpt,
        excerpt,
    ));
}

#[test]
fn related_claim_can_be_grounded_without_being_direct() {
    let excerpt = "Chicken soup is often served as a comfort food during cold weather.";
    let finding = ScholarFinding {
        semantic_support: None,
        subject: "Chicken soup".into(),
        relation: "is served as".into(),
        object: "comfort food during cold weather".into(),
        evidence_text: excerpt.into(),
        confidence: 0.98,
        valid_from: None,
        valid_until: None,
        source_quality: "reference".into(),
        source_updated_at: None,
        proposition: Default::default(),
    };
    assert!(claim_is_grounded(
        &finding.subject,
        &finding.relation,
        &finding.object,
        &finding.evidence_text,
        excerpt,
    ));
    assert_eq!(
        calibrate_research_relevance(
            "What ingredients are commonly used in chicken soup?",
            &finding,
            "related"
        ),
        "related"
    );
}

#[test]
fn headings_and_generic_claims_are_not_treated_as_evidence() {
    let excerpt = "Section: Canned chicken soup\n\n#### Canned chicken soup";
    assert!(!claim_is_grounded(
        "Chicken soup",
        "has encyclopedia summary",
        "Section: Canned chicken soup",
        "Section: Canned chicken soup",
        excerpt,
    ));
}

#[test]
fn claim_evidence_must_be_an_exact_passage() {
    let excerpt = "Chicken soup commonly includes noodles and carrots.";
    assert!(!claim_is_grounded(
        "Chicken soup",
        "includes",
        "noodles and celery",
        "Chicken soup commonly includes noodles and celery.",
        excerpt,
    ));
}

#[test]
fn wikipedia_html_keeps_headings_and_lists_but_drops_citation_clutter() {
    let html = r##"
        <h2>Ingredients</h2>
        <span class="mw-editsection"><a href="#edit">edit</a></span>
        <p>Main ingredients include:<sup class="reference"><a href="#cite_note-1">[1]</a></sup></p>
        <ul><li>Chicken broth</li><li>Carrots</li><li>Celery</li></ul>
        <ol class="references"><li id="cite_note-1">External citation</li></ol>
    "##;
    let text = html_to_text(html).unwrap();
    assert!(text.contains("Ingredients"));
    assert!(text.contains("Main ingredients include:"));
    assert!(text.contains("Chicken broth"));
    assert!(text.contains("Carrots"));
    assert!(text.contains("Celery"));
    assert!(!text.contains("edit"));
    assert!(!text.contains("[1]"));
    assert!(!text.contains("cite_note"));
    assert!(!text.contains("External citation"));
}
