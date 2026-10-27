use super::*;

#[test]
fn a_claim_subject_cannot_be_relabelled_as_its_reporting_source() {
    let evidence = vec![
        json!({"source":"https://example.org/biography","passage":"Alexander Fleming discovered penicillin in 1928."}),
    ];
    let mut calls = 0;
    let answer = ground_research_reply("Who discovered penicillin?",
        "Alexander Fleming discovered penicillin in 1928. This fact is attributed to the source Alexander Fleming as stated in the passage.",
        &evidence, |prompt| { calls += 1;
            if prompt.contains("Check the ANSWER's factual assertions") {
                Ok(if calls <= 2 {
                    json!({"verdict":"revise","supported":false,"addresses_request":true,"reason":"The discoverer is not identified as the reporting source."}).to_string()
                } else {
                    json!({"verdict":"accept","supported":true,"addresses_request":true,"reason":"The source names the discoverer without making him its publisher."}).to_string()
                })
            } else { Ok("According to the source, Alexander Fleming discovered penicillin in 1928.".into()) }
        }).unwrap();
    assert_eq!(calls, 4);
    assert!(!answer.contains("source Alexander Fleming"));
    assert!(answer.contains("discovered penicillin"));
}

#[test]
fn source_references_and_evidenced_speakers_are_reviewed_without_phrase_guards() {
    for (passage, draft) in [
        (
            "The laboratory measured two amperes.",
            "Source 1 reports that the laboratory measured two amperes. source_1",
        ),
        (
            "Ada Voss said the observatory opened in 1974.",
            "According to Ada Voss, the observatory opened in 1974. source_1",
        ),
        (
            "The archive records an opening date of 1974.",
            "The archive's account gives 1974 as the opening date. source_1",
        ),
    ] {
        let evidence = vec![single_source_evidence(
            "https://example.org/report",
            passage,
        )];
        let mut calls = 0;
        let result =
            ground_research_reply("What does the source report?", draft, &evidence, |prompt| {
                calls += 1;
                assert!(prompt.contains("Check every named attribution"));
                Ok(
                    json!({"verdict":"accept","supported":true,"addresses_request":true,
                "reason":"The source attribution is appropriate to the supplied passage."})
                    .to_string(),
                )
            })
            .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(
            result,
            resolve_source_labels(draft, &["https://example.org/report"])
        );
    }
}

#[test]
fn conditional_sources_cannot_become_an_outcome_or_a_denial() {
    let passage = "The Orion detector might detect a new particle if the project receives funding.";
    let evidence = vec![
        json!({"source":"https://example.org/proposal","passage":passage,
        "evidence_state":{"modality":"conditional","gate_status":"pending_review"}}),
    ];
    for draft in [
        "The Orion detector observed a new particle.",
        "The Orion detector might detect a particle, but it has not observed one.",
        "No evidence of an observation exists.",
    ] {
        let answer = ground_research_reply("Did the Orion detector observe a new particle?", draft, &evidence,
            |_| Ok(json!({"verdict":"accept","supported":true,"addresses_request":true,"reason":"The source mentions detection."}).to_string())).unwrap();
        assert!(answer.contains(passage));
        assert!(!answer.contains("has not observed"));
        assert!(!answer.contains("No evidence"));
    }
}

