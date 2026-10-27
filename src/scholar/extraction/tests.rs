use super::*;

#[test]
fn parsing_preserves_claim_meaning_for_semantic_review() {
    for reference in ["by the source", "according to this article", "by a passage"] {
        let extraction = parse_extraction(
            &serde_json::json!({"claim":{"subject":"Exact dates","relation":"cannot be accurately determined","object":reference,"evidence_text":"Exact dates cannot be accurately determined.","confidence":0.98,"question_relevance":"direct"}}).to_string(),
        )
        .unwrap();
        let finding = extraction.claim.unwrap();
        assert_eq!(
            finding.relation, "cannot be accurately determined",
            "{reference}"
        );
        assert_eq!(finding.object, reference, "{reference}");
    }
}

#[test]
fn unsupported_source_reference_requires_a_reviewed_repair() {
    let passage = "Exact dates cannot be accurately determined.";
    let mut calls = 0;
    let attempt = extract_scholar_claim_with(
        "Can the dates be determined?",
        "Dating report",
        passage,
        "reference",
        None,
        None,
        |messages| {
            calls += 1;
            Ok(match calls {
                1 => serde_json::json!({"claim":{"subject":"Exact dates","relation":"cannot be accurately determined","object":"by the source","evidence_text":passage,"confidence":0.98,"question_relevance":"direct"}}).to_string(),
                2 | 3 => serde_json::json!({"verdict":"insufficient","reason":"The claimed actor is absent from the passage.","evidence_text":null}).to_string(),
                4 => {
                    assert!(messages[0].content.contains("bounded repair attempt"));
                    serde_json::json!({"claim":{"subject":"Exact dates","relation":"cannot be","object":"accurately determined","evidence_text":passage,"confidence":0.98,"question_relevance":"direct"}}).to_string()
                }
                5 | 6 => serde_json::json!({"verdict":"entailed","reason":"The source states this uncertainty directly.","evidence_text":passage}).to_string(),
                7 => serde_json::json!({"matches_property":true,"reason":"The claim directly states whether exact dates can be determined."}).to_string(),
                _ => panic!("repair must remain bounded"),
            })
        },
    );
    assert_eq!(attempt.status, "accepted", "{}", attempt.reason);
    assert_eq!(calls, 7);
    let finding = attempt.finding.unwrap();
    assert_eq!(finding.relation, "cannot be");
    assert_eq!(finding.object, "accurately determined");
    assert!(finding.semantic_support.is_some());
}

#[test]
fn event_years_and_unquoted_scopes_do_not_become_claim_metadata() {
    let passage = "In 2012, Riverton Museum displayed an astrolabe.";
    let attempt = evaluate_extraction(
        &serde_json::json!({"claim":{"subject":"Riverton Museum","relation":"displayed","object":"an astrolabe","evidence_text":passage,"confidence":0.98,"question_relevance":"direct","valid_from":"2012","context_scope":"current collection, if established"}}).to_string(),
        passage,
        "reference",
        None,
    );
    assert_eq!(attempt.status, "accepted", "{}", attempt.reason);
    let finding = attempt.finding.unwrap();
    assert_eq!(finding.valid_from, None);
    assert_eq!(finding.proposition.context_scope, None);
    assert!(finding.evidence_text.starts_with("In 2012"));
}

#[test]
fn dated_measurements_keep_the_event_date_and_value_in_the_claim() {
    let passage = "The 2010 census recorded Willow Bay's population as 1,200.";
    let raw = serde_json::json!({"claim":{
        "subject":"Willow Bay",
        "relation":"had a census-recorded population in 2010 of",
        "object":"1,200",
        "evidence_text":passage,
        "confidence":0.98,
        "question_relevance":"direct",
        "valid_from":"2010",
        "valid_until":null
    }})
    .to_string();
    let mut reviews = 0;
    let attempt = evaluate_extraction_semantically(
        &raw,
        "What population did Willow Bay have in 2010 and in 2020?",
        "Willow Bay census",
        passage,
        "reference",
        None,
        |_| {
            reviews += 1;
            Ok(serde_json::json!({
                "verdict":"entailed",
                "reason":"The quote reports this census value for the named year.",
                "evidence_text":passage
            })
            .to_string())
        },
    );
    assert_eq!(attempt.status, "accepted", "{}", attempt.reason);
    let finding = attempt.finding.unwrap();
    assert!(finding.relation.contains("2010"));
    assert_eq!(finding.object, "1,200");
    assert_eq!(finding.valid_from, None);
    assert_eq!(reviews, 2);
}

