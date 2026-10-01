//! Selects one unresolved public knowledge gap for Scholar to investigate.

use std::collections::{BTreeMap, HashSet};
use std::io;
use std::path::Path;

use crate::knowledge::KnowledgeMap;
use crate::storage::{LearningStore, StoredAssistantClaim};

#[derive(Debug, PartialEq, Eq)]
struct Gap {
    kind: &'static str,
    key: String,
    reference: String,
    question: String,
}

pub fn process_once(database_path: &Path, knowledge_path: &Path) -> io::Result<usize> {
    let mut store = LearningStore::open(database_path)?;
    let map = KnowledgeMap::load(knowledge_path)?;
    let db_claims = store.active_semantic_claims()?;
    let active_refs: HashSet<String> = map
        .entries()
        .iter()
        .filter(|claim| claim.retracted_by.is_none())
        .map(|claim| format!("json:{}", claim.id))
        .chain(
            db_claims
                .iter()
                .map(|detail| format!("db:{}", detail.claim.id)),
        )
        .collect();
    let assistant_claims = store.assistant_claims(500)?;
    let gaps = select_gaps(&assistant_claims, &map, &db_claims, &active_refs);
    for gap in gaps {
        if store.scout_topic_exists(gap.kind, &gap.key)? {
            continue;
        }
        return Ok(usize::from(store.enqueue_scout_research(
            gap.kind,
            &gap.key,
            &gap.reference,
            &gap.question,
        )?));
    }
    Ok(0)
}

fn select_gaps(
    assistant_claims: &[StoredAssistantClaim],
    map: &KnowledgeMap,
    db_claims: &[crate::storage::SemanticClaimDetail],
    active_refs: &HashSet<String>,
) -> Vec<Gap> {
    let mut gaps = Vec::new();
    for verdict in ["challenged", "inconclusive"] {
        for claim in assistant_claims
            .iter()
            .filter(|claim| claim.verdict == verdict)
        {
            if !public_topic(&claim.subject, &claim.relation) {
                continue;
            }
            // A compatible map claim already supplies this fact. A retracted
            // comparison is no longer a reason to investigate a challenge.
            if verdict == "inconclusive" && !claim.evidence_refs.is_empty()
                || verdict == "challenged"
                    && !claim
                        .evidence_refs
                        .iter()
                        .any(|id| active_refs.contains(id))
            {
                continue;
            }
            let key = format!(
                "{}|{}",
                canonical(&claim.subject),
                canonical(&claim.relation)
            );
            gaps.push(Gap {
                kind: if verdict == "challenged" {
                    "challenged"
                } else {
                    "uncertain"
                },
                key,
                reference: format!("assistant_claim:{}", claim.id),
                question: format!(
                    "What is the {} of {}?",
                    claim.relation.trim(),
                    claim.subject.trim()
                ),
            });
        }
    }

    let mut subjects: BTreeMap<String, (String, HashSet<String>, String)> = BTreeMap::new();
    for claim in map
        .entries()
        .iter()
        .filter(|claim| claim.retracted_by.is_none())
    {
        if public_topic(&claim.subject, &claim.relation) {
            let item = subjects
                .entry(canonical(&claim.subject))
                .or_insert_with(|| {
                    (
                        claim.subject.clone(),
                        HashSet::new(),
                        format!("json:{}", claim.id),
                    )
                });
            item.1.insert(canonical(&claim.relation));
        }
    }
    for detail in db_claims {
        let claim = &detail.claim;
        if public_topic(&claim.subject, &claim.relation) {
            let item = subjects
                .entry(canonical(&claim.subject))
                .or_insert_with(|| {
                    (
                        claim.subject.clone(),
                        HashSet::new(),
                        format!("db:{}", claim.id),
                    )
                });
            item.1.insert(canonical(&claim.relation));
        }
    }
    for (key, (subject, relations, reference)) in subjects {
        if relations.len() == 1 {
            gaps.push(Gap {
                kind: "sparse",
                key,
                reference,
                question: format!("What are the key facts about {subject}?"),
            });
        }
    }
    gaps
}

fn canonical(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn public_topic(subject: &str, relation: &str) -> bool {
    let subject = canonical(subject);
    let relation = canonical(relation);
    !subject.is_empty()
        && !relation.is_empty()
        && subject.chars().count() <= 100
        && relation.chars().count() <= 100
        && !subject.contains(['\n', '\r', '<', '>'])
        && !relation.contains(['\n', '\r', '<', '>'])
        && !matches!(
            subject.as_str(),
            "user" | "the user" | "i" | "me" | "you" | "lyra" | "the assistant"
        )
        && ![
            "lives in", "is named", "name", "prefers", "owns", "address", "email", "phone",
        ]
        .iter()
        .any(|private| relation.contains(private))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{SemanticClaim, SemanticClaimDetail};

    fn assistant(id: i64, subject: &str, verdict: &str, refs: &[&str]) -> StoredAssistantClaim {
        StoredAssistantClaim {
            id,
            reply_id: "r".into(),
            subject: subject.into(),
            relation: "height".into(),
            object: "330m".into(),
            evidence_text: "reply".into(),
            verdict: verdict.into(),
            reason: String::new(),
            evidence_refs: refs.iter().map(|v| (*v).into()).collect(),
            confidence_before: 0.5,
            confidence_after: 0.1,
        }
    }

    #[test]
    fn challenged_gap_comes_first_and_retracted_evidence_is_ignored() {
        let claims = [
            assistant(1, "Old building", "challenged", &["db:90"]),
            assistant(2, "Hagia Sophia", "challenged", &["db:1"]),
            assistant(3, "Tower", "inconclusive", &[]),
            assistant(4, "the user", "inconclusive", &[]),
        ];
        let active = HashSet::from(["db:1".into()]);
        let gaps = select_gaps(&claims, &KnowledgeMap::default(), &[], &active);
        assert_eq!(gaps.len(), 2);
        assert_eq!(gaps[0].kind, "challenged");
        assert_eq!(gaps[0].reference, "assistant_claim:2");
        assert_eq!(gaps[1].kind, "uncertain");
    }

    #[test]
    fn one_relation_is_sparse_but_two_are_not() {
        let db_claims = [
            SemanticClaimDetail {
                claim: SemanticClaim {
                    id: 1,
                    subject: "Hagia Sophia".into(),
                    relation: "location".into(),
                    object: "Istanbul".into(),
                    source: "source".into(),
                    source_observation_id: None,
                },
                sources: vec![],
            },
            SemanticClaimDetail {
                claim: SemanticClaim {
                    id: 2,
                    subject: "Cricket".into(),
                    relation: "is".into(),
                    object: "sport".into(),
                    source: "source".into(),
                    source_observation_id: None,
                },
                sources: vec![],
            },
            SemanticClaimDetail {
                claim: SemanticClaim {
                    id: 3,
                    subject: "Cricket".into(),
                    relation: "played with".into(),
                    object: "bat".into(),
                    source: "source".into(),
                    source_observation_id: None,
                },
                sources: vec![],
            },
        ];
        let gaps = select_gaps(&[], &KnowledgeMap::default(), &db_claims, &HashSet::new());
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].key, "hagia sophia");
        assert_eq!(gaps[0].reference, "db:1");
    }
}
