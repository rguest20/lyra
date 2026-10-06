//! Selects one unresolved public knowledge gap for Scholar to investigate.

use std::collections::{BTreeMap, HashSet};
use std::io;
use std::path::Path;

use crate::inference::LocalModel;
use crate::knowledge::KnowledgeMap;
use crate::storage::{LearningStore, StoredAssistantClaim};

#[derive(Debug, PartialEq, Eq)]
struct Gap {
    kind: &'static str,
    key: String,
    reference: String,
    question: String,
}

enum ScoutRequest<'a> {
    Map {
        subject: &'a str,
        known: &'a [String],
        previous: &'a [String],
    },
    Model {
        previous_topics: &'a [String],
    },
}

pub fn process_once(
    database_path: &Path,
    knowledge_path: &Path,
    model: &mut LocalModel,
) -> io::Result<usize> {
    process_once_with_request(database_path, knowledge_path, |request| match request {
        ScoutRequest::Map {
            subject,
            known,
            previous,
        } => model.scout_question(subject, known, previous),
        ScoutRequest::Model { previous_topics } => model.scout_model_lead(previous_topics),
    })
}

#[cfg(test)]
pub(crate) fn process_once_with(
    database_path: &Path,
    knowledge_path: &Path,
    mut ask: impl FnMut(&str, &[String], &[String]) -> io::Result<String>,
) -> io::Result<usize> {
    process_once_with_request(database_path, knowledge_path, |request| match request {
        ScoutRequest::Map {
            subject,
            known,
            previous,
        } => ask(subject, known, previous),
        ScoutRequest::Model { .. } => Ok(String::new()),
    })
}

