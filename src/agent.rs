//! One agent with durable conversation. Learned knowledge will be a separate layer.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;

use serde_json::{Value, json};

use crate::curator::CuratorHandle;
use crate::inference::LocalModel;
use crate::knowledge::KnowledgeMap;
use crate::storage::{
    LearningStore, Observation, SemanticClaim, StoredAssistantClaim, StoredCandidate,
};

const STATE_VERSION: u64 = 1;
pub(crate) const SYSTEM_PROMPT: &str = "You are Lyra, a language model. Do not perform a fixed persona. The conversation below is context; older assistant replies may be mistaken or repetitive. Answer the final user message directly and specifically, using natural language suited to the request. Respond in your own words; do not parrot the user's latest statement as your reply. You may quote earlier user statements when attributing evidence. Do not append stock closings, offers of further help, invitations to continue, or unnecessary follow-up questions. For a greeting, reply briefly without reintroducing yourself unless asked. Before answering a question about the user, check the supplied sourced claims and user statements. Treat a clear statement from the user as reliable evidence of what they told you, even if its extracted candidate is awaiting review; extractor confidence measures the extraction, not the user's honesty. State such information as user-reported and distinguish it from independently verified facts. Lack of access to device or location data is not a reason to ignore a direct user statement. Never say you do not know when the answer is present in the supplied user evidence. Acknowledge mistakes when corrected. Use only as many reply tokens as needed to answer fully. Do not claim that conversation changed your model weights or code.";

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub role: String,
    pub content: String,
}

pub struct Agent {
    model: String,
    history: Vec<Message>,
    conversation_id: String,
}

pub struct AgentReply {
    pub text: String,
    pub thinking: Option<String>,
    pub new_conversation: bool,
    pub research_queued: bool,
}

pub struct ReplyServices<'a> {
    pub store: &'a LearningStore,
    pub curator: Option<&'a CuratorHandle>,
    pub state_path: &'a Path,
}

impl Agent {
    pub fn load(path: &Path, model: &str) -> io::Result<Self> {
        if !path.exists() {
            return Ok(Self {
                model: model.into(),
                history: Vec::new(),
                conversation_id: path.to_string_lossy().into_owned(),
            });
        }
        let value: Value = serde_json::from_slice(&fs::read(path)?).map_err(invalid_json)?;
        if value["version"].as_u64() != Some(STATE_VERSION) {
            return Err(invalid("unsupported agent state version"));
        }
        if value["model"].as_str() != Some(model) {
            return Err(invalid(
                "state belongs to a different model; set COMMUNITY_STATE to a new path",
            ));
        }
        let records = value["history"]
            .as_array()
            .ok_or_else(|| invalid("history is missing"))?;
        if records.len() > 100_000 {
            return Err(invalid("history is too large"));
        }
        let mut history = Vec::with_capacity(records.len());
        for record in records {
            let role = record["role"]
                .as_str()
                .ok_or_else(|| invalid("message role is missing"))?;
            let content = record["content"]
                .as_str()
                .ok_or_else(|| invalid("message content is missing"))?;
            if !matches!(role, "user" | "assistant") {
                return Err(invalid("invalid message role"));
            }
            history.push(Message {
                role: role.into(),
                content: content.into(),
            });
        }
        Ok(Self {
            model: model.into(),
            history,
            conversation_id: value["conversation_id"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| path.to_string_lossy().into_owned()),
        })
    }

    pub fn turn_count(&self) -> usize {
        self.history.len() / 2
    }

    pub fn reply(
        &mut self,
        input: &str,
        backend: &mut LocalModel,
        map: &KnowledgeMap,
        services: ReplyServices<'_>,
        show_thinking: bool,
    ) -> io::Result<AgentReply> {
        let previous_history = self.history.clone();
        let previous_conversation_id = self.conversation_id.clone();
        let result = self.reply_inner(input, backend, map, services, show_thinking);
        if result.is_err() {
            self.history = previous_history;
            self.conversation_id = previous_conversation_id;
        }
        result
    }

