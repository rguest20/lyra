//! Persistence boundaries for qualified semantic support.
use super::*;

#[test]
fn a_reviewed_identity_scope_cannot_be_removed_before_storage() {
    let passage = "In the Earth-One universe, Clark Kent is the civilian identity of Superman.";
    let mut calls = 0;
    let mut alias = crate::entities::resolve_with("Clark Kent", "Identity", passage, |_| {
        calls += 1;
        Ok(if calls == 1 {
            serde_json::json!({"aliases":[{"name":"Clark Kent","alias":"Superman","evidence_text":passage,"identity_scope":"Earth-One universe"}]}).to_string()
        } else { serde_json::json!({"verdict":"entailed","reason":"Identity is stated within the named universe.","evidence_text":passage}).to_string() })
    }).unwrap().pop().unwrap();
    let scope = alias.identity_scope.take();
    let store = LearningStore::open(std::path::Path::new(":memory:")).unwrap();
    assert!(
        store
            .record_entity_alias(None, "https://example.org/identity", passage, &alias)
            .is_err()
    );
    alias.identity_scope = scope;
    store
        .record_entity_alias(None, "https://example.org/identity", passage, &alias)
        .unwrap();
    let aliases = store.entity_aliases().unwrap();
    assert!(!aliases.same("Clark Kent", "Superman"));
    assert!(aliases.same_in_scope("Clark Kent", "Superman", "Earth-One universe"));
}

#[test]
fn qualified_findings_survive_review_context_and_support_refresh() {
    let path = std::env::temp_dir().join(format!(
        "lyra-qualified-review-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let passage =
        "The astronomer said the crystal may not conduct electricity in Europe during the trial.";
    let proposition = PropositionMetadata {
        attribution: Some("The astronomer".into()),
        geographic_scope: Some("Europe".into()),
        context_scope: Some("the trial".into()),
        polarity: ClaimPolarity::Negated,
        modality: ClaimModality::Possible,
    };
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("scope-review", 1, "What was reported about the crystal?")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    store
        .finish_scholar_job(
            &job.id,
            "Crystal report",
            "https://example.org/crystal",
            passage,
            Some(ScholarFinding {
                semantic_support: None,
                subject: "the crystal".into(),
                relation: "may not conduct".into(),
                object: "electricity".into(),
                evidence_text: passage.into(),
                confidence: 0.98,
                valid_from: None,
                valid_until: None,
                source_quality: "reference".into(),
                source_updated_at: None,
                proposition: proposition.clone(),
            }),
            None,
        )
        .unwrap();
    let id = store.scholar_findings(&job.id).unwrap()[0].id;
    drop(store);
    let mut store = LearningStore::open(&path).unwrap();
    let context = store.scholar_review_context(id).unwrap().unwrap();
    assert_eq!(context["attribution"], "The astronomer");
    assert_eq!(context["geographic_scope"], "Europe");
    assert_eq!(context["context_scope"], "the trial");
    assert_eq!(context["polarity"], "negated");
    assert_eq!(context["modality"], "possible");
    let finding = crate::semantic::candidate_from_context(&context).unwrap();
    assert_eq!(finding.proposition, proposition);
    let support = crate::semantic::assess_with("What was reported about the crystal?", "Crystal report", passage, &finding,
        |_| Ok(serde_json::json!({"verdict":"entailed","reason":"The source preserves uncertainty and attribution.","evidence_text":passage}).to_string())).unwrap();
    assert_eq!(support.verdict(), "entailed");
    let mut legacy = serde_json::to_value(&support).unwrap();
    for field in ["attribution", "polarity", "modality"] {
        legacy.as_object_mut().unwrap().remove(field);
    }
    let legacy: crate::semantic::SemanticSupport = serde_json::from_value(legacy).unwrap();
    assert!(!legacy.matches_claim(&finding));
    assert!(store.record_scholar_semantic_support(id, &legacy).is_err());
    store.record_scholar_semantic_support(id, &support).unwrap();
    assert_eq!(
        store.scholar_review_context(id).unwrap().unwrap()["semantic_support"]["modality"],
        "possible"
    );
    store
        .connection
        .execute(
            "UPDATE scholar_findings SET polarity='asserted' WHERE id=?1",
            [id],
        )
        .unwrap();
    assert!(store.record_scholar_semantic_support(id, &support).is_err());
    assert!(store.active_semantic_claims().unwrap().is_empty());
    drop(store);
    let _ = std::fs::remove_file(path);
}
