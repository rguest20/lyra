use super::*;

#[test]
fn answer_rewrites_and_structured_reviews_have_distinct_generation_modes() {
    let passage = "Ada Voss founded Meridian Observatory in 1974.";
    let evidence = vec![json!({"source":"https://example.org/history","passage":passage})];
    let mut modes = Vec::new();
    let answer = ground_research_reply_with_scope(
        "Who founded Meridian Observatory?",
        "The observatory was founded in 1986.",
        &evidence,
        None,
        |_, is_answer| {
            modes.push(is_answer);
            Ok(match modes.len() {
                1 => json!({"supported":false,"addresses_request":false,"reason":"Wrong date and missing founder."}).to_string(),
                2 => passage.into(),
                3 => json!({"supported":true,"addresses_request":true,"reason":"The rewritten answer follows the source."}).to_string(),
                _ => panic!("answer repair must remain bounded"),
            })
        },
    ).unwrap();
    assert_eq!(modes, [false, true, false]);
    assert!(answer.starts_with(passage));
}

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
                Ok(json!({"supported":true,"addresses_request":true,"reason":"The source names the discoverer."}).to_string())
            } else { Ok("According to the source, Alexander Fleming discovered penicillin in 1928.".into()) }
        }).unwrap();
    assert_eq!(calls, 3);
    assert!(!answer.contains("source Alexander Fleming"));
    assert!(answer.contains("discovered penicillin"));
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
            |_| Ok(json!({"supported":true,"addresses_request":true,"reason":"The source mentions detection."}).to_string())).unwrap();
        assert!(answer.contains(passage));
        assert!(!answer.contains("has not observed"));
        assert!(!answer.contains("No evidence"));
    }
}