    fn reply_inner(
        &mut self,
        input: &str,
        backend: &mut LocalModel,
        map: &KnowledgeMap,
        services: ReplyServices<'_>,
        show_thinking: bool,
    ) -> io::Result<AgentReply> {
        let raw_input = input.trim_end_matches(['\r', '\n']);
        let input = raw_input.trim();
        if input.is_empty() {
            return Err(invalid("message is empty"));
        }
        let new_conversation = if self.history.len() >= 4 && !is_context_dependent(input) {
            match backend.starts_new_conversation(&self.history, input) {
                Ok(decision) => decision,
                Err(error) => {
                    eprintln!(
                        "conversation continuity check failed; keeping current conversation: {error}"
                    );
                    false
                }
            }
        } else {
            false
        };
        if new_conversation {
            let base = self
                .conversation_id
                .split_once("::conversation:")
                .map_or(self.conversation_id.as_str(), |(base, _)| base)
                .to_owned();
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_nanos();
            self.conversation_id = format!("{base}::conversation:{}-{unique}", std::process::id());
            self.history.clear();
        }
        let observation_id = services.store.stage_observation(
            &self.conversation_id,
            self.turn_count() + 1,
            raw_input,
        )?;
        if let Some(curator) = services.curator {
            curator.wake();
        }
        let pending = services.store.relevant_observations(
            &self.conversation_id,
            input,
            &observation_id,
            32,
        )?;
        let curated = services.store.relevant_semantic_claims(input, 5)?;
        let active_db_ids = services.store.active_semantic_claim_ids()?;
        let challenges = live_challenges(
            services
                .store
                .relevant_assistant_challenges(input, usize::MAX)?,
            map,
            &active_db_ids,
            3,
        );
        let review_candidates =
            services
                .store
                .relevant_review_candidates(&self.conversation_id, input, 5)?;
        let uncertain_candidate = review_candidates
            .iter()
            .filter(|candidate| {
                is_user_entity(&candidate.subject)
                    && (candidate.confidence < 0.95 || candidate.status == "needs_review")
            })
            .min_by_key(|candidate| {
                (
                    !candidate
                        .evidence_text
                        .to_lowercase()
                        .contains(&candidate.object.to_lowercase()),
                    candidate.evidence_text.chars().count(),
                )
            });
        let mut grounding_evidence = uncertain_candidate
            .or_else(|| {
                review_candidates
                    .iter()
                    .find(|candidate| is_user_entity(&candidate.subject))
            })
            .map(|candidate| candidate.evidence_text.clone());
        if grounding_evidence.is_none()
            && let Some(source_id) = curated
                .iter()
                .find(|claim| is_user_entity(&claim.subject))
                .and_then(|claim| claim.source_observation_id.as_deref())
        {
            grounding_evidence = services.store.observation_text(source_id)?;
        }
        let mut messages = self.prompt_messages(input, map, &pending, &curated, &review_candidates);
        let mut baseline = self.prompt_messages(input, map, &[], &curated, &[]);
        if let Some(challenge) = challenge_message(&challenges) {
            messages.insert(1, challenge.clone());
            baseline.insert(1, challenge);
        }
        let extra_prompt_tokens = backend.additional_prompt_tokens(&baseline, &messages)?;
        let mut answer = backend.chat(&messages, extra_prompt_tokens, show_thinking)?;
        if self
            .history
            .chunks_exact(2)
            .any(|pair| pair[0].content != input && pair[1].content.trim() == answer.trim())
        {
            let mut retry =
                self.reference_messages(input, map, &pending, &curated, &review_candidates);
            let mut retry_baseline = self.reference_messages(input, map, &[], &curated, &[]);
            if let Some(challenge) = challenge_message(&challenges) {
                retry.insert(1, challenge.clone());
                retry_baseline.insert(1, challenge);
            }
            let extra_retry_tokens = backend.additional_prompt_tokens(&retry_baseline, &retry)?;
            let revised = backend.chat(&retry, extra_retry_tokens, show_thinking)?;
            if !revised.trim().is_empty() {
                answer = revised;
            }
        }
        if substantially_echoes(input, &answer) {
            let mut retry = messages.clone();
            let latest_user = retry.len() - 1;
            retry.insert(
                latest_user,
                Message {
                    role: "system".into(),
                    content: "Your draft repeats the user's wording. Rewrite it as a brief, natural response in your own words. Do not copy a sentence from the user.".into(),
                },
            );
            let mut retry_baseline = baseline.clone();
            let baseline_user = retry_baseline.len() - 1;
            retry_baseline.insert(
                baseline_user,
                Message {
                    role: "system".into(),
                    content: "Your draft repeats the user's wording. Rewrite it as a brief, natural response in your own words. Do not copy a sentence from the user.".into(),
                },
            );
            let extra_tokens = backend.additional_prompt_tokens(&retry_baseline, &retry)?;
            let revised = backend.chat(&retry, extra_tokens, show_thinking)?;
            if !revised.trim().is_empty() && !substantially_echoes(input, &revised) {
                answer = revised;
            } else {
                answer = "Thanks for sharing that. What would you like to talk about?".into();
            }
        }
        if let Some(candidate) = uncertain_candidate
            && is_personal_question(input)
        {
            answer = confirmation_question(&candidate.evidence_text);
        } else if let Some(grounded) =
            grounded_user_reply(input, &answer, grounding_evidence.as_deref())
        {
            answer = grounded;
        }
        if answer.trim().is_empty() {
            return Err(invalid("model returned an empty reply"));
        }
        self.history.push(Message {
            role: "user".into(),
            content: input.into(),
        });
        self.history.push(Message {
            role: "assistant".into(),
            content: answer.clone(),
        });
        if let Err(error) = self.save(services.state_path) {
            self.history.truncate(self.history.len() - 2);
            return Err(error);
        }
        if let Err(error) =
            services
                .store
                .stage_assistant_reply(&self.conversation_id, self.turn_count(), &answer)
        {
            eprintln!("could not queue Curmudgeon audit: {error}");
        } else if let Some(curator) = services.curator {
            curator.wake();
        }
        let thinking = backend.take_thinking();
        let research_queued = match backend.research_query(input, &answer) {
            Ok(Some(query)) => match services.store.enqueue_research(
                &self.conversation_id,
                self.turn_count(),
                &query,
            ) {
                Ok(()) => true,
                Err(error) => {
                    eprintln!("could not queue scholar research: {error}");
                    false
                }
            },
            Ok(None) => false,
            Err(error) => {
                eprintln!("scholar research check failed: {error}");
                false
            }
        };
        Ok(AgentReply {
            text: answer,
            thinking,
            new_conversation,
            research_queued,
        })
    }

