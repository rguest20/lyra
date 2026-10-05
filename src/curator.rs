//! Background extraction of sourced candidate claims from staged observations.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};

use serde::Deserialize;
use serde_json::Value;

use crate::agent::Message;
use crate::inference::LocalModel;
use crate::maintenance;
use crate::storage::{CandidateClaim, LearningStore, Observation, valid_claim_period};

const LEASE_SECONDS: u64 = 600;
const MAX_BATCH: usize = 4;
const MAX_CANDIDATES: usize = 16;

enum Command {
    Wake,
    Stop,
}

pub struct CuratorHandle {
    sender: mpsc::SyncSender<Command>,
    thread: Option<JoinHandle<()>>,
}

/// Process a bounded batch. The browser invokes this through a short-lived
/// maintenance subprocess so model memory is available between turns.
pub fn process_once_limited(
    database_path: &str,
    model: &mut LocalModel,
    limit: usize,
) -> io::Result<usize> {
    let mut store = LearningStore::open(Path::new(database_path))?;
    let mut processed = 0;
    for _ in 0..limit.min(MAX_BATCH) {
        let Some(observation) = store.claim_next_observation(LEASE_SECONDS)? else {
            break;
        };
        match extract_observation(model, &observation) {
            Ok(extraction) => {
                if let Err(error) = store.finish_extraction(
                    &observation.id,
                    &extraction.claims,
                    extraction.review_reason.as_deref(),
                ) {
                    let _ = store.hold_observation(&observation.id, &error.to_string());
                    eprintln!("semantic curator could not save candidates: {error}");
                }
            }
            Err(error) => {
                eprintln!(
                    "semantic curator held observation {}: {error}",
                    observation.id
                );
                store.hold_observation(&observation.id, &error.to_string())?;
            }
        }
        processed += 1;
    }
    Ok(processed)
}

impl CuratorHandle {
    pub fn spawn(
        database_path: String,
        knowledge_path: String,
        model: Arc<Mutex<LocalModel>>,
    ) -> Self {
        let (sender, receiver) = wake_queue();
        let thread = thread::Builder::new()
            .name("semantic-curator".into())
            .spawn(move || run(database_path, knowledge_path, model, receiver));
        let thread = match thread {
            Ok(thread) => Some(thread),
            Err(error) => {
                eprintln!("could not start semantic curator: {error}");
                None
            }
        };
        Self { sender, thread }
    }

    pub fn wake(&self) {
        send_wake(&self.sender);
    }

    pub fn stop(mut self) {
        let _ = self.sender.send(Command::Stop);
        self.join();
    }

    fn join(&mut self) {
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            eprintln!("semantic curator stopped unexpectedly");
        }
    }
}

impl Drop for CuratorHandle {
    fn drop(&mut self) {
        let _ = self.sender.send(Command::Stop);
        self.join();
    }
}

fn run(
    database_path: String,
    knowledge_path: String,
    model: Arc<Mutex<LocalModel>>,
    receiver: mpsc::Receiver<Command>,
) {
    loop {
        if drain_commands(&receiver) {
            break;
        }
        match model.lock() {
            Ok(mut guard) => {
                if let Err(error) = maintenance::run_pass(
                    Path::new(&database_path),
                    Path::new(&knowledge_path),
                    &mut guard,
                    "chat",
                ) {
                    eprintln!("semantic maintenance pass failed: {error}");
                }
            }
            Err(_) => eprintln!("semantic maintenance model lock was poisoned"),
        }

        if !wait_for_next_pass(&receiver, maintenance::interval()) {
            break;
        }
    }
}

fn wait_for_next_pass(receiver: &mpsc::Receiver<Command>, interval: std::time::Duration) -> bool {
    matches!(
        receiver.recv_timeout(interval),
        Ok(Command::Wake) | Err(mpsc::RecvTimeoutError::Timeout)
    )
}