#[test]
fn independent_source_clauses_cannot_become_a_causal_explanation() {
    let passage = "Meridian Laboratory did not demonstrate superconductivity; its experiment measured magnetic susceptibility.";
    let evidence = vec![json!({"source":"https://example.org/report","passage":passage})];
    let draft = "Meridian Laboratory did not demonstrate superconductivity because its experiment measured magnetic susceptibility instead.";
    let mut calls = 0;
    let answer = ground_research_reply("Did Meridian Laboratory demonstrate superconductivity?", draft, &evidence,
        |prompt| { calls += 1;
            if prompt.contains("Check the ANSWER's factual assertions") {
                Ok(json!({"supported":true,"addresses_request":true,"reason":"Both observations occur in the source."}).to_string())
            } else { Ok(passage.into()) }
        }).unwrap();
    assert_eq!(calls, 3);
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
            if prompt.contains("Check the ANSWER's factual assertions") {
                Ok(json!({"supported":true,"addresses_request":true,"reason":"The source names the founder."}).to_string())
            } else { Ok("According to the source, Ada Voss founded Meridian Observatory in 1974.".into()) }
        }).unwrap();
    assert_eq!(calls, 3);
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
        |_| Ok(json!({"supported":true,"addresses_request":true,"reason":"The passage states this function."}).to_string())).unwrap();
    assert!(!answer.contains("Source_1"));
    assert!(answer.contains("https://example.org/report"));
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
        let evidence = vec![json!({"source":"https://example.org/reference","passage":passage})];
        let mut reviews = 0;
        let result = ground_research_reply(question, broken, &evidence, |prompt| {
            if prompt.contains("Check the ANSWER's factual assertions") {
                reviews += 1;
                Ok(json!({"supported":true,"addresses_request":true,"reason":"The source lists the items."}).to_string())
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
        |_| Ok(r#"{"supported":true,"addresses_request":true,"reason":"Limited source assertion."}"#.into())).unwrap();
    assert!(answer.contains("does not establish independent corroboration"));
    assert!(!answer.contains("still current"));
}

#[test]
fn a_physics_current_is_not_a_currentness_request() {
    let evidence = vec![json!({"source":"https://example.org/reference",
        "passage":"Electric current is a flow of electric charge.",
        "evidence_state":{"gate_status":"unverified_temporal"}})];
    let answer = ground_research_reply("What is electric current?", "The source defines electric current as a flow of electric charge.",
        &evidence, |_| Ok(r#"{"supported":true,"addresses_request":true,"reason":"Source definition is attributed."}"#.into())).unwrap();
    assert!(!answer.contains("still current"));
    assert!(answer.contains("flow of electric charge"));
}

#[test]
fn temporal_gate_is_enforced_even_when_semantic_use_is_incorrectly_direct() {
    let evidence = vec![json!({"source":"https://example.org/catalogue",
        "passage":"The archive lists War and Peace and Northanger Abbey.",
        "evidence_state":{"gate_status":"unverified_temporal","research_relevance":"direct"}})];
    let answer = ground_research_reply(
        "What titles are currently available?",
        "The titles are no longer available.",
        &evidence,
        |_| panic!("temporal policy must survive a positive but incorrect model verdict"),
    )
    .unwrap();
    assert!(answer.contains("War and Peace and Northanger Abbey"));
    assert!(!answer.contains("no longer"));
    assert!(answer.contains("haven't established whether this is still current"));
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
    let answer = ground_research_reply(
        "What does the museum currently exhibit?",
        "The current collection does not include an astrolabe or reflecting telescope.",
        &evidence,
        |_| panic!("source-literal historical context requires no invented current assertion"),
    )
    .unwrap();
    assert!(answer.contains(passage));
    assert!(answer.contains("haven't established whether this is still current"));
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
        view[0]["limits"]
            .to_string()
            .contains("does not establish current status")
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
            if prompt.contains("Check the ANSWER's factual assertions") {
                Ok(r#"{"supported":true,"addresses_request":true,"reason":"Qualified provenance answer."}"#.into())
            } else {
                Ok("Sciencewire reports the Helios Laboratory flight test in 2020. Independent corroboration is not established. source_1".into())
            }
        }).unwrap();
    assert_eq!(calls, 3);
    assert!(!answer.contains("Helios Institute"));
    assert!(answer.contains("Helios Laboratory"));
}

#[test]
fn explicit_source_fallback_preserves_compound_names_across_domains() {
    for (question, passage) in [
        (
            "Which book is best? If you can't tell, list the titles.",
            "The catalogue lists War and Peace, Sense and Sensibility, and Northanger Abbey.",
        ),
        (
            "Which musical act is your favourite? If sources don't identify one, enumerate the alternatives.",
            "The programme lists Simon and Garfunkel, Earth, Wind & Fire, and Queen.",
        ),
        (
            "Which destination is best? If you cannot determine that, give the options.",
            "The itinerary includes Trinidad and Tobago, Antigua and Barbuda, and Saint Lucia.",
        ),
    ] {
        let evidence = vec![json!({"source":"https://example.org/reference","passage":passage})];
        let reply = ground_research_reply(question, "No supported ranking.", &evidence, |prompt| {
            if prompt.contains("Check the ANSWER's factual assertions") {
                Ok(r#"{"supported":true,"addresses_request":false,"reason":"No ranking available."}"#.into())
            } else {
                Ok("No supported ranking.".into())
            }
        }).unwrap();
        assert!(reply.contains(passage), "compound wording changed: {reply}");
        assert!(reply.contains("https://example.org/reference"));
        assert!(reply.contains("I can't tell which is best"));
    }
}

#[test]
fn fallback_requires_an_explicit_list_request_not_a_topic_word_or_substring() {
    let evidence = vec![json!({"source":"https://example.org/reference",
        "passage":"The catalogue lists War and Peace and Northanger Abbey."})];
    for question in [
        "What are the asbestos alternatives? If you can't tell, list them.",
        "Which flavour is best? If you can't tell, say so.",
        "Which book is best? If you can't tell, say so.",
    ] {
        assert!(
            explicit_best_question_fallback(question, &evidence).is_none(),
            "{question}"
        );
    }
}

#[test]
fn grounding_rewrites_metadata_even_when_fact_reviewer_accepts_the_draft() {
    let evidence = vec![json!({"source":"https://example.org/aurora",
        "passage":"Aurora Observatory opened in 2001.",
        "evidence_state":{"finding_status":"needs_review","gate_status":"awaiting_corroboration",
            "extraction_confidence":0.99}})];
    let mut calls = 0;
    let answer = ground_research_reply("When did Aurora Observatory open?",
        "Aurora Observatory opened in 2001, with extraction confidence of 0.99.", &evidence,
        |prompt| {
            calls += 1;
            if prompt.contains("Check the ANSWER's factual assertions") {
                Ok(r#"{"supported":true,"addresses_request":true,"reason":"The opening date is in the passage."}"#.into())
            } else {
                // The repair generator must not receive raw scores or review enums either.
                let data = prompt.split("Data: ").last().unwrap();
                assert!(!data.contains("extraction_confidence"));
                assert!(!data.contains("awaiting_corroboration"));
                Ok("The source dates Aurora Observatory's opening to 2001. source_1".into())
            }
        }).unwrap();
    assert_eq!(calls, 3);
    assert!(answer.contains("2001"));
    assert!(!answer.contains("confidence"));
}

#[test]
fn factual_questions_do_not_acquire_unrequested_rankings() {
    let draft =
        "The source dates the opening to 2001. I can't tell which is best from these sources.";
    assert!(reply_style_failure("When did Aurora Observatory open?", draft).is_some());
    assert!(reply_style_failure("Which observatory is best?", draft).is_none());
    assert!(
        reply_style_failure(
            "What does the extraction_confidence field measure?",
            "extraction_confidence measures the extractor's confidence."
        )
        .is_none()
    );
}

#[test]
fn answer_generator_gets_source_limits_and_shared_lineage_without_internal_scores() {
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
    let public = model_answer_evidence("When did Aurora Observatory open?", &evidence, None);
    let text = serde_json::to_string(&public).unwrap();
    for internal in [
        "extraction_confidence",
        "source_archive_id",
        "gate_status",
        "needs_review",
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
    assert!(public[0]["limits"].to_string().contains("current"));
    assert!(public[2]["limits"].to_string().contains("disagreement"));
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
            Ok(r#"{"supported":true,"addresses_request":true,"reason":"Source-attributed products and an explicit limitation."}"#.into())
        }).unwrap();
        assert_eq!(calls, 1, "{limitation}");
        assert!(answer.contains("Cofresh makes Indian snack foods"));
    }
}

#[test]
fn an_independence_denial_cannot_hide_a_positive_claim_in_another_clause() {
    for answer in [
        "One report is not independent. These reports independently corroborate the claim.",
        "These sources are not independent, but the claim is independently corroborated.",
    ] {
        assert!(asserts_unverified_independence(answer, &[]), "{answer}");
    }
}

#[test]
fn factual_support_alone_cannot_approve_an_answer_to_another_question() {
    let mut calls = 0;
    let result = ground_research_reply("Which crisps suit this use?", "Firm tofu is a soup ingredient.", &[], |_| {
        calls += 1;
        Ok(if calls == 2 { "Still discussing soup.".into() } else { r#"{"supported":true,"addresses_request":false,"reason":"Soup facts do not answer the crisp question"}"#.into() })
    }).unwrap();
    assert_eq!(result, missing_research_answer());
    assert_eq!(calls, 3);
    assert!(reply_style_failure("What are the best flavour of crisps to drop an egg into?", "I can't verify the best flavour of crisps to drop an egg into. Do you mean a cooked dish or a raw egg?").is_none());
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
                1 => Ok(r#"{"supported":true,"addresses_request":false,"reason":"The source does not rank a best flavour."}"#.into()),
                2 => {
                    assert!(prompt.contains("What\'s the best crisp flavour?"));
                    Ok("The source does not rank the flavours. I'm not sure which is best, but my guess is Cheese and Onion. It lists Cheese and Onion, Salt and Vinegar, and Ready Salted. source_1".into())
                }
                3 => Ok(r#"{"supported":true,"addresses_request":true,"reason":"Follows the requested fallback and marks the guess uncertain."}"#.into()),
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
fn explicit_fallback_returns_the_literal_list_if_model_review_still_fails() {
    let evidence = vec![json!({
        "source":"https://en.wikipedia.org/wiki/Walkers_(snack_foods)",
        "passage":"Walkers sells crisps in a variety of flavours; the main are Cheese and Onion, Salt and Vinegar, and Ready Salted.",
        "evidence_state":{"finding_status":"needs_review","gate_status":"awaiting_corroboration"}
    })];
    let answer = ground_research_reply(
        "What is the best Walkers crisp flavour? If you can't tell from the sources, list the flavours. I'm happy with a guess if you're unsure; say so.",
        "I can't tell which is best. The source names Cheese and Onion and Salt and Vinegar.",
        &evidence,
        |_| Ok(r#"{"supported":false,"addresses_request":false,"reason":"Does not determine a best flavour"}"#.into()),
    )
    .unwrap();

    assert!(answer.contains("I can't tell which is best"));
    assert!(answer.contains("Cheese and Onion, Salt and Vinegar, and Ready Salted"));
    assert!(answer.contains("https://en.wikipedia.org/wiki/Walkers_(snack_foods)"));
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
                assert!(data.contains("does not establish current status"));
            }
            assert!(prompt.contains("Walkers range includes paprika"));
            match calls {
                1 => {
                    Ok(r#"{"supported":false,"addresses_request":false,"reason":"Historical examples do not establish the complete current range."}"#.into())
                }
                2 => Ok("The source mentions cheese and onion and salt and vinegar as historical examples, but it does not establish Walkers' complete current flavour range. An active claim also lists paprika, though its time scope is unclear. source_1".into()),
                3 => Ok(r#"{"supported":true,"addresses_request":true,"reason":"Gives historical examples, flags the unresolved scope, and avoids completeness."}"#.into()),
                _ => panic!("grounding review must stay bounded"),
            }
        },
    )
    .unwrap();

    assert_eq!(calls, 3);
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
                Ok(r#"{"supported":true,"addresses_request":true,"reason":"Both passages state the same claim."}"#.into())
            } else {
                assert!(prompt.contains("Independent corroboration requires"));
                Ok("Both pages repeat the same Newswire report, so this is not independent corroboration.".into())
            }
        },
    )
    .unwrap();

    assert_eq!(reviews, 2);
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
                Ok(r#"{"supported":true,"addresses_request":true,"reason":"The date is in a passage."}"#.into())
            } else {
                assert!(prompt.contains("supplied evidence marks this claim as conflicted"));
                Ok("The sources conflict: one gives 1998 and the other gives 2000, so the date is unresolved.".into())
            }
        },
    ).unwrap();

    assert_eq!(reviews, 2);
    assert!(answer.contains("sources conflict"));
    assert!(answer.contains("1998"));
    assert!(answer.contains("2000"));
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
                1 => {
                    Ok(r#"{"supported":false,"addresses_request":true,"reason":"The passage names Cheese and Onion as one flavour, not two."}"#.into())
                }
                2 => {
                    assert!(prompt.contains("Cheese and Onion, Salt and Vinegar and Ready Salted"));
                    Ok("The passage lists Cheese and Onion, Salt and Vinegar, and Ready Salted as flavours. source_1".into())
                }
                3 => Ok(r#"{"supported":true,"addresses_request":true,"reason":"Preserves each compound name as listed."}"#.into()),
                _ => panic!("grounding review must stay bounded"),
            }
        },
    )
    .unwrap();

    assert_eq!(calls, 3);
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
            Ok(if calls == 2 {
                "Still an unsupported answer".into()
            } else {
                r#"{"supported":false,"addresses_request":false,"reason":"Does not address the intended use"}"#.into()
            })
        },
    )
    .unwrap();
    assert_eq!(calls, 3);
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
                1 => {
                    r#"{"supported":false,"addresses_request":false,"reason":"The passage does not name a discoverer."}"#
                        .into()
                }
                2 => "This source does not name the discoverer: https://example.org/element"
                    .into(),
                3 => {
                    r#"{"supported":true,"addresses_request":true,"reason":"The answer acknowledges missing evidence."}"#
                        .into()
                }
                _ => panic!("review must be bounded"),
            })
        },
    )
    .unwrap();
    assert_eq!(calls, 3);
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
            Ok(if calls == 2 {
                "Still unsupported.".into()
            } else {
                r#"{"supported":false,"addresses_request":false,"reason":"Unsupported option"}"#
                    .into()
            })
        },
    )
    .unwrap();
    assert_eq!(calls, 3);
    assert_eq!(result, missing_research_answer());
    assert!(!result.contains("Walkers"));
    assert!(!result.contains("https://"));
}