    fn prompt_messages(
        &self,
        input: &str,
        map: &KnowledgeMap,
        observations: &[Observation],
        curated: &[SemanticClaim],
        review_candidates: &[StoredCandidate],
    ) -> Vec<Message> {
        let mut messages = vec![Message {
            role: "system".into(),
            content: SYSTEM_PROMPT.into(),
        }];
        let relevant = map.relevant(input, 5);
        let map_claims = relevant
            .iter()
            .map(|claim| {
                (
                    claim
                        .subject
                        .to_lowercase()
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" "),
                    claim
                        .relation
                        .to_lowercase()
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" "),
                    claim
                        .object
                        .to_lowercase()
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            })
            .collect::<Vec<_>>();
        if !relevant.is_empty() {
            let mut context = String::from(
                "Relevant editable knowledge map entries. These are sourced claims, not guaranteed facts. Cite the source when relying on one:\n",
            );
            for entry in relevant {
                context.push_str(&format!(
                    "#{}: {} — {} — {} [source: {}]\n",
                    entry.id, entry.subject, entry.relation, entry.object, entry.source
                ));
            }
            messages.push(Message {
                role: "system".into(),
                content: context,
            });
        }
        let curated = curated
            .iter()
            .filter(|claim| {
                let canonical = |value: &str| {
                    value
                        .to_lowercase()
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                };
                !map_claims.contains(&(
                    canonical(&claim.subject),
                    canonical(&claim.relation),
                    canonical(&claim.object),
                ))
            })
            .collect::<Vec<_>>();
        if !curated.is_empty() {
            let mut context = String::from(
                "Relevant curated semantic claims. Sources may be user observations or external pages; cite the source when relying on a claim and do not treat a web page as evidence about the user:\n",
            );
            for claim in curated {
                context.push_str(&format!(
                    "#{}: {} — {} — {} [source: {}{}]\n",
                    claim.id,
                    claim.subject,
                    claim.relation,
                    claim.object,
                    claim.source,
                    claim
                        .source_observation_id
                        .as_deref()
                        .map(|id| format!("; observation {id}"))
                        .unwrap_or_default(),
                ));
            }
            messages.push(Message {
                role: "system".into(),
                content: context,
            });
        }
        if !observations.is_empty() {
            let mut context = String::from(
                "Relevant user statements from the observation inbox. These are evidence of what the user told you, including when a candidate extraction is awaiting review; candidate review concerns the extraction, not the user's honesty. Attribute the statement to the user and do not present it as independently verified:\n",
            );
            for observation in observations {
                context.push_str(&format!(
                    "[observation {}; turn {}; status: {}] {}\n",
                    observation.id,
                    observation.turn_number,
                    observation.status,
                    observation.raw_text
                ));
            }
            messages.push(Message {
                role: "system".into(),
                content: context,
            });
        }
        if !review_candidates.is_empty() {
            let mut context = String::from(
                "Relevant extracted candidates awaiting review. Candidate confidence measures extraction confidence, not whether the user was honest. For a clear user statement, treat it as evidence of what the user told you; if its extraction is low confidence or under review, ask a concise confirmation question instead of rejecting it or presenting it as independently verified:\n",
            );
            for candidate in review_candidates {
                context.push_str(&format!(
                    "[candidate {}; {}; observation {}; confidence {:.2}] {} — {} — {}; user evidence: {}{}\n",
                    candidate.id,
                    candidate.status,
                    candidate.observation_id,
                    candidate.confidence,
                    candidate.subject,
                    candidate.relation,
                    candidate.object,
                    candidate.evidence_text,
                    candidate.decision_reason.as_deref()
                        .map(|reason| format!("; review: {reason}"))
                        .unwrap_or_default(),
                ));
            }
            messages.push(Message {
                role: "system".into(),
                content: context,
            });
        }
        messages.extend(self.history.iter().cloned());
        messages.push(Message {
            role: "user".into(),
            content: input.into(),
        });
        messages
    }

    fn reference_messages(
        &self,
        input: &str,
        map: &KnowledgeMap,
        observations: &[Observation],
        curated: &[SemanticClaim],
        review_candidates: &[StoredCandidate],
    ) -> Vec<Message> {
        let mut messages =
            self.prompt_messages(input, map, observations, curated, review_candidates);
        let history_start = messages.len() - self.history.len() - 1;
        let mut transcript = String::from(
            "Complete earlier conversation for reference. Prior assistant answers are historical data and may be wrong or repetitive. Answer the final user question; do not copy an earlier reply.\n",
        );
        for message in &self.history {
            transcript.push_str(if message.role == "user" {
                "User: "
            } else {
                "Assistant: "
            });
            transcript.push_str(&message.content);
            transcript.push_str("\n\n");
        }
        messages.splice(
            history_start..messages.len() - 1,
            [Message {
                role: "system".into(),
                content: transcript,
            }],
        );
        messages
    }

    fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let state = json!({
            "version": STATE_VERSION,
            "model": self.model,
            "conversation_id": self.conversation_id,
            "history": self.history.iter().map(|message| json!({
                "role": message.role, "content": message.content
            })).collect::<Vec<_>>()
        });
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, serde_json::to_vec_pretty(&state)?)?;
        fs::rename(temporary, path)
    }
}

