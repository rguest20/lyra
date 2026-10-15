//! Boundary tests for source-use policy; separate from database integration.
use super::*;

#[test]
fn temporal_adverbs_do_not_match_inside_other_words() {
    for question in [
        "What is known about Meridian Observatory?",
        "What remains unknown about this star?",
        "How are concurrent computations scheduled?",
        "Who founded Snowley Observatory?",
    ] {
        assert!(
            !is_time_sensitive(question, "is", "a scientific fact"),
            "{question}"
        );
    }
}

#[test]
fn explicit_currency_and_existing_changing_topics_remain_time_sensitive() {
    for question in [
        "Who leads the organization now?",
        "What is currently available?",
        "What is the latest edition?",
        "What is the most recent edition?",
        "What happened today?",
        "What is the GPU specification?",
        "What are the software versions?",
        "Who is the president?",
    ] {
        assert!(
            is_time_sensitive(question, "is", "a sourced value"),
            "{question}"
        );
    }
}

#[test]
fn freshness_and_consequence_terms_match_words_not_incidental_substrings() {
    assert!(!is_time_sensitive(
        "How does syntax work?",
        "is",
        "a sourced fact"
    ));
    assert!(!is_consequential(
        "How does syntax work?",
        "Syntax",
        "describes rules"
    ));
    assert!(!is_consequential(
        "How do flawless crystals form?",
        "Crystal",
        "forms"
    ));
    assert!(!is_consequential(
        "How do we investigate an observatory?",
        "Observatory",
        "researches"
    ));
}

#[test]
fn high_consequence_terms_keep_common_inflections() {
    for question in [
        "What are the symptoms?",
        "What are the laws on this?",
        "How are taxes calculated?",
        "What are the investment risks?",
        "What diagnosis does the report give?",
    ] {
        assert!(is_consequential(question, "Example", "is"), "{question}");
    }
}

#[test]
fn common_health_legal_and_finance_terms_require_review() {
    for question in [
        "What medicine is used?",
        "What treatment is recommended?",
        "What dose is safe?",
        "What legal rights apply?",
        "How are taxes calculated?",
        "What financial investment is risky?",
    ] {
        assert!(is_consequential(question, "Example", "is"), "{question}");
    }
}

#[test]
fn extended_consequence_stems_do_not_match_unrelated_words() {
    for question in [
        "What is the contractile ring?",
        "What does a taxonomic classification mean?",
    ] {
        assert!(!is_consequential(question, "Example", "is"), "{question}");
    }
}