#[test]
fn property_reconsideration_recovers_a_specific_part_of_a_multi_part_question() {
    let passage = "The 2010 census recorded Willow Bay's population as 1,200.";
    let attempt = evaluate_extraction(
        &serde_json::json!({"claim":{
            "subject":"Willow Bay",
            "relation":"had a census-recorded population in 2010 of",
            "object":"1,200",
            "evidence_text":passage,
            "confidence":0.98,
            "question_relevance":"direct"
        }})
        .to_string(),
        passage,
        "Willow Bay census",
        None,
    );
    let finding = attempt.finding.unwrap();
    let mut calls = 0;
    let mut ask = |messages: &[Message]| {
        calls += 1;
        Ok(match calls {
            1 => {
                assert!(
                    messages[0]
                        .content
                        .contains("one explicit component or time slice of a multi-part question")
                );
                serde_json::json!({
                    "matches_property":false,
                    "reason":"This claim only answers one of the requested years."
                })
                .to_string()
            }
            2 => {
                assert!(
                    messages[0]
                        .content
                        .contains("Independently reconsider a prior finding")
                );
                serde_json::json!({
                    "matches_property":true,
                    "request_component":"2010",
                    "reason":"This source directly answers the requested 2010 component."
                })
                .to_string()
            }
            _ => panic!("partial property reconsideration must be bounded"),
        })
    };
    let (matches, _) = property_repair::requested_property_match(
        "What population did Willow Bay have in 2010 and in 2020?",
        &finding,
        passage,
        &mut ask,
    )
    .unwrap();
    assert!(matches);
    assert_eq!(calls, 2);
}

#[test]
fn rejected_list_claim_recovers_through_a_bounded_semantic_repair() {
    let passage = "Birds recorded on Alder Island include puffins and Arctic terns.";
    let raw = serde_json::json!({"claim":{"subject":"puffins","relation":"include","object":"Arctic terns","evidence_text":passage,"confidence":0.98,"question_relevance":"direct"}}).to_string();
    let mut calls = 0;
    let recovered = extract_scholar_claim_with(
        "Which birds are recorded on Alder Island?",
        "Alder Island",
        passage,
        "reference",
        None,
        None,
        |messages| {
            calls += 1;
            Ok(match calls {
                1 => raw.clone(),
                2 => serde_json::json!({"verdict":"insufficient","reason":"The quote does not state a relation between list members.","evidence_text":null}).to_string(),
                3 => serde_json::json!({"verdict":"insufficient","reason":"The listed birds do not assert a relation between the two members.","evidence_text":null}).to_string(),
                4 => {
                    assert!(messages[0].content.contains("bounded repair attempt"));
                    serde_json::json!({"claim":{"subject":"Birds recorded on Alder Island","relation":"include","object":"puffins and Arctic terns","evidence_text":passage,"confidence":0.98,"question_relevance":"direct"}}).to_string()
                }
                5 => serde_json::json!({"verdict":"entailed","reason":"The source names both birds as members of the recorded list.","evidence_text":passage}).to_string(),
                6 => serde_json::json!({"verdict":"entailed","reason":"The claim names the containing list and its members accurately.","evidence_text":passage}).to_string(),
                7 => serde_json::json!({"matches_property":true,"reason":"The claim gives the birds recorded on the island."}).to_string(),
                _ => panic!("list extraction repair must remain bounded"),
            })
        },
    );
    assert_eq!(recovered.status, "accepted", "{}", recovered.reason);
    assert_eq!(calls, 7);
    let finding = recovered.finding.unwrap();
    assert_eq!(finding.subject, "Birds recorded on Alder Island");
    assert_eq!(finding.object, "puffins and Arctic terns");
    assert!(finding.semantic_support.is_some());

    let unrelated = evaluate_extraction_semantically(
        &raw,
        "Which birds are recorded on Birch Island?",
        "Alder Island",
        passage,
        "reference",
        None,
        |_| {
            Ok(serde_json::json!({"verdict":"insufficient","reason":"The source does not name Birch Island.","evidence_text":null}).to_string())
        },
    );
    assert_eq!(unrelated.status, "ungrounded");
    assert!(unrelated.finding.is_none());
}