fn wake_queue() -> (mpsc::SyncSender<Command>, mpsc::Receiver<Command>) {
    mpsc::sync_channel(1)
}

fn send_wake(sender: &mpsc::SyncSender<Command>) {
    let _ = sender.try_send(Command::Wake);
}

fn drain_commands(receiver: &mpsc::Receiver<Command>) -> bool {
    loop {
        match receiver.try_recv() {
            Ok(Command::Wake) => {}
            Ok(Command::Stop) | Err(mpsc::TryRecvError::Disconnected) => return true,
            Err(mpsc::TryRecvError::Empty) => return false,
        }
    }
}

fn extract_observation(
    model: &mut LocalModel,
    observation: &Observation,
) -> io::Result<ExtractionResult> {
    let (context, available_ids) = load_context(observation)?;
    let mut user_content = String::new();
    if !context.is_empty() {
        user_content.push_str("Earlier conversation, for resolving references and explicit user confirmations only:\n");
        for message in &context {
            let id = message.content.lines().next().unwrap_or_default();
            let content = message
                .content
                .split_once('\n')
                .map_or(message.content.as_str(), |(_, content)| content);
            user_content.push_str(&format!("[{id}] {}: {content}\n", message.role));
        }
    }
    user_content.push_str("\nUser observation to extract from:\n");
    user_content.push_str(&observation.raw_text);

    let messages = [
        Message {
            role: "system".into(),
            content: format!(
                "/no_think\nExtract every explicit factual claim asserted by the user in the final observation, including claims about the user. Do not skip first-person statements. Examples: `I am Ryan` -> subject `the user`, relation `is named`, object `Ryan`, evidence_text `I am Ryan`; `I live in Bristol` -> subject `the user`, relation `lives in`, object `Bristol`, evidence_text `I live in Bristol`; `I prefer tea` -> subject `the user`, relation `prefers`, object `tea`, evidence_text `I prefer tea`. These examples show the general transformation; use open relation text for other claims. If the final observation is an affirmative confirmation (such as `yes`, `that's right`, or `correct`) to the immediately preceding assistant message explicitly asking the user to confirm a user fact, extract that confirmed fact too. Use the exact confirmation as evidence_text, cite the relevant available user and assistant context IDs, and explain the confirmation in resolution. Do not infer confirmation from an unrelated yes/no question or from an assistant assertion without a direct confirmation request. Return strict JSON exactly shaped like {{\"claims\":[{{\"subject\":\"...\",\"relation\":\"...\",\"object\":\"...\",\"confidence\":0.0,\"evidence_text\":\"exact substring from the final observation\",\"context_message_ids\":[],\"resolution\":null}}],\"review_reason\":null}}. Include every field, especially confidence, for each claim. Do not infer plausible facts. Do not treat earlier assistant statements as evidence; use earlier messages only to resolve an unambiguous reference in the user observation. If a factual claim appears intended but is ambiguous or cannot be resolved, set review_reason to a short explanation. For ordinary messages with no factual claim, return an empty claims array and null review_reason. Available context IDs: {}. Return no markdown or commentary.",
                available_ids.join(", ")
            ),
        },
        Message {
            role: "system".into(),
            content: "/no_think\nFor each claim, also return valid_from and valid_until as ISO YYYY-MM-DD only when the exact date appears in the final user observation. Set both to null when the claim has no explicitly stated validity period. Do not infer that an old or current user fact continues beyond what they said.".into(),
        },
        Message {
            role: "user".into(),
            content: user_content,
        },
    ];
    let reply = model.chat_for_extraction(&messages)?;
    let parsed = parse_response(&reply)?;
    if parsed.claims.len() > MAX_CANDIDATES {
        return Err(invalid("extractor returned too many candidate claims"));
    }
    let mut claims = Vec::new();
    let mut review_reasons = parsed.review_reason.into_iter().collect::<Vec<_>>();
    for candidate in parsed.claims {
        match validate_candidate(candidate, observation, &available_ids) {
            Ok(claim) => {
                if claim.status == "needs_review"
                    && let Some(reason) = &claim.decision_reason
                {
                    review_reasons.push(reason.clone());
                }
                claims.push(claim);
            }
            Err(error) => review_reasons.push(error.to_string()),
        }
    }
    let review_reason = if review_reasons.is_empty() {
        None
    } else {
        Some(review_reasons.join("; ").chars().take(500).collect())
    };
    Ok(ExtractionResult {
        claims,
        review_reason,
    })
}

