//! Checks Lyra's untrusted factual assertions against sourced map claims.

use std::io;
use std::path::Path;

use serde::Deserialize;

use crate::agent::Message;
use crate::inference::LocalModel;
use crate::knowledge::KnowledgeMap;
use crate::storage::{AssistantClaim, ClaimJudgment, LearningStore};

const MAX_CLAIMS: usize = 4;
const MAGNITUDE_CHALLENGE_RATIO: f64 = 3.0;

#[derive(Clone, Debug)]
struct ReferenceClaim {
    id: String,
    subject: String,
    relation: String,
    object: String,
}

#[derive(Deserialize)]
struct Extraction {
    claims: Vec<ExtractedClaim>,
}

#[derive(Deserialize)]
struct ExtractedClaim {
    subject: String,
    relation: String,
    object: String,
    evidence_text: String,
    #[serde(default)]
    confidence: f64,
}

pub fn process_once(
    database_path: &Path,
    knowledge_path: &Path,
    model: &mut LocalModel,
) -> io::Result<usize> {
    let mut store = LearningStore::open(database_path)?;
    let Some(job) = store.claim_assistant_reply()? else {
        return Ok(0);
    };
    let result = (|| {
        let claims = extract_claims(model, &job.reply_text)?;
        let map = KnowledgeMap::load(knowledge_path)?;
        let mut references = map
            .entries()
            .iter()
            .filter(|claim| claim.retracted_by.is_none())
            .map(|claim| ReferenceClaim {
                id: format!("json:{}", claim.id),
                subject: claim.subject.clone(),
                relation: claim.relation.clone(),
                object: claim.object.clone(),
            })
            .collect::<Vec<_>>();
        references.extend(store.active_semantic_claims()?.into_iter().map(|detail| {
            ReferenceClaim {
                id: format!("db:{}", detail.claim.id),
                subject: detail.claim.subject,
                relation: detail.claim.relation,
                object: detail.claim.object,
            }
        }));
        Ok::<_, io::Error>(
            claims
                .iter()
                .map(|claim| judge(claim, &references))
                .collect::<Vec<_>>(),
        )
    })();
    match result {
        Ok(judgments) => store.finish_assistant_reply(&job.id, &judgments)?,
        Err(error) => {
            store.hold_assistant_reply(&job.id, &error.to_string())?;
            eprintln!("curmudgeon held reply {}: {error}", job.id);
        }
    }
    Ok(1)
}