#[test]
fn semantic_rejection_is_not_overridden_by_exact_claim_wording() {
    let quote = "The exact date of discovery of most elements cannot be accurately determined.";
    let raw = serde_json::json!({"claim":{"subject":"The exact date of discovery of most elements","relation":"cannot be","object":"accurately determined","evidence_text":quote,"confidence":0.98,"question_relevance":"direct"}}).to_string();
    let result = evaluate_extraction_semantically(
        &raw,
        "What does the source say about whether exact discovery dates can be determined?",
        "Element discoveries",
        quote,
        "reference",
        None,
        |_| {
            Ok(serde_json::json!({"verdict":"insufficient","reason":"The source does not provide the specific dates.","evidence_text":null}).to_string())
        },
    );
    assert_eq!(result.status, "ungrounded", "{}", result.reason);
    assert!(result.finding.is_none());
}

#[test]
fn property_repair_ranks_the_span_with_request_specific_content_first() {
    let passage = "Discoveries are presented in chronological order.\nThe elements are listed generally in the order in which each was first defined as the pure element, as the exact date of discovery of most elements cannot be accurately determined.";
    let spans = property_repair::rank_property_spans(
        "What does the source say about whether exact element discovery dates can be determined?",
        "The discoveries of the 118 chemical elements",
        passage,
    );
    let spans: serde_json::Value = serde_json::from_str(&spans).unwrap();
    let first = spans[0]["property_clause"].as_str().unwrap();
    assert!(first.contains("exact date of discovery"));
    assert!(!first.contains("listed generally"));
    assert!(spans[0].get("sentence_context").is_none());
}

#[test]
fn semantic_review_expands_a_clause_quote_to_its_complete_sentence_only() {
    let passage = "Earlier fact. The elements are listed by order, as exact discovery dates cannot be determined. A separate fact follows.";
    let clause = "as exact discovery dates cannot be determined.";
    assert_eq!(
        property_repair::expand_quote_to_sentence(passage, clause),
        Some("The elements are listed by order, as exact discovery dates cannot be determined.")
    );
}

#[test]
fn knowability_question_cannot_be_answered_by_chronology_even_if_model_says_yes() {
    let passage = "Discoveries are presented here in chronological order.";
    let parsed = evaluate_extraction(
            &serde_json::json!({"claim":{"subject":"Discoveries","relation":"are presented in","object":"chronological order","evidence_text":passage,"confidence":0.98,"question_relevance":"direct"}}).to_string(),
            passage,
            "reference",
            None,
        );
    let finding = parsed.finding.unwrap();
    let mut ask = |_: &[Message]| {
        Ok(
            serde_json::json!({"matches_property":true,"reason":"The subject is the same."})
                .to_string(),
        )
    };
    let result = property_repair::requested_property_match(
        "Can exact discovery dates be determined, and if so, when?",
        &finding,
        passage,
        &mut ask,
    )
    .unwrap();
    assert!(!result.0);
    assert!(result.1.contains("epistemic status"));
}

