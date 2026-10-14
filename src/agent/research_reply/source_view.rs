//! Model-facing quotations and material source-use restrictions.
use serde_json::{Value, json};

pub(super) fn model_answer_evidence(
    question: &str,
    evidence: &[Value],
    currentness: Option<bool>,
) -> Vec<Value> {
    evidence
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let state = &entry["evidence_state"];
            let mut limits = Vec::new();
            if state["finding_status"] != "promoted" || state["gate_status"] != "eligible" {
                limits.push("Attribute this assertion to its source.".to_owned());
            }
            if currentness != Some(false) && (state["research_relevance"] == "historical_context"
                || state["gate_status"] == "unverified_temporal"
                || state["freshness"] == "stale"
                || (state["topic_class"] == "recent" && state["currency"] != "current"))
            {
                limits.push("The evidence does not establish current status. Preserve any date in the passage and qualify historical examples.".into());
            }
            let disagreements = state["conflicting_claims"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if state["gate_status"] == "conflict" || !disagreements.is_empty() {
                limits.push("An unresolved disagreement may affect this assertion. Explain the different accounts and any scope uncertainty; do not choose one as settled.".into());
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
                limits.push("Independence from other passages has not been verified.".into());
            }
            for (field, description) in [
                ("attribution", "Attribution stated in the source"),
                ("geographic_scope", "Explicit geographic scope"),
                ("context_scope", "Explicit contextual scope"),
            ] {
                if let Some(value) = state[field].as_str().filter(|s|!s.trim().is_empty()) {
                    limits.push(format!("{description}: {value}. Preserve this qualification."));
                }
            }
            let mut view = json!({
                "source": format!("source_{}", index + 1),
                "passage": entry.get("passage"),
                "limits": limits,
                "source_relationships": {
                    "same_origin_as": same_origin,
                    "distinct_verified_origins": distinct_origins,
                },
                "source_date": state.get("published_at"),
                "possible_disagreements": disagreements,
            });
            if disagreements.is_empty() {
                view.as_object_mut().unwrap().remove("possible_disagreements");
            }
            if same_origin.is_empty() && distinct_origins.is_empty() {
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
