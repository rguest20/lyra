//! Model-facing quotations and material source-use restrictions.
use serde_json::{Value, json};

pub(super) fn model_answer_evidence(
    question: &str,
    evidence: &[Value],
    currentness: Option<bool>,
) -> Vec<Value> {
    let include_source_relationships = crate::research::source_independence_request(question);
    evidence
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let state = &entry["evidence_state"];
            let mut qualifications = Vec::new();
            if state["finding_status"] != "promoted" || state["gate_status"] != "eligible" {
                qualifications.push("source_attribution_required");
            }
            if currentness != Some(false)
                && (state["research_relevance"] == "historical_context"
                    || state["gate_status"] == "unverified_temporal"
                    || state["freshness"] == "stale"
                    || (state["topic_class"] == "recent" && state["currency"] != "current"))
            {
                qualifications.push("current_status_not_established");
            }
            let disagreements = state["conflicting_claims"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if state["gate_status"] == "conflict" || !disagreements.is_empty() {
                qualifications.push("reports_conflict");
            }
            let mut same_origin = Vec::new();
            let mut distinct_origins = Vec::new();
            if let Some(origin) = verified_origin(entry) {
                for (other_index, other) in evidence.iter().enumerate() {
                    if other_index == index {
                        continue;
                    }
                    if let Some(other_origin) = verified_origin(other) {
                        if origin == other_origin {
                            same_origin.push(format!("source_{}", other_index + 1));
                        } else {
                            distinct_origins.push(format!("source_{}", other_index + 1));
                        }
                    }
                }
            } else if crate::research::source_independence_request(question) {
                qualifications.push("independence_unverified");
            }
            for (field, qualification) in [
                ("attribution", "source_attribution"),
                ("geographic_scope", "geographic_scope"),
                ("context_scope", "context_scope"),
            ] {
                if state[field]
                    .as_str()
                    .is_some_and(|value| !value.trim().is_empty())
                {
                    qualifications.push(qualification);
                }
            }
            let mut view = json!({
                "source": format!("source_{}", index + 1),
                "passage": entry.get("passage"),
                "required_qualifications": qualifications,
                "attribution": state.get("attribution"),
                "geographic_scope": state.get("geographic_scope"),
                "context_scope": state.get("context_scope"),
                "source_relationships": {
                    "same_origin_as": same_origin,
                    "distinct_verified_origins": distinct_origins,
                },
                "source_date": state.get("published_at"),
                "possible_disagreements": disagreements,
            });
            if disagreements.is_empty() {
                view.as_object_mut()
                    .unwrap()
                    .remove("possible_disagreements");
            }
            if !include_source_relationships
                || (same_origin.is_empty() && distinct_origins.is_empty())
            {
                view.as_object_mut().unwrap().remove("source_relationships");
            }
            if view["source_date"].is_null() {
                view.as_object_mut().unwrap().remove("source_date");
            }
            view
        })
        .collect()
}

fn verified_origin(value: &Value) -> Option<&str> {
    let state = &value["evidence_state"];
    (state["independence_verified"] == true)
        .then(|| state["independence_key"].as_str())
        .flatten()
        .filter(|key| !key.trim().is_empty())
}