fn process_once_with_request(
    database_path: &Path,
    knowledge_path: &Path,
    mut ask: impl FnMut(ScoutRequest<'_>) -> io::Result<String>,
) -> io::Result<usize> {
    let mut store = LearningStore::open(database_path)?;
    let map = KnowledgeMap::load(knowledge_path)?;
    if let Some(task) = store.claim_scholar_feedback("new_source")? {
        crate::diagnostics::event(
            "scout.research_feedback",
            serde_json::json!({
                "feedback_id": task.id, "root_finding_id": task.root_finding_id,
                "rejected_finding_id": task.finding_id, "scholar_job_id": task.job_id,
                "attempt": task.attempt, "max_attempts": 3, "question": task.question,
                "guidance": task.guidance,
                "excluded_source": task.exclusion_reason.as_ref().map(|_| crate::diagnostics::safe_public_url(&task.source_url)),
                "exclusion_reason": task.exclusion_reason,
                "exclusion_scope": "root_finding_retry_chain",
                "excluded_sources": store.scholar_feedback_excluded_sources(&task.job_id)?.iter().map(|url| crate::diagnostics::safe_public_url(url)).collect::<Vec<_>>(),
            }),
        );
        return Ok(1);
    }
    // Advance background coverage on every pass, including passes where an
    // urgent challenged or due claim is handled first.
    let review_batch = store.next_claims_for_maintenance(8)?;
    for (id, subject, relation, object) in store.challenged_claim_topics(8)? {
        let key = format!("claim:{id}");
        if store.scout_topic_exists("challenged", &key)? {
            continue;
        }
        let question = format!(
            "Investigate the unresolved challenge to the claim that {subject} {relation} {object}. Find independent, trustworthy evidence and report whether it supports, contradicts, or qualifies this claim."
        );
        return Ok(usize::from(store.enqueue_scout_research_with_reason(
            "challenged",
            &key,
            &format!("db:{id}"),
            &question,
            "Unresolved challenge on an active map claim",
        )?));
    }
    for (id, subject, relation, object) in store.due_claim_topics(8)? {
        let key = format!("claim:{id}");
        if store.scout_topic_exists("stale", &key)? {
            continue;
        }
        let question = format!(
            "Find current, trustworthy evidence for whether {subject} {relation} {object} remains accurate. Report evidence that supports or contradicts the claim."
        );
        return Ok(usize::from(store.enqueue_scout_research_with_reason(
            "stale",
            &key,
            &format!("db:{id}"),
            &question,
            "Claim has passed its scheduled review date",
        )?));
    }
    for claim in &review_batch {
        if matches!(
            claim.relation.as_str(),
            "has encyclopedia summary" | "has encyclopedia detail"
        ) && public_topic(&claim.subject, &claim.relation)
        {
            let reference = format!("db:{}", claim.id);
            if store.scout_expansion_count_for_subject(&claim.subject)? >= 5 {
                continue;
            }
            let previous = store.scout_questions_for_subject(&claim.subject)?;
            let known = store.scout_known_claims_for_subject(&claim.subject)?;
            let mut rejected = previous.clone();
            for _ in 0..2 {
                let raw = ask(ScoutRequest::Map {
                    subject: &claim.subject,
                    known: &known,
                    previous: &rejected,
                })?;
                if let Some(question) = focused_question(&raw, &claim.subject, &previous) {
                    let key = format!("claim:{}|{}", claim.id, canonical(&question));
                    if !store.scout_topic_exists("expansion", &key)? {
                        return Ok(usize::from(store.enqueue_scout_research_with_reason(
                            "expansion",
                            &key,
                            &reference,
                            &question,
                            "Encyclopedia claim leaves a specific detail unexplained",
                        )?));
                    }
                }
                rejected.push(raw.trim().to_owned());
            }
            continue;
        }
        if !public_topic(&claim.subject, &claim.relation)
            || claim.relation_count > 1
            || claim.evidence_count >= 2
        {
            continue;
        }
        let key = format!("claim:{}", claim.id);
        if store.scout_topic_exists("sparse", &key)? {
            continue;
        }
        let question = format!(
            "Find another trustworthy source or an important missing relation about {}. Existing claim: {} {} {}.",
            claim.subject, claim.subject, claim.relation, claim.object
        );
        return Ok(usize::from(store.enqueue_scout_research_with_reason(
            "sparse",
            &key,
            &format!("db:{}", claim.id),
            &question,
            "Rotating map scan found one relation or sparse evidence",
        )?));
    }
    let active_refs: HashSet<String> = map
        .entries()
        .iter()
        .filter(|claim| claim.retracted_by.is_none())
        .map(|claim| format!("json:{}", claim.id))
        .chain(
            store
                .active_semantic_claim_ids()?
                .into_iter()
                .map(|id| format!("db:{id}")),
        )
        .collect();
    let assistant_claims = store.assistant_claims(500)?;
    let db_claims = store.active_semantic_claims()?;
    let gaps = select_gaps(&assistant_claims, &map, &db_claims, &active_refs);
    for gap in gaps {
        if store.scout_topic_exists(gap.kind, &gap.key)? {
            continue;
        }
        return Ok(usize::from(store.enqueue_scout_research_with_reason(
            gap.kind,
            &gap.key,
            &gap.reference,
            &gap.question,
            match gap.kind {
                "challenged" => "Assistant claim conflicts with active evidence",
                "uncertain" => "Assistant claim lacks comparable map evidence",
                _ => "Subject has too few known relations",
            },
        )?));
    }
    let previous_topics = store.recent_model_topics()?;
    let Some(probe_id) = store.begin_model_probe(
        "open public knowledge",
        "Eligible map-derived gaps exhausted; bounded model-derived lead",
    )?
    else {
        return Ok(0);
    };
    let raw = match ask(ScoutRequest::Model {
        previous_topics: &previous_topics,
    }) {
        Ok(raw) => raw,
        Err(error) => {
            store.complete_model_probe(probe_id, "failed", &error.to_string(), None, None, None)?;
            return Err(error);
        }
    };
    let Some((topic, question)) = parse_model_lead(&raw) else {
        store.complete_model_probe(probe_id, "rejected", &raw, None, None, None)?;
        crate::diagnostics::event(
            "scout.model_probe_decision",
            serde_json::json!({
                "probe_id": probe_id,
                "decision": "rejected",
                "reason": "model lead did not match the required topic and question format",
                "model_output_excerpt": raw.chars().take(300).collect::<String>(),
            }),
        );
        return Ok(0);
    };
    let topic_key = canonical(&topic);
    let covered = previous_topics
        .iter()
        .any(|old| canonical(old) == topic_key)
        || !store.scout_known_claims_for_subject(&topic)?.is_empty()
        || map
            .entries()
            .iter()
            .any(|claim| claim.retracted_by.is_none() && canonical(&claim.subject) == topic_key);
    if covered {
        store.complete_model_probe(
            probe_id,
            "rejected",
            &raw,
            Some(&topic),
            Some(&question),
            None,
        )?;
        crate::diagnostics::event(
            "scout.model_probe_decision",
            serde_json::json!({
                "probe_id": probe_id,
                "decision": "rejected",
                "reason": "suggested topic was already covered or recently explored",
                "topic": topic,
                "question": question,
            }),
        );
        return Ok(0);
    }
    let key = format!("{topic_key}|{}", canonical(&question));
    let queued = store.enqueue_scout_research_with_reason(
        "model_probe",
        &key,
        &format!("model_probe:{probe_id}"),
        &question,
        "Model suggested a public question after map gaps were exhausted",
    )?;
    let job_id = if queued {
        store.model_probe_job_id(probe_id)?
    } else {
        None
    };
    store.complete_model_probe(
        probe_id,
        if queued { "queued" } else { "rejected" },
        &raw,
        Some(&topic),
        Some(&question),
        job_id.as_deref(),
    )?;
    crate::diagnostics::event(
        "scout.model_probe_decision",
        serde_json::json!({
            "probe_id": probe_id,
            "decision": if queued { "queued" } else { "rejected" },
            "reason": if queued { "fresh public research lead" } else { "research queue declined the lead" },
            "topic": topic,
            "question": question,
            "scholar_job_id": job_id,
        }),
    );
    Ok(usize::from(queued))
}

fn parse_model_lead(raw: &str) -> Option<(String, String)> {
    let mut lines = raw.lines();
    let topic = lines.next()?.trim().strip_prefix("TOPIC:")?.trim();
    let question = lines.next()?.trim().strip_prefix("QUESTION:")?.trim();
    if lines.next().is_some()
        || topic.is_empty()
        || topic.chars().count() > 80
        || topic.contains(['<', '>'])
        || !public_topic(topic, "research topic")
        || model_lead_question(question, topic).is_none()
    {
        return None;
    }
    Some((topic.to_owned(), question.to_owned()))
}

fn model_lead_question(raw: &str, topic: &str) -> Option<String> {
    let question = raw.trim();
    let normalized = canonical(question);
    let words = normalized
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    let normalized_topic = canonical(topic);
    if question.chars().count() > 160
        || !question.ends_with('?')
        || question.contains(['\n', '\r', '<', '>'])
        || question.matches('?').count() != 1
        || words.len() < 6
        || normalized.starts_with("what are the key facts about ")
        || normalized == format!("what is {normalized_topic}?")
    {
        return None;
    }
    Some(question.to_owned())
}

fn focused_question(raw: &str, subject: &str, previous: &[String]) -> Option<String> {
    let question = raw
        .trim()
        .strip_prefix("QUESTION:")
        .unwrap_or(raw.trim())
        .trim();
    let normalized = canonical(question);
    let subject = canonical(subject);
    if question.chars().count() > 160
        || !question.ends_with('?')
        || question.contains(['\n', '\r', '<', '>'])
        || question.matches('?').count() != 1
        || !normalized.contains(&subject)
        || normalized == format!("what is {subject}?")
        || normalized == format!("what are the key facts about {subject}?")
        || previous
            .iter()
            .any(|old| canonical(old) == normalized || similar_question(old, question, &subject))
    {
        return None;
    }
    Some(question.to_owned())
}

fn similar_question(left: &str, right: &str, subject: &str) -> bool {
    let terms = |question: &str| -> HashSet<String> {
        let lower = question.to_lowercase();
        lower
            .split(|ch: char| !ch.is_alphanumeric())
            .filter(|term| term.len() >= 4)
            .filter(|term| !subject.split_whitespace().any(|word| word == *term))
            .filter(|term| {
                !matches!(
                    *term,
                    "what"
                        | "when"
                        | "where"
                        | "which"
                        | "that"
                        | "this"
                        | "with"
                        | "from"
                        | "about"
                        | "many"
                        | "much"
                )
            })
            .map(str::to_owned)
            .collect()
    };
    let left = terms(left);
    let right = terms(right);
    let smallest = left.len().min(right.len());
    smallest >= 2 && left.intersection(&right).count() * 4 >= smallest * 3
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
                    "Find reliable evidence that supports, contradicts, or qualifies this assertion: {} {} {}.",
                    claim.subject.trim(),
                    claim.relation.trim().replace('_', " "),
                    claim.object.trim(),
                ),
            });
        }
    }

    let mut subjects: BTreeMap<String, (String, HashSet<String>, String)> = BTreeMap::new();
    let mut encyclopedia_subjects = HashSet::new();
    for claim in map
        .entries()
        .iter()
        .filter(|claim| claim.retracted_by.is_none())
    {
        if public_topic(&claim.subject, &claim.relation) {
            if matches!(
                claim.relation.as_str(),
                "has encyclopedia summary" | "has encyclopedia detail"
            ) {
                encyclopedia_subjects.insert(canonical(&claim.subject));
            }
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
            if matches!(
                claim.relation.as_str(),
                "has encyclopedia summary" | "has encyclopedia detail"
            ) {
                encyclopedia_subjects.insert(canonical(&claim.subject));
            }
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
        if relations.len() == 1 && !encyclopedia_subjects.contains(&key) {
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
        && !(matches!(
            subject.as_str(),
            "yes" | "no" | "okay" | "ok" | "thanks" | "hello"
        ) && matches!(
            relation.as_str(),
            "statement" | "response" | "fact" | "acknowledgement"
        ))
        && ![
            "lives in", "is named", "name", "prefers", "owns", "address", "email", "phone",
        ]
        .iter()
        .any(|private| relation.contains(private))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{ScholarFinding, SemanticClaim, SemanticClaimDetail};

    #[test]
    fn model_only_lead_is_queued_after_map_gaps_and_never_becomes_a_claim() {
        let directory = std::env::temp_dir().join(format!(
            "community-model-probe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let database = directory.join("learning.db");
        let knowledge = directory.join("knowledge.json");
        let mut calls = 0;
        assert_eq!(
            process_once_with_request(&database, &knowledge, |request| {
                calls += 1;
                match request {
                    ScoutRequest::Model { previous_topics } => {
                        assert!(previous_topics.is_empty());
                        Ok("TOPIC: Byzantine Empire\nQUESTION: What was the administrative role of the Byzantine Empire's themes?".into())
                    }
                    ScoutRequest::Map { .. } => panic!("empty map has no map-derived gap"),
                }
            }).unwrap(),
            1
        );
        assert_eq!(calls, 1);
        let mut store = LearningStore::open(&database).unwrap();
        let probe = store.model_probes().unwrap().remove(0);
        assert_eq!(probe.status, "queued");
        assert_eq!(probe.topic.as_deref(), Some("Byzantine Empire"));
        assert!(probe.scholar_job_id.is_some());
        let investigation = store.scout_investigations().unwrap().remove(0);
        assert_eq!(investigation.trigger_kind, "model_probe");
        assert_eq!(
            investigation.trigger_ref,
            format!("model_probe:{}", probe.id)
        );
        assert!(store.active_semantic_claims().unwrap().is_empty());
        let job = store.claim_scholar_job().unwrap().unwrap();
        store
            .finish_scholar_job(
                &job.id,
                "Byzantine Empire",
                "https://en.wikipedia.org/wiki/Byzantine_Empire",
                "The Byzantine Empire used themes as military-administrative districts.",
                Some(ScholarFinding {
                    semantic_support: None,
                    subject: "Byzantine Empire".into(),
                    relation: "used".into(),
                    object: "themes as military-administrative districts".into(),
                    evidence_text:
                        "The Byzantine Empire used themes as military-administrative districts."
                            .into(),
                    confidence: 0.99,
                    valid_from: None,
                    valid_until: None,
                    source_quality: "reference".into(),
                    source_updated_at: None,
                }),
                None,
            )
            .unwrap();
        assert!(store.active_semantic_claims().unwrap().is_empty());
        assert_eq!(
            store.scholar_findings(&job.id).unwrap()[0].status,
            "needs_review"
        );
        assert!(
            store.scholar_findings(&job.id).unwrap()[0]
                .decision_reason
                .as_deref()
                .unwrap_or_default()
                .contains("one secondary source")
        );
        drop(store);
        assert_eq!(
            process_once_with_request(&database, &knowledge, |_| panic!("probe is cooling down"))
                .unwrap(),
            0
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn logged_biodiversity_question_is_valid_without_exact_topic_word_overlap() {
        assert!(parse_model_lead("TOPIC: The impact of climate change on biodiversity  \nQUESTION: How does rising global temperatures affect species distribution and ecosystem resilience?").is_some());
        assert!(!public_topic("Yes", "statement"));
        assert!(public_topic("Yes", "formed in"));
    }

    #[test]
    fn model_lead_parser_rejects_unspecific_or_extra_output() {
        assert!(parse_model_lead("TOPIC: User\nQUESTION: Where does User live?").is_none());
        assert!(parse_model_lead("TOPIC: Mars\nQUESTION: What is Mars?").is_none());
        assert!(
            parse_model_lead("TOPIC: Mars\nQUESTION: What are Mars's moons?\nANSWER: two")
                .is_none()
        );
    }

    #[test]
    fn summary_gap_asks_a_focused_question_and_scholar_can_add_a_new_relation() {
        let directory = std::env::temp_dir().join(format!(
            "community-scout-expansion-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let database = directory.join("learning.db");
        let knowledge = directory.join("knowledge.json");
        let mut map = KnowledgeMap::default();
        map.add(
            "Chicken soup",
            "has encyclopedia summary",
            "Chicken soup is a soup made from chicken, usually with various other ingredients.",
            "test encyclopedia",
        )
        .unwrap();
        map.save(&knowledge).unwrap();
        let mut store = LearningStore::open(&database).unwrap();
        store.import_knowledge(&map).unwrap();
        drop(store);

        let question = "What other ingredients are commonly used in Chicken soup?";
        assert_eq!(
            process_once_with(&database, &knowledge, |subject, known, previous| {
                assert_eq!(subject, "Chicken soup");
                assert!(
                    known
                        .iter()
                        .any(|claim| claim.contains("other ingredients"))
                );
                assert!(previous.is_empty());
                Ok(format!("QUESTION: {question}"))
            })
            .unwrap(),
            1
        );
        let mut store = LearningStore::open(&database).unwrap();
        let investigation = store.scout_investigations().unwrap().remove(0);
        assert_eq!(investigation.trigger_kind, "expansion");
        assert_eq!(investigation.question, question);
        assert!(
            store
                .scholar_job_is_expansion(&investigation.scholar_job_id)
                .unwrap()
        );
        let job = store.claim_scholar_job().unwrap().unwrap();
        store
            .finish_scholar_job(
                &job.id,
                "Chicken soup",
                "https://en.wikipedia.org/wiki/Chicken_soup",
                "Chicken soup includes noodles.",
                Some(ScholarFinding {
                    semantic_support: None,
                    subject: "Chicken soup".into(),
                    relation: "includes".into(),
                    object: "noodles".into(),
                    evidence_text: "Chicken soup includes noodles.".into(),
                    confidence: 0.99,
                    valid_from: None,
                    valid_until: None,
                    source_quality: "reference".into(),
                    source_updated_at: None,
                }),
                None,
            )
            .unwrap();
        let finding_id = store.scholar_findings(&job.id).unwrap()[0].id;
        assert_eq!(
            store.scholar_findings(&job.id).unwrap()[0].status,
            "needs_review"
        );
        store
            .review_scholar_finding(finding_id, true, "Reviewed the cited passage")
            .unwrap();
        assert_eq!(store.active_semantic_claims().unwrap().len(), 2);
        assert_eq!(
            store.scout_questions_for_subject("Chicken soup").unwrap(),
            [question]
        );
        drop(store);
        rusqlite::Connection::open(&database)
            .unwrap()
            .execute(
                "UPDATE scout_investigations SET created_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-6 minutes')",
                [],
            )
            .unwrap();
        // The durable scan cursor visits the newly added claim before wrapping
        // back to the encyclopedia summary on the next pass.
        assert_eq!(
            process_once_with(&database, &knowledge, |_, _, _| {
                panic!("the new relation is already covered")
            })
            .unwrap(),
            0
        );
        assert_eq!(
            process_once_with(&database, &knowledge, |subject, known, previous| {
                assert_eq!(subject, "Chicken soup");
                assert!(known.iter().any(|claim| claim.contains("noodles")));
                assert_eq!(previous, &[question]);
                Ok("QUESTION: Why is Chicken soup considered a comfort food?".into())
            })
            .unwrap(),
            1
        );
        let store = LearningStore::open(&database).unwrap();
        let next = store.scout_investigations().unwrap().remove(0);
        assert_eq!(next.trigger_kind, "expansion");
        assert!(next.question.contains("comfort food"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn focused_question_rejects_repeated_or_generic_prompts() {
        let question = "What other ingredients are in Chicken soup?";
        assert_eq!(
            focused_question(question, "Chicken soup", &[]).as_deref(),
            Some(question)
        );
        assert!(focused_question(question, "Chicken soup", &[question.into()]).is_none());
        assert!(
            focused_question(
                "What other ingredients are commonly used in Chicken soup besides water?",
                "Chicken soup",
                &[question.into()],
            )
            .is_none()
        );
        assert!(focused_question("What is Chicken soup?", "Chicken soup", &[]).is_none());
        assert!(
            focused_question(
                "What are the key facts about Chicken soup?",
                "Chicken soup",
                &[]
            )
            .is_none()
        );
        assert!(focused_question("What ingredients are in stew?", "Chicken soup", &[]).is_none());
    }

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
                    ..SemanticClaim::default()
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
                    ..SemanticClaim::default()
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
                    ..SemanticClaim::default()
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
