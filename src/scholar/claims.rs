//! Grounding checks and deterministic claim extraction from source passages.

use super::*;

pub(super) fn calibrate_research_relevance(
    question: &str,
    finding: &ScholarFinding,
    model_relevance: &str,
) -> String {
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

pub(super) fn structured_list_finding(question: &str, passage: &str) -> Option<ScholarFinding> {
    let query_words = crate::storage::relevant_words(question);
    let lines = passage.lines().collect::<Vec<_>>();
    for (heading_index, line) in lines.iter().enumerate() {
        let heading = line.trim().trim_start_matches('#').trim();
        if !line.trim_start().starts_with('#') {
            continue;
        }
        let tokens = heading.split_whitespace().collect::<Vec<_>>();
        if tokens.len() < 2 {
            continue;
        }
        let category = tokens
            .last()?
            .trim_matches(|character: char| !character.is_alphanumeric());
        let category_words = crate::storage::relevant_words(category);
        if crate::storage::lexical_overlap(&query_words, &category_words) == 0 {
            continue;
        }
        let subject = tokens[..tokens.len() - 1].join(" ");
        let subject_words = crate::storage::relevant_words(&subject);
        if subject_words.is_empty()
            || crate::storage::lexical_overlap(&query_words, &subject_words) == 0
        {
            continue;
        }
        let mut items = Vec::new();
        let mut last_item = None;
        for (offset, candidate_line) in lines.iter().enumerate().skip(heading_index + 1) {
            let candidate_line = candidate_line.trim();
            let Some(item) = candidate_line
                .strip_prefix("* ")
                .or_else(|| candidate_line.strip_prefix("- "))
                .or_else(|| candidate_line.strip_prefix("• "))
            else {
                if !items.is_empty() || offset > heading_index + 2 {
                    break;
                }
                continue;
            };
            let label = item.split_once(':').map_or(item, |(label, _)| label);
            let label = label.trim_matches(|character: char| {
                character.is_whitespace() || matches!(character, '*' | '_')
            });
            if !(3..=100).contains(&label.chars().count()) || !passage.contains(label) {
                break;
            }
            items.push(label.to_owned());
            last_item = Some(offset);
            if items.len() >= 8 {
                break;
            }
        }
        if items.len() < 2 {
            continue;
        }
        let evidence_text = lines[heading_index..=last_item?].join("\n");
        let finding = ScholarFinding {
            subject,
            relation: format!("lists {}", category.to_ascii_lowercase()),
            object: items.join("; "),
            evidence_text,
            confidence: 0.8,
            valid_from: None,
            valid_until: None,
            source_quality: "unknown".into(),
            source_updated_at: None,
        };
        if finding.object.chars().count() <= 500
            && finding.evidence_text.chars().count() <= 2_200
            && crate::storage::evidence_supports_claim(&finding)
        {
            return Some(finding);
        }
    }
    None
}

pub(super) fn explicit_list_finding(
    question: &str,
    page_title: &str,
    excerpt: &str,
    source_updated_at: Option<String>,
) -> Option<ScholarFinding> {
    const LIST_TOPICS: &[&str] = &[
        "ingredient",
        "component",
        "material",
        "cause",
        "symptom",
        "step",
        "feature",
        "example",
        "type",
    ];
    let query_words = crate::storage::relevant_words(question);
    let topic = LIST_TOPICS.iter().find(|topic| {
        query_words
            .iter()
            .any(|word| word == **topic || word.starts_with(**topic))
    })?;
    let lines = excerpt.lines().collect::<Vec<_>>();
    let section_subject = lines
        .iter()
        .filter_map(|line| line.trim().strip_prefix("Section:"))
        .map(str::trim)
        .find(|title| !title.is_empty())
        .unwrap_or(page_title);
    for (index, line) in lines.iter().enumerate() {
        let introducer = line.trim();
        let introducer_words = crate::storage::relevant_words(introducer);
        if !introducer.ends_with(':')
            || !introducer_words
                .iter()
                .any(|word| word == *topic || word.starts_with(*topic))
        {
            continue;
        }
        let mut items = Vec::new();
        let mut last_item_index = index;
        for (item_index, item_line) in lines.iter().enumerate().skip(index + 1) {
            let item_line = item_line.trim();
            if item_line.is_empty() {
                if !items.is_empty() {
                    break;
                }
                continue;
            }
            let Some(item) = item_line
                .strip_prefix("* ")
                .or_else(|| item_line.strip_prefix("- "))
                .or_else(|| item_line.strip_prefix("• "))
            else {
                break;
            };
            let item = item.trim();
            if item.is_empty() || item.chars().count() > 180 || items.len() >= 12 {
                return None;
            }
            items.push((item_line.to_owned(), item.to_owned()));
            last_item_index = item_index;
        }
        if items.len() < 2 {
            continue;
        }
        let section_start = lines[..=index]
            .iter()
            .rposition(|line| line.trim().starts_with("Section:"))
            .unwrap_or(index);
        let evidence_text = lines[section_start..=last_item_index].join("\n");
        let object = items
            .into_iter()
            .map(|(_, item)| item)
            .collect::<Vec<_>>()
            .join("; ");
        let relation = format!("lists {topic}s");
        if !claim_is_grounded(section_subject, &relation, &object, &evidence_text, excerpt) {
            continue;
        }
        return Some(ScholarFinding {
            subject: section_subject.to_owned(),
            relation,
            object,
            evidence_text,
            confidence: 0.99,
            valid_from: None,
            valid_until: None,
            source_quality: "reference".into(),
            source_updated_at,
        });
    }
    None
}

pub(super) fn claim_is_grounded(
    subject: &str,
    relation: &str,
    object: &str,
    evidence: &str,
    excerpt: &str,
) -> bool {
    if evidence.trim().is_empty()
        || evidence.chars().count() > 4_500
        || !excerpt.contains(evidence)
        || evidence
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
    {
        return false;
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
        return false;
    }

    let covered = |terms: &std::collections::BTreeSet<String>| {
        crate::storage::lexical_overlap(terms, &evidence_words) * 100 / terms.len()
    };
    let new_object_detail = object_words
        .difference(&subject_words)
        .collect::<std::collections::BTreeSet<_>>();
    covered(&subject_words) >= 75
        && covered(&object_words) >= 75
        && !new_object_detail.is_empty()
        && !matches!(
            relation.trim().to_ascii_lowercase().as_str(),
            "has encyclopedia summary" | "has encyclopedia detail"
        )
}