struct ExtractionResult {
    claims: Vec<CandidateClaim>,
    review_reason: Option<String>,
}

#[derive(Deserialize)]
struct ExtractionResponse {
    claims: Vec<ExtractedClaim>,
    #[serde(default)]
    review_reason: Option<String>,
}

#[derive(Deserialize)]
struct ExtractedClaim {
    subject: String,
    relation: String,
    object: String,
    evidence_text: String,
    #[serde(default)]
    context_message_ids: Vec<String>,
    resolution: Option<String>,
    #[serde(default)]
    confidence: f64,
    #[serde(default)]
    valid_from: Option<String>,
    #[serde(default)]
    valid_until: Option<String>,
}

fn parse_response(reply: &str) -> io::Result<ExtractionResponse> {
    let start = reply
        .find('{')
        .ok_or_else(|| invalid("extractor did not return JSON"))?;
    let end = reply
        .rfind('}')
        .ok_or_else(|| invalid("extractor returned incomplete JSON"))?;
    serde_json::from_str(&reply[start..=end])
        .map_err(|error| invalid(&format!("extractor returned invalid JSON: {error}")))
}

fn validate_candidate(
    candidate: ExtractedClaim,
    observation: &Observation,
    available_ids: &[String],
) -> io::Result<CandidateClaim> {
    for field in [&candidate.subject, &candidate.relation, &candidate.object] {
        check_field(field)?;
    }
    let invalid_evidence = candidate.evidence_text.is_empty()
        || !observation.raw_text.contains(&candidate.evidence_text);
    let invalid_period = !valid_claim_period(
        candidate.valid_from.as_deref(),
        candidate.valid_until.as_deref(),
    ) || [
        candidate.valid_from.as_deref(),
        candidate.valid_until.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|date| !observation.raw_text.contains(date));
    let evidence_text = if invalid_evidence {
        observation.raw_text.chars().take(500).collect()
    } else {
        candidate.evidence_text
    };
    if !candidate.confidence.is_finite() || !(0.0..=1.0).contains(&candidate.confidence) {
        return Err(invalid("candidate confidence is outside 0.0 to 1.0"));
    }
    let known_context = candidate
        .context_message_ids
        .iter()
        .all(|id| available_ids.contains(id));
    let is_confirmation = is_confirmation_response(&observation.raw_text);
    let cites_assistant_confirmation = candidate
        .context_message_ids
        .iter()
        .any(|id| id.ends_with(":assistant"));
    let missing_context_resolution = candidate.context_message_ids.is_empty()
        || candidate.resolution.as_deref().is_none_or(str::is_empty)
        || !known_context;
    let unresolved_reference = (contains_reference_pronoun(&evidence_text)
        && missing_context_resolution)
        || (is_confirmation && (!cites_assistant_confirmation || missing_context_resolution));
    let (status, reason) = if invalid_evidence {
        (
            "needs_review",
            Some(
                "extractor evidence was not an exact source span; stored source excerpt for review",
            ),
        )
    } else if invalid_period {
        (
            "needs_review",
            Some("claim dates are invalid or not explicit in the observation"),
        )
    } else if candidate.confidence < 0.80 {
        ("needs_review", Some("extractor confidence below 0.80"))
    } else if !known_context {
        ("needs_review", Some("candidate cites unavailable context"))
    } else if unresolved_reference {
        ("needs_review", Some("reference is not clearly resolved"))
    } else {
        ("proposed", None)
    };
    Ok(CandidateClaim {
        subject: candidate.subject.trim().into(),
        relation: candidate.relation.trim().into(),
        object: candidate.object.trim().into(),
        evidence_text,
        context_message_ids: candidate.context_message_ids,
        resolution: candidate.resolution,
        confidence: candidate.confidence,
        status: status.into(),
        decision_reason: reason.map(str::to_owned),
        valid_from: candidate.valid_from,
        valid_until: candidate.valid_until,
    })
}