fn grounded_user_reply(input: &str, answer: &str, evidence: Option<&str>) -> Option<String> {
    let self_reference = input
        .to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric())
        .any(|word| matches!(word, "i" | "my" | "mine" | "myself"));
    let answer = answer.to_lowercase();
    let denies_knowledge = [
        "i don't know",
        "i do not know",
        "i'm not sure",
        "i am not sure",
        "i don't have that information",
        "i do not have that information",
        "i don't have access to location data",
        "i do not have access to location data",
        "i don't have access to personal information",
        "i do not have access to personal information",
        "i can't access your location",
        "i cannot access your location",
        "i can't access location data",
        "i cannot access location data",
    ]
    .iter()
    .any(|phrase| answer.contains(phrase));
    if self_reference && denies_knowledge {
        evidence
            .filter(|evidence| !evidence.trim().is_empty())
            .map(|evidence| {
                format!(
                    "Based on what you told me: “{}” I haven't independently verified it.",
                    evidence.trim()
                )
            })
    } else {
        None
    }
}

fn challenge_message(challenges: &[StoredAssistantClaim]) -> Option<Message> {
    if challenges.is_empty() {
        return None;
    }
    let mut content = String::from(
        "Curmudgeon flagged these earlier assistant assertions as inconsistent with sourced claims. Do not repeat them as facts. A challenge is evidence of inconsistency, not proof of a corrected answer. If relevant, acknowledge uncertainty and cite the compared claim IDs:\n",
    );
    for claim in challenges {
        content.push_str(&format!(
            "[assistant claim #{}; usable confidence {:.2}] {} — {} — {}; {}; compared with {}\n",
            claim.id,
            claim.confidence_after,
            claim.subject,
            claim.relation,
            claim.object,
            claim.reason,
            claim.evidence_refs.join(", "),
        ));
    }
    Some(Message {
        role: "system".into(),
        content,
    })
}

