//! Replies whose assertions follow directly from exact excerpts and verified
//! source-use metadata. This is a bounded fallback, not an intent classifier.
#[cfg(test)]
use serde_json::Value;

pub(super) struct LimitedAnswer {
    pub text: String,
    pub kind: &'static str,
}

#[cfg(test)]
pub(super) fn source_limited_answer_with_scope(
    question: &str,
    evidence: &[Value],
    currentness: Option<bool>,
) -> Option<LimitedAnswer> {
    let evidence = evidence
        .iter()
        .map(super::source_view::SourceUseEvidence::from_json)
        .collect::<Vec<_>>();
    source_limited_answer_with_source_use(
        &evidence,
        currentness,
        crate::research::source_independence_request(question),
    )
}

pub(super) fn source_limited_answer_with_source_use(
    evidence: &[super::source_view::SourceUseEvidence<'_>],
    currentness: Option<bool>,
    provenance_request: bool,
) -> Option<LimitedAnswer> {
    if evidence.is_empty()
        || evidence.len() > 3
        || evidence
            .iter()
            .any(super::source_view::SourceUseEvidence::has_disagreements)
    {
        return None;
    }
    let provenance_request = provenance_request
        || evidence
            .iter()
            .any(|entry| entry.research_relevance == Some("provenance"));
    // Only a resolved contextual assessment can authorize a direct historical
    // answer. If it is unresolved and the evidence is time-scoped, route through
    // the reviewed answer path, which retains the currentness qualification.
    let has_temporal_scope = evidence
        .iter()
        .any(super::source_view::SourceUseEvidence::has_temporal_limit);
    if currentness.is_none() && has_temporal_scope && !provenance_request {
        return None;
    }
    let current_request = currentness == Some(true);
    // The relevance reviewer has already decided this source is useful only
    // as historical context for this request. Preserve that more specific
    // relationship even if the separate currentness classifier misses a cue.
    let historical_context = evidence
        .iter()
        .all(|entry| entry.research_relevance == Some("historical_context"));
    let temporal = (current_request
        && evidence.iter().all(|entry| {
            entry.research_relevance == Some("historical_context")
                || entry.gate_status == Some("unverified_temporal")
        }))
        || historical_context;
    let origin = evidence[0].verified_origin();
    let one_verified_origin = provenance_request
        && origin.is_some()
        && evidence
            .iter()
            .all(|entry| entry.verified_origin() == origin);
    // A conditional source cannot establish either an actual outcome or its
    // definite absence. Quoting it preserves the condition without generating
    // an unsupported conclusion. This changes reply wording, never promotion.
    let scoped_stance = evidence.iter().all(|entry| {
        entry.polarity == Some("negated")
            || matches!(entry.modality, Some("conditional" | "possible"))
    });
    if !temporal && !one_verified_origin && !scoped_stance {
        return None;
    }
    let mut reports: Vec<(&str, Vec<&str>)> = Vec::new();
    for entry in evidence {
        let passage = entry.passage?.trim();
        let source = entry.source?.trim();
        if passage.is_empty()
            || source.is_empty()
            || passage.chars().count() > 500
            || crate::scholar::passages::is_site_chrome(passage)
        {
            return None;
        }
        // A shortened quote must not silently discard an explicit scope or
        // attribution carried by the stored finding. Use full review otherwise.
        for (field, _) in super::source_view::SOURCE_SCOPE_FIELDS {
            if entry
                .scoped_value(field)
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
        .map(|(passage, sources)| format!("“{passage}” (Source: {})", sources.join(", ")))
        .collect::<Vec<_>>();
    if one_verified_origin {
        paragraphs.push("The supplied evidence represents one reporting source; it does not establish independent corroboration.".into());
    }
    if temporal {
        paragraphs.push("The source does not establish whether this remains current.".into());
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
        assert!(answer.text.starts_with('“'));
        assert!(!answer.text.starts_with("The source reports"));
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
        assert!(answer.text.starts_with('“'));
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