fn load_context(observation: &Observation) -> io::Result<(Vec<Message>, Vec<String>)> {
    let state_path = observation
        .conversation_id
        .split_once("::conversation:")
        .map_or(observation.conversation_id.as_str(), |(path, _)| path);
    let path = Path::new(state_path);
    if !path.exists() || observation.turn_number <= 1 {
        return Ok((Vec::new(), Vec::new()));
    }
    let state: Value = serde_json::from_slice(&fs::read(path)?).map_err(invalid_json)?;
    let saved_conversation_id = state["conversation_id"].as_str().unwrap_or(state_path);
    if saved_conversation_id != observation.conversation_id {
        return Ok((Vec::new(), Vec::new()));
    }
    let records = state["history"]
        .as_array()
        .ok_or_else(|| invalid("conversation history is missing"))?;
    let preceding_count = observation.turn_number.saturating_sub(1).saturating_mul(2);
    if records.len() < preceding_count {
        return Ok((Vec::new(), Vec::new()));
    }
    let start = preceding_count.saturating_sub(24);
    let mut candidates = Vec::new();
    for (index, record) in records[start..preceding_count].iter().enumerate() {
        let role = record["role"]
            .as_str()
            .ok_or_else(|| invalid("conversation role is missing"))?;
        let content = record["content"]
            .as_str()
            .ok_or_else(|| invalid("conversation message is missing"))?;
        let message_index = start + index;
        if !matches!(role, "user" | "assistant") {
            continue;
        }
        let turn = message_index / 2 + 1;
        candidates.push(ContextMessage {
            id: context_id_parts(turn, role),
            role: role.into(),
            content: content.to_owned(),
        });
    }
    let terms = meaningful_terms(&observation.raw_text);
    let include_recent = contains_reference_pronoun(&observation.raw_text)
        || is_confirmation_response(&observation.raw_text);
    let recent_start = candidates.len().saturating_sub(2);
    let mut selected = candidates
        .into_iter()
        .enumerate()
        .filter(|(index, message)| {
            (include_recent && *index >= recent_start)
                || terms
                    .iter()
                    .any(|term| message.content.to_lowercase().contains(term))
        })
        .map(|(_, message)| message)
        .collect::<Vec<_>>();
    if selected.len() > 12 {
        selected = selected.split_off(selected.len() - 12);
    }
    let ids = selected.iter().map(|message| message.id.clone()).collect();
    let messages = selected
        .into_iter()
        .map(|message| Message {
            role: message.role,
            content: format!("{}\n{}", message.id, message.content),
        })
        .collect();
    Ok((messages, ids))
}

struct ContextMessage {
    id: String,
    role: String,
    content: String,
}

fn meaningful_terms(text: &str) -> Vec<String> {
    const STOP_WORDS: &[&str] = &[
        "about", "after", "again", "also", "been", "being", "could", "from", "have", "into",
        "just", "like", "more", "some", "than", "that", "them", "then", "there", "they", "this",
        "what", "when", "where", "which", "while", "with", "would",
    ];
    text.to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| word.chars().count() >= 3 && !STOP_WORDS.contains(word))
        .map(str::to_owned)
        .collect()
}

fn context_id_parts(turn: usize, role: &str) -> String {
    format!("turn:{turn}:{role}")
}

fn contains_reference_pronoun(text: &str) -> bool {
    text.to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric())
        .any(|word| {
            matches!(
                word,
                "it" | "this" | "that" | "he" | "she" | "they" | "them" | "you" | "your"
            )
        })
}