fn live_challenges(
    challenges: Vec<StoredAssistantClaim>,
    map: &KnowledgeMap,
    active_db_ids: &HashSet<i64>,
    limit: usize,
) -> Vec<StoredAssistantClaim> {
    challenges
        .into_iter()
        .filter(|challenge| {
            challenge.evidence_refs.iter().any(|reference| {
                reference
                    .strip_prefix("json:")
                    .and_then(|id| id.parse::<u64>().ok())
                    .is_some_and(|id| {
                        map.entries()
                            .iter()
                            .any(|claim| claim.id == id && claim.retracted_by.is_none())
                    })
                    || reference
                        .strip_prefix("db:")
                        .and_then(|id| id.parse::<i64>().ok())
                        .is_some_and(|id| active_db_ids.contains(&id))
            })
        })
        .take(limit)
        .collect()
}

fn is_personal_question(input: &str) -> bool {
    let normalized = input.trim().to_lowercase();
    (normalized.contains('?')
        && [
            "where ", "who ", "what ", "do ", "can ", "have ", "is ", "am ",
        ]
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
        || normalized.starts_with("tell me"))
        && normalized
            .split(|ch: char| !ch.is_alphanumeric())
            .any(|word| matches!(word, "i" | "my" | "mine" | "myself"))
}

fn is_context_dependent(input: &str) -> bool {
    if is_personal_question(input) {
        return true;
    }
    let normalized = input.trim().to_lowercase();
    if normalized.starts_with("what about")
        || normalized.starts_with("and ")
        || normalized.starts_with("also ")
    {
        return true;
    }
    normalized
        .split(|ch: char| !ch.is_alphanumeric())
        .any(|word| {
            matches!(
                word,
                "that"
                    | "those"
                    | "this"
                    | "these"
                    | "it"
                    | "they"
                    | "them"
                    | "same"
                    | "again"
                    | "either"
                    | "too"
                    | "also"
            )
        })
}

fn confirmation_question(evidence: &str) -> String {
    format!(
        "Correct me if I'm wrong, but you said “{}.” Is that right?",
        evidence.trim().trim_end_matches(['.', '!', '?'])
    )
}

fn substantially_echoes(input: &str, answer: &str) -> bool {
    const MIN_ECHO_WORDS: usize = 6;
    let input_words = normalized_words(input);
    let answer_words = normalized_words(answer);
    input_words.len() >= MIN_ECHO_WORDS
        && input_words.windows(MIN_ECHO_WORDS).any(|phrase| {
            answer_words
                .windows(MIN_ECHO_WORDS)
                .any(|window| window == phrase)
        })
}