#[test]
fn currency_repair_can_extract_a_historical_property_without_claiming_currentness() {
    let passage = "In 2012, Riverton Museum displayed an astrolabe.";
    let mut calls = 0;
    let result = extract_scholar_claim_with(
        "What instruments does Riverton Museum currently exhibit?",
        "Museum",
        passage,
        "reference",
        None,
        None,
        |messages| {
            calls += 1;
            if calls == 1 {
                assert!(
                    messages[0]
                        .content
                        .contains("never turn one member into the subject")
                );
                assert!(
                    messages[0]
                        .content
                        .contains("A page/article publication date")
                );
                assert!(
                    messages[0]
                        .content
                        .contains("An event, observation, example, or measurement date")
                );
            }
            Ok(match calls {
                    1 => serde_json::json!({"claim":null}).to_string(),
                2 | 3 => serde_json::json!({"requires_present_state":true,"requires_latest_knowledge":false,"uncertain":false,"reason":"The user asks about what is exhibited at present."}).to_string(),
                4 => serde_json::json!({"property_spans":["instruments", "exhibit"],"constraint_spans":[]}).to_string(),
                5 => {
                        assert!(!messages[1].content.contains("currently"));
                        assert!(messages[1].content.contains("Riverton Museum"));
                        serde_json::json!({"claim":{"subject":"Riverton Museum","relation":"displayed",
                            "object":"an astrolabe","evidence_text":passage,"confidence":0.98,"question_relevance":"direct"}}).to_string()
                    },
                6 | 7 => serde_json::json!({"verdict":"entailed","reason":"The source states the dated exhibit.","evidence_text":passage}).to_string(),
                8 => serde_json::json!({"matches_property":true,"reason":"The dated exhibit directly names an instrument."}).to_string(),
                    _ => panic!("repair must be bounded"),
                })
        },
    );
    assert_eq!(result.status, "accepted", "{}", result.reason);
    assert_eq!(calls, 8);
    let finding = result.finding.unwrap();
    assert_eq!(finding.evidence_text, passage);
    assert!(finding.valid_from.is_none());
    assert!(finding.valid_until.is_none());
}

#[test]
fn physical_current_does_not_trigger_temporal_property_repair() {
    let passage = "In 2012, Meridian Laboratory measured an electric current of two amperes.";
    let mut calls = 0;
    let result = extract_scholar_claim_with(
        "What current did Meridian Laboratory measure?",
        "Laboratory report",
        passage,
        "reference",
        None,
        None,
        |messages| {
            calls += 1;
            Ok(match calls {
                1 => serde_json::json!({"claim":null}).to_string(),
                2 | 3 => serde_json::json!({"requires_present_state":false,"requires_latest_knowledge":false,"uncertain":false,"reason":"The question asks for a physical measurement."}).to_string(),
                4 => {
                    assert!(messages[0].content.contains("bounded repair attempt"));
                    assert!(messages[1].content.contains("previous extraction failed validation"));
                    serde_json::json!({"claim":{"subject":"Meridian Laboratory","relation":"measured","object":"an electric current of two amperes","evidence_text":passage,"confidence":0.98,"question_relevance":"direct"}}).to_string()
                }
                5 => serde_json::json!({"verdict":"entailed","reason":"The quoted passage states the measured electrical quantity.","evidence_text":passage}).to_string(),
                6 => serde_json::json!({"verdict":"entailed","reason":"The source directly states the measured quantity.","evidence_text":passage}).to_string(),
                7 => serde_json::json!({"matches_property":true,"reason":"The claim states the measured electrical quantity."}).to_string(),
                _ => panic!("repair must be bounded"),
            })
        },
    );
    assert_eq!(calls, 7);
    assert_eq!(result.status, "accepted", "{}", result.reason);
}

