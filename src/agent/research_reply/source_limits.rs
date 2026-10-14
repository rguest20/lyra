//! Replies whose assertions follow directly from exact excerpts and verified
//! source-use metadata. This is a bounded fallback, not an intent classifier.
use serde_json::Value;

pub(super) struct LimitedAnswer {
    pub text: String,
    pub kind: &'static str,
}

pub(super) fn source_limited_answer_with_scope(
    question: &str,
    evidence: &[Value],
    currentness: Option<bool>,
) -> Option<LimitedAnswer> {
    if evidence.is_empty()
        || evidence.len() > 3
        || evidence.iter().any(|entry| {
            let state = &entry["evidence_state"];
            state["gate_status"] == "conflict"
                || state["conflicting_claims"]
                    .as_array()
                    .is_some_and(|claims| !claims.is_empty())
        })
    {
        return None;
    }
    let words = super::normalized_words(question);
    let provenance_request = crate::research::source_independence_request(question)
        || evidence
            .iter()
            .any(|entry| entry["evidence_state"]["research_relevance"] == "provenance");
    // Unambiguous temporal cues or a reviewed, source-anchored historical use.
    // The word "current" alone can denote a physical quantity, not currency.
    let current_request = words
        .iter()
        .any(|word| matches!(word.as_str(), "currently" | "today" | "now" | "latest"))
        || (words.iter().any(|word| word == "current")
            && evidence.iter().any(|entry| {
                entry["evidence_state"]["research_relevance"] == "historical_context"
            }));
    let current_request = currentness.unwrap_or(current_request);
    let temporal = current_request
        && evidence.iter().all(|entry| {
            let state = &entry["evidence_state"];
            state["research_relevance"] == "historical_context"
                || state["gate_status"] == "unverified_temporal"
        });
    let origin = evidence[0]["evidence_state"]["independence_key"]
        .as_str()
        .filter(|key| !key.trim().is_empty());
    let one_verified_origin = provenance_request
        && origin.is_some()
        && evidence.iter().all(|entry| {
            let state = &entry["evidence_state"];
            state["independence_verified"] == true && state["independence_key"].as_str() == origin
        });
    // A conditional source cannot establish either an actual outcome or its
    // definite absence. Quoting it preserves the condition without generating
    // an unsupported conclusion. This changes reply wording, never promotion.
    let scoped_stance = evidence.iter().all(|entry| {
        let state = &entry["evidence_state"];
        state["polarity"] == "negated"
            || matches!(state["modality"].as_str(), Some("conditional" | "possible"))
    });
    if !temporal && !one_verified_origin && !scoped_stance {
        return None;
    }
    let mut reports: Vec<(&str, Vec<&str>)> = Vec::new();
    for entry in evidence {
        let passage = entry["passage"].as_str()?.trim();
        let source = entry["source"].as_str()?.trim();
        if passage.is_empty()
            || source.is_empty()
            || passage.chars().count() > 500
            || crate::scholar::passages::is_site_chrome(passage)
        {
            return None;
        }
        // A shortened quote must not silently discard an explicit scope or
        // attribution carried by the stored finding. Use full review otherwise.
        for field in ["context_scope", "geographic_scope", "attribution"] {
            if entry["evidence_state"][field]
                .as_str()
                .is_some_and(|scope| !scope.trim().is_empty() && !passage.contains(scope))
            {
                return None;
            }
        }
        if let Some((_, sources)) = reports
            .iter_mut()
            .find(|(existing, _)| *existing == passage)
        {
            if !sources.contains(&source) {
                sources.push(source);
            }
        } else {
            reports.push((passage, vec![source]));
        }
    }
    let mut paragraphs = reports
        .iter()
        .map(|(passage, sources)| {
            format!(
                "The source reports: “{passage}” (Sources: {})",
                sources.join(", ")
            )
        })
        .collect::<Vec<_>>();
    if one_verified_origin {
        paragraphs.push("The supplied evidence represents one reporting source; it does not establish independent corroboration.".into());
    }
    if temporal {
        paragraphs.push("I haven't established whether this is still current.".into());
    }
    Some(LimitedAnswer {
        text: paragraphs.join("\n\n"),
        kind: match (temporal, one_verified_origin) {
            (true, true) => "temporal_and_shared_origin",
            (true, false) => "temporal",
            (false, true) => "shared_origin",
            (false, false) => "source_stance",
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn negated_source_facts_remain_useful_without_an_invented_explanation() {
        let passage = "The laboratory did not demonstrate superconductivity; its experiment measured magnetic susceptibility.";
        let evidence = vec![
            json!({"source":"https://example.org/experiment","passage":passage,
            "evidence_state":{"polarity":"negated","modality":"factual"}}),
        ];
        let answer = source_limited_answer(
            "Did the experiment demonstrate superconductivity?",
            &evidence,
        )
        .unwrap();
        assert!(answer.text.contains(passage));
        assert!(!answer.text.contains("because"));
        assert!(!answer.text.contains("still current"));
    }

    #[test]
    fn a_literal_reply_cannot_omit_a_material_scope() {
        let evidence = vec![
            json!({"source":"https://example.org/report","passage":"Helios Laboratory opened in 2020.",
            "evidence_state":{"gate_status":"unverified_temporal","context_scope":"Earth One"}}),
        ];
        assert!(
            source_limited_answer(
                "When did it open, and is that still its status today?",
                &evidence
            )
            .is_none()
        );
    }

    #[test]
    fn shared_verified_origin_deduplicates_quotes_and_retains_all_citations() {
        let passage =
            "Helios Laboratory completed its first flight test in 2020, according to Sciencewire.";
        let evidence = vec![
            json!({"source":"https://one.example/report","passage":passage,"evidence_state":{
                "gate_status":"unverified_temporal","independence_key":"wire","independence_verified":true}}),
            json!({"source":"https://two.example/report","passage":passage,"evidence_state":{
                "gate_status":"unverified_temporal","independence_key":"wire","independence_verified":true}}),
        ];
        let answer =
            source_limited_answer("Is the claim independently corroborated?", &evidence).unwrap();
        assert_eq!(answer.text.matches(passage).count(), 1);
        assert!(answer.text.contains("https://one.example/report"));
        assert!(answer.text.contains("https://two.example/report"));
        assert!(
            answer
                .text
                .contains("does not establish independent corroboration")
        );
        assert!(!answer.text.contains("still current"));
        assert!(!answer.text.contains("wire;"));
    }

    #[test]
    fn unknown_or_distinct_origins_cannot_be_rendered_as_one_verified_origin() {
        for (verified, second_origin) in [(false, "one"), (true, "two")] {
            let evidence = vec![
                json!({"source":"https://one.example/report","passage":"A source assertion.","evidence_state":{
                    "independence_key":"one","independence_verified":verified}}),
                json!({"source":"https://two.example/report","passage":"A source assertion.","evidence_state":{
                    "independence_key":second_origin,"independence_verified":verified}}),
            ];
            assert!(
                source_limited_answer("Is this independently corroborated?", &evidence).is_none()
            );
        }
    }
}

#[cfg(test)]
pub(super) fn source_limited_answer(question: &str, evidence: &[Value]) -> Option<LimitedAnswer> {
    source_limited_answer_with_scope(question, evidence, None)
}
