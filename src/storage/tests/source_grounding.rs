//! Storage regressions for source grounding.
use super::*;

#[test]
fn scholar_evidence_must_support_the_relation_and_object() {
    let claim = ScholarFinding {
        semantic_support: None,
        subject: "Hellenic Police".into(),
        relation: "first editor".into(),
        object: "Dimitrios Mallios".into(),
        evidence_text: "Dimitrios Mallios is Chief of the Hellenic Police.".into(),
        confidence: 0.99,
        valid_from: None,
        valid_until: None,
        source_quality: "reference".into(),
        source_updated_at: None,
        proposition: Default::default(),
    };
    assert!(!evidence_supports_claim(&claim));
}

#[test]
fn scholar_evidence_supports_each_item_in_a_cited_source_list() {
    let evidence_text = "Usually, the main ingredients of chicken soup are:\n* Chicken broth\n* Chicken\n* Noodles\n* Carrots";
    let claim = ScholarFinding {
        semantic_support: None,
        subject: "Chicken soup".into(),
        relation: "lists ingredients".into(),
        object: "Chicken broth; Chicken; Noodles; Carrots".into(),
        evidence_text: evidence_text.into(),
        confidence: 0.99,
        valid_from: None,
        valid_until: None,
        source_quality: "reference".into(),
        source_updated_at: None,
        proposition: Default::default(),
    };
    assert!(evidence_supports_claim(&claim));

    let unsupported_item = ScholarFinding {
        semantic_support: None,
        object: "Chicken broth; Chicken; Noodles; Peas".into(),
        ..claim
    };
    assert!(!evidence_supports_claim(&unsupported_item));
}

#[test]
fn site_footer_text_cannot_ground_a_crisp_flavour_finding() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research(
            "crisps-footer",
            1,
            "What flavours are offered by Smith's Crisps?",
        )
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    let flavour_claim = "Smith's Crisps - Potato crisps in ready salted, salt and vinegar or cheese and onion flavoured 5-pack multipacks.";
    let source_excerpt = format!(
        "{flavour_claim}\n\n{}",
        include_str!("../../../tests/fixtures/cofresh-footer.txt")
    );
    store
        .finish_scholar_job_with_profile_and_relevance(
            &job.id,
            "Smith's Crisps",
            "https://www.cbinsights.com/company/cofresh",
            &source_excerpt,
            Some(ScholarFinding {
                semantic_support: None,
                subject: "Smith's Crisps".into(),
                relation: "is a".into(),
                object: "potato crisps in ready salted, salt and vinegar or cheese and onion flavoured 5-pack multipacks".into(),
                evidence_text: flavour_claim.into(),
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
    let assessment = store
        .scholar_source_assessment(finding.id)
        .unwrap()
        .unwrap();
    assert!(!assessment.direct_support.unwrap_or(false));
    assert_eq!(assessment.gate_status, "unsupported");
    assert!(store.active_semantic_claims().unwrap().is_empty());
    assert!(
        store
            .relevant_scholar_evidence("Smith's Crisps flavours", 5)
            .unwrap()
            .is_empty()
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn site_footer_text_cannot_ground_a_scientist_claim() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research(
            "scientist-footer",
            1,
            "Which scientist led the Kepler-186 survey?",
        )
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    let evidence = "Dr. Amina Patel led the Kepler-186 survey in 2024.";
    let source_excerpt = format!(
        "{evidence}\n\n{}",
        include_str!("../../../tests/fixtures/cofresh-footer.txt")
    );
    store
        .finish_scholar_job_with_profile_and_relevance(
            &job.id,
            "Kepler-186 research",
            "https://www.cbinsights.com/company/astro-research",
            &source_excerpt,
            Some(ScholarFinding {
                semantic_support: None,
                subject: "Dr. Amina Patel".into(),
                relation: "led".into(),
                object: "the Kepler-186 survey in 2024".into(),
                evidence_text: evidence.into(),
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
    let assessment = store
        .scholar_source_assessment(finding.id)
        .unwrap()
        .unwrap();
    assert!(!assessment.direct_support.unwrap_or(false));
    assert_eq!(assessment.gate_status, "unsupported");
    assert!(store.active_semantic_claims().unwrap().is_empty());
    assert!(
        store
            .relevant_scholar_evidence("Kepler-186 scientist", 5)
            .unwrap()
            .is_empty()
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn scholar_finding_links_to_a_versioned_source_and_exact_unicode_byte_span() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("exact-source-link", 1, "What is the Sun?")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    let quote = "The Sun is a star.";
    let source_excerpt = format!("🌱🌱 {quote}");
    store
        .finish_scholar_job_with_profile_and_relevance(
            &job.id,
            "Sun",
            "https://example.org/sun",
            &source_excerpt,
            Some(ScholarFinding {
                semantic_support: None,
                subject: "Sun".into(),
                relation: "is a".into(),
                object: "star".into(),
                evidence_text: quote.into(),
                confidence: 0.9,
                valid_from: None,
                valid_until: None,
                source_quality: "unknown".into(),
                source_updated_at: Some("2026-10-01T00:00:00Z".into()),
                proposition: Default::default(),
            }),
            None,
            &ScholarSourceProfile::default(),
            "direct",
        )
        .unwrap();
    drop(store);
    let store = LearningStore::open(&path).unwrap();
    let finding = store.scholar_findings(&job.id).unwrap().remove(0);
    let source_id = finding.source_archive_id.unwrap();
    let start = finding.evidence_byte_start.unwrap() as usize;
    let end = finding.evidence_byte_end.unwrap() as usize;
    let source = store.retained_source_detail(source_id).unwrap();
    assert_eq!(source["published_at"], "2026-10-01T00:00:00Z");
    assert_eq!(
        std::str::from_utf8(&source["body"].as_str().unwrap().as_bytes()[start..end]).unwrap(),
        quote
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn source_specific_list_is_directly_supported_but_held_for_review() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("list-evidence", 1, "What are ingredients in chicken soup?")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    let passage =
        "## Simple Chicken Soup Ingredients\n* Chicken breasts\n* Chicken stock\n* Carrots";
    store
        .finish_scholar_job_with_evidence(
            &job.id,
            &[ScholarEvidence {
                source_title: "Simple Chicken Soup".into(),
                source_url: "https://example.org/chicken-soup".into(),
                source_excerpt: passage.into(),
                finding: Some(ScholarFinding {
                    semantic_support: None,
                    subject: "Simple Chicken Soup".into(),
                    relation: "lists ingredients".into(),
                    object: "Chicken breasts; Chicken stock; Carrots".into(),
                    evidence_text: passage.into(),
                    confidence: 0.8,
                    valid_from: None,
                    valid_until: None,
                    source_quality: "unknown".into(),
                    source_updated_at: None,
                    proposition: Default::default(),
                }),
                profile: ScholarSourceProfile::default(),
                research_relevance: "related".into(),
            }],
            None,
        )
        .unwrap();
    let finding = store.scholar_findings(&job.id).unwrap().remove(0);
    let assessment = store
        .scholar_source_assessment(finding.id)
        .unwrap()
        .unwrap();
    assert_eq!(assessment.direct_support, Some(true));
    assert_eq!(assessment.topic_class, "historical_scientific");
    assert_eq!(assessment.gate_status, "pending_review");
    assert!(store.active_semantic_claims().unwrap().is_empty());
    drop(store);
    let _ = std::fs::remove_file(path);
}
