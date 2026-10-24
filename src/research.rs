//! Model-assisted lookup intent and evidence selection, independent of wording.
use crate::errors::invalid_data as invalid;
use crate::{agent::Message, storage::ScholarEvidenceForAnswer};
use serde::{Deserialize, Serialize};
use std::io;

mod json;
pub(crate) use json::{
    DecisionRationale, deserialize_json_object, json_object, json_object_as, json_object_slice,
    json_object_with_delimiter_recovery,
};
mod request;
pub(crate) use request::{
    RequestIntent, property_request_with, request_context, request_intent_with,
};
mod currentness;
pub(crate) use currentness::currentness_request_with;
mod selection;
pub(crate) use selection::select_evidence_with_context;
pub use selection::select_sections_with;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ResearchPlan {
    pub question: String,
    pub queries: Vec<String>,
}

/// Narrow source-assessment cues, shared by selection and reply policy.
/// These never establish the truth or independence of a source themselves.
pub(crate) fn source_independence_request(question: &str) -> bool {
    let words = question
        .to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    words.iter().any(|word| word.starts_with("corroborat"))
        || (words.iter().any(|word| word.starts_with("independen"))
            && words.iter().any(|word| {
                matches!(
                    word.as_str(),
                    "source"
                        | "sources"
                        | "report"
                        | "reports"
                        | "evidence"
                        | "verified"
                        | "verification"
                )
            }))
}

/// Private-fact guard, separate from semantic intent: asking one's own favourite
/// cannot be answered by replacing it with public product research.
pub(crate) fn private_fact_request(question: &str) -> bool {
    let normalized = question
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let field = [
        "my favourite",
        "my favorite",
        "my name",
        "my address",
        "my birthday",
        "my password",
        "my phone number",
        "my email",
    ]
    .iter()
    .any(|phrase| format!(" {normalized} ").contains(&format!(" {phrase} ")));
    (field
        && [
            "what is my ",
            "what s my ",
            "what are my ",
            "where is my ",
            "which is my ",
            "do you know my ",
            "tell me my ",
            "can you tell me my ",
        ]
        .iter()
        .any(|prefix| normalized.starts_with(prefix)))
        || [
            "where do i live",
            "who am i",
            "what do i prefer",
            "do i prefer",
            "do i like",
            "do i know",
        ]
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
        || (["what ", "which "]
            .iter()
            .any(|prefix| normalized.starts_with(prefix))
            && ["is my favourite", "is my favorite"]
                .iter()
                .any(|phrase| normalized.contains(phrase)))
}