fn normalized_words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect()
}

fn is_user_entity(subject: &str) -> bool {
    matches!(subject.trim().to_lowercase().as_str(), "user" | "the user")
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn invalid_json(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::ScholarFinding;

    #[test]
    fn scholar_claim_reaches_next_prompt_without_changing_model_files() {
        let directory = std::env::temp_dir().join(format!(
            "community-scholar-prompt-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let database = directory.join("learning.db");
        let adapter = directory.join("adapter.safetensors");
        let checkpoint = directory.join("model.safetensors");
        let mut store = LearningStore::open(&database).unwrap();
        store
            .enqueue_research("conversation", 1, "photosynthesis")
            .unwrap();
        let job = store.claim_scholar_job().unwrap().unwrap();
        store
            .finish_scholar_job(
                &job.id,
                "Photosynthesis",
                "https://en.wikipedia.org/wiki/Photosynthesis",
                "Photosynthesis converts light energy into chemical energy.",
                Some(ScholarFinding {
                    subject: "Photosynthesis".into(),
                    relation: "converts".into(),
                    object: "light energy into chemical energy".into(),
                    evidence_text: "Photosynthesis converts light energy into chemical energy."
                        .into(),
                    confidence: 0.98,
                }),
                None,
            )
            .unwrap();
        drop(store);

        let store = LearningStore::open(&database).unwrap();
        let claims = store
            .relevant_semantic_claims("What does photosynthesis convert?", 5)
            .unwrap();
        assert_eq!(claims.len(), 1);
        let agent = Agent {
            model: "test".into(),
            history: Vec::new(),
            conversation_id: "conversation".into(),
        };
        let prompt = agent.prompt_messages(
            "What does photosynthesis convert?",
            &KnowledgeMap::default(),
            &[],
            &claims,
            &[],
        );
        assert!(
            prompt[1]
                .content
                .contains("Photosynthesis — converts — light energy into chemical energy")
        );
        assert!(
            prompt[1]
                .content
                .contains("https://en.wikipedia.org/wiki/Photosynthesis")
        );
        assert!(!adapter.exists());
        assert!(!checkpoint.exists());
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn retracted_evidence_stops_an_old_challenge_from_entering_prompt() {
        let mut map = KnowledgeMap::default();
        let id = map
            .add("cricket bat", "length", "90cm", "test source")
            .unwrap();
        let challenge = StoredAssistantClaim {
            id: 1,
            reply_id: "reply".into(),
            subject: "cricket bat".into(),
            relation: "length".into(),
            object: "400m".into(),
            evidence_text: "A cricket bat is 400m long.".into(),
            verdict: "challenged".into(),
            reason: "Magnitude mismatch".into(),
            evidence_refs: vec![format!("json:{id}")],
            confidence_before: 0.5,
            confidence_after: 0.1,
        };
        assert_eq!(
            live_challenges(vec![challenge.clone()], &map, &HashSet::new(), 3).len(),
            1
        );
        map.retract(id, "source withdrawn").unwrap();
        assert!(live_challenges(vec![challenge], &map, &HashSet::new(), 3).is_empty());
    }

    #[test]
    fn state_round_trip_and_model_boundary() {
        let path =
            std::env::temp_dir().join(format!("community-agent-{}.json", std::process::id()));
        let mut agent = Agent::load(&path, "model-a").unwrap();
        agent.history.push(Message {
            role: "user".into(),
            content: "hello".into(),
        });
        agent.history.push(Message {
            role: "assistant".into(),
            content: "hi".into(),
        });
        agent.save(&path).unwrap();
        let loaded = Agent::load(&path, "model-a").unwrap();
        assert_eq!(loaded.turn_count(), 1);
        assert_eq!(loaded.conversation_id, agent.conversation_id);
        assert!(Agent::load(&path, "model-b").is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn only_relevant_active_claims_enter_the_prompt() {
        let agent = Agent {
            model: "test".into(),
            history: Vec::new(),
            conversation_id: "test".into(),
        };
        let mut map = KnowledgeMap::default();
        let willow = map
            .add("willow", "grows beside", "streams", "notebook page 3")
            .unwrap();
        map.add("moon", "orbits", "earth", "astronomy book")
            .unwrap();
        let messages = agent.prompt_messages("Tell me about willow", &map, &[], &[], &[]);
        assert!(messages[1].content.contains("notebook page 3"));
        assert!(!messages[1].content.contains("astronomy book"));
        map.retract(willow, "correction").unwrap();
        assert_eq!(
            agent
                .prompt_messages("Tell me about willow", &map, &[], &[], &[])
                .len(),
            2
        );
    }

    #[test]
    fn system_guidance_does_not_inject_example_personal_facts() {
        assert!(SYSTEM_PROMPT.contains("user-reported"));
        assert!(SYSTEM_PROMPT.contains("independently verified"));
        assert!(!SYSTEM_PROMPT.contains("Shifnal"));
    }

    #[test]
    fn curated_self_claim_is_supplied_for_personal_questions() {
        let agent = Agent {
            model: "test".into(),
            history: Vec::new(),
            conversation_id: "test".into(),
        };
        let claim = SemanticClaim {
            id: 3,
            subject: "the user".into(),
            relation: "is named".into(),
            object: "Ryan".into(),
            source: "user observation abc".into(),
            source_observation_id: Some("abc".into()),
        };
        let messages = agent.prompt_messages(
            "Do you know my name?",
            &KnowledgeMap::default(),
            &[],
            &[claim],
            &[],
        );
        assert!(messages[1].content.contains("Ryan"));
        assert!(messages[1].content.contains("observation abc"));
    }

    #[test]
    fn unreviewed_personal_candidate_is_labeled_in_prompt() {
        let agent = Agent {
            model: "test".into(),
            history: Vec::new(),
            conversation_id: "test".into(),
        };
        let candidate = StoredCandidate {
            id: "candidate-a".into(),
            observation_id: "observation-a".into(),
            subject: "the user".into(),
            relation: "is named".into(),
            object: "Ryan".into(),
            evidence_text: "I am Ryan".into(),
            confidence: 0.0,
            status: "needs_review".into(),
            decision_reason: Some("extractor omitted confidence".into()),
            semantic_claim_id: None,
        };
        let messages = agent.prompt_messages(
            "Do you know my name?",
            &KnowledgeMap::default(),
            &[],
            &[],
            &[candidate],
        );
        assert!(messages[1].content.contains("awaiting review"));
        assert!(messages[1].content.contains("Ryan"));
        assert!(messages[1].content.contains("I am Ryan"));
    }

    #[test]
    fn sourced_user_evidence_overrides_a_false_unknown_reply() {
        assert_eq!(
            grounded_user_reply(
                "Do you know my name?",
                "I don't know your name.",
                Some("I am Ryan."),
            )
            .as_deref(),
            Some("Based on what you told me: “I am Ryan.” I haven't independently verified it.")
        );
        assert_eq!(
            grounded_user_reply(
                "Where do I live?",
                "I don't have access to location data or personal information.",
                Some("I live in Shifnal"),
            )
            .as_deref(),
            Some(
                "Based on what you told me: “I live in Shifnal” I haven't independently verified it."
            )
        );
        assert!(
            grounded_user_reply("Tell me a joke", "I don't know.", Some("I am Ryan.")).is_none()
        );
        assert!(
            grounded_user_reply("Do you know my name?", "Hello!", Some("I am Ryan.")).is_none()
        );
    }

    #[test]
    fn uncertain_personal_evidence_prompts_for_confirmation() {
        assert!(is_personal_question("Where do I live?"));
        assert!(is_personal_question("Do you know my name?"));
        assert!(!is_personal_question("Tell me about Shifnal."));
        assert!(is_context_dependent(
            "Are you more confident about those now?"
        ));
        assert!(is_context_dependent("What about my name?"));
        assert!(!is_context_dependent(
            "I mean, generally, about your world understanding?"
        ));
        assert_eq!(
            confirmation_question("I live in Shifnal."),
            "Correct me if I'm wrong, but you said “I live in Shifnal.” Is that right?"
        );
    }

    #[test]
    fn detects_long_copied_phrases_but_allows_normal_acknowledgements() {
        assert!(substantially_echoes(
            "I am well. My name is Ryan. It's a pleasure to meet you.",
            "Hello! My name is Ryan. It's a pleasure to meet you!",
        ));
        assert!(!substantially_echoes(
            "I am well. My name is Ryan. It's a pleasure to meet you.",
            "Nice to meet you, Ryan. I'm doing well, thanks for asking.",
        ));
        assert!(!substantially_echoes(
            "Hello Lyra",
            "Hello! How can I help?"
        ));
    }

    #[test]
    fn full_history_reaches_prompt_in_original_order() {
        let repeated = "Hello! How can I assist you today?";
        let agent = Agent {
            model: "test".into(),
            history: vec![
                Message {
                    role: "user".into(),
                    content: "Hello".into(),
                },
                Message {
                    role: "assistant".into(),
                    content: repeated.into(),
                },
                Message {
                    role: "user".into(),
                    content: "What is your reply limit?".into(),
                },
                Message {
                    role: "assistant".into(),
                    content: repeated.into(),
                },
            ],
            conversation_id: "test".into(),
        };
        let messages =
            agent.prompt_messages("You seem stuck", &KnowledgeMap::default(), &[], &[], &[]);
        assert_eq!(messages.len(), 6);
        assert_eq!(messages[1].content, "Hello");
        assert_eq!(messages[2].content, repeated);
        assert_eq!(messages[3].content, "What is your reply limit?");
        assert_eq!(messages[4].content, repeated);
        assert_eq!(messages[5].content, "You seem stuck");
    }

    #[test]
    fn more_than_twelve_turns_remain_available_in_both_prompts() {
        let mut agent = Agent {
            model: "test".into(),
            history: Vec::new(),
            conversation_id: "test".into(),
        };
        for index in 0..18 {
            agent.history.push(Message {
                role: "user".into(),
                content: format!("Question {index}"),
            });
            agent.history.push(Message {
                role: "assistant".into(),
                content: format!("Answer {index}"),
            });
        }
        let map = KnowledgeMap::default();
        let regular = agent.prompt_messages("Current question", &map, &[], &[], &[]);
        assert_eq!(regular.len(), 38);
        assert_eq!(regular[1].content, "Question 0");
        assert_eq!(regular.last().unwrap().content, "Current question");
        let retry = agent.reference_messages("Current question", &map, &[], &[], &[]);
        assert_eq!(retry.len(), 3);
        assert!(retry[1].content.contains("Question 0"));
        assert!(retry[1].content.contains("Answer 17"));
        assert_eq!(retry[2].content, "Current question");
    }

    #[test]
    fn relevant_pending_observations_are_labeled_before_latest_prompt() {
        let path =
            std::env::temp_dir().join(format!("community-agent-context-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = LearningStore::open(&path).unwrap();
        store
            .stage_observation("conversation-a", 1, "I live in Bristol.")
            .unwrap();
        store
            .stage_observation("conversation-a", 2, "I like painting.")
            .unwrap();
        let current = store
            .stage_observation("conversation-a", 3, "Where do I live?")
            .unwrap();
        let pending = store
            .relevant_observations("conversation-a", "Where do I live?", &current, 8)
            .unwrap();
        let agent = Agent {
            model: "test".into(),
            history: Vec::new(),
            conversation_id: "test".into(),
        };
        let messages = agent.prompt_messages(
            "Where do I live?",
            &KnowledgeMap::default(),
            &pending,
            &[],
            &[],
        );
        assert!(messages[1].content.contains("Relevant user statements"));
        assert!(messages[1].content.contains("I live in Bristol."));
        assert!(!messages[1].content.contains("I like painting."));
        assert_eq!(messages.last().unwrap().content, "Where do I live?");
        drop(store);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }
}
