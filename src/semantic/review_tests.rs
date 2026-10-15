use super::*;

fn candidate(subject: &str, relation: &str, object: &str) -> ScholarFinding {
    candidate_from_context(
        &serde_json::json!({"subject":subject,"relation":relation,"object":object}),
    )
    .unwrap()
}

fn run_two_pass(
    question: &str,
    passage: &str,
    finding: &ScholarFinding,
    review: &str,
) -> SemanticSupport {
    let mut responses = [
        serde_json::json!({"verdict":"entailed","reason":"The source supports it.","evidence_text":passage}).to_string(),
        review.to_owned(),
    ].into_iter();
    assess_with(question, "Test source", passage, finding, |_| {
        Ok(responses
            .next()
            .expect("semantic review should be requested"))
    })
    .unwrap()
}

fn review(verdict: &str, reason: &str, evidence: Option<&str>) -> String {
    serde_json::json!({"verdict":verdict,"reason":reason,"evidence_text":evidence}).to_string()
}

#[test]
fn adversarial_review_catches_member_container_transfer() {
    let passage = "Birds recorded on Alder Island include puffins and Arctic terns.";
    let bad = run_two_pass(
        "What birds are there?",
        passage,
        &candidate("puffins", "include", "Arctic terns"),
        &review(
            "insufficient",
            "The passage lists both birds under the broader category.",
            None,
        ),
    );
    assert_eq!(bad.model_verdict, "entailed");
    assert_eq!(bad.review_verdict.as_deref(), Some("insufficient"));
    assert_eq!(bad.verdict(), "insufficient");

    let good = run_two_pass(
        "What birds are there?",
        passage,
        &candidate(
            "Birds recorded on Alder Island",
            "include",
            "puffins and Arctic terns",
        ),
        &review(
            "entailed",
            "The source directly lists both birds.",
            Some(passage),
        ),
    );
    assert_eq!(good.verdict(), "entailed");
}

#[test]
fn adversarial_review_preserves_disproof_and_uncertainty() {
    let disproven = "Flat Earth is an archaic and scientifically disproven conception of the Earth's shape as a plane or disk.";
    let rejected = run_two_pass(
        "Is Flat Earth true?",
        disproven,
        &candidate(
            "Flat Earth",
            "is",
            "a true conception of the Earth as a plane or disk",
        ),
        &review(
            "contradicted",
            "The source describes the conception as scientifically disproven.",
            None,
        ),
    );
    assert_eq!(rejected.verdict(), "contradicted");

    let suggested =
        "It has been suggested that chicken tikka masala is derived from butter chicken.";
    let rejected = run_two_pass(
        "Where did the dish originate?",
        suggested,
        &candidate("chicken tikka masala", "is derived from", "butter chicken"),
        &review(
            "qualified",
            "The source attributes this as a suggestion rather than an established fact.",
            None,
        ),
    );
    assert_eq!(rejected.verdict(), "qualified");

    let faithful = run_two_pass(
        "Where might the dish originate?",
        suggested,
        &candidate(
            "chicken tikka masala",
            "may be derived from",
            "butter chicken",
        ),
        &review(
            "entailed",
            "The candidate preserves the source's uncertainty.",
            Some(suggested),
        ),
    );
    assert_eq!(faithful.verdict(), "entailed");
}

#[test]
fn review_rejects_imported_entity_and_single_example_generalization() {
    let passage = "My favourite crisp flavour is barbecue.";
    let finding = candidate("Walkers", "sells", "barbecue crisps");
    let rejected = run_two_pass(
        "Which crisp flavours does Walkers sell?",
        passage,
        &finding,
        &review(
            "insufficient",
            "The passage describes the speaker's preference, not Walkers' products.",
            None,
        ),
    );
    assert_eq!(rejected.verdict(), "insufficient");

    let passage = "Walkers' listed flavour is Ready Salted.";
    let finding = candidate("Walkers", "sells", "crisps in a variety of flavours");
    let rejected = run_two_pass(
        "What flavours does Walkers sell?",
        passage,
        &finding,
        &review(
            "insufficient",
            "One listed flavour does not establish a variety.",
            None,
        ),
    );
    assert_eq!(rejected.verdict(), "insufficient");
}

#[test]
fn uncertain_review_does_not_silently_approve_initial_entailment() {
    let passage = "The museum opened in 2012.";
    let finding = candidate("The museum", "opened in", "2012");
    let mut responses = [
        serde_json::json!({"verdict":"entailed","reason":"The source gives the date.","evidence_text":passage}).to_string(),
        "not valid JSON".to_owned(),
    ].into_iter();
    let result = assess_with("When did it open?", "Museum", passage, &finding, |_| {
        Ok(responses.next().unwrap())
    });
    assert!(
        result.is_err(),
        "malformed adversarial review must not approve the claim"
    );
}

#[test]
fn second_adjudication_can_correct_a_false_negative_without_the_question_context() {
    let passage = "Meridian Laboratory did not demonstrate superconductivity; its experiment measured magnetic susceptibility.";
    let mut finding = candidate(
        "Meridian Laboratory",
        "did not demonstrate",
        "superconductivity",
    );
    finding.evidence_text = passage.to_owned();
    let mut responses = [
        review(
            "insufficient",
            "The source does not specify the experiment object.",
            None,
        ),
        review(
            "entailed",
            "The source explicitly states the laboratory did not demonstrate superconductivity.",
            Some(passage),
        ),
    ]
    .into_iter();
    let support = assess_with(
        "Did Meridian Laboratory demonstrate superconductivity?",
        "Experiment report",
        passage,
        &finding,
        |messages| {
            if messages[0]
                .content
                .contains("independent second semantic adjudicator")
            {
                assert!(!messages[1].content.contains("research_question"));
            }
            Ok(responses.next().unwrap())
        },
    )
    .unwrap();
    assert_eq!(support.model_verdict, "insufficient");
    assert_eq!(support.review_verdict.as_deref(), Some("entailed"));
    assert_eq!(support.verdict(), "entailed");
    assert!(support.supports(&finding, passage));
}