#[test]
fn research_review_does_not_publish_failed_or_malformed_reviews() {
    let mut calls = 0;
    let result = ground_research_reply("Who?", "Unsupported name", &[], |_| {
        calls += 1;
        Ok(if calls == 2 {
            "Still unsupported".into()
        } else {
            r#"{"supported":false,"addresses_request":false,"reason":"Missing evidence"}"#.into()
        })
    })
    .unwrap();
    assert_eq!(result, missing_research_answer());
    assert_eq!(calls, 3);
    for response in [
        "not JSON",
        r#"{"reason":"No verdict"}"#,
        r#"{"supported":true}"#,
    ] {
        assert!(ground_research_reply("Who?", "Draft", &[], |_| Ok(response.into())).is_err());
    }
}

#[test]
fn alternative_offer_of_help_triggers_a_rewrite() {
    assert_eq!(
        reply_style_failure(
            "Thank you",
            "You're welcome. I'm happy to clarify anything else you need."
        ),
        Some("reply appends a stock invitation")
    );
    assert_eq!(
        strip_stock_invitation("You're welcome. I'm happy to clarify anything else you need."),
        "You're welcome."
    );
}

#[test]
fn screenshot_reply_is_rewritten_as_a_brief_acknowledgement() {
    let input = "Much better. Thank you for being honest about being unsure :)";
    let draft = "You're welcome! I appreciate the honesty about the limitation. If you need more details on Oganesson or anything else, feel free to ask.";
    let result = repair_reply_style(input, draft, |instruction| {
        assert!(instruction.contains("Preserve factual uncertainty"));
        assert!(instruction.contains("If the user thanks you"));
        Ok("You're welcome.".into())
    })
    .unwrap();
    assert_eq!(result, "You're welcome.");
}

#[test]
fn source_assessment_narration_is_repaired_without_inventing_a_discoverer() {
    let draft = "The user has requested the discoverer of the most recent element. The provided source indicates that Oganesson is a synthetic chemical element.";
    let result = repair_reply_style("Who discovered the most recent element?", draft, |_| {
        Ok("This source names the element but doesn't name its discoverer.".into())
    })
    .unwrap();
    assert!(result.contains("doesn't name its discoverer"));
    assert!(reply_style_failure("Who discovered it?", &result).is_none());
}

#[test]
fn stock_closing_trimming_preserves_unicode_and_the_actual_answer() {
    assert_eq!(
        strip_stock_invitation(
            "İstanbul is mentioned in this source. If you need more details, feel free to ask."
        ),
        "İstanbul is mentioned in this source."
    );
    let reply = "This source does not name the discoverer.";
    assert_eq!(
        repair_reply_style("Who discovered it?", reply, |_| panic!(
            "good replies should not trigger rewriting"
        ))
        .unwrap(),
        reply
    );
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
