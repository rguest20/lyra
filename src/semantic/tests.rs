use super::*;

#[test]
fn approval_quotations_must_preserve_explicit_qualifications() {
    let passage = "In the Earth-One universe, Clark Kent is the civilian identity of Superman.";
    let finding = candidate_from_context(&serde_json::json!({"subject":"Clark Kent",
        "relation":"is the civilian identity of","object":"Superman","context_scope":"Earth-One universe"})).unwrap();
    let incomplete = "Clark Kent is the civilian identity of Superman.";
    let output = serde_json::json!({"verdict":"entailed","reason":"The names refer to the same identity.","evidence_text":incomplete});
    assert!(parse(&output.to_string(), &finding, passage).is_err());
    let output = serde_json::json!({"verdict":"entailed","reason":"The identity is scoped to the stated universe.","evidence_text":passage});
    assert!(parse(&output.to_string(), &finding, passage).is_ok());
}

#[test]
fn evidence_entailment_is_not_primed_by_answer_requirements() {
    let passage = "In 2018, Walkers sold cheese and onion crisps.";
    let finding =
        candidate_from_context(&serde_json::json!({"subject":"Walkers","relation":"sold",
        "object":"cheese and onion crisps","evidence_text":passage}))
        .unwrap();
    let mut packets = Vec::new();
    for question in [
        "What did Walkers sell in 2018?",
        "What is the complete current Walkers range?",
    ] {
        let support = assess_with(question, "Source", passage, &finding, |messages| {
            packets.push(messages[1].content.clone());
            Ok(serde_json::json!({"verdict":"entailed","reason":"The candidate is the dated source assertion.","evidence_text":passage}).to_string())
        }).unwrap();
        assert_eq!(support.verdict(), "entailed");
    }
    assert_eq!(
        packets[0], packets[1],
        "Answer sufficiency must not change the source/claim entailment task"
    );
}

fn candidate() -> ScholarFinding {
    candidate_from_context(&serde_json::json!({"subject":"The world","relation":"is","object":"round","evidence_text":"The world is round."})).unwrap()
}

#[test]
fn semantic_support_binds_every_proposition_qualification() {
    let finding = candidate();
    let passage = "The world is spherical.";
    let support = parse(&serde_json::json!({"verdict":"entailed","reason":"The source states the shape.","evidence_text":passage}).to_string(), &finding, passage).unwrap();
    assert!(support.matches_claim(&finding));
    let mut altered = finding.clone();
    altered.proposition.attribution = Some("Another speaker".into());
    assert!(!support.matches_claim(&altered));
    let mut altered = finding.clone();
    altered.proposition.polarity = crate::storage::ClaimPolarity::Negated;
    assert!(!support.matches_claim(&altered));
    let mut altered = finding.clone();
    altered.proposition.modality = crate::storage::ClaimModality::Possible;
    assert!(!support.matches_claim(&altered));
}

#[test]
fn assessment_allows_paraphrase_and_is_bound_to_claim_quote_and_dates() {
    let mut finding = candidate();
    let passage = "The world is spherical.";
    let support = assess_with("What shape is the world?", "World", passage, &finding, |messages| {
        assert!(messages[0].content.contains("topic similarity"));
        assert!(messages[0].content.contains("A property of a part or member does not automatically apply to the whole"));
        assert!(messages[0].content.contains("candidate's actual subject/relation/object is entailed by candidate_evidence_to_check_first"));
        assert!(messages[1].content.contains("candidate_evidence_to_check_first"));
        assert!(!messages[1].content.contains("source_passage"));
        Ok(serde_json::json!({"verdict":"entailed","reason":"Spherical entails round in this shape context.","evidence_text":passage}).to_string())
    }).unwrap();
    finding.evidence_text = passage.into();
    assert!(support.supports(&finding, passage));
    finding.object = "flat".into();
    assert!(!support.supports(&finding, passage));
    finding.object = "round".into();
    finding.valid_from = Some("2026-10-05".into());
    assert!(!support.supports(&finding, passage));
    finding.valid_from = None;
    finding.evidence_text = "The world is spherical".into();
    assert!(!support.supports(&finding, passage));
    assert!(!support.supports(&candidate(), "An unrelated passage."));
}

#[test]
fn semantic_verdict_never_rescues_a_missing_or_invented_source_quote() {
    let finding = candidate();
    for raw in [
        r#"{"verdict":"entailed","reason":"Same meaning.","evidence_text":null}"#,
        r#"{"verdict":"entailed","reason":"Same meaning.","evidence_text":"The world is round."}"#,
        r#"{"verdict":"similar","reason":"Same topic.","evidence_text":"The world is spherical."}"#,
        r#"{"verdict":"entailed","reason":"","evidence_text":"The world is spherical."}"#,
    ] {
        assert!(parse(raw, &finding, "The world is spherical.").is_err());
    }
}

#[test]
fn long_audit_explanation_is_bounded_without_losing_the_verdict() {
    let finding = candidate();
    let support = parse(&serde_json::json!({"verdict":"entailed","reason":"A".repeat(700),"evidence_text":"The world is spherical."}).to_string(),&finding,"The world is spherical.").unwrap();
    assert_eq!(support.verdict(), "entailed");
    assert!(support.reason().chars().count() <= 500);
    assert!(support.reason().ends_with("[truncated]"));
}

#[test]
fn qualified_contradicted_and_insufficient_are_not_support() {
    for verdict in ["qualified", "contradicted", "insufficient"] {
        let mut finding = candidate();
        let support = parse(&serde_json::json!({"verdict":verdict,"reason":"The claim is stronger than the source.","evidence_text":null}).to_string(),&finding,"The world is not exactly spherical.").unwrap();
        finding.evidence_text = support.evidence().into();
        assert!(!support.supports(&finding, "The world is not exactly spherical."));
    }
}