pub fn plan_with(
    question: &str,
    known_evidence: &str,
    mut ask: impl FnMut(&[Message]) -> io::Result<String>,
) -> io::Result<Option<ResearchPlan>> {
    if private_fact_request(question) {
        return Ok(None);
    }
    let system = "/no_think\nPlan a lookup for missing PUBLIC facts needed by the current request (the last message). Prior user questions supply context. Known evidence contains SOURCE PASSAGES only, not a model draft. A best/preference request has factual prerequisites: neutral available choices or properties, never proof of an objective favourite. Research available candidates and relevant properties before making a recommendation. Do not research whether an assistant's invented answer is best. Search terms must describe the user's topic, not an unsupported answer. Preserve requested entities and constraints. For best X for purpose Y, keep purpose Y and research relevant properties, not merely a generic list of X. Skip lookup only when the passages address the concrete request and its constraints, not merely the same topic. Partial context does not fill missing facts needed for the requested comparison, use or recommendation. Private user facts, acknowledgments, corrections without a new factual request, and creative tasks need no lookup. Return ONLY JSON {\"needs_research\":true,\"question\":\"neutral factual prerequisite\",\"queries\":[\"public topic phrase\",\"synonym or alternate title\"]}, at most two queries. Otherwise return {\"needs_research\":false,\"question\":null,\"queries\":[]}.";
    let mut messages = vec![
        Message {
            role: "system".into(),
            content: format!(
                "{system} Explicit named constraints to preserve in the question and a query: {:?}",
                crate::entities::possessive_names(question)
            ),
        },
        Message {
            role: "user".into(),
            content: serde_json::json!({"request":question,"known_evidence":known_evidence})
                .to_string(),
        },
    ];
    for attempt in 0..2 {
        let raw = ask(&messages)?;
        crate::diagnostics::research_decision(
            "conversation.fact_research_assessed",
            serde_json::json!({
                "request":question,
                "attempt":attempt+1,
                "model_output":crate::diagnostics::text_excerpt(&raw, 1200),
            }),
        );
        let parsed = parse_plan(&raw).and_then(|plan| {
            if let Some(plan) = &plan {
                for name in crate::entities::possessive_names(question) {
                    if !crate::entities::mentions_name(&plan.question, &name)
                        || !plan
                            .queries
                            .iter()
                            .any(|q| crate::entities::mentions_name(q, &name))
                    {
                        return Err(invalid(&format!(
                            "lookup must preserve the explicitly requested entity {name}"
                        )));
                    }
                }
            }
            Ok(plan)
        });
        match parsed {
            Ok(plan) => return Ok(plan),
            Err(error) if attempt == 0 => {
                messages.push(Message {
                    role: "assistant".into(),
                    content: raw,
                });
                messages.push(Message {
                    role: "user".into(),
                    content: format!("Your assessment was invalid: {error}. Correct it and return the required JSON object only."),
                });
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!()
}

pub fn parse_plan(raw: &str) -> io::Result<Option<ResearchPlan>> {
    let value = json_object(raw)?;
    match value["needs_research"].as_bool() {
        Some(false) => Ok(None),
        Some(true) => {
            let mut plan: ResearchPlan =
                serde_json::from_value(value).map_err(|_| invalid("lookup plan is incomplete"))?;
            plan.question = plan.question.trim().to_owned();
            for query in &mut plan.queries {
                *query = query.trim().trim_end_matches('?').trim().to_owned();
            }
            if plan.question.trim().is_empty()
                || plan.question.chars().count() > 300
                || plan.question.contains(['\n', '\r'])
                || plan.queries.is_empty()
                || plan.queries.len() > 2
                || plan.queries.iter().any(|q| {
                    q.trim().is_empty() || q.chars().count() > 120 || q.contains(['\n', '\r', '?'])
                })
            {
                return Err(invalid(
                    "lookup plan needs one bounded factual question and one or two short queries",
                ));
            }
            Ok(Some(plan))
        }
        None => Err(invalid("lookup plan omitted needs_research")),
    }
}

#[derive(serde::Deserialize)]
struct RetrievalQueryExpansion {
    queries: Vec<String>,
}

pub fn retrieval_queries_with(
    question: &str,
    mut ask: impl FnMut(&[Message]) -> io::Result<String>,
) -> io::Result<Vec<String>> {
    let messages = [Message {role:"system".into(),content:"/no_think\nGenerate up to three short phrases to retrieve existing evidence for the request, including alternate names, regional terms or spelling variants. Preserve named entities and constraints. These are retrieval leads, not asserted facts. Do not include private information. Return only JSON {\"queries\":[\"entity and requested property\",\"synonym and property\"]}.".into()}, Message {role:"user".into(),content:question.into()}];
    let expansion: RetrievalQueryExpansion = json_object_as(&ask(&messages)?)?;
    if expansion.queries.len() > 3
        || expansion
            .queries
            .iter()
            .any(|query| query.trim().is_empty() || query.chars().count() > 120)
    {
        return Err(invalid("invalid retrieval expansion"));
    }
    Ok(expansion.queries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn select_test_evidence_with_context(
        question: &str,
        candidates: Vec<ScholarEvidenceForAnswer>,
        entity_constraints: &[String],
        aliases: &crate::storage::EntityAliases,
        topic_queries: &[String],
        mut ask: impl FnMut(&[Message]) -> io::Result<String>,
    ) -> io::Result<Vec<ScholarEvidenceForAnswer>> {
        select_evidence_with_context(
            question,
            candidates,
            entity_constraints,
            aliases,
            topic_queries,
            |messages| {
                if messages[0]
                    .content
                    .contains("Extract requested PROPERTY or RELATION words")
                {
                    let data: serde_json::Value =
                        serde_json::from_str(&messages[1].content).unwrap();
                    let request = data["request"].as_str().unwrap();
                    return Ok(serde_json::json!({
                        "property_spans":[request],
                        "constraint_spans":[]
                    })
                    .to_string());
                }
                ask(messages)
            },
        )
    }

    #[test]
    fn conflicting_temporal_assessments_remain_unresolved() {
        let mut calls = 0;
        let classification = currentness_request_with(
            "What population did Willow Bay have in 2010 and in 2020?",
            |messages| {
                calls += 1;
                match calls {
                    1 => {
                        assert!(messages[0].content.contains(
                            "A changing property can be requested at a specified earlier time"
                        ));
                        Ok(r#"{"requires_present_state":true,"requires_latest_knowledge":false,"uncertain":false,"reason":"Population changes, and the user asks for figures from 2010 and 2020."}"#.into())
                    }
                    2 => {
                        assert!(
                            messages[0]
                                .content
                                .contains("Independently reconsider whether answering")
                        );
                        Ok(r#"{"requires_present_state":false,"requires_latest_knowledge":false,"uncertain":false,"reason":"The two specified past years fully define the requested values; no present-day data is needed."}"#.into())
                    }
                    _ => panic!("currentness reconsideration must be bounded"),
                }
            },
        );
        assert_eq!(classification, None);
        assert_eq!(calls, 2);
    }

    #[test]
    fn selected_quotes_restore_source_apostrophes_and_keep_original_text() {
        let passage = "The Orion Nebula is seen in Orion’s sword.";
        let mut candidate = finding(105, passage);
        candidate.subject = "Orion Nebula".into();
        let mut calls = 0;
        let selected = select_test_evidence_with_context("Where is the Orion Nebula seen?", vec![candidate],
            &["Orion Nebula".into()], &crate::storage::EntityAliases::default(), &[], |_| {
                calls += 1;
                Ok(if calls == 1 { serde_json::json!({"selections":[{"id":1,"passage":"The Orion Nebula is seen in Orion's sword.","reason":"Describes its location"}]}).to_string() }
                    else { serde_json::json!({"matches_property":true,"reason":"The location answers where."}).to_string() })
            }).unwrap();
        assert_eq!(selected[0].evidence_text, passage);
        assert_eq!(calls, 2);
    }

    #[test]
    fn minimal_property_decisions_require_a_boolean_and_bounded_reason() {
        for (verdict, expected) in [
            (
                serde_json::json!({"matches_property":true,"reason":"Founding matches the requested initiator relationship."}),
                1,
            ),
            (serde_json::json!({"matches_property":true}), 0),
            (
                serde_json::json!({"matches_property":"true","reason":"Wrong field type."}),
                0,
            ),
            (
                serde_json::json!({"matches_property":false,"reason":"Not the requested property."}),
                0,
            ),
        ] {
            let passage = "Ada Voss founded Meridian Observatory.";
            let mut candidate = finding(102, passage);
            candidate.subject = "Meridian Observatory".into();
            let mut calls = 0;
            let selected = select_test_evidence_with_context("Who initiated Meridian Observatory?",vec![candidate],
                &["Meridian Observatory".into()],&crate::storage::EntityAliases::default(),&[],|_| {
                    calls += 1;
                    Ok(if calls == 1 {serde_json::json!({"selections":[{"id":1,"passage":passage,"reason":"Names the initiator"}]}).to_string()}
                        else {verdict.to_string()})
                }).unwrap();
            assert_eq!(selected.len(), expected, "{verdict}");
        }
    }

    #[test]
    fn lexical_overlap_cannot_approve_the_wrong_property() {
        let passage = "Meridian Observatory discusses who founded other observatories.";
        let mut candidate = finding(101, passage);
        candidate.subject = "Meridian Observatory".into();
        let mut calls = 0;
        let selected = select_test_evidence_with_context("Who founded Meridian Observatory?", vec![candidate],
            &["Meridian Observatory".into()], &crate::storage::EntityAliases::default(), &[], |_| {
                calls += 1;
                Ok(if calls == 1 {serde_json::json!({"selections":[{"id":1,"passage":passage,"reason":"Contains founder words"}]}).to_string()}
                    else {r#"{"matches_property":false,"use":"irrelevant","reason":"Does not identify the requested observatory's founder."}"#.into()})
            }).unwrap();
        assert!(selected.is_empty());
    }

    #[test]
    fn yes_no_retrieval_keeps_evidence_that_negates_the_requested_property() {
        let passage = "Meridian Laboratory did not demonstrate superconductivity.";
        let mut candidate = finding(103, passage);
        candidate.subject = "Meridian Laboratory".into();
        candidate.proposition.polarity = crate::storage::ClaimPolarity::Negated;
        let mut calls = 0;
        let mut relevance_prompt = String::new();
        let selected = select_test_evidence_with_context(
            "Did Meridian Laboratory demonstrate superconductivity?",
            vec![candidate],
            &["Meridian Laboratory".into()],
            &crate::storage::EntityAliases::default(),
            &[],
            |messages| {
                calls += 1;
                if calls == 1 {
                    Ok(serde_json::json!({"selections":[{"id":1,"passage":passage,"reason":"Answers the yes-or-no claim"}]}).to_string())
                } else {
                    relevance_prompt = messages[0].content.clone();
                    Ok(r#"{"matches_property":true,"reason":"The passage addresses the same property with negative polarity."}"#.into())
                }
            },
        )
        .unwrap();

        assert_eq!(selected.len(), 1);
        assert!(relevance_prompt.contains("Polarity changes the answer"));
    }

    #[test]
    fn identical_verified_origin_companions_survive_asymmetric_model_selection() {
        let passage =
            "Helios Laboratory completed its first flight test in 2020, according to Sciencewire.";
        let mut first = finding(95, passage);
        first.subject = "Helios Laboratory".into();
        first.independence_key = Some("science-wire-report".into());
        let mut second = first.clone();
        second.id = 96;
        let mut unrelated = first.clone();
        unrelated.id = 97;
        unrelated.source_span.evidence_text = "Helios Laboratory appointed a new director.".into();
        let mut unknown_origin = second.clone();
        unknown_origin.id = 98;
        unknown_origin.independence_verified = false;
        let mut different_subject = second.clone();
        different_subject.id = 99;
        different_subject.subject = "Another Laboratory".into();
        let mut different_scope = second.clone();
        different_scope.id = 100;
        different_scope.proposition.context_scope = Some("a different continuity".into());
        let mut calls = 0;
        let selected = select_test_evidence_with_context("Is the claim independently corroborated?",
            vec![first, second, unrelated, unknown_origin, different_subject, different_scope], &["Helios Laboratory".into()],
            &crate::storage::EntityAliases::default(), &[], |_| {
                calls += 1;
                Ok(if calls == 1 {serde_json::json!({"selections":[{"id":1,"passage":passage,"reason":"Assess the reported origin"}]}).to_string()}
                    else {r#"{"matches_property":true,"use":"provenance","request_span":"independently corroborated","reason":"Compare known source origins"}"#.into()})
            }).unwrap();
        assert_eq!(
            selected
                .iter()
                .map(|finding| finding.id)
                .collect::<Vec<_>>(),
            vec![95, 96]
        );
        assert_eq!(selected[0].evidence_text, selected[1].evidence_text);
    }

    #[test]
    fn historical_context_requires_both_source_date_and_currentness_limit() {
        let passage = "In 2012, Riverton Museum displayed an astrolabe.";
        for verdict in [
            serde_json::json!({"matches_property":true,"use":"historical_context","request_span":"currently exhibit","time_span":"2012","reason":"Past examples"}),
            serde_json::json!({"matches_property":true,"use":"historical_context","request_span":"currently exhibit","time_span":"2015","limitation":"Not current evidence","reason":"Past examples"}),
            serde_json::json!({"matches_property":true,"use":"historical_context","request_span":"currently exhibit","time_span":"Riverton Museum","limitation":"Not current evidence","reason":"Past examples"}),
        ] {
            let mut candidate = finding(94, passage);
            candidate.subject = "Riverton Museum".into();
            let mut calls = 0;
            let selected = select_test_evidence_with_context("What instruments does Riverton Museum currently exhibit?",
                vec![candidate], &["Riverton Museum".into()], &crate::storage::EntityAliases::default(), &[], |messages| {
                    if messages[1].content.contains("extract_property_request") {
                        return Ok(serde_json::json!({"property_spans":["instruments","exhibit"],"constraint_spans":[]}).to_string());
                    }
                    calls += 1;
                    Ok(if calls == 1 {serde_json::json!({"selections":[{"id":1,"passage":passage,"reason":"Dated context"}]}).to_string()} else {verdict.to_string()})
                }).unwrap();
            assert!(selected.is_empty(), "{verdict}");
        }
    }

    #[test]
    fn grounding_basics_retains_explicitly_limited_historical_context() {
        let passage = "In 2012, Riverton Museum displayed an astrolabe and a reflecting telescope.";
        let mut candidate = finding(93, passage);
        candidate.subject = "Riverton Museum".into();
        let mut calls = 0;
        let selected = select_test_evidence_with_context("What instruments does Riverton Museum currently exhibit?",
            vec![candidate], &["Riverton Museum".into()], &crate::storage::EntityAliases::default(), &[], |messages| {
                    if messages[1].content.contains("extract_property_request") {
                        return Ok(serde_json::json!({"property_spans":["instruments","exhibit"],"constraint_spans":[]}).to_string());
                    }
                calls += 1;
                Ok(if calls == 1 {serde_json::json!({"selections":[{"id":1,"passage":passage,"reason":"Dated context with an explicit currentness limit"}]}).to_string()}
                else {r#"{"matches_property":true,"use":"historical_context","request_span":"currently exhibit","time_span":"2012","limitation":"The 2012 display does not establish today's collection.","reason":"Historical examples help explain what is known without asserting current exhibits."}"#.into()})
            }).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].research_relevance, "historical_context");
    }

    #[test]
    fn semantic_reconsideration_recovers_historical_property_after_false_rejection() {
        let passage = "In 2012, Riverton Museum displayed an astrolabe and a reflecting telescope.";
        let mut candidate = finding(92, passage);
        candidate.subject = "Riverton Museum".into();
        candidate.gate_status = "unverified_temporal".into();
        candidate.topic_class = "recent".into();
        candidate.freshness = "stale".into();
        candidate.currency = "historical".into();
        let mut calls = 0;
        let selected = select_test_evidence_with_context(
            "What instruments does Riverton Museum currently exhibit?",
            vec![candidate],
            &["Riverton Museum".into()],
            &crate::storage::EntityAliases::default(),
            &[],
            |messages| {
                calls += 1;
                match calls {
                    1 => Ok(serde_json::json!({"selections":[{"id":1,"passage":passage,"reason":"States the museum's displayed instruments."}]}).to_string()),
                    2 => Ok(serde_json::json!({"matches_property":false,"reason":"This is old evidence and may not describe the current collection."}).to_string()),
                    3 => {
                        assert!(messages[0].content.contains("Independently review a prior rejection"));
                        Ok(serde_json::json!({
                            "matches_property":false,
                            "use":"historical_context",
                            "request_span":"currently exhibit",
                            "time_span":"2012",
                            "limitation":"The 2012 display does not establish today's collection.",
                            "reason":"This old evidence describes a past display, not current holdings."
                        }).to_string())
                    }
                    4 => {
                        assert!(messages[0].content.contains("conflicts with itself"));
                        Ok(serde_json::json!({
                            "matches_property":false,
                            "reason":"The source does not prove current holdings."
                        }).to_string())
                    }
                    5 => {
                        assert!(messages[0].content.contains("useful, explicitly historical partial answer"));
                        Ok(serde_json::json!({
                            "matches_property":true,
                            "use":"historical_context",
                            "request_span":"currently exhibit",
                            "time_span":"2012",
                            "limitation":"The 2012 display does not establish today's collection.",
                            "reason":"This is a dated instance of the requested display property and can be reported with its date."
                        }).to_string())
                    }
                    _ => panic!("property reconsideration must be bounded"),
                }
            },
        )
        .unwrap();

        assert_eq!(calls, 5);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].research_relevance, "historical_context");
    }

    #[test]
    fn malformed_relevance_json_gets_one_semantic_retry_before_failing_closed() {
        let passage = "In 2012, Riverton Museum displayed an astrolabe.";
        let mut candidate = finding(97, passage);
        candidate.subject = "Riverton Museum".into();
        candidate.gate_status = "unverified_temporal".into();
        let mut calls = 0;
        let selected = select_test_evidence_with_context(
            "What instruments does Riverton Museum currently exhibit?",
            vec![candidate],
            &["Riverton Museum".into()],
            &crate::storage::EntityAliases::default(),
            &[],
            |messages| {
                calls += 1;
                match calls {
                    1 => Ok(serde_json::json!({"selections":[{"id":1,"passage":passage,"reason":"Dated evidence"}]}).to_string()),
                    2 => Ok(r#"{"matches_property":false,"reason":"Current holdings are unknown."}"#.into()),
                    3 => Ok(r#"{"matches_property":true,"use":"historical_context","request_span":"currently exhibit","time_span":"2012","limitation":"Current holdings are unknown.","reason":"A dated display is relevant"#.into()),
                    4 => {
                        assert!(messages.last().unwrap().content.contains("not valid JSON"));
                        Ok(serde_json::json!({
                            "matches_property":true,
                            "use":"historical_context",
                            "request_span":"currently exhibit",
                            "time_span":"2012",
                            "limitation":"The dated display does not establish today's collection.",
                            "reason":"This is a dated instance of the requested property."
                        }).to_string())
                    }
                    _ => panic!("malformed review repair must be bounded to one retry"),
                }
            },
        )
        .unwrap();

        assert_eq!(calls, 4);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].research_relevance, "historical_context");
    }

    #[test]
    fn entity_constraints_reject_cross_entity_transfer_across_domains() {
        for (question, requested, actual, passage) in [
            (
                "Who initiated Meridian Observatory?",
                "Meridian Observatory",
                "Harbour Observatory",
                "Ada Voss founded Harbour Observatory.",
            ),
            (
                "Which books does Northbridge Press publish?",
                "Northbridge Press",
                "Southbridge Press",
                "Southbridge Press publishes War and Peace.",
            ),
            (
                "Which instruments does Riverton Museum exhibit?",
                "Riverton Museum",
                "Hillcrest Museum",
                "Hillcrest Museum exhibits astrolabes.",
            ),
        ] {
            let mut candidate = finding(91, passage);
            candidate.subject = actual.into();
            candidate.object = "source property".into();
            let selected = select_test_evidence_with_context(
                question,
                vec![candidate],
                &[requested.into()],
                &crate::storage::EntityAliases::default(),
                &[],
                |_| {
                    Ok(serde_json::json!({"selections":[{"id":1,"passage":passage,
                        "reason":"Related subject matter"}]})
                    .to_string())
                },
            )
            .unwrap();
            assert!(
                selected.is_empty(),
                "transferred {actual} facts to {requested}"
            );
        }
    }

    #[test]
    fn provenance_evidence_does_not_require_the_passage_to_say_corroborated() {
        let passage = "Orion Foundation opened its first clinic in 2019, according to Newswire.";
        let mut first = finding(71, passage);
        first.subject = "Orion Foundation".into();
        first.object = "first clinic in 2019".into();
        first.independence_key = Some("shared-wire-report".into());
        let mut second = first.clone();
        second.id = 72;
        let mut calls = 0;
        let selected = select_test_evidence_with_context(
            "Is Orion Foundation's claim independently corroborated?", vec![first, second],
            &["Orion Foundation".into()], &crate::storage::EntityAliases::default(), &[],
            |messages| {
                calls += 1;
                if calls == 1 {
                    Ok(serde_json::json!({"selections":[
                        {"id":1,"passage":passage,"reason":"Identify the claim and its declared origin"},
                        {"id":2,"passage":passage,"reason":"Compare the copied report's lineage"}
                    ]}).to_string())
                } else {
                    assert!(messages[1].content.contains("shared-wire-report"));
                    Ok(r#"{"matches_property":true,"use":"provenance","request_span":"independently corroborated","reason":"Lineage identifies whether these are independent sources."}"#.into())
                }
            },
        ).unwrap();
        assert_eq!(
            selected.iter().map(|f| f.id).collect::<Vec<_>>(),
            vec![71, 72]
        );
        assert_eq!(calls, 1);
    }

    #[test]
    fn semantic_relation_review_accepts_synonyms_without_a_keyword_equivalence_table() {
        let mut candidate = finding(81, "Dana founded Orion Workshop.");
        candidate.subject = "Dana".into();
        candidate.object = "Orion Workshop".into();
        let mut calls = 0;
        let selected = select_test_evidence_with_context(
            "Who initiated Orion Workshop?", vec![candidate], &["Orion Workshop".into()],
            &crate::storage::EntityAliases::default(), &[], |messages| {
                calls += 1;
                Ok(if calls == 1 {
                    r#"{"selections":[{"id":1,"passage":"Dana founded Orion Workshop.","reason":"Names the initiator"}]}"#
                } else {
                    assert!(messages[1].content.contains("initiated"));
                    r#"{"matches_property":true,"use":"direct","request_span":"initiated","reason":"Founded establishes who initiated the workshop."}"#
                }.into())
            },
        ).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(calls, 2);
    }

    #[test]
    fn semantic_relation_review_cannot_accept_an_entity_only_or_missing_request_anchor() {
        for verdict in [
            r#"{"matches_property":true,"use":"direct","request_span":"Orion Workshop","reason":"Same entity."}"#,
            r#"{"matches_property":true,"use":"prerequisite","reason":"Same entity."}"#,
            r#"{"matches_property":false,"use":"direct","request_span":"initiated","reason":"Does not name an initiator."}"#,
        ] {
            let mut candidate = finding(82, "Orion Workshop paid its taxes.");
            candidate.subject = "Orion Workshop".into();
            let mut calls = 0;
            let selected = select_test_evidence_with_context(
                "Who initiated Orion Workshop?", vec![candidate], &["Orion Workshop".into()],
                &crate::storage::EntityAliases::default(), &[], |_| {
                    calls += 1;
                    Ok(if calls == 1 {r#"{"selections":[{"id":1,"passage":"Orion Workshop paid its taxes.","reason":"Same workshop"}]}"#} else {verdict}.into())
                },
            ).unwrap();
            assert!(selected.is_empty());
        }
    }

    fn select_unconstrained(
        question: &str,
        candidates: Vec<ScholarEvidenceForAnswer>,
        mut ask: impl FnMut(&[Message]) -> io::Result<String>,
    ) -> io::Result<Vec<ScholarEvidenceForAnswer>> {
        select_test_evidence_with_context(
            question,
            candidates,
            &[],
            &crate::storage::EntityAliases::default(),
            &[],
            |messages| {
                let data: serde_json::Value = serde_json::from_str(&messages[1].content).unwrap();
                if data.get("passage").is_some() {
                    Ok(serde_json::json!({"matches_property":true,"use":"direct","request_span":question,
                        "reason":"Mock approves property for transport and exact-span tests"}).to_string())
                } else {
                    ask(messages)
                }
            },
        )
    }

    fn finding(id: i64, passage: &str) -> ScholarEvidenceForAnswer {
        crate::test_support::answer_evidence_fixture(
            id,
            "snacks",
            "include",
            "flavours",
            "https://example.org/source",
            passage,
        )
    }

    #[test]
    fn persisted_cookie_footer_is_excluded_before_semantic_selection() {
        let footer = finding(34, include_str!("../tests/fixtures/cofresh-footer.txt"));
        let mut useful = finding(
            33,
            "Smith's Crisps are sold in ready salted, salt and vinegar or cheese and onion flavours.",
        );
        useful.gate_status = "awaiting_corroboration".into();
        let selected = select_unconstrained("Which crisp flavours are available?", vec![footer, useful.clone()], |messages| {
            assert!(!messages[1].content.contains("I ACCEPT"));
            assert!(!messages[1].content.contains("Popular Research"));
            assert!(!messages[1].content.contains("extraction_confidence"));
            assert!(!messages[1].content.contains("source_archive_id"));
            assert!(messages[1].content.contains("independence_verified"));
            Ok(serde_json::json!({"selections":[{"id":1,"passage":useful.evidence_text,"reason":"Lists available crisp flavours, not a ranking"}]}).to_string())
        }).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, 33);
        assert_eq!(selected[0].gate_status, "awaiting_corroboration");
        assert_eq!(selected[0].finding_status, "needs_review");
    }

    #[test]
    fn selected_evidence_requires_an_actual_span_and_relevance_rationale() {
        let original = finding(
            1,
            "Unrelated introduction. Walkers sells Ready Salted crisps. Other irrelevant text.",
        );
        let selected = select_unconstrained("What Walkers flavours exist?", vec![original.clone()], |_| Ok(r#"{"selections":[{"id":1,"passage":"Walkers sells Ready Salted crisps.","reason":"Names an available flavour"}]}"#.into())).unwrap();
        assert_eq!(
            selected[0].evidence_text,
            "Walkers sells Ready Salted crisps."
        );
        for raw in [
            r#"{"selections":[{"id":1,"passage":"Walkers sells Vanilla crisps.","reason":"Names a flavour"}]}"#,
            r#"{"selections":[{"id":1,"passage":"Walkers sells Ready Salted crisps."}]}"#,
            r#"{"selections":[{"id":999,"passage":"Walkers sells Ready Salted crisps.","reason":"Names a flavour"}]}"#,
        ] {
            assert!(
                select_unconstrained("What flavours?", vec![original.clone()], |_| Ok(raw.into()))
                    .is_err()
            );
        }
    }

    #[test]
    fn invalid_extra_selection_does_not_discard_valid_evidence() {
        let original = finding(
            57,
            "Walkers sells crisps in a variety of flavours; the main are Cheese and Onion, Salt and Vinegar, and Ready Salted.",
        );
        let selected = select_unconstrained(
            "What is the best Walkers flavour?",
            vec![original],
            |_| {
                Ok(serde_json::json!({"selections":[
                    {"id":1,"passage":"Walkers sells crisps in a variety of flavours; the main are Cheese and Onion, Salt and Vinegar, and Ready Salted.","reason":"Lists flavours for a tentative preference"},
                    {"id":999,"passage":"A fabricated second result.","reason":"Another option"}
                ]}).to_string())
            },
        )
        .unwrap();

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, 57);
    }

    #[test]
    fn brand_specific_lookup_rejects_a_different_brand_as_product_evidence() {
        let smiths = finding(
            33,
            "Smith's Crisps are sold in ready salted, salt and vinegar or cheese and onion flavours.",
        );
        let lays = finding(
            38,
            "Lay's potato chips in Indonesia are available in honey butter, sour cream and onion, nori seaweed, beef barbecue, classic salty, and salmon teriyaki flavors.",
        );
        let selected = select_test_evidence_with_context(
            "What crisp flavours does Walkers currently sell?",
            vec![smiths, lays],
            &["Walkers".into()],
            &crate::storage::EntityAliases::default(),
            &[],
            |_| {
                Ok(serde_json::json!({"selections":[
                    {"id":1,"passage":"Smith's Crisps are sold in ready salted, salt and vinegar or cheese and onion flavours.","reason":"A related UK crisp brand has this flavour list"},
                    {"id":2,"passage":"Lay's potato chips in Indonesia are available in honey butter, sour cream and onion, nori seaweed, beef barbecue, classic salty, and salmon teriyaki flavors.","reason":"Lists crisp flavours from a different brand"}
                ]}).to_string())
            },
        )
        .unwrap();

        assert!(
            selected.is_empty(),
            "another brand's catalogue cannot establish Walkers' product range"
        );
    }

    #[test]
    fn brand_question_does_not_select_a_topically_unrelated_prerequisite() {
        let walkers = finding(
            56,
            "Walkers produced cheese and onion crisps in 1954 and later sold salt and vinegar crisps.",
        );
        let renewable = finding(
            12,
            "Renewable energy is increasingly used in developing countries as an alternative to fossil fuels.",
        );
        let selected = select_test_evidence_with_context(
            "What crisp flavours does Walkers sell?",
            vec![walkers, renewable],
            &[],
            &crate::storage::EntityAliases::default(),
            &[],
            |messages| {
                let data: serde_json::Value = serde_json::from_str(&messages[1].content).unwrap();
                if let Some(passage) = data["passage"].as_str() {
                    return Ok(serde_json::json!({"matches_property":!passage.starts_with("Renewable"),
                        "use":"direct","request_span":"flavours","reason":"Only the product passage describes the requested property"}).to_string());
                }
                Ok(serde_json::json!({"selections":[
                    {"id":1,"passage":"Walkers produced cheese and onion crisps in 1954 and later sold salt and vinegar crisps.","reason":"Names two Walkers flavours"},
                    {"id":2,"passage":"Renewable energy is increasingly used in developing countries as an alternative to fossil fuels.","reason":"Useful background on energy trends"}
                ]}).to_string())
            },
        )
        .unwrap();

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, 56);
        assert!(!selected.iter().any(|evidence| evidence.id == 12));
    }

    #[test]
    fn lookup_repairs_dropped_brand_without_using_a_generated_answer() {
        let mut calls = 0;
        let plan = plan_with("Are you sure about Walkers' Vanilla crisps?", "[]", |messages| {
            calls += 1;
            let context: serde_json::Value = serde_json::from_str(&messages[1].content).unwrap();
            assert_eq!(context["known_evidence"], "[]");
            assert!(context.get("draft").is_none());
            if calls == 1 {
                Ok(r#"{"needs_research":true,"question":"What snack flavours exist?","queries":["snack flavours"]}"#.into())
            } else {
                assert_eq!(messages[2].role, "assistant");
                assert!(messages[3].content.contains("Walkers"));
                Ok(r#"{"needs_research":true,"question":"What flavours of Walkers crisps exist?","queries":["Walkers crisps flavours"]}"#.into())
            }
        }).unwrap().unwrap();
        assert_eq!(calls, 2);
        assert!(plan.question.contains("Walkers"));
    }

    #[test]
    fn personal_preferences_are_not_replaced_by_public_product_research() {
        assert!(
            plan_with(
                "What is my favourite crisp flavour?",
                "I do not know.",
                |_| panic!("a private fact must not become a public lookup")
            )
            .unwrap()
            .is_none()
        );
        assert!(!private_fact_request("What Walkers flavour should I try?"));
        assert!(!private_fact_request(
            "My favourite is Ready Salted. Which Walkers flavours are similar?"
        ));
        assert!(!private_fact_request(
            "What does my namespace configuration mean?"
        ));
    }

    #[test]
    fn section_selection_rejects_unknown_indices_and_accepts_fenced_json() {
        let sections = vec![
            ("3".into(), "Flavours".into()),
            ("4".into(), "References".into()),
        ];
        assert_eq!(
            select_sections_with("What flavors exist?", &sections, |_| Ok(
                "```json\n{\"indices\":[\"3\"]}\n```".into()
            ))
            .unwrap(),
            vec!["3"]
        );
        assert!(
            select_sections_with("What flavors exist?", &sections, |_| Ok(
                r#"{"indices":["99"]}"#.into()
            ))
            .is_err()
        );
        assert_eq!(
            select_sections_with("What flavors exist?", &sections, |_| Ok(
                r#"{"indices":["3","3"]}"#.into()
            ))
            .unwrap(),
            vec!["3"]
        );
        assert!(
            select_sections_with("What flavors exist?", &sections, |_| Ok(
                r#"{"indices":[3]}"#.into()
            ))
            .is_err()
        );
    }
    #[test]
    fn subjective_request_keeps_a_factual_prerequisite_and_alternate_query() {
        let plan = plan_with("What's the best flavour crisps for Walkers", "The sources don't specify.", |messages| {
            assert!(messages[0].content.contains("factual prerequisites"));
            Ok(r#"{"needs_research":true,"question":"Which flavours of Walkers potato crisps are available?","queries":["Walkers crisps","Walkers potato chips"]}"#.into())
        }).unwrap().unwrap();
        assert!(plan.question.contains("Walkers"));
        assert_eq!(plan.queries[1], "Walkers potato chips");
        assert!(
            parse_plan(r#"{"needs_research":false,"question":null,"queries":[]}"#)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            parse_plan(
                r#"{"needs_research":true,"question":"Flavours?","queries":["What flavours?"]}"#
            )
            .unwrap()
            .unwrap()
            .queries,
            vec!["What flavours"]
        );
        assert!(parse_plan(r#"{"needs_research":true,"question":"Reference?","queries":["docs?version=current"]}"#).is_err());
        assert!(parse_plan("NO_RESEARCH").is_err());
    }

    #[test]
    fn retrieval_expansion_keeps_its_bounded_string_contract() {
        let queries = retrieval_queries_with("Where is Meridian Bay?", |_| {
            Ok(r#"{"queries":[" Meridian Bay ","regional name"]}"#.into())
        })
        .unwrap();
        assert_eq!(queries, [" Meridian Bay ", "regional name"]);

        assert!(
            retrieval_queries_with("anything", |_| Ok(r#"{"queries":[]}"#.into()))
                .unwrap()
                .is_empty()
        );
        for response in [
            r#"{"queries":["one","two","three","four"]}"#.to_owned(),
            r#"{"queries":[""]}"#.to_owned(),
            r#"{"queries":[42]}"#.to_owned(),
            format!(r#"{{"queries":["{}"]}}"#, "x".repeat(121)),
        ] {
            assert!(retrieval_queries_with("anything", |_| Ok(response.clone())).is_err());
        }
    }
}