#[test]
fn property_mismatch_repairs_from_a_ranked_span_of_the_same_multi_fact_source() {
    let passage = "The discoveries of the 118 chemical elements known to exist as of 2026 are presented here in chronological order.\nThe elements are listed generally in the order in which each was first defined as the pure element, as the exact date of discovery of most elements cannot be accurately determined.\nThere are plans to synthesize more elements, and it is not known how many elements are possible.";
    let chronology = "The discoveries of the 118 chemical elements known to exist as of 2026 are presented here in chronological order.";
    let date_limit = "The elements are listed generally in the order in which each was first defined as the pure element, as the exact date of discovery of most elements cannot be accurately determined.";
    let mut calls = 0;
    let result = extract_scholar_claim_with(
        "What does the source say about whether exact element discovery dates can be determined?",
        "Element discoveries",
        passage,
        "reference",
        None,
        None,
        |messages| {
            calls += 1;
            Ok(match calls {
                    1 => serde_json::json!({"claim":{
                        "subject":"The discoveries of the 118 chemical elements",
                        "relation":"are listed",
                        "object":"in chronological order",
                        "evidence_text":chronology,
                        "confidence":0.98,
                        "question_relevance":"direct"
                    }}).to_string(),
                    2 => serde_json::json!({
                        "verdict":"entailed",
                        "reason":"The claim matches the quoted source sentence.",
                        "evidence_text": chronology
                    }).to_string(),
                    3 => serde_json::json!({
                        "verdict":"entailed",
                        "reason":"The candidate faithfully represents the source passage.",
                        "evidence_text":chronology
                    }).to_string(),
                    4 | 5 => {
                        assert!(messages[0].content.contains(if calls == 4 { "strict question-to-claim entailment" } else { "Independently reconsider" }));
                        if calls == 4 { assert!(messages[0].content.contains("chronology/order does not answer whether something is knowable or determinable")); }
                        serde_json::json!({
                            "matches_property":false,
                            "reason":"Chronological ordering does not state whether exact discovery dates can be determined."
                        }).to_string()
                    }
                    6 => {
                        assert!(messages[0].content.contains("one bounded same-passage repair"));
                        assert!(messages[0].content.contains("identify the requested answer type"));
                        assert!(messages[1].content.contains("exact date of discovery of most elements cannot be accurately determined"));
                        assert!(!messages[1].content.contains("There are plans to synthesize"));
                        assert!(messages[1].content.contains("Requested property (preserve this exact request)"));
                        serde_json::json!({"claim":{
                            "subject":"The exact date of discovery of most elements",
                            "relation":"cannot be",
                            "object":"accurately determined",
                            "evidence_text":date_limit,
                            "confidence":0.98,
                            "question_relevance":"direct"
                        }}).to_string()
                    }
                    7 => serde_json::json!({
                        "verdict":"entailed",
                        "reason":"The repaired claim preserves the source's stated limitation.",
                        "evidence_text":date_limit
                    }).to_string(),
                    8 => serde_json::json!({
                        "verdict":"entailed",
                        "reason":"The repaired claim preserves the source's uncertainty about dates.",
                        "evidence_text":date_limit
                    }).to_string(),
                    9 => serde_json::json!({
                        "matches_property":true,
                        "reason":"The claim directly states the source's limit on determining exact discovery dates."
                    }).to_string(),
                    _ => panic!("property repair must have a bounded call count"),
                })
        },
    );
    assert_eq!(calls, 9);
    assert_eq!(result.status, "accepted", "{}", result.reason);
    let finding = result.finding.unwrap();
    assert_eq!(finding.evidence_text, date_limit);
    assert_eq!(finding.relation, "cannot be");
}

