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
            Ok(r#"{"requests_up_to_date_info":false}"#.into())
        });
        assert_eq!(scope, Some(false));
        let evidence = vec![json!({"source":"https://example.org/experiment",
            "passage":"In 2012, the laboratory measured two amperes.","evidence_state":{"gate_status":"unverified_temporal","research_relevance":"historical_context"}})];
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
    fn named_physical_quantity_is_not_a_freshness_request() {
        assert_eq!(
            currentness_with("What is electric current?", |_| {
                panic!("named physical quantity needs no model disambiguation")
            }),
            Some(false)
        );
    }

    #[test]
    fn current_adjective_can_use_a_bounded_historical_reply() {
        let question = "What is the complete current range?";
        let scope = currentness_with(question, |_| {
            Ok(r#"{"requests_up_to_date_info":true}"#.into())
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
        assert!(answer.text.contains("haven't established"));
    }

    #[test]
    fn invalid_intent_output_preserves_the_existing_policy_path() {
        for raw in [
            r#"{}"#,
            r#"{"requests_up_to_date_info":"latest"}"#,
            r#"{"requests_up_to_date_info":null}"#,
            "bad JSON",
        ] {
            assert!(currentness_with("What is the current range?", |_| Ok(raw.into())).is_none());
        }
        assert_eq!(
            currentness_with("What happened today?", |_| panic!(
                "unambiguous cue needs no extra call"
            )),
            Some(true)
        );
        assert!(currentness_with("Who founded it?", |_| panic!("no current ambiguity")).is_none());
    }
}