#[test]
fn independent_source_clauses_cannot_become_a_causal_explanation() {
    let passage = "Meridian Laboratory did not demonstrate superconductivity; its experiment measured magnetic susceptibility.";
    let evidence = vec![single_source_evidence(
        "https://example.org/report",
        passage,
    )];
    let draft = "Meridian Laboratory did not demonstrate superconductivity because its experiment measured magnetic susceptibility instead.";
    let mut calls = 0;
    let answer = ground_research_reply("Did Meridian Laboratory demonstrate superconductivity?", draft, &evidence,
        |prompt| { calls += 1;
            if prompt.contains("tentative answer interpretation") {
                Ok(r#"{"verdict":"unqualified","reason":"The answer presents the causal link as fact."}"#.into())
            } else if prompt.contains("Check the ANSWER's factual assertions") {
                Ok(json!({"verdict":"accept","supported":true,"addresses_request":true,"reason":"Both observations occur in the source."}).to_string())
            } else { Ok(passage.into()) }
        }).unwrap();
    assert_eq!(calls, 4);
    assert!(!answer.contains("because"));
    assert!(answer.contains("did not demonstrate"));
}

#[test]
fn an_unseen_location_cannot_be_added_despite_positive_review() {
    let evidence = vec![
        json!({"source":"https://example.org/founder", "passage":"Ada Voss founded Meridian Observatory in 1974."}),
    ];
    let mut calls = 0;
    let answer = ground_research_reply("Who founded Meridian Observatory?",
        "Ada Voss founded Meridian Observatory in 1974. Meridian Observatory is located in California.", &evidence,
        |prompt| { calls += 1;
            if prompt.contains("tentative answer interpretation") {
                Ok(r#"{"verdict":"unqualified","reason":"The location is not supported."}"#.into())
            } else if prompt.contains("Check the ANSWER's factual assertions") {
                Ok(json!({"verdict":"accept","supported":true,"addresses_request":true,"reason":"The source names the founder."}).to_string())
            } else { Ok("According to the source, Ada Voss founded Meridian Observatory in 1974.".into()) }
        }).unwrap();
    assert_eq!(calls, 4);
    assert!(!answer.contains("California"));
    assert!(answer.contains("Ada Voss"));
}

#[test]
fn source_labels_resolve_case_variants_without_leaking_internal_identifiers() {
    let evidence = vec![
        json!({"source":"https://example.org/report", "passage":"Solara keeps vaccines cold."}),
    ];
    let answer = ground_research_reply("What does Solara do?",
        "According to Source_1, Solara keeps vaccines cold.", &evidence,
        |_| Ok(json!({"verdict":"accept","supported":true,"addresses_request":true,"reason":"The passage states this function."}).to_string())).unwrap();
    assert!(!answer.contains("Source_1"));
    assert!(answer.contains("https://example.org/report"));
}

#[test]
fn repeated_structural_failure_gets_one_final_repair_to_preserve_source_wording() {
    let passage = "Mira Vale wrote Aster and Ash and Sea and Silence.";
    let evidence = vec![single_source_evidence("https://example.org/works", passage)];
    let split = "Mira Vale wrote Aster, Ash, Sea, and Silence.";
    let corrected = "The source says, ‘Mira Vale wrote Aster and Ash and Sea and Silence.’ It does not show where one title ends and another begins.";
    let mut calls = 0;
    let answer = ground_research_reply_with_scope(
        "What books did Mira Vale write?",
        split,
        &evidence,
        None,
        |prompt, is_answer| {
            calls += 1;
            if prompt.contains("tentative answer interpretation") {
                return Ok(r#"{"verdict":"unqualified","reason":"The answer does not state uncertainty at the disputed title boundary.","uncertainty_quote":"","gap_quote":"","basis_quote":"Mira Vale wrote Aster and Ash and Sea and Silence"}"#.into());
            }
            if is_answer {
                if calls == 3 {
                    return Ok(split.into());
                }
                assert_eq!(calls, 6);
                assert!(prompt.contains("first preserve its complete relevant wording verbatim"));
                assert!(prompt.contains("A prior rewrite changed source wording"));
                return Ok(corrected.into());
            }
            assert!(prompt.contains("Check the ANSWER's factual assertions"));
            Ok(json!({"verdict":"accept","supported":true,"addresses_request":true,
                "reason":"The answer quotes the source wording and identifies the unresolved title boundary."}).to_string())
        },
    )
    .unwrap();
    assert_eq!(calls, 7);
    assert!(answer.starts_with(corrected));
    assert!(answer.contains("https://example.org/works"));
}

#[test]
fn unresolved_structural_rewrites_require_review_of_the_source_literal_partial() {
    let passage = "Mira Vale wrote Aster and Ash and Sea and Silence.";
    let evidence = vec![json!({
        "source":"https://example.org/works",
        "passage":passage
    })];
    let split = "Mira Vale wrote Aster, Ash, Sea, and Silence.";
    let mut calls = 0;
    let answer = ground_research_reply_with_scope(
        "What books did Mira Vale write?",
        split,
        &evidence,
        None,
        |prompt, is_answer| {
            calls += 1;
            if prompt.contains("Assess whether this answer can be shown") {
                assert!(!prompt.contains("https://example.org/works"));
                return Ok(json!({"verdict":"useful","all_assertions_supported":true,
                    "reason":"The exact passage helps answer the request while preserving the unresolved boundaries.",
                    "contribution_quote":passage,
                    "uncertainty_quote":"I can't reliably separate these into distinct items.",
                    "basis_quote":passage}).to_string());
            }
            if is_answer {
                Ok(split.into())
            } else {
                Ok(
                    json!({"verdict":"revise","supported":false,"addresses_request":true,
                    "reason":"The answer invents boundaries between the joined terms."})
                    .to_string(),
                )
            }
        },
    )
    .unwrap();

    assert_eq!(calls, 7);
    assert!(answer.contains(passage));
    assert!(answer.contains("I can't reliably separate these into distinct items."));
    assert!(answer.contains("https://example.org/works"));
    assert!(!answer.contains("Aster, Ash, Sea, and Silence"));
}

#[test]
fn rejected_source_literal_partial_cannot_bypass_review_after_failed_repairs() {
    let passage = "Mira Vale wrote Aster and Ash and Sea and Silence.";
    let evidence = vec![single_source_evidence("https://example.org/works", passage)];
    let split = "Mira Vale wrote Aster, Ash, Sea, and Silence.";
    for response in [
        "not JSON",
        r#"{"verdict":"uncertain"}"#,
        r#"{"verdict":"not_useful","reason":"This does not answer the request."}"#,
    ] {
        let mut partial_reviews = 0;
        let answer = ground_research_reply_with_scope(
            "What books did Mira Vale write?",
            split,
            &evidence,
            None,
            |prompt, is_answer| {
                if prompt.contains("Assess whether this answer can be shown") {
                    partial_reviews += 1;
                    return Ok(response.into());
                }
                Ok(if is_answer {
                    split.into()
                } else {
                    json!({"verdict":"revise","supported":false,"addresses_request":true,
                        "reason":"Unsupported item boundaries."})
                    .to_string()
                })
            },
        )
        .unwrap();
        assert_eq!(partial_reviews, 1);
        assert_eq!(answer, missing_research_answer());
    }
}

#[test]
fn unsupported_shortened_name_is_repaired_through_semantic_review() {
    let passage = "Mira Vale wrote Aster and Ash and Sea and Silence.";
    let evidence = vec![single_source_evidence("https://example.org/works", passage)];
    let shortened = "Mira Vale wrote a single book titled Aster and Ash.";
    let corrected = format!(
        "The source says ‘{passage}’ I can't determine the distinct titles from that wording."
    );
    let mut calls = 0;
    let answer = ground_research_reply("What books did Mira Vale write?", shortened, &evidence,
        |prompt| {
            calls += 1;
            Ok(match calls {
                1 | 2 => json!({"verdict":"revise","supported":false,"addresses_request":true,
                    "reason":"The passage does not establish a single book with that shortened name."}).to_string(),
                3 => {
                    assert!(prompt.contains(shortened));
                    corrected.clone()
                },
                4 => json!({"verdict":"accept","supported":true,"addresses_request":true,
                    "reason":"The answer preserves the source wording and withholds a split."}).to_string(),
                _ => panic!("semantic repair must remain bounded"),
            })
        }).unwrap();
    assert_eq!(calls, 4);
    assert!(answer.starts_with(&corrected));
    assert!(!answer.contains("single book"));
}

#[test]
fn source_conjunctions_cannot_be_split_despite_positive_model_review() {
    for (question, passage, broken, repaired) in [
        (
            "What flavours are listed?",
            "The flavours include cheese and onion and salt and vinegar.",
            "The flavours include cheese, onion, and salt and vinegar.",
            "The source lists ‘cheese and onion and salt and vinegar’.",
        ),
        (
            "What books did Mira Vale write?",
            "Mira Vale wrote Aster and Ash and Sea and Silence.",
            "Mira Vale wrote Aster, Ash, Sea, and Silence.",
            "The source says Mira Vale wrote ‘Aster and Ash and Sea and Silence’.",
        ),
        (
            "Who founded the observatory?",
            "Dana Vale and Rowan Patel founded the observatory.",
            "The founders were Dana Vale, Rowan Patel.",
            "The source names Dana Vale and Rowan Patel as the founders.",
        ),
    ] {
        let evidence = vec![single_source_evidence(
            "https://example.org/reference",
            passage,
        )];
        let mut reviews = 0;
        let result = ground_research_reply(question, broken, &evidence, |prompt| {
            if prompt.contains("Check the ANSWER's factual assertions") {
                reviews += 1;
                Ok(json!({"verdict":"accept","supported":true,"addresses_request":true,"reason":"The source lists the items."}).to_string())
            } else {
                Ok(repaired.into())
            }
        }).unwrap();
        assert_eq!(reviews, 2, "{passage}");
        assert!(result.starts_with(repaired), "{result}");
    }
}

#[test]
fn provenance_limit_does_not_become_a_temporal_disclaimer() {
    let evidence = vec![json!({"source":"https://example.org/report",
        "passage":"Helios Laboratory completed a flight test in 2020, according to Sciencewire.",
        "evidence_state":{"gate_status":"unverified_temporal","independence_key":"wire-report",
            "independence_verified":true}})];
    let answer = ground_research_reply("Is the claim independently corroborated?", "Draft", &evidence,
        |_| Ok(r#"{"verdict":"accept","supported":true,"addresses_request":true,"reason":"Limited source assertion."}"#.into())).unwrap();
    assert!(answer.contains("does not establish independent corroboration"));
    assert!(!answer.contains("still current"));
}

#[test]
fn a_physics_current_is_not_a_currentness_request() {
    let evidence = vec![json!({"source":"https://example.org/reference",
        "passage":"Electric current is a flow of electric charge.",
        "evidence_state":{"gate_status":"unverified_temporal"}})];
    let answer = ground_research_reply_with_scope("What is electric current?", "The source defines electric current as a flow of electric charge.",
        &evidence, Some(false), |_, _| Ok(r#"{"verdict":"accept","supported":true,"addresses_request":true,"reason":"Source definition is attributed."}"#.into())).unwrap();
    assert!(!answer.contains("still current"));
    assert!(answer.contains("flow of electric charge"));
}

#[test]
fn temporal_gate_is_enforced_even_when_semantic_use_is_incorrectly_direct() {
    let evidence = vec![json!({"source":"https://example.org/catalogue",
        "passage":"The archive lists War and Peace and Northanger Abbey.",
        "evidence_state":{"gate_status":"unverified_temporal","research_relevance":"direct"}})];
    let answer = ground_research_reply_with_scope(
        "What titles are currently available?",
        "The titles are no longer available.",
        &evidence,
        Some(true),
        |_, _| panic!("temporal policy must survive a positive but incorrect model verdict"),
    )
    .unwrap();
    assert!(answer.contains("War and Peace and Northanger Abbey"));
    assert!(!answer.contains("no longer"));
    assert!(answer.contains("does not establish whether this remains current"));
    let conflicting = vec![
        json!({"source":"https://example.org/other","passage":"A different list.",
        "evidence_state":{"gate_status":"unverified_temporal","conflicting_claims":["a conflicting current list"]}}),
    ];
    assert!(source_limited_answer("What is currently available?", &conflicting).is_none());
}

#[test]
fn historical_only_evidence_cannot_become_a_present_day_exclusion() {
    let passage = "In 2012, Riverton Museum displayed an astrolabe and a reflecting telescope.";
    let evidence = vec![
        json!({"source":"https://example.org/archive","passage":passage,
        "evidence_state":{"research_relevance":"historical_context"}}),
    ];
    let answer = ground_research_reply_with_scope(
        "What does the museum currently exhibit?",
        "The current collection does not include an astrolabe or reflecting telescope.",
        &evidence,
        Some(true),
        |_, _| panic!("source-literal historical context requires no invented current assertion"),
    )
    .unwrap();
    assert!(answer.contains(passage));
    assert!(answer.contains("does not establish whether this remains current"));
    assert!(!answer.contains("does not include"));
    assert!(answer.contains("https://example.org/archive"));
    let mixed = vec![
        evidence[0].clone(),
        json!({"source":"https://example.org/current",
        "passage":"The museum currently exhibits a sundial.","evidence_state":{"research_relevance":"direct"}}),
    ];
    assert!(source_limited_answer("What is currently displayed?", &mixed).is_none());
}

#[test]
fn entity_anchor_guard_checks_late_names_and_ignores_url_paths() {
    let evidence = vec![json!({"source":"https://example.org/Helios-Institute",
        "passage":"Ada Voss founded Meridian Observatory. Helios Laboratory completed a flight test."})];
    let answer = "Ada Voss. Meridian Observatory. Ada Voss. Meridian Observatory. Helios Laboratory. Helios Institute shares its origin.";
    assert_eq!(
        unsupported_entity_name(answer, &evidence).as_deref(),
        Some("Helios Institute")
    );
    assert!(unsupported_entity_name("According to the source, Ada Voss founded Meridian Observatory. Helios Laboratory completed a test.", &evidence).is_none());
    let explicit_alias =
        vec![json!({"passage":"Helios Laboratory is also known as Helios Institute."})];
    assert!(
        unsupported_entity_name(
            "Helios Institute is the alternative name of Helios Laboratory.",
            &explicit_alias
        )
        .is_none()
    );
}

#[test]
fn historical_selection_limits_currentness_even_when_original_gate_is_eligible() {
    let view = model_answer_evidence(
        "What does the museum currently display?",
        &[
            json!({"passage":"In 2012, Riverton Museum displayed an astrolabe.",
        "evidence_state":{"finding_status":"promoted","gate_status":"eligible","currency":"current",
            "research_relevance":"historical_context"}}),
        ],
        None,
    );
    assert!(
        view[0]["source_guidance"]
            .to_string()
            .contains("does not establish present status")
    );
    assert!(view[0]["passage"].as_str().unwrap().contains("2012"));
}

#[test]
fn grounding_basics_rejects_invented_entity_extension_despite_positive_review() {
    let evidence = vec![json!({"source":"https://example.org/report",
        "passage":"Helios Laboratory completed its first flight test in 2020, according to Sciencewire."})];
    let mut calls = 0;
    let answer = ground_research_reply("Is the claim independently corroborated?",
        "Helios Laboratory and Helios Institute share a common origin. Independent corroboration is not established.",
        &evidence, |prompt| {
            calls += 1;
            if prompt.contains("tentative answer interpretation") {
                Ok(r#"{"verdict":"unqualified","reason":"The institute name is unsupported."}"#.into())
            } else if prompt.contains("Check the ANSWER's factual assertions") {
                Ok(r#"{"verdict":"accept","supported":true,"addresses_request":true,"reason":"Qualified provenance answer."}"#.into())
            } else {
                Ok("Sciencewire reports the Helios Laboratory flight test in 2020. Independent corroboration is not established. source_1".into())
            }
        }).unwrap();
    assert_eq!(calls, 4);
    assert!(!answer.contains("Helios Institute"));
    assert!(answer.contains("Helios Laboratory"));
}

#[test]
fn partial_answer_requests_are_interpreted_from_the_question_not_phrase_gates() {
    for (question, passage, partial_answer) in [
        (
            "Which book would you pick? If a ranking is unavailable, the titles would still help.",
            "The catalogue lists War and Peace, Sense and Sensibility, and Northanger Abbey.",
            "I can't rank them from these sources, but the catalogue lists War and Peace, Sense and Sensibility, and Northanger Abbey.",
        ),
        (
            "What's your favourite musical act? At least tell me who appears on the programme.",
            "The programme lists Simon and Garfunkel, Earth, Wind & Fire, and Queen.",
            "The programme names Simon and Garfunkel, Earth, Wind & Fire, and Queen; it doesn't establish a favourite.",
        ),
        (
            "Which destination is best? Even without deciding that, what places are named?",
            "The itinerary includes Trinidad and Tobago, Antigua and Barbuda, and Saint Lucia.",
            "The itinerary includes Trinidad and Tobago, Antigua and Barbuda, and Saint Lucia, but doesn't rank them.",
        ),
    ] {
        let evidence = vec![single_source_evidence(
            "https://example.org/reference",
            passage,
        )];
        let mut reviews = 0;
        let reply = ground_research_reply(question, "No supported ranking.", &evidence, |prompt| {
            if prompt.contains("Check the ANSWER's factual assertions") {
                reviews += 1;
                Ok(if reviews == 1 {
                    r#"{"verdict":"accept","supported":true,"addresses_request":false,"reason":"The useful partial answer is missing."}"#.into()
                } else {
                    r#"{"verdict":"accept","supported":true,"addresses_request":true,"reason":"It gives the source-listed options and leaves the ranking unresolved."}"#.into()
                })
            } else {
                assert!(prompt.contains(question));
                Ok(partial_answer.to_owned())
            }
        }).unwrap();
        assert!(reply.contains(partial_answer), "{question}: {reply}");
        assert!(reply.contains("https://example.org/reference"));
    }
}

#[test]
fn grounding_rewrites_metadata_even_when_fact_reviewer_accepts_the_draft() {
    let evidence = vec![json!({"source":"https://example.org/aurora",
        "passage":"Aurora Observatory opened in 2001.",
        "evidence_state":{"finding_status":"needs_review","gate_status":"awaiting_corroboration",
            "extraction_confidence":0.99}})];
    let mut calls = 0;
    let mut reviews = 0;
    let answer = ground_research_reply("When did Aurora Observatory open?",
        "Aurora Observatory opened in 2001, with extraction confidence of 0.99.", &evidence,
        |prompt| {
            calls += 1;
            if prompt.contains("Check the ANSWER's factual assertions") {
                reviews += 1;
                Ok(if reviews <= 2 {
                    r#"{"verdict":"revise","supported":false,"addresses_request":true,"reason":"Remove internal review metadata."}"#.into()
                } else {
                    r#"{"verdict":"accept","supported":true,"addresses_request":true,"reason":"The opening date is in the passage."}"#.into()
                })
            } else {
                // The repair generator must not receive raw scores or review enums either.
                let data = prompt.split("Data: ").last().unwrap();
                assert!(!data.contains("extraction_confidence"));
                assert!(!data.contains("awaiting_corroboration"));
                Ok("The source dates Aurora Observatory's opening to 2001. source_1".into())
            }
        }).unwrap();
    assert_eq!(calls, 4);
    assert!(answer.contains("2001"));
    assert!(!answer.contains("confidence"));
}

#[test]
fn reviewed_caveats_reach_answer_generation_and_are_kept_beside_the_claim() {
    let caveat = json!({
        "statement":"Operation outside Norway remains unknown.",
        "applies_to":"Finland",
        "basis":"question_scope",
        "evidence_text":null
    });
    let evidence = vec![json!({
        "source":"https://example.org/device",
        "passage":"The device is used in Norway.",
        "evidence_state":{
            "finding_status":"needs_review",
            "gate_status":"awaiting_corroboration",
            "review_caveats":[caveat]
        }
    })];
    let view = model_answer_evidence("Does the device work in Finland?", &evidence, None);
    assert_eq!(view[0]["review_caveats"][0]["applies_to"], "Finland");
    assert_eq!(
        view[0]["review_caveats"][0]["statement"],
        "Operation outside Norway remains unknown."
    );

    let mut calls = 0;
    let draft =
        "The source says the device is used in Norway. Its operation in Finland is unknown.";
    let answer = ground_research_reply(
        "Does the device work in Finland?",
        draft,
        &evidence,
        |prompt| {
            calls += 1;
            assert!(prompt.contains("Operation outside Norway remains unknown."));
            assert!(prompt.contains("Finland"));
            Ok(json!({"verdict":"accept","supported":true,"addresses_request":true,
                "reason":"The answer keeps the supported location and unresolved requested scope together."}).to_string())
        },
    )
    .unwrap();
    assert_eq!(calls, 1);
    assert!(answer.contains("used in Norway"));
    assert!(answer.contains("operation in Finland is unknown"));
}

#[test]
fn answer_generator_gets_qualification_codes_and_shared_lineage_without_internal_scores() {
    let evidence = vec![
        json!({"source":"https://one.example/story","passage":"Aurora Observatory opened in 2001.",
            "evidence_state":{"finding_status":"needs_review","gate_status":"unverified_temporal",
                "confidence":1.0,"extraction_confidence":1.0,"source_archive_id":99,
                "independence_key":"shared-wire","independence_verified":true}}),
        json!({"source":"https://two.example/story","passage":"Aurora Observatory opened in 2001.",
            "evidence_state":{"finding_status":"needs_review","gate_status":"awaiting_corroboration",
                "independence_key":"shared-wire","independence_verified":true}}),
        json!({"source":"https://three.example/story","passage":"Aurora Observatory opened in 2003.",
            "evidence_state":{"finding_status":"needs_review","gate_status":"conflict",
                "independence_key":"separate-wire","independence_verified":true,
                "conflicting_claims":["Aurora Observatory opened in 2001."]}}),
    ];
    let answer_view = model_answer_evidence("When did Aurora Observatory open?", &evidence, None);
    assert!(
        answer_view
            .iter()
            .all(|source| source.get("source_relationships").is_none())
    );
    let public = model_answer_evidence(
        "Are these independent sources for when Aurora Observatory opened?",
        &evidence,
        None,
    );
    let text = serde_json::to_string(&public).unwrap();
    for internal in [
        "extraction_confidence",
        "source_archive_id",
        "gate_status",
        "needs_review",
        "source_attribution_required",
        "current_status_not_established",
        "reports_conflict",
        "shared-wire",
        "https://",
    ] {
        assert!(
            !text.contains(internal),
            "generator should not see {internal}"
        );
    }
    assert_eq!(
        public[0]["source_relationships"]["same_origin_as"],
        json!(["source_2"])
    );
    assert_eq!(
        public[1]["source_relationships"]["same_origin_as"],
        json!(["source_1"])
    );
    assert_eq!(
        public[0]["source_relationships"]["distinct_verified_origins"],
        json!(["source_3"])
    );
    assert_eq!(
        public[2]["source_relationships"]["same_origin_as"],
        json!([])
    );
    assert!(!text.contains("publication_"));
    assert!(public[0].get("source_date").is_none());
    assert!(
        public[0]["source_guidance"]
            .to_string()
            .contains("does not establish present status")
    );
    assert!(
        public[2]["source_guidance"]
            .to_string()
            .contains("conflicting accounts")
    );
    assert!(!text.contains("Attribute this assertion to its source."));
    assert!(text.contains("Aurora Observatory opened in 2001."));
}

#[test]
fn lack_of_corroboration_is_a_supported_limitation_not_an_independence_claim() {
    let evidence = vec![json!({"source":"https://example.org/reference",
        "passage":"Cofresh makes Indian snack foods.",
        "evidence_state":{"independence_verified":false}})];
    for limitation in [
        "Independent corroboration is not yet established by the cited secondary source.",
        "One secondary source does not establish independent corroboration.",
        "This finding needs independent corroboration from other sources.",
        "This is provisional due to a lack of independent corroboration.",
    ] {
        let draft = format!("The source says Cofresh makes Indian snack foods. {limitation}");
        let mut calls = 0;
        let answer = ground_research_reply("What does Cofresh make?", &draft, &evidence, |_| {
            calls += 1;
            Ok(r#"{"verdict":"accept","supported":true,"addresses_request":true,"reason":"Source-attributed products and an explicit limitation."}"#.into())
        }).unwrap();
        assert_eq!(calls, 1, "{limitation}");
        assert!(answer.contains("Cofresh makes Indian snack foods"));
    }
}

#[test]
fn a_rejected_independence_claim_is_not_published_due_to_wording_variants() {
    let evidence = vec![json!({"source":"https://example.org/report",
        "passage":"The report describes the finding.",
        "evidence_state":{"independence_verified":false}})];
    for draft in [
        "The sources aren't independent; still, they independently corroborate the finding.",
        "There is only one origin, but this finding has independent corroboration.",
    ] {
        let answer = ground_research_reply("Is this independently corroborated?", draft, &evidence, |_| {
            Ok(r#"{"verdict":"revise","supported":false,"addresses_request":true,"reason":"The evidence does not establish independent sources."}"#.into())
        }).unwrap();
        assert_eq!(answer, missing_research_answer());
    }
}

#[test]
fn factual_support_alone_cannot_approve_an_answer_to_another_question() {
    let mut calls = 0;
    let result = ground_research_reply("Which crisps suit this use?", "Firm tofu is a soup ingredient.", &[], |_| {
        calls += 1;
        Ok(if calls == 2 { "Still discussing soup.".into() } else { r#"{"verdict":"accept","supported":true,"addresses_request":false,"reason":"Soup facts do not answer the crisp question"}"#.into() })
    }).unwrap();
    assert_eq!(result, missing_research_answer());
    assert_eq!(calls, 3);
}

#[test]
fn explicit_fallback_and_uncertain_preference_can_answer_a_best_question() {
    let evidence = vec![json!({
        "source":"https://example.org/walkers",
        "passage":"Walkers sells Cheese and Onion, Salt and Vinegar, and Ready Salted crisps.",
        "evidence_state":{"finding_status":"needs_review","gate_status":"awaiting_corroboration","independence_verified":false}
    })];
    let mut calls = 0;
    let answer = ground_research_reply(
        "What's the best crisp flavour? If the sources don't identify one, list the flavours.",
        "I'm not sure which is best, but my guess is Cheese and Onion. The source lists Cheese and Onion, Salt and Vinegar, and Ready Salted.",
        &evidence,
        |prompt| {
            calls += 1;
            match calls {
                1 => Ok(r#"{"verdict":"accept","supported":true,"addresses_request":false,"reason":"The source does not rank a best flavour."}"#.into()),
                2 => {
                    assert!(prompt.contains("What\'s the best crisp flavour?"));
                    Ok("The source does not rank the flavours. I'm not sure which is best, but my guess is Cheese and Onion. It lists Cheese and Onion, Salt and Vinegar, and Ready Salted. source_1".into())
                }
                3 => Ok(r#"{"verdict":"accept","supported":true,"addresses_request":true,"reason":"Follows the requested fallback and marks the guess uncertain."}"#.into()),
                _ => panic!("grounding review must stay bounded"),
            }
        },
    )
    .unwrap();

    assert_eq!(calls, 3);
    assert!(answer.contains("I'm not sure"));
    assert!(answer.contains("my guess is Cheese and Onion"));
    assert!(answer.contains("Salt and Vinegar, and Ready Salted"));
    assert!(answer.ends_with("https://example.org/walkers"));
}

#[test]
fn rejected_partial_answer_is_not_replaced_by_an_unreviewed_literal_dump() {
    let evidence = vec![json!({
        "source":"https://en.wikipedia.org/wiki/Walkers_(snack_foods)",
        "passage":"Walkers sells crisps in a variety of flavours; the main are Cheese and Onion, Salt and Vinegar, and Ready Salted.",
        "evidence_state":{"finding_status":"needs_review","gate_status":"awaiting_corroboration"}
    })];
    let answer = ground_research_reply(
        "What is the best Walkers crisp flavour? If you can't tell from the sources, list the flavours. I'm happy with a guess if you're unsure; say so.",
        "I can't tell which is best. The source names Cheese and Onion and Salt and Vinegar.",
        &evidence,
        |_| Ok(r#"{"verdict":"revise","supported":false,"addresses_request":false,"reason":"Does not determine a best flavour"}"#.into()),
    )
    .unwrap();

    assert_eq!(answer, missing_research_answer());
    assert!(!answer.contains("Cheese and Onion"));
}

#[test]
fn exhausted_conclusion_reviews_get_one_grounded_partial_answer_recovery() {
    let evidence = vec![json!({
        "source":"https://example.org/catalogue",
        "passage":"The 2024 catalogue lists the Orion, Vega, and Lyra models.",
        "evidence_state":{
            "finding_status":"needs_review",
            "gate_status":"unverified_temporal",
            "freshness":"stale",
            "research_relevance":"direct",
            "conflicting_claims":["A later catalogue may list a different range."]
        }
    })];
    let mut reviews = 0;
    let mut rewrites = 0;
    let answer = ground_research_reply_with_scope(
        "Which models are currently available?",
        "The Orion, Vega, and Lyra models are currently available.",
        &evidence,
        Some(true),
        |prompt, _| {
            if prompt.contains("You are a second semantic reviewer") {
                return Ok(r#"{"verdict":"accept","supported":true,"addresses_request":true,"reason":"The answer may be useful with a date qualification."}"#.into());
            }
            if prompt.contains("Check the ANSWER's factual assertions") {
                reviews += 1;
                if reviews == 3 {
                    assert!(prompt.contains("The 2024 catalogue lists"));
                    assert!(prompt.contains("does not establish"));
                    return Ok(r#"{"verdict":"accept","supported":true,"addresses_request":true,"reason":"It gives the dated source information and limits the currentness claim."}"#.into());
                }
                return Ok(if reviews == 2 {
                    r#"{"verdict":"revise","supported":true,"addresses_request":false,"reason":"The source cannot establish present availability, but its dated list is supported."}"#.into()
                } else {
                    r#"{"verdict":"revise","supported":false,"addresses_request":false,"reason":"The source cannot establish present availability."}"#.into()
                });
            }
            rewrites += 1;
            Ok(if rewrites < 2 {
                "I can't confirm which models are currently available."
            } else {
                "I can't confirm the current range, but the 2024 catalogue lists the Orion, Vega, and Lyra models; it doesn't establish whether they remain available. source_1"
            }.into())
        },
    )
    .unwrap();

    assert_eq!(reviews, 3);
    assert_eq!(rewrites, 2);
    assert!(answer.contains("2024 catalogue lists the Orion, Vega, and Lyra models"));
    assert!(answer.contains("doesn't establish whether they remain available"));
    assert!(answer.contains("https://example.org/catalogue"));
}

#[test]
fn evidence_based_best_guess_can_pass_when_uncertainty_names_its_gap() {
    let evidence = vec![json!({
        "source":"https://example.org/catalogue",
        "passage":"The 2024 catalogue lists the Orion, Vega, and Lyra models.",
        "evidence_state":{"gate_status":"unverified_temporal","freshness":"stale"}
    })];
    let draft = "I can't verify today's range, but my tentative guess is that Orion, Vega, and Lyra remain available because they appear in the 2024 catalogue. I don't have a current catalogue to confirm that.";
    let answer = ground_research_reply(
        "Which models are currently available?",
        draft,
        &evidence,
        |prompt| {
            assert!(prompt.contains("tentative guess"));
            assert!(prompt.contains("specific claim"));
            assert!(prompt.contains("gap that prevents verification"));
            Ok(r#"{"verdict":"accept","supported":true,"addresses_request":true,"reason":"The possible current availability is explicitly identified as a guess based on the dated catalogue, and the lack of current evidence is stated."}"#.into())
        },
    )
    .unwrap();

    assert!(answer.starts_with("I can't verify today's range"));
    assert!(answer.contains("tentative guess"));
    assert!(answer.contains("2024 catalogue"));
    assert!(answer.contains("https://example.org/catalogue"));
}

#[test]
fn partial_flavour_evidence_cannot_be_presented_as_a_complete_current_catalogue() {
    let evidence = vec![json!({
        "source":"https://example.org/walkers-history",
        "passage":"Walkers produced cheese and onion crisps in 1954 and later sold salt and vinegar crisps.",
        "evidence_state":{
            "finding_status":"needs_review",
            "gate_status":"awaiting_corroboration",
            "review_reason":"only one independent source is available",
            "research_relevance":"direct",
            "extraction_confidence":0.99,
            "freshness":"stale","topic_class":"recent",
            "conflicting_claims":["Walkers range includes paprika"],
            "independence_verified":false
        }
    })];
    let mut calls = 0;
    let answer = ground_research_reply(
        "What crisp flavours does Walkers currently sell?",
        "Walkers' complete current range is cheese and onion and salt and vinegar.",
        &evidence,
        |prompt| {
            calls += 1;
            if prompt.contains("Check the ANSWER's factual assertions") {
                assert!(prompt.contains("awaiting_corroboration"));
                assert!(prompt.contains("stale"));
                assert!(prompt.contains("conflicting_claims"));
            } else {
                let data = prompt.split("Data: ").last().unwrap();
                assert!(!data.contains("extraction_confidence"));
                assert!(!data.contains("awaiting_corroboration"));
                assert!(data.contains("does not establish present status"));
            }
            assert!(prompt.contains("Walkers range includes paprika"));
            match calls {
                1 | 2 => {
                    Ok(r#"{"verdict":"revise","supported":false,"addresses_request":false,"reason":"Historical examples do not establish the complete current range."}"#.into())
                }
                3 => Ok("The source mentions cheese and onion and salt and vinegar as historical examples, but it does not establish Walkers' complete current flavour range. An active claim also lists paprika, though its time scope is unclear. source_1".into()),
                4 => Ok(r#"{"verdict":"accept","supported":true,"addresses_request":true,"reason":"Gives historical examples, flags the unresolved scope, and avoids completeness."}"#.into()),
                _ => panic!("grounding review must stay bounded"),
            }
        },
    )
    .unwrap();

    assert_eq!(calls, 4);
    assert!(answer.contains("historical examples"));
    assert!(answer.contains("does not establish"));
    assert!(answer.contains("time scope is unclear"));
    assert!(!answer.contains("complete current range is"));
}

#[test]
fn duplicate_unverified_origin_cannot_be_called_independent_corroboration() {
    let evidence = vec![
        json!({
            "source":"https://one.example/story",
            "passage":"Example Organization opened its first clinic in 2019, according to Newswire.",
            "evidence_state":{"independence_key":"wire-report-17","independence_verified":false}
        }),
        json!({
            "source":"https://two.example/story",
            "passage":"Example Organization opened its first clinic in 2019, according to Newswire.",
            "evidence_state":{"independence_key":"wire-report-17","independence_verified":false}
        }),
    ];
    let mut reviews = 0;
    let answer = ground_research_reply(
        "Is this independently corroborated?",
        "The claim is independently corroborated.",
        &evidence,
        |prompt| {
            if prompt.contains("Check the ANSWER's factual assertions") {
                reviews += 1;
                Ok(if reviews <= 2 {
                    r#"{"verdict":"revise","supported":false,"addresses_request":true,"reason":"The two pages share one underlying report and are not independent."}"#.into()
                } else {
                    r#"{"verdict":"accept","supported":true,"addresses_request":true,"reason":"The answer clearly explains the shared origin."}"#.into()
                })
            } else {
                assert!(prompt.contains("Reconsider the prior user-facing draft independently"));
                assert!(!prompt.contains("review_feedback"));
                Ok("Both pages repeat the same Newswire report, so this is not independent corroboration.".into())
            }
        },
    )
    .unwrap();

    assert_eq!(reviews, 3);
    assert!(answer.contains("not independent corroboration"));
    assert!(!answer.contains("The claim is independently corroborated"));
}

#[test]
fn known_conflict_cannot_be_silently_resolved_to_one_date() {
    let evidence = vec![
        json!({
            "source":"https://a.example/history",
            "passage":"Example Organization began operating in 1998.",
            "evidence_state":{
                "subject":"Example Organization","relation":"began operating","object":"1998",
                "gate_status":"conflict","conflicting_claims":["Example Organization began operating in 2000"]
            }
        }),
        json!({
            "source":"https://b.example/history",
            "passage":"Example Organization began operating in 2000.",
            "evidence_state":{
                "subject":"Example Organization","relation":"began operating","object":"2000",
                "gate_status":"conflict","conflicting_claims":["Example Organization began operating in 1998"]
            }
        }),
    ];
    let mut reviews = 0;
    let answer = ground_research_reply(
        "When did the organization begin operating?",
        "It began operating in 1998.",
        &evidence,
        |prompt| {
            if prompt.contains("Check the ANSWER's factual assertions") {
                reviews += 1;
                Ok(if reviews <= 2 {
                    r#"{"verdict":"revise","supported":false,"addresses_request":true,"reason":"Two supplied accounts give different dates; retain both."}"#.into()
                } else {
                    r#"{"verdict":"accept","supported":true,"addresses_request":true,"reason":"Both dates are attributed and the disagreement remains unresolved."}"#.into()
                })
            } else {
                assert!(prompt.contains("Reconsider the prior user-facing draft independently"));
                assert!(!prompt.contains("review_feedback"));
                Ok("The sources conflict: one gives 1998 and the other gives 2000, so the date is unresolved.".into())
            }
        },
    ).unwrap();

    assert_eq!(reviews, 3);
    assert!(answer.contains("sources conflict"));
    assert!(answer.contains("1998"));
    assert!(answer.contains("2000"));
}

#[test]
fn useful_supported_answer_can_pass_as_partial_without_becoming_verified_knowledge() {
    let evidence = vec![
        json!({"source":"https://a.example/history",
            "passage":"Example Organization began operating in 1998."}),
        json!({"source":"https://b.example/history",
            "passage":"Example Organization began operating in 2000."}),
    ];
    let draft = "The first source reports 1998; the second reports 2000. The evidence does not settle which date is correct.";
    let mut calls = 0;
    let answer = ground_research_reply(
        "When did Example Organization begin operating?",
        draft,
        &evidence,
        |prompt| {
            calls += 1;
            if prompt.contains("Assess whether this answer can be shown") {
                return Ok(json!({
                    "verdict":"useful",
                    "all_assertions_supported":true,
                    "reason":"The answer attributes both relevant dates and states that the evidence does not decide between them.",
                    "contribution_quote":"The first source reports 1998; the second reports 2000.",
                    "uncertainty_quote":"The evidence does not settle which date is correct.",
                    "basis_quote":"Example Organization began operating in 1998."
                }).to_string());
            }
            Ok(json!({
                "verdict":"uncertain",
                "supported":false,
                "addresses_request":false,
                "reason":"The answer does not settle which conflicting date is correct."
            }).to_string())
        },
    )
    .unwrap();

    assert_eq!(calls, 3);
    assert!(answer.starts_with(draft));
    assert!(answer.contains("https://a.example/history"));
    assert!(answer.contains("https://b.example/history"));
}

#[test]
fn attributed_accounts_can_remain_supported_when_sources_disagree() {
    let evidence = vec![
        json!({
            "source":"https://a.example/history",
            "passage":"Example Organization began operating in 1998.",
            "evidence_state":{"gate_status":"conflict"}
        }),
        json!({
            "source":"https://b.example/history",
            "passage":"Example Organization began operating in 2000.",
            "evidence_state":{"gate_status":"conflict"}
        }),
    ];
    let draft = "Example Organization began operating in 1998 (https://a.example/history), while another source gives 2000 (https://b.example/history); the evidence does not resolve the date.";
    let mut reviews = 0;
    let answer = ground_research_reply(
        "When did Example Organization begin operating?",
        draft,
        &evidence,
        |prompt| {
            if prompt.contains("Check the ANSWER's factual assertions") {
                reviews += 1;
                assert!(prompt.contains(
                    "supported, assess whether each factual assertion follows from its cited passage, not whether all sources agree"
                ));
                Ok(r#"{"verdict":"accept","supported":true,"addresses_request":true,"reason":"Each date is attributed to a passage that states it, and the answer leaves the disagreement unresolved."}"#.into())
            } else {
                panic!("a supported, relevant account of unresolved disagreement needs no rewrite");
            }
        },
    )
    .unwrap();

    assert_eq!(reviews, 1);
    assert_eq!(answer, draft);
}

#[test]
fn answer_preserves_compound_names_as_single_list_items() {
    let evidence = vec![json!({
        "source":"https://example.org/walkers",
        "passage":"Walkers sells the main flavours: Cheese and Onion, Salt and Vinegar and Ready Salted."
    })];
    let mut calls = 0;
    let answer = ground_research_reply(
        "What flavours does Walkers sell?",
        "Walkers sells Cheese and Onion as separate flavours: Cheese and Onion.",
        &evidence,
        |prompt| {
            calls += 1;
            match calls {
                1 | 2 => {
                    Ok(r#"{"verdict":"revise","supported":false,"addresses_request":true,"reason":"The passage names Cheese and Onion as one flavour, not two."}"#.into())
                }
                3 => {
                    assert!(prompt.contains("Cheese and Onion, Salt and Vinegar and Ready Salted"));
                    Ok("The passage lists Cheese and Onion, Salt and Vinegar, and Ready Salted as flavours. source_1".into())
                }
                4 => Ok(r#"{"verdict":"accept","supported":true,"addresses_request":true,"reason":"Preserves each compound name as listed."}"#.into()),
                _ => panic!("grounding review must stay bounded"),
            }
        },
    )
    .unwrap();

    assert_eq!(calls, 4);
    assert!(answer.contains("Cheese and Onion"));
    assert!(answer.contains("Salt and Vinegar"));
    assert!(!answer.contains("Cheese, Onion"));
}

#[test]
fn failed_grounding_never_dumps_cookie_banner_and_crisp_quote() {
    let evidence = vec![
        json!({"source":"https://www.cbinsights.com/company/cofresh","passage":include_str!("../../../tests/fixtures/cofresh-footer.txt")}),
        json!({"source":"https://en.wikipedia.org/wiki/Walkers_(snack_foods)","passage":"Smith's Crisps - Potato crisps in ready salted, salt and vinegar or cheese and onion flavoured 5-pack multipacks."}),
    ];
    let mut calls = 0;
    let answer = ground_research_reply(
        "What are the best flavour of crisps to drop an egg into?",
        "An unsupported answer",
        &evidence,
        |_| {
            calls += 1;
            Ok(if calls == 3 {
                "Still an unsupported answer".into()
            } else {
                r#"{"verdict":"revise","supported":false,"addresses_request":false,"reason":"Does not address the intended use"}"#.into()
            })
        },
    )
    .unwrap();
    assert_eq!(calls, 4);
    assert_eq!(answer, missing_research_answer());
    assert!(!answer.contains("I ACCEPT"));
    assert!(!answer.contains("Smith's"));
    assert!(!answer.contains("Here's what I found"));
}

#[test]
fn ungrounded_discoverer_is_rewritten_and_reviewed_again() {
    let evidence = vec![
        json!({"source":"https://example.org/element","passage":"Oganesson is a synthetic element."}),
    ];
    let mut calls = 0;
    let result = ground_research_reply(
        "Who discovered it?",
        "Eugene discovered it.",
        &evidence,
        |prompt| {
            calls += 1;
            assert!(prompt.contains("Oganesson is a synthetic element."));
            Ok(match calls {
                1 | 2 => {
                    r#"{"verdict":"revise","supported":false,"addresses_request":false,"reason":"The passage does not name a discoverer."}"#
                        .into()
                }
                3 => "This source does not name the discoverer: https://example.org/element"
                    .into(),
                4 => {
                    r#"{"verdict":"accept","supported":true,"addresses_request":true,"reason":"The answer acknowledges missing evidence."}"#
                        .into()
                }
                _ => panic!("review must be bounded"),
            })
        },
    )
    .unwrap();
    assert_eq!(calls, 4);
    assert!(!result.contains("Eugene"));
    assert!(result.contains("does not name"));
}

#[test]
fn failed_summary_review_reports_limitation_without_dumping_evidence_or_using_url_as_proof() {
    let evidence = vec![
        json!({"source":"https://example.org/unproven-product-name","passage":"Walkers sells Cheese and Onion, Salt and Vinegar and Ready Salted."}),
    ];
    let mut calls = 0;
    let result = ground_research_reply(
        "What flavours exist?",
        "An invented option.",
        &evidence,
        |prompt| {
            calls += 1;
            assert!(!prompt.contains("https://example.org/unproven-product-name"));
            Ok(if calls == 3 {
                "Still unsupported.".into()
            } else {
                r#"{"verdict":"revise","supported":false,"addresses_request":false,"reason":"Unsupported option"}"#
                    .into()
            })
        },
    )
    .unwrap();
    assert_eq!(calls, 4);
    assert_eq!(result, missing_research_answer());
    assert!(!result.contains("Walkers"));
    assert!(!result.contains("https://"));
}

#[test]
fn research_review_does_not_publish_failed_or_malformed_reviews() {
    let mut calls = 0;
    let result = ground_research_reply("Who?", "Unsupported name", &[], |_| {
        calls += 1;
        Ok(if calls == 3 {
            "Still unsupported".into()
        } else {
            r#"{"verdict":"revise","supported":false,"addresses_request":false,"reason":"Missing evidence"}"#.into()
        })
    });
    assert!(result.is_err());
    assert_eq!(calls, 3);
    for response in [
        "not JSON",
        r#"{"reason":"No verdict"}"#,
        r#"{"verdict":"accept","supported":true}"#,
    ] {
        assert!(ground_research_reply("Who?", "Draft", &[], |_| Ok(response.into())).is_err());
    }
}

#[test]
fn unrelated_map_claim_is_absent_from_the_actual_answer_prompt() {
    let mut map = KnowledgeMap::default();
    map.add(
        "the Hagia Sophia",
        "is taller than",
        "the Great Pyramid of Giza",
        "user observation",
    )
    .unwrap();
    let agent = Agent {
        model: "test".into(),
        history: Vec::new(),
        conversation_id: "test".into(),
    };
    for input in [
        "Who discovered the most recent element?",
        "Much better. Thank you for being honest about being unsure :)",
    ] {
        let messages = agent.prompt_messages(input, &map, &[], &[], &[]);
        assert!(
            messages
                .iter()
                .all(|message| !message.content.contains("Hagia Sophia"))
        );
    }
}
