use super::super::*;

fn assess(source: &str, subject: &str, relation: &str, object: &str) -> SemanticSupport {
    let finding = candidate_from_context(
        &serde_json::json!({"subject":subject,"relation":relation,"object":object}),
    )
    .unwrap();
    // Reproduce the live 2B failures: the model wrongly returns entailed.
    parse(&serde_json::json!({"verdict":"entailed","reason":"The source supports it.","evidence_text":source}).to_string(),&finding,source).unwrap()
}

#[test]
fn source_list_items_cannot_become_containers_despite_model_approval() {
    for (passage, subject, object) in [
        (
            "Birds recorded on Alder Island include puffins and Arctic terns.",
            "puffins",
            "Arctic terns",
        ),
        (
            "The instruments include a spectrograph and a telescope.",
            "a spectrograph",
            "a telescope",
        ),
        (
            "The collection lists Aster and Ash and Sea and Silence.",
            "Aster",
            "Sea and Silence",
        ),
    ] {
        let support = assess(passage, subject, "include", object);
        assert_ne!(support.verdict(), "entailed", "{passage}");
    }
    assert_eq!(
        assess(
            "The birds include puffins and Arctic terns.",
            "The birds",
            "include",
            "puffins and Arctic terns"
        )
        .verdict(),
        "entailed"
    );
    assert_eq!(
        assess(
            "Puffins are recorded on Alder Island.",
            "Puffins",
            "are recorded on",
            "Alder Island"
        )
        .verdict(),
        "entailed"
    );
}

#[test]
fn live_disproof_and_dropped_attribution_errors_override_model_approval() {
    let disproven = "Flat Earth is an archaic and scientifically disproven conception of the Earth's shape as a plane or disk.";
    let rejected = assess(
        disproven,
        "Flat Earth",
        "is",
        "a true conception of the Earth as a plane or disk",
    );
    assert_eq!(rejected.model_verdict, "entailed");
    assert_eq!(rejected.verdict(), "contradicted");
    assert!(rejected.stance_override.is_some());
    let suggested =
        "It has been suggested that chicken tikka masala is derived from butter chicken.";
    assert_eq!(
        assess(
            suggested,
            "chicken tikka masala",
            "is derived from",
            "butter chicken"
        )
        .verdict(),
        "qualified"
    );
    assert_eq!(
        assess(
            suggested,
            "chicken tikka masala",
            "may be derived from",
            "butter chicken"
        )
        .verdict(),
        "entailed"
    );
    assert_eq!(
        assess(
            disproven,
            "Flat Earth",
            "is",
            "a scientifically disproven conception of the Earth's shape as a plane or disk"
        )
        .verdict(),
        "entailed"
    );
}

#[test]
fn marker_substrings_do_not_rescue_missing_qualifications() {
    let source = "It has been suggested that the sauce contains mayonnaise.";
    assert_eq!(
        assess(source, "The sauce", "contains", "mayonnaise").verdict(),
        "qualified"
    );
}

#[test]
fn question_entity_cannot_be_imported_as_source_attribution() {
    let finding = candidate_from_context(&serde_json::json!({
        "subject":"Walkers",
        "relation":"sells",
        "object":"barbecue crisps"
    }))
    .unwrap();
    let question = "Which crisp flavours does Walkers sell?";
    let passage = "My favourite crisp flavour is barbecue.";
    let support = assess_with(question, "Crisp discussion", passage, &finding, |_| {
        Ok(serde_json::json!({
            "verdict":"entailed",
            "reason":"It names a crisp flavour.",
            "evidence_text":passage
        })
        .to_string())
    })
    .unwrap();

    assert_eq!(support.model_verdict, "entailed");
    assert_eq!(support.verdict(), "insufficient");
    assert!(support.reason.contains("named entity is absent"));
}

#[test]
fn matching_question_entity_in_source_remains_eligible_for_entailment() {
    let finding = candidate_from_context(&serde_json::json!({
        "subject":"Walkers",
        "relation":"sells",
        "object":"barbecue crisps"
    }))
    .unwrap();
    let question = "Which crisp flavours does Walkers sell?";
    let passage = "Walkers sells barbecue crisps.";
    let support = assess_with(question, "Walkers products", passage, &finding, |_| {
        Ok(serde_json::json!({
            "verdict":"entailed",
            "reason":"The source names the product.",
            "evidence_text":passage
        })
        .to_string())
    })
    .unwrap();

    assert_eq!(support.verdict(), "entailed");
}

#[test]
fn one_source_example_does_not_establish_a_variety_but_an_explicit_list_can() {
    let single = "## WALKERS CRISPS\n\nLegendary Ready Salted";
    let finding = candidate_from_context(&serde_json::json!({
        "subject":"Walkers",
        "relation":"sells",
        "object":"crisps in a variety of flavours"
    }))
    .unwrap();
    let unsupported = assess_with(
        "What crisp flavours does Walkers sell?",
        "Product",
        single,
        &finding,
        |_| {
            Ok(serde_json::json!({
                "verdict":"entailed",
                "reason":"The source names a crisp flavour.",
                "evidence_text":single
            })
            .to_string())
        },
    )
    .unwrap();
    assert_eq!(unsupported.model_verdict, "entailed");
    assert_eq!(unsupported.verdict(), "insufficient");
    assert!(unsupported.reason().contains("one example"));

    let listed = "Walkers sells crisps in a variety of flavours; the main are Cheese and Onion, Salt and Vinegar, and Ready Salted.";
    let supported = assess_with(
        "What crisp flavours does Walkers sell?",
        "Walkers",
        listed,
        &finding,
        |_| {
            Ok(serde_json::json!({
                "verdict":"entailed",
                "reason":"The source explicitly lists several flavours.",
                "evidence_text":listed
            })
            .to_string())
        },
    )
    .unwrap();
    assert_eq!(supported.verdict(), "entailed");
}