fn extract_claims(model: &mut LocalModel, reply: &str) -> io::Result<Vec<AssistantClaim>> {
    if reply.trim().is_empty() {
        return Ok(Vec::new());
    }
    let messages = [
        Message {
            role: "system".into(),
            content: "/no_think\nExtract up to four independent factual assertions made by the assistant in the final reply. This is an audit, not permission to add facts to the map. Ignore greetings, questions, offers, and statements merely attributing information to the user. Return strict JSON only: {\"claims\":[{\"subject\":\"...\",\"relation\":\"...\",\"object\":\"...\",\"evidence_text\":\"exact substring of reply\",\"confidence\":0.0}]}. Use open relation text. Confidence measures extraction certainty, not truth. If there are no factual assertions return {\"claims\":[]}.".into(),
        },
        Message { role: "user".into(), content: reply.into() },
    ];
    let extraction = model
        .chat_for_extraction(&messages)
        .and_then(|text| parse_extraction(&text));
    let mut claims = extraction
        .as_ref()
        .ok()
        .map(|parsed| {
            parsed
                .claims
                .iter()
                .take(MAX_CLAIMS)
                .filter_map(|claim| {
                    let fields = [
                        &claim.subject,
                        &claim.relation,
                        &claim.object,
                        &claim.evidence_text,
                    ];
                    (fields.iter().all(|field| safe_field(field))
                        && reply.contains(&claim.evidence_text)
                        && claim.confidence.is_finite()
                        && (0.0..=1.0).contains(&claim.confidence))
                    .then_some(AssistantClaim {
                        subject: claim.subject.trim().into(),
                        relation: claim.relation.trim().into(),
                        object: claim.object.trim().into(),
                        evidence_text: claim.evidence_text.clone(),
                        extraction_confidence: claim.confidence,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for measurement_claim in fallback_measurement_claims(reply) {
        claims.retain(|claim| {
            let same_measurement = match (
                measurement(&claim.object),
                measurement(&measurement_claim.object),
            ) {
                (Some((left, left_kind)), Some((right, right_kind))) => {
                    left_kind == right_kind && (left - right).abs() < 0.000_001
                }
                _ => false,
            };
            !(claim
                .evidence_text
                .contains(&measurement_claim.evidence_text)
                || (canonical(&claim.subject) == canonical(&measurement_claim.subject)
                    && canonical_relation(&claim.relation)
                        == canonical_relation(&measurement_claim.relation)
                    && same_measurement))
        });
        claims.push(measurement_claim);
    }
    if claims.len() > MAX_CLAIMS {
        claims = claims.split_off(claims.len() - MAX_CLAIMS);
    }
    if claims.is_empty()
        && let Err(error) = extraction
    {
        return Err(error);
    }
    Ok(claims)
}

fn parse_extraction(text: &str) -> io::Result<Extraction> {
    let start = text
        .find('{')
        .ok_or_else(|| invalid("curmudgeon extractor did not return JSON"))?;
    let end = text
        .rfind('}')
        .ok_or_else(|| invalid("curmudgeon extractor returned incomplete JSON"))?;
    serde_json::from_str(&text[start..=end]).map_err(|error| {
        invalid(&format!(
            "curmudgeon extractor returned invalid JSON: {error}"
        ))
    })
}

fn fallback_measurement_claims(reply: &str) -> Vec<AssistantClaim> {
    reply
        .split(". ")
        .filter_map(|sentence| {
            let evidence = sentence.trim().trim_end_matches('.');
            let (subject, rest) = evidence.split_once(" is ")?;
            let subject = subject
                .trim()
                .trim_start_matches("The ")
                .trim_start_matches("A ");
            if subject.is_empty() || subject.chars().count() > 100 {
                return None;
            }
            let relation = if rest.contains(" tall") || rest.contains(" high") {
                "height"
            } else if rest.contains(" long") {
                "length"
            } else {
                return None;
            };
            let tokens = rest.split_whitespace().collect::<Vec<_>>();
            let object = tokens.iter().enumerate().find_map(|(index, token)| {
                if measurement(token).is_some() {
                    return Some((*token).to_owned());
                }
                let next = tokens.get(index + 1)?;
                let pair = format!("{token} {next}");
                measurement(&pair).map(|_| pair)
            })?;
            Some(AssistantClaim {
                subject: subject.into(),
                relation: relation.into(),
                object: object.trim_end_matches(',').into(),
                evidence_text: evidence.into(),
                extraction_confidence: 0.8,
            })
        })
        .take(MAX_CLAIMS)
        .collect()
}

fn judge(claim: &AssistantClaim, references: &[ReferenceClaim]) -> ClaimJudgment {
    let mut verdict = "inconclusive";
    let mut challenge_type = None;
    let mut reason =
        "No directly comparable sourced claim was found; this is not verification.".to_owned();
    let mut evidence_refs = Vec::new();
    for reference in references {
        if canonical(&claim.subject) != canonical(&reference.subject)
            || canonical_relation(&claim.relation) != canonical_relation(&reference.relation)
        {
            continue;
        }
        if canonical(&claim.object) == canonical(&reference.object) {
            evidence_refs.push(reference.id.clone());
            if verdict != "challenged" {
                reason = "A matching sourced claim was found; no challenge raised. This is not independent verification.".into();
            }
            continue;
        }
        if let (Some((left, left_kind)), Some((right, right_kind))) =
            (measurement(&claim.object), measurement(&reference.object))
        {
            if left_kind == right_kind && left > 0.0 && right > 0.0 {
                let ratio = (left / right).max(right / left);
                if ratio >= MAGNITUDE_CHALLENGE_RATIO {
                    verdict = "challenged";
                    challenge_type = Some("magnitude_inconsistency".to_owned());
                    reason = format!(
                        "Measurement differs by roughly {ratio:.0}× from a sourced claim for the same subject and relation."
                    );
                    evidence_refs.push(reference.id.clone());
                } else {
                    evidence_refs.push(reference.id.clone());
                    if verdict != "challenged" {
                        reason = "A comparable sourced measurement is within the magnitude challenge threshold; no challenge raised. This is not verification.".into();
                    }
                }
            }
        } else if measurement(&claim.object).is_none() && measurement(&reference.object).is_none() {
            verdict = "challenged";
            challenge_type = Some("direct_conflict".to_owned());
            reason =
                "A sourced claim gives a different value for the same subject and relation.".into();
            evidence_refs.push(reference.id.clone());
        }
    }
    ClaimJudgment {
        claim: claim.clone(),
        verdict: verdict.into(),
        challenge_type,
        reason,
        evidence_refs,
    }
}

fn measurement(text: &str) -> Option<(f64, &'static str)> {
    let lower = text.trim().to_lowercase();
    let split_at = lower.find(|ch: char| !(ch.is_ascii_digit() || ch == '.'))?;
    let number: f64 = lower[..split_at].parse().ok()?;
    let unit = lower[split_at..]
        .trim()
        .chars()
        .take_while(|ch| ch.is_alphabetic())
        .collect::<String>();
    let (factor, kind) = match unit.as_str() {
        "m" | "metre" | "metres" | "meter" | "meters" => (1.0, "length"),
        "km" | "kilometre" | "kilometres" | "kilometer" | "kilometers" => (1000.0, "length"),
        "cm" | "centimetre" | "centimetres" | "centimeter" | "centimeters" => (0.01, "length"),
        "mm" | "millimetre" | "millimetres" | "millimeter" | "millimeters" => (0.001, "length"),
        "ft" | "feet" | "foot" => (0.3048, "length"),
        _ => return None,
    };
    Some((number * factor, kind))
}

fn canonical(value: &str) -> String {
    value
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn canonical_relation(value: &str) -> String {
    let value = canonical(value);
    if value.contains("height") || value.contains("tall") || value.contains("high") {
        "height".into()
    } else if value.contains("length") || value.contains("long") {
        "length".into()
    } else {
        value
    }
}

fn safe_field(value: &str) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= 500
        && !value.chars().any(char::is_control)
        && !value.contains("<|")
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::{AssistantClaim, ReferenceClaim, fallback_measurement_claims, judge, measurement};

    fn claim(subject: &str, relation: &str, object: &str) -> AssistantClaim {
        AssistantClaim {
            subject: subject.into(),
            relation: relation.into(),
            object: object.into(),
            evidence_text: format!("{subject} {relation} {object}"),
            extraction_confidence: 0.9,
        }
    }
    fn reference(id: &str, subject: &str, relation: &str, object: &str) -> ReferenceClaim {
        ReferenceClaim {
            id: id.into(),
            subject: subject.into(),
            relation: relation.into(),
            object: object.into(),
        }
    }

    #[test]
    fn glaring_magnitude_conflict_is_challenged_with_evidence_id() {
        let result = judge(
            &claim("cricket bat", "length", "400m"),
            &[reference("db:7", "cricket bat", "length", "90cm")],
        );
        assert_eq!(result.verdict, "challenged");
        assert_eq!(
            result.challenge_type.as_deref(),
            Some("magnitude_inconsistency")
        );
        assert_eq!(result.evidence_refs, ["db:7"]);
    }

    #[test]
    fn compatible_and_uncheckable_claims_remain_inconclusive() {
        let references = [reference("db:7", "cricket bat", "length", "90cm")];
        let compatible = judge(&claim("cricket bat", "length", "0.9m"), &references);
        assert_eq!(compatible.verdict, "inconclusive");
        assert_eq!(compatible.evidence_refs, ["db:7"]);
        assert!(compatible.reason.contains("within the magnitude"));
        let odd = judge(&claim("Hagia Sophia", "height", "330m"), &references);
        assert_eq!(odd.verdict, "inconclusive");
        assert!(odd.evidence_refs.is_empty());
    }

    #[test]
    fn direct_conflict_uses_same_subject_and_relation() {
        let result = judge(
            &claim("Hagia Sophia", "located in", "Athens"),
            &[reference(
                "json:2",
                "Hagia Sophia",
                "located in",
                "Istanbul",
            )],
        );
        assert_eq!(result.challenge_type.as_deref(), Some("direct_conflict"));
        assert_eq!(result.evidence_refs, ["json:2"]);
    }

    #[test]
    fn same_building_height_six_times_larger_is_challenged() {
        let result = judge(
            &claim("Hagia Sophia", "height", "330m"),
            &[reference("db:8", "Hagia Sophia", "height", "55m")],
        );
        assert_eq!(
            result.challenge_type.as_deref(),
            Some("magnitude_inconsistency")
        );
        assert_eq!(result.evidence_refs, ["db:8"]);
    }

    #[test]
    fn measurement_fallback_extracts_plain_assistant_assertion() {
        assert_eq!(measurement("400m").unwrap().0, 400.0);
        let claims = fallback_measurement_claims("A cricket bat is 400m long.");
        assert_eq!(claims[0].subject, "cricket bat");
        assert_eq!(claims[0].relation, "length");
        assert_eq!(claims[0].object, "400m");
    }
}
