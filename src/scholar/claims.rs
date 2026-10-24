//! Grounding checks and deterministic claim extraction from source passages.

use super::*;

pub(super) fn calibrate_research_relevance(
    question: &str,
    finding: &ScholarFinding,
    model_relevance: &str,
) -> String {
    if matches!(model_relevance, "direct" | "related" | "tangential")
        && finding
            .semantic_support
            .as_ref()
            .is_some_and(|support| support.supports(finding, &finding.evidence_text))
    {
        return model_relevance.to_owned();
    }
    let question_words = crate::storage::relevant_words(question);
    let finding_words = crate::storage::relevant_words(&format!(
        "{} {} {}",
        finding.subject, finding.relation, finding.object
    ));
    let coverage = if question_words.is_empty() {
        0
    } else {
        crate::storage::lexical_overlap(&question_words, &finding_words) * 100
            / question_words.len()
    };
    let lexical_ceiling = if coverage >= 60 {
        "direct"
    } else if coverage >= 25 {
        "related"
    } else {
        "tangential"
    };
    let model_level = match model_relevance {
        "direct" => 2,
        "related" => 1,
        "tangential" => 0,
        _ => match lexical_ceiling {
            "direct" => 2,
            "related" => 1,
            _ => 0,
        },
    };
    let lexical_level = match lexical_ceiling {
        "direct" => 2,
        "related" => 1,
        _ => 0,
    };
    match model_level.min(lexical_level) {
        2 => "direct",
        1 => "related",
        _ => "tangential",
    }
    .to_owned()
}

pub(super) fn select_finding(
    extracted: Option<ScholarFinding>,
    expansion: bool,
) -> Option<ScholarFinding> {
    extracted.filter(|claim| !expansion || claim.relation != "has encyclopedia summary")
}

#[cfg(test)]
pub(super) fn claim_is_grounded(
    subject: &str,
    relation: &str,
    object: &str,
    evidence: &str,
    excerpt: &str,
) -> bool {
    claim_grounding_failure(subject, relation, object, evidence, excerpt).is_none()
}

pub(super) fn claim_grounding_failure(
    subject: &str,
    relation: &str,
    object: &str,
    evidence: &str,
    excerpt: &str,
) -> Option<&'static str> {
    if evidence.trim().is_empty()
        || evidence.chars().count() > 4_500
        || !excerpt.contains(evidence)
        || evidence
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
    {
        return Some(
            "evidence span is empty, oversized, absent from the passage, or contains control characters",
        );
    }

    let content = evidence
        .lines()
        .filter(|line| {
            let line = line.trim();
            !line.is_empty() && !line.starts_with('#') && !line.starts_with("Section:")
        })
        .collect::<Vec<_>>()
        .join(" ");
    let evidence_words = crate::storage::relevant_words(&content);
    let subject_words = crate::storage::relevant_words(subject);
    let object_words = crate::storage::relevant_words(object);
    if content.trim().is_empty() || subject_words.is_empty() || object_words.is_empty() {
        return Some("evidence or claim fields contain no usable factual terms");
    }

    let covered = |terms: &std::collections::BTreeSet<String>| {
        crate::storage::lexical_overlap(terms, &evidence_words) * 100 / terms.len()
    };
    let new_object_detail = object_words
        .difference(&subject_words)
        .collect::<std::collections::BTreeSet<_>>();
    if covered(&subject_words) < 75 {
        return Some("subject terms are not sufficiently present in the evidence span");
    }
    if covered(&object_words) < 75 {
        return Some("object terms are not sufficiently present in the evidence span");
    }
    if new_object_detail.is_empty() {
        return Some("object adds no factual detail beyond the subject");
    }
    if matches!(
        relation.trim().to_ascii_lowercase().as_str(),
        "has encyclopedia summary" | "has encyclopedia detail"
    ) {
        return Some("relation is a generic encyclopedia summary/detail, not a supported fact");
    }
    None
}
