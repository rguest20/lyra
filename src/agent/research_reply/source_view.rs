//! Model-facing quotations and material source-use restrictions.
use crate::storage::ScholarEvidenceForAnswer;
use serde_json::{Value, json};

pub(super) const SOURCE_SCOPE_FIELDS: [(&str, &str); 3] = [
    ("attribution", "Preserve this attribution when relevant"),
    (
        "geographic_scope",
        "Keep the claim within this geographic scope",
    ),
    ("context_scope", "Keep the claim within this context"),
];

pub(super) struct SourceUseEvidence<'a> {
    pub source: Option<&'a str>,
    pub passage: Option<&'a str>,
    pub finding_status: Option<&'a str>,
    pub gate_status: Option<&'a str>,
    pub attribution: Option<&'a str>,
    pub geographic_scope: Option<&'a str>,
    pub context_scope: Option<&'a str>,
    pub research_relevance: Option<&'a str>,
    pub topic_class: Option<&'a str>,
    pub freshness: Option<&'a str>,
    pub currency: Option<&'a str>,
    pub independence_key: Option<&'a str>,
    pub independence_verified: Option<bool>,
    pub published_at: Option<&'a str>,
    pub polarity: Option<&'a str>,
    pub modality: Option<&'a str>,
    pub conflicting_claims: Vec<&'a str>,
}

impl<'a> SourceUseEvidence<'a> {
    pub(super) fn from_finding(finding: &'a ScholarEvidenceForAnswer) -> Self {
        Self {
            source: Some(&finding.source_url),
            passage: Some(&finding.evidence_text),
            finding_status: Some(&finding.finding_status),
            gate_status: Some(&finding.gate_status),
            attribution: finding.proposition.attribution.as_deref(),
            geographic_scope: finding.proposition.geographic_scope.as_deref(),
            context_scope: finding.proposition.context_scope.as_deref(),
            research_relevance: Some(&finding.research_relevance),
            topic_class: Some(&finding.topic_class),
            freshness: Some(&finding.freshness),
            currency: Some(&finding.currency),
            independence_key: finding.independence_key.as_deref(),
            independence_verified: Some(finding.independence_verified),
            published_at: finding.published_at.as_deref(),
            polarity: Some(finding.proposition.polarity.as_str()),
            modality: Some(finding.proposition.modality.as_str()),
            conflicting_claims: finding
                .conflicting_claims
                .iter()
                .map(String::as_str)
                .collect(),
        }
    }

    #[cfg(test)]
    pub(super) fn from_json(entry: &'a Value) -> Self {
        let state = &entry["evidence_state"];
        Self {
            source: entry["source"].as_str(),
            passage: entry["passage"].as_str(),
            finding_status: state["finding_status"].as_str(),
            gate_status: state["gate_status"].as_str(),
            attribution: state["attribution"].as_str(),
            geographic_scope: state["geographic_scope"].as_str(),
            context_scope: state["context_scope"].as_str(),
            research_relevance: state["research_relevance"].as_str(),
            topic_class: state["topic_class"].as_str(),
            freshness: state["freshness"].as_str(),
            currency: state["currency"].as_str(),
            independence_key: state["independence_key"].as_str(),
            independence_verified: state["independence_verified"].as_bool(),
            published_at: state["published_at"].as_str(),
            polarity: state["polarity"].as_str(),
            modality: state["modality"].as_str(),
            conflicting_claims: state["conflicting_claims"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect(),
        }
    }

    pub(super) fn has_temporal_limit(&self) -> bool {
        self.research_relevance == Some("historical_context")
            || self.gate_status == Some("unverified_temporal")
            || self.freshness == Some("stale")
            || (self.topic_class == Some("recent") && self.currency != Some("current"))
    }

    pub(super) fn has_disagreements(&self) -> bool {
        self.gate_status == Some("conflict") || !self.conflicting_claims.is_empty()
    }

    pub(super) fn verified_origin(&self) -> Option<&'a str> {
        self.independence_key
            .filter(|key| self.independence_verified == Some(true) && !key.trim().is_empty())
    }

    pub(super) fn scoped_value(&self, field: &str) -> Option<&'a str> {
        match field {
            "attribution" => self.attribution,
            "geographic_scope" => self.geographic_scope,
            "context_scope" => self.context_scope,
            _ => None,
        }
    }
}

#[cfg(test)]
pub(super) fn model_answer_evidence(
    question: &str,
    evidence: &[Value],
    currentness: Option<bool>,
) -> Vec<Value> {
    let evidence = evidence
        .iter()
        .map(SourceUseEvidence::from_json)
        .collect::<Vec<_>>();
    model_answer_source_use(
        &evidence,
        currentness,
        crate::research::source_independence_request(question),
    )
}

pub(super) fn model_answer_source_use(
    evidence: &[SourceUseEvidence<'_>],
    currentness: Option<bool>,
    include_source_relationships: bool,
) -> Vec<Value> {
    evidence
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let mut source_guidance: Vec<String> = Vec::new();
            if entry.finding_status != Some("promoted")
                || entry.gate_status != Some("eligible")
            {
                source_guidance.push(
                    "Attribute the statement to this source.".into(),
                );
            }
            if currentness != Some(false) && entry.has_temporal_limit() {
                source_guidance.push(
                    "Keep this information dated; it does not establish present status.".into(),
                );
            }
            let disagreements = &entry.conflicting_claims;
            if entry.has_disagreements() {
                source_guidance.push(
                    "Report this account alongside conflicting accounts; do not choose a winner unless the evidence does.".into(),
                );
            }
            let mut same_origin = Vec::new();
            let mut distinct_origins = Vec::new();
            if let Some(origin) = entry.verified_origin() {
                for (other_index, other) in evidence.iter().enumerate() {
                    if other_index == index {
                        continue;
                    }
                    if let Some(other_origin) = other.verified_origin() {
                        if origin == other_origin {
                            same_origin.push(format!("source_{}", other_index + 1));
                        } else {
                            distinct_origins.push(format!("source_{}", other_index + 1));
                        }
                    }
                }
            } else if include_source_relationships {
                source_guidance
                    .push("Independence from other reports has not been verified.".into());
            }
            for (field, qualification) in SOURCE_SCOPE_FIELDS {
                if let Some(value) = entry
                    .scoped_value(field)
                    .filter(|value| !value.trim().is_empty())
                {
                    source_guidance.push(format!("{qualification}: {value}."));
                }
            }
            let mut view = json!({
                "source": format!("source_{}", index + 1),
                "passage": entry.passage,
                "source_guidance": source_guidance,
                "attribution": entry.attribution,
                "geographic_scope": entry.geographic_scope,
                "context_scope": entry.context_scope,
                "source_relationships": {
                    "same_origin_as": same_origin,
                    "distinct_verified_origins": distinct_origins,
                },
                "source_date": entry.published_at,
                "possible_disagreements": disagreements,
            });
            view.as_object_mut().unwrap().retain(|field, _| match field.as_str() {
                "possible_disagreements" => !disagreements.is_empty(),
                "source_relationships" => {
                    include_source_relationships
                        && (!same_origin.is_empty() || !distinct_origins.is_empty())
                }
                "source_date" => entry.published_at.is_some(),
                _ => true,
            });
            view
        })
        .collect()
}