#[test]
fn rejected_property_repair_does_not_keep_the_original_irrelevant_fact() {
    let passage = "The discoveries of the 118 chemical elements known to exist as of 2026 are presented here in chronological order.\nThe exact date of discovery of most elements cannot be accurately determined.";
    let chronology = "The discoveries of the 118 chemical elements known to exist as of 2026 are presented here in chronological order.";
    let mut calls = 0;
    let result = extract_scholar_claim_with(
        "What does the source say about exact element discovery dates?",
        "Element discoveries",
        passage,
        "reference",
        None,
        None,
        |_| {
            calls += 1;
            Ok(match calls {
                1 => serde_json::json!({"claim":{
                    "subject":"The discoveries of the 118 chemical elements",
                    "relation":"are listed",
                    "object":"in chronological order",
                    "evidence_text":chronology,
                    "confidence":0.98,
                    "question_relevance":"direct"
                }})
                .to_string(),
                2 | 7 => serde_json::json!({
                    "verdict":"entailed",
                    "reason":"The claim matches the quoted source sentence.",
                    "evidence_text":chronology
                })
                .to_string(),
                3 | 8 => serde_json::json!({
                    "verdict":"entailed",
                    "reason":"The original chronology statement is accurately represented.",
                    "evidence_text":chronology
                })
                .to_string(),
                4 | 5 | 9 | 10 => serde_json::json!({
                    "matches_property":false,
                    "reason":"This sentence describes chronology, not discovery-date certainty."
                })
                .to_string(),
                6 => serde_json::json!({"claim":{
                    "subject":"The discoveries of the 118 chemical elements",
                    "relation":"are listed",
                    "object":"in chronological order",
                    "evidence_text":chronology,
                    "confidence":0.98,
                    "question_relevance":"direct"
                }})
                .to_string(),
                _ => panic!("property repair must have a bounded call count"),
            })
        },
    );
    assert_eq!(calls, 10);
    assert!(result.finding.is_none());
    assert_eq!(result.status, "ungrounded");
    assert!(
        result
            .reason
            .contains("requested-property repair was rejected")
    );
    assert!(
        result.model_output.is_some(),
        "retain the candidate for diagnosis"
    );
}

#[test]
fn malformed_property_review_removes_the_candidate_from_eligible_findings() {
    let passage = "The Trapezium Cluster is a group of stars in the Orion Nebula.";
    let mut calls = 0;
    let result = extract_scholar_claim_with(
        "What is the approximate age of the Trapezium stars?",
        "Trapezium",
        passage,
        "reference",
        None,
        None,
        |_| {
            calls += 1;
            Ok(match calls {
                1 => serde_json::json!({"claim":{
                    "subject":"Trapezium Cluster",
                    "relation":"is",
                    "object":"a group of stars in the Orion Nebula",
                    "evidence_text":passage,
                    "confidence":0.98,
                    "question_relevance":"direct"
                }})
                .to_string(),
                2 | 3 => serde_json::json!({
                    "verdict":"entailed",
                    "reason":"The passage states the claim.",
                    "evidence_text":passage
                })
                .to_string(),
                4 => "{\"matches_property\":".into(),
                _ => panic!("malformed property review must not trigger another extraction"),
            })
        },
    );
    assert_eq!(calls, 4);
    assert!(result.finding.is_none());
    assert_eq!(result.status, "ungrounded");
    assert!(result.reason.contains("requested-property review failed"));
    assert!(
        result.model_output.is_some(),
        "retain the candidate for diagnosis"
    );
}

#[test]
fn injected_malformed_extraction_reaches_one_repair_then_abstains() {
    let mut calls = Vec::new();
    let result = extract_scholar_claim_with(
        "Which options?",
        "Source",
        "Options are listed.",
        "reference",
        None,
        None,
        |messages| {
            calls.push(messages[0].content.clone());
            Ok(if calls.len() == 1 {
                "{\"claim\":"
            } else {
                "{\"claim\":null}"
            }
            .into())
        },
    );
    assert_eq!(calls.len(), 2);
    assert!(calls[1].contains("one bounded repair attempt"));
    assert!(result.finding.is_none());
    assert_eq!(result.status, "no_claim");
}

#[test]
fn malformed_repair_cannot_trigger_a_third_extraction_attempt() {
    let mut calls = 0;
    let result = extract_scholar_claim_with(
        "Which options?",
        "Source",
        "Options are listed.",
        "reference",
        None,
        None,
        |_| {
            calls += 1;
            Ok("{\"claim\":".into())
        },
    );
    assert_eq!(calls, 2);
    assert!(result.finding.is_none());
    assert_ne!(result.status, "accepted");
}