fn is_confirmation_response(text: &str) -> bool {
    let normalized = text
        .trim()
        .to_lowercase()
        .replace('\'', "")
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    [
        "yes",
        "yes thats right",
        "yes that is right",
        "yes correct",
        "thats right",
        "that is right",
        "correct",
        "exactly",
        "you got it",
        "right",
    ]
    .iter()
    .any(|confirmation| normalized == *confirmation)
}

fn check_field(field: &str) -> io::Result<()> {
    if field.trim().is_empty()
        || field.chars().count() > 500
        || field.chars().any(char::is_control)
        || field.contains("<|")
    {
        return Err(invalid("candidate fields must be 1 to 500 safe characters"));
    }
    Ok(())
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn invalid_json(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

#[cfg(test)]
mod tests {
    use super::{
        Command, ExtractedClaim, contains_reference_pronoun, is_confirmation_response,
        load_context, parse_response, send_wake, validate_candidate, wait_for_next_pass,
        wake_queue,
    };
    use crate::storage::Observation;

    #[test]
    fn terminal_maintenance_wakes_on_an_idle_interval() {
        let (sender, receiver) = std::sync::mpsc::channel();
        assert!(wait_for_next_pass(
            &receiver,
            std::time::Duration::from_millis(1)
        ));
        sender.send(Command::Stop).unwrap();
        assert!(!wait_for_next_pass(
            &receiver,
            std::time::Duration::from_secs(1)
        ));
    }

    #[test]
    fn duplicate_wake_requests_coalesce_in_a_bounded_queue() {
        let (sender, receiver) = wake_queue();
        send_wake(&sender);
        send_wake(&sender);
        assert!(matches!(receiver.try_recv(), Ok(Command::Wake)));
        assert!(receiver.try_recv().is_err());
    }

    fn observation() -> Observation {
        Observation {
            id: "observation-a".into(),
            conversation_id: "conversation-a".into(),
            turn_number: 2,
            raw_text: "I gave it to you.".into(),
            status: "processing".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn parses_structured_claims_and_keeps_resolved_references_proposed() {
        let response = parse_response(
            r#"{"claims":[{"subject":"Lyra","relation":"name was given by","object":"the user","evidence_text":"I gave it to you.","context_message_ids":["turn:1:assistant"],"resolution":"it refers to Lyra's name","confidence":0.91}]}"#,
        )
        .unwrap();
        let claim = validate_candidate(
            response.claims.into_iter().next().unwrap(),
            &observation(),
            &["turn:1:assistant".into()],
        )
        .unwrap();
        assert_eq!(claim.status, "proposed");
        assert_eq!(claim.evidence_text, "I gave it to you.");
    }

    #[test]
    fn unsupported_evidence_is_held_and_unresolved_pronouns_are_reviewed() {
        assert!(contains_reference_pronoun("I gave it to you."));
        let bad_evidence = ExtractedClaim {
            subject: "Lyra".into(),
            relation: "name was given by".into(),
            object: "the user".into(),
            evidence_text: "I named Lyra".into(),
            context_message_ids: Vec::new(),
            resolution: None,
            confidence: 0.99,
            valid_from: None,
            valid_until: None,
        };
        let claim = validate_candidate(bad_evidence, &observation(), &[]).unwrap();
        assert_eq!(claim.status, "needs_review");
        assert_eq!(claim.evidence_text, observation().raw_text);

        let unresolved = ExtractedClaim {
            subject: "Lyra".into(),
            relation: "name was given by".into(),
            object: "the user".into(),
            evidence_text: "I gave it to you.".into(),
            context_message_ids: Vec::new(),
            resolution: None,
            confidence: 0.99,
            valid_from: None,
            valid_until: None,
        };
        let claim = validate_candidate(unresolved, &observation(), &[]).unwrap();
        assert_eq!(claim.status, "needs_review");
    }

    #[test]
    fn confirmations_need_an_assistant_question_and_resolution() {
        let yes = Observation {
            raw_text: "Yes, that's right.".into(),
            turn_number: 3,
            ..observation()
        };
        assert!(is_confirmation_response(&yes.raw_text));
        let claim = |context_message_ids: Vec<String>, resolution: Option<&str>| ExtractedClaim {
            subject: "the user".into(),
            relation: "lives in".into(),
            object: "Shifnal".into(),
            evidence_text: "Yes, that's right.".into(),
            context_message_ids,
            resolution: resolution.map(str::to_owned),
            confidence: 0.99,
            valid_from: None,
            valid_until: None,
        };
        let unsupported = validate_candidate(
            claim(vec!["turn:2:user".into()], Some("Confirms location.")),
            &yes,
            &["turn:2:user".into()],
        )
        .unwrap();
        assert_eq!(unsupported.status, "needs_review");
        let supported = validate_candidate(
            claim(
                vec!["turn:2:assistant".into()],
                Some("Confirms the location asked about."),
            ),
            &yes,
            &["turn:2:assistant".into()],
        )
        .unwrap();
        assert_eq!(supported.status, "proposed");
    }

    #[test]
    fn affirmative_confirmation_receives_the_previous_assistant_question_as_context() {
        let path = std::env::temp_dir().join(format!(
            "community-curator-context-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let state = serde_json::json!({
            "history": [
                {"role":"user", "content":"I live in Shifnal."},
                {"role":"assistant", "content":"You told me Shifnal."},
                {"role":"user", "content":"Where do I live?"},
                {"role":"assistant", "content":"Correct me if I'm wrong, but you said you live in Shifnal, right?"}
            ]
        });
        std::fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
        let confirmation = Observation {
            id: "confirmation".into(),
            conversation_id: path.to_string_lossy().into_owned(),
            turn_number: 3,
            raw_text: "Yes".into(),
            status: "processing".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        let (context, ids) = load_context(&confirmation).unwrap();
        assert_eq!(ids, ["turn:2:user", "turn:2:assistant"]);
        assert!(context.last().unwrap().content.contains("live in Shifnal"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn old_conversation_does_not_read_a_new_conversation_history() {
        let path = std::env::temp_dir().join(format!(
            "community-curator-reset-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let original_id = path.to_string_lossy().into_owned();
        let state = serde_json::json!({
            "conversation_id": format!("{original_id}::conversation:new"),
            "history": [
                {"role":"user", "content":"I live in Bristol."},
                {"role":"assistant", "content":"You told me Bristol."}
            ]
        });
        std::fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
        let old_observation = Observation {
            conversation_id: original_id,
            turn_number: 2,
            raw_text: "Yes".into(),
            ..observation()
        };
        let (context, ids) = load_context(&old_observation).unwrap();
        assert!(context.is_empty());
        assert!(ids.is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn ambiguous_extraction_can_be_preserved_for_review() {
        let response = parse_response(
            r#"{"claims":[],"review_reason":"The user appears to refer to a prior name, but the referent is unclear."}"#,
        )
        .unwrap();
        assert!(response.claims.is_empty());
        assert_eq!(
            response.review_reason.as_deref(),
            Some("The user appears to refer to a prior name, but the referent is unclear.")
        );
    }

    #[test]
    fn missing_confidence_is_kept_as_a_low_confidence_review_candidate() {
        let response = parse_response(
            r#"{"claims":[{"subject":"the user","relation":"is named","object":"Ryan","evidence_text":"I am Ryan","context_message_ids":[],"resolution":null}]}"#,
        )
        .unwrap();
        let claim = validate_candidate(
            response.claims.into_iter().next().unwrap(),
            &Observation {
                raw_text: "I am Ryan".into(),
                ..observation()
            },
            &[],
        )
        .unwrap();
        assert_eq!(claim.confidence, 0.0);
        assert_eq!(claim.status, "needs_review");
    }
}
