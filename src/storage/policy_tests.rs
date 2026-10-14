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
