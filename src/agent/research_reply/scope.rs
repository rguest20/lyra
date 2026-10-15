//! Apply shared time-intent classification to answer presentation policy.
use super::*;

pub(super) fn currentness_with(
    question: &str,
    ask: impl FnMut(&[Message]) -> io::Result<String>,
) -> Option<bool> {
    crate::research::currentness_request_with(question, ask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantity_intent_changes_narration_without_changing_evidence_gate() {
        let scope = currentness_with("What current did the laboratory measure?", |_| {
            Ok(r#"{"requires_present_state":false,"requires_latest_knowledge":false,"uncertain":false,"reason":"It asks for a measured physical quantity."}"#.into())
        });
        assert_eq!(scope, Some(false));
        let evidence = vec![json!({"source":"https://example.org/experiment",
            "passage":"In 2012, the laboratory measured two amperes.","evidence_state":{"gate_status":"unverified_temporal","research_relevance":"direct"}})];
        let view = super::super::source_view::model_answer_evidence(
            "What current did the laboratory measure?",
            &evidence,
            scope,
        );
        assert!(
            !serde_json::to_string(&view)
                .unwrap()
                .contains("does not establish current status")
        );
        assert_eq!(
            evidence[0]["evidence_state"]["gate_status"],
            "unverified_temporal"
        );
        assert!(
            super::super::source_limits::source_limited_answer_with_scope(
                "What current did the laboratory measure?",
                &evidence,
                scope
            )
            .is_none()
        );
    }

    #[test]
    fn named_physical_quantity_is_classified_by_meaning() {
        assert_eq!(
            currentness_with("What is electric current?", |messages| {
                assert!(messages[0].content.contains("Use the request's meaning"));
                Ok(r#"{"requires_present_state":false,"requires_latest_knowledge":false,"uncertain":false,"reason":"It asks about a physical concept."}"#.into())
            }),
            Some(false)
        );
    }

    #[test]
    fn current_adjective_can_use_a_bounded_historical_reply() {
        let question = "What is the complete current range?";
        let scope = currentness_with(question, |_| {
            Ok(r#"{"requires_present_state":true,"requires_latest_knowledge":false,"uncertain":false,"reason":"The user asks for the range available now."}"#.into())
        });
        let evidence = vec![
            json!({"source":"https://example.org/history","passage":"In 2012, the range included an astrolabe.",
            "evidence_state":{"gate_status":"unverified_temporal"}}),
        ];
        let answer = super::super::source_limits::source_limited_answer_with_scope(
            question, &evidence, scope,
        )
        .unwrap();
        assert!(answer.text.contains("2012"));
        assert!(
            answer
                .text
                .contains("does not establish whether this remains current")
        );
    }

    #[test]
    fn relevance_review_can_preserve_a_historical_partial_answer_when_currentness_is_missed() {
        let evidence = vec![json!({"source":"https://example.org/history",
            "passage":"In 2012, Riverton Museum displayed an astrolabe.",
            "evidence_state":{"gate_status":"unverified_temporal","research_relevance":"historical_context"}})];
        let answer = super::super::source_limits::source_limited_answer_with_scope(
            "What does Riverton Museum currently exhibit?",
            &evidence,
            Some(false),
        )
        .unwrap();
        assert!(
            answer
                .text
                .contains("In 2012, Riverton Museum displayed an astrolabe.")
        );
        assert!(
            answer
                .text
                .contains("does not establish whether this remains current")
        );
    }

    #[test]
    fn malformed_or_uncertain_output_gets_one_bounded_reconsideration() {
        for raw in [
            r#"{}"#,
            r#"{"requires_present_state":false,"requires_latest_knowledge":false,"uncertain":false,"reason":""}"#,
            r#"{"requires_present_state":false,"requires_latest_knowledge":false,"uncertain":true,"reason":"The request could have either meaning."}"#,
            "bad JSON",
        ] {
            let mut calls = 0;
            assert!(
                currentness_with("How does the range stand today?", |messages| {
                    calls += 1;
                    if calls == 2 {
                        assert!(
                            messages.len() > 2,
                            "retry should include reconsideration context"
                        );
                    }
                    Ok(raw.into())
                })
                .is_none()
            );
            assert_eq!(calls, 2);
        }
        assert_eq!(
            currentness_with("How does the range stand today?", |_| {
                Ok(r#"{"requires_present_state":true,"requires_latest_knowledge":false,"uncertain":false,"reason":"The answer depends on the situation today."}"#.into())
            }),
            Some(true),
        );
        assert_eq!(
            currentness_with("Who founded it?", |_| {
                Ok(r#"{"requires_present_state":false,"requires_latest_knowledge":false,"uncertain":false,"reason":"This asks about a historical fact."}"#.into())
            }),
            Some(false),
        );
    }

    #[test]
    fn unresolved_currentness_cannot_use_the_literal_source_shortcut() {
        let evidence = vec![
            json!({"source":"https://example.org/history","passage":"In 2012, the range included an astrolabe.","evidence_state":{"gate_status":"unverified_temporal","research_relevance":"historical_context"}}),
        ];
        assert!(
            super::super::source_limits::source_limited_answer_with_scope(
                "How does the range stand today?",
                &evidence,
                None,
            )
            .is_none()
        );
    }
}
