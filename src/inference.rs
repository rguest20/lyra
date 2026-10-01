//! BF16 Qwen3 inference with a small, trainable output adapter.
use crate::agent::Message;
use candle_core::{DType, Device, Tensor};
use candle_nn::{AdamW, Init, Optimizer, ParamsAdamW, VarBuilder, VarMap};
use candle_transformers::{
    generation::{LogitsProcessor, Sampling},
    models::qwen3::{Config, Model},
};
use safetensors::{
    Dtype, SafeTensors,
    tensor::{TensorView, serialize_to_file},
};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tokenizers::Tokenizer;

const MAX_REPLY: usize = 4096;
const MAX_EXTRACTION_REPLY: usize = 768;
const MAX_BOUNDARY_REPLY: usize = 24;
const MAX_RESEARCH_REPLY: usize = 96;
const MIN_REPLY: usize = 256;
const MAX_TRAIN: usize = 128;
const RANK: usize = 8;
const PREFILL_CHUNK: usize = 256;

pub struct LocalModel {
    base: Model,
    fresh_base: Model,
    output_weight: Tensor,
    consolidated_head: bool,
    down: Tensor,
    up: Tensor,
    vars: VarMap,
    adapter_path: PathBuf,
    tokenizer: Tokenizer,
    device: Device,
    max_context: usize,
    max_prompt: usize,
    last_thinking: Option<String>,
}

impl LocalModel {
    pub fn load(dir: &Path, adapter_path: &Path, cpu: bool) -> io::Result<Self> {
        let checkpoint = dir.join("model.safetensors");
        Self::load_checkpoint(dir, &checkpoint, adapter_path, cpu)
    }

    pub fn load_checkpoint(
        dir: &Path,
        checkpoint: &Path,
        adapter_path: &Path,
        cpu: bool,
    ) -> io::Result<Self> {
        let device = if cpu {
            Device::Cpu
        } else {
            Device::new_metal(0).map_err(other)?
        };
        let config: Config =
            serde_json::from_slice(&fs::read(dir.join("config.json"))?).map_err(other)?;
        let max_context = config.max_position_embeddings;
        let max_prompt = max_context
            .checked_sub(MIN_REPLY)
            .ok_or_else(|| invalid("model context is smaller than the minimum reply reserve"))?;
        // SAFETY: Candle keeps the mapped checkpoint alive through the builder.
        let weights =
            unsafe { VarBuilder::from_mmaped_safetensors(&[checkpoint], DType::BF16, &device) }
                .map_err(other)?;
        let base = Model::new(&config, weights.clone()).map_err(other)?;
        let output_name = if weights.contains_tensor("lm_head.weight") {
            "lm_head.weight"
        } else {
            if !config.tie_word_embeddings {
                return Err(invalid("untied checkpoint is missing lm_head.weight"));
            }
            "model.embed_tokens.weight"
        };
        let output_weight = weights
            .get((config.vocab_size, config.hidden_size), output_name)
            .map_err(other)?;
        let tokenizer = Tokenizer::from_file(dir.join("tokenizer.json")).map_err(other)?;
        for token in ["<|im_end|>", "<|endoftext|>"] {
            if tokenizer.token_to_id(token).is_none() {
                return Err(invalid(&format!("tokenizer is missing {token}")));
            }
        }
        let mut vars = VarMap::new();
        let vb = VarBuilder::from_varmap(&vars, DType::F32, &device);
        let down = vb
            .get_with_hints(
                (config.hidden_size, RANK),
                "down",
                Init::Randn {
                    mean: 0.0,
                    stdev: 0.01,
                },
            )
            .map_err(other)?;
        let up = vb
            .get_with_hints((RANK, config.vocab_size), "up", Init::Const(0.0))
            .map_err(other)?;
        if adapter_path.exists() {
            vars.load(adapter_path).map_err(other)?;
        }
        let fresh_base = base.clone();
        Ok(Self {
            base,
            fresh_base,
            output_weight,
            consolidated_head: output_name == "lm_head.weight",
            down,
            up,
            vars,
            adapter_path: adapter_path.into(),
            tokenizer,
            device,
            max_context,
            max_prompt,
            last_thinking: None,
        })
    }

    fn prefill(&mut self, ids: &[u32]) -> candle_core::Result<Tensor> {
        let mut last = None;
        for (index, chunk) in ids.chunks(PREFILL_CHUNK).enumerate() {
            let input = Tensor::new(chunk, &self.device)?.unsqueeze(0)?;
            last = Some(self.base.forward(&input, index * PREFILL_CHUNK)?.narrow(
                1,
                chunk.len() - 1,
                1,
            )?);
        }
        last.ok_or_else(|| candle_core::Error::Msg("empty model prompt".into()))
    }

    fn logits(&self, hidden: &Tensor) -> candle_core::Result<Tensor> {
        let hidden = hidden.squeeze(1)?;
        let baseline = hidden
            .matmul(&self.output_weight.t()?)?
            .to_dtype(DType::F32)?;
        let delta = hidden
            .to_dtype(DType::F32)?
            .matmul(&self.down)?
            .matmul(&self.up)?;
        baseline + delta
    }

    pub fn chat(
        &mut self,
        messages: &[Message],
        extra_prompt_tokens: usize,
        show_thinking: bool,
    ) -> io::Result<String> {
        let temperature = if self.consolidated_head { 0.1 } else { 0.7 };
        self.chat_with_reply_cap(
            messages,
            extra_prompt_tokens,
            MAX_REPLY,
            temperature,
            show_thinking,
        )
    }

    pub fn chat_for_extraction(&mut self, messages: &[Message]) -> io::Result<String> {
        self.chat_with_reply_cap(messages, 0, MAX_EXTRACTION_REPLY, 0.1, false)
    }

    pub fn probe(&mut self, prompt: &str) -> io::Result<String> {
        let messages = [
            Message {
                role: "system".into(),
                content: "/no_think\nAnswer the question briefly.".into(),
            },
            Message {
                role: "user".into(),
                content: prompt.into(),
            },
        ];
        self.chat_with_reply_cap(&messages, 0, 80, 0.1, false)
    }

    pub fn probe_chat(&mut self, prompt: &str) -> io::Result<String> {
        let messages = [
            Message {
                role: "system".into(),
                content: crate::agent::SYSTEM_PROMPT.into(),
            },
            Message {
                role: "user".into(),
                content: prompt.into(),
            },
        ];
        self.chat_with_reply_cap(&messages, 0, 128, 0.1, false)
    }

    /// Ask the local model whether the latest user message starts a separate
    /// conversational thread. An unparseable judgment conservatively continues.
    pub fn starts_new_conversation(
        &mut self,
        history: &[Message],
        latest: &str,
    ) -> io::Result<bool> {
        let messages = boundary_messages(history, latest);
        let result = self.chat_with_reply_cap(&messages, 0, MAX_BOUNDARY_REPLY, 0.1, false)?;
        Ok(boundary_decision(&result))
    }

    /// Return a short public search query only when the reply lacks a useful
    /// answer to a factual question. The scholar never receives chat history.
    pub fn research_query(&mut self, question: &str, answer: &str) -> io::Result<Option<String>> {
        if obvious_knowledge_gap(question, answer) {
            return Ok(Some(question.trim().to_owned()));
        }
        let messages = [
            Message {
                role: "system".into(),
                content: "/no_think\nDecide whether an encyclopedia lookup could fill a factual gap in the assistant's answer. If the answer already addresses the question, or the question is about the user, the conversation, advice, a creative task, or a personal opinion, return exactly NO_RESEARCH. If external public information is needed, return exactly SEARCH: followed by a short encyclopedia search phrase. Remove all names and personal details supplied by the user unless they are public figures or topics. Do not add explanation.".into(),
            },
            Message {
                role: "user".into(),
                content: format!("Question: {question}\nAssistant reply: {answer}"),
            },
        ];
        let result = self.chat_with_reply_cap(&messages, 0, MAX_RESEARCH_REPLY, 0.1, false)?;
        Ok(result
            .strip_prefix("SEARCH:")
            .map(str::trim)
            .filter(|query| {
                !query.is_empty() && query.chars().count() <= 160 && !query.contains('\n')
            })
            .map(str::to_owned))
    }

    pub fn search_phrase(&mut self, question: &str) -> io::Result<Option<String>> {
        let messages = [
            Message {
                role: "system".into(),
                content: "/no_think\nTurn the research question into a short encyclopedia search phrase naming its main subject. Return exactly SEARCH: followed by 2 to 8 words. Remove question words and omit personal details. Example: `Who was the first editor of The Journal of Hellenic Studies?` -> `SEARCH: The Journal of Hellenic Studies`. Return no explanation.".into(),
            },
            Message { role: "user".into(), content: question.into() },
        ];
        let result = self.chat_with_reply_cap(&messages, 0, MAX_RESEARCH_REPLY, 0.1, false)?;
        Ok(result
            .strip_prefix("SEARCH:")
            .map(str::trim)
            .filter(|phrase| {
                !phrase.is_empty() && phrase.chars().count() <= 120 && !phrase.contains('\n')
            })
            .map(str::to_owned))
    }

    fn chat_with_reply_cap(
        &mut self,
        messages: &[Message],
        extra_prompt_tokens: usize,
        base_reply_cap: usize,
        temperature: f64,
        show_thinking: bool,
    ) -> io::Result<String> {
        self.last_thinking = None;
        let ids = self
            .tokenizer
            .encode(format_prompt(messages, show_thinking), true)
            .map_err(other)?
            .get_ids()
            .to_vec();
        if ids.is_empty() || ids.len() > self.max_prompt {
            return Err(invalid(&format!(
                "full conversation has {} prompt tokens; this model supports at most {} while reserving at least {} tokens for the reply",
                ids.len(),
                self.max_prompt,
                MIN_REPLY
            )));
        }
        let reply_limit = reply_budget(
            ids.len(),
            extra_prompt_tokens,
            self.max_context,
            base_reply_cap,
        );
        let eos = self.tokenizer.token_to_id("<|im_end|>").unwrap();
        let endoftext = self.tokenizer.token_to_id("<|endoftext|>").unwrap();
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(other)?
            .as_nanos() as u64;
        let mut sampler = LogitsProcessor::from_sampling(
            seed,
            Sampling::TopKThenTopP {
                k: 20,
                p: 0.8,
                temperature,
            },
        );
        self.base = self.fresh_base.clone();
        let device = self.device.clone();
        let output = device
            .with_context(|| -> candle_core::Result<Vec<u32>> {
                let mut hidden = self.prefill(&ids)?;
                let mut output = Vec::new();
                for step in 0..reply_limit {
                    let token = sampler.sample(&self.logits(&hidden)?.squeeze(0)?)?;
                    if token == eos || token == endoftext {
                        break;
                    }
                    output.push(token);
                    if step + 1 < reply_limit {
                        hidden = self.base.forward(
                            &Tensor::new(&[token], &self.device)?.unsqueeze(0)?,
                            ids.len() + step,
                        )?;
                    }
                }
                Ok(output)
            })
            .map_err(other)?;
        let decoded = self.tokenizer.decode(&output, true).map_err(other)?;
        if show_thinking {
            let (thinking, answer) = split_thinking(&decoded)?;
            self.last_thinking = thinking.filter(|text| !text.is_empty());
            Ok(answer.to_owned())
        } else {
            visible_reply(&decoded).map(str::to_owned)
        }
    }

    pub fn take_thinking(&mut self) -> Option<String> {
        self.last_thinking.take()
    }

    pub fn additional_prompt_tokens(
        &self,
        baseline: &[Message],
        expanded: &[Message],
    ) -> io::Result<usize> {
        let baseline = self.prompt_ids(baseline)?.len();
        let expanded = self.prompt_ids(expanded)?.len();
        Ok(expanded.saturating_sub(baseline))
    }

    fn prompt_ids(&self, messages: &[Message]) -> io::Result<Vec<u32>> {
        self.tokenizer
            .encode(format_prompt(messages, false), true)
            .map(|encoding| encoding.get_ids().to_vec())
            .map_err(other)
    }

    /// Train the output adapter on one reviewed prompt/answer pair and persist it.
    pub fn train(&mut self, prompt: &str, answer: &str) -> io::Result<(usize, f32)> {
        self.train_with_system("/no_think", prompt, answer)
    }

    pub fn train_with_system(
        &mut self,
        system: &str,
        prompt: &str,
        answer: &str,
    ) -> io::Result<(usize, f32)> {
        if prompt.trim().is_empty() || answer.trim().is_empty() {
            return Err(invalid("training prompt and answer must be nonempty"));
        }
        let messages = [
            Message {
                role: "system".into(),
                content: system.into(),
            },
            Message {
                role: "user".into(),
                content: prompt.into(),
            },
        ];
        let ids = self
            .tokenizer
            .encode(format_prompt(&messages, false), true)
            .map_err(other)?
            .get_ids()
            .to_vec();
        let mut targets = self
            .tokenizer
            .encode(answer, false)
            .map_err(other)?
            .get_ids()
            .to_vec();
        if targets.is_empty() || targets.len() > MAX_TRAIN {
            return Err(invalid("training answer exceeds 128 tokens"));
        }
        targets.push(self.tokenizer.token_to_id("<|im_end|>").unwrap());
        if ids.len() + targets.len() > self.max_context {
            return Err(invalid("training example exceeds context limit"));
        }
        let mut optimizer = AdamW::new(
            self.vars.all_vars(),
            ParamsAdamW {
                lr: 0.001,
                weight_decay: 0.0,
                ..Default::default()
            },
        )
        .map_err(other)?;
        self.base = self.fresh_base.clone();
        let device = self.device.clone();
        let result = device
            .with_context(|| -> candle_core::Result<(usize, f32)> {
                let mut hidden = self.prefill(&ids)?;
                let mut sum = 0.0;
                for (step, &target) in targets.iter().enumerate() {
                    let logits = self.logits(&hidden.detach())?;
                    let label = Tensor::new(&[target], &self.device)?;
                    let loss = candle_nn::loss::cross_entropy(&logits, &label)?;
                    sum += loss.to_scalar::<f32>()?;
                    optimizer.backward_step(&loss)?;
                    if step + 1 < targets.len() {
                        hidden = self.base.forward(
                            &Tensor::new(&[target], &self.device)?.unsqueeze(0)?,
                            ids.len() + step,
                        )?;
                    }
                }
                Ok((targets.len(), sum / targets.len() as f32))
            })
            .map_err(other)?;
        if let Some(parent) = self
            .adapter_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let temp = self.adapter_path.with_extension("safetensors.tmp");
        self.vars.save(&temp).map_err(other)?;
        fs::rename(temp, &self.adapter_path)?;
        Ok(result)
    }

    /// Fold the trained output adapter into an independent output head in a
    /// new checkpoint. The source checkpoint is never modified.
    pub fn save_merged_checkpoint(&self, source: &Path, destination: &Path) -> io::Result<()> {
        if destination.exists() {
            return Err(invalid("destination checkpoint already exists"));
        }
        let down = self
            .down
            .flatten_all()
            .and_then(|tensor| tensor.to_vec1::<f32>())
            .map_err(other)?;
        let up = self
            .up
            .flatten_all()
            .and_then(|tensor| tensor.to_vec1::<f32>())
            .map_err(other)?;
        let source_bytes = fs::read(source)?;
        let tensors = SafeTensors::deserialize(&source_bytes).map_err(other)?;
        let original_head = tensors
            .tensor("lm_head.weight")
            .or_else(|_| tensors.tensor("model.embed_tokens.weight"))
            .map_err(other)?;
        if original_head.dtype() != Dtype::BF16 {
            return Err(invalid("checkpoint output head must be BF16"));
        }
        let shape = original_head.shape().to_vec();
        if shape.len() != 2 {
            return Err(invalid("checkpoint output head must be a matrix"));
        }
        let mut merged = original_head.data().to_vec();
        merge_output_head(&mut merged, shape[0], shape[1], &down, &up)?;
        let mut output = tensors
            .tensors()
            .into_iter()
            .filter(|(name, _)| name != "lm_head.weight")
            .collect::<Vec<_>>();
        output.push((
            "lm_head.weight".to_owned(),
            TensorView::new(Dtype::BF16, shape, &merged).map_err(other)?,
        ));
        if let Some(parent) = destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let temporary = destination.with_extension("safetensors.tmp");
        if temporary.exists() {
            return Err(invalid("temporary checkpoint already exists"));
        }
        serialize_to_file(output, None, &temporary).map_err(other)?;
        fs::rename(temporary, destination)
    }
}

fn merge_output_head(
    head: &mut [u8],
    vocab: usize,
    hidden: usize,
    down: &[f32],
    up: &[f32],
) -> io::Result<()> {
    if head.len() != vocab * hidden * 2 || down.len() != hidden * RANK || up.len() != RANK * vocab {
        return Err(invalid("adapter and output head shapes do not match"));
    }
    for token in 0..vocab {
        for dimension in 0..hidden {
            let index = (token * hidden + dimension) * 2;
            let previous =
                f32::from_bits((u16::from_le_bytes([head[index], head[index + 1]]) as u32) << 16);
            let mut delta = 0.0;
            for rank in 0..RANK {
                delta += down[dimension * RANK + rank] * up[rank * vocab + token];
            }
            let value = previous + delta;
            if !value.is_finite() {
                return Err(invalid("merged checkpoint has nonfinite weights"));
            }
            let bits = value.to_bits();
            let rounded = ((bits + 0x7fff + ((bits >> 16) & 1)) >> 16) as u16;
            head[index..index + 2].copy_from_slice(&rounded.to_le_bytes());
        }
    }
    Ok(())
}

fn obvious_knowledge_gap(question: &str, answer: &str) -> bool {
    let public_question = question.contains('?')
        && !question
            .to_lowercase()
            .split(|ch: char| !ch.is_alphanumeric())
            .any(|word| matches!(word, "i" | "me" | "my" | "mine" | "you" | "your"));
    let answer = answer.trim().to_lowercase();
    public_question
        && question.chars().count() <= 160
        && [
            "i don't know",
            "i do not know",
            "i'm not sure",
            "i am not sure",
            "i can't answer",
            "i cannot answer",
            "could you clarify",
            "what would you like to talk about?",
        ]
        .iter()
        .any(|phrase| answer.contains(phrase))
}

fn reply_budget(
    prompt_tokens: usize,
    extra_prompt_tokens: usize,
    max_context: usize,
    base_reply_cap: usize,
) -> usize {
    base_reply_cap
        .saturating_add(extra_prompt_tokens)
        .min(max_context.saturating_sub(prompt_tokens))
}

fn boundary_messages(history: &[Message], latest: &str) -> Vec<Message> {
    let mut transcript = String::new();
    for message in history.iter().rev().take(16).rev() {
        transcript.push_str(if message.role == "user" {
            "User: "
        } else {
            "Assistant: "
        });
        transcript.push_str(&message.content);
        transcript.push('\n');
    }
    vec![
        Message {
            role: "system".into(),
            content: "/no_think\nYou judge conversational continuity. Decide whether the latest user message has moved to a distinct conversational thread such that earlier dialogue is likely to confuse the reply. Judge meaning, intent, and discourse function, not shared or different words. Treat a change from one domain to an independent domain as a new conversation even when no switch is announced. For example, moving from recalling the user's personal details to discussing the agent's general world knowledge is a new thread; asking whether the agent is more confident about those personal details is a follow-up. A request to disregard prior context or start afresh also begins a new conversation, even if phrased indirectly. Continue for follow-up questions, corrections, confirmations, clarifications, and questions that depend on earlier turns. When uncertain, continue. Return exactly NEW_CONVERSATION or CONTINUE, with no explanation.".into(),
        },
        Message {
            role: "user".into(),
            content: format!("Recent conversation:\n{transcript}\nLatest user message:\n{latest}"),
        },
    ]
}

fn boundary_decision(reply: &str) -> bool {
    reply.trim().eq_ignore_ascii_case("NEW_CONVERSATION")
}

fn format_prompt(messages: &[Message], show_thinking: bool) -> String {
    let mut out = String::new();
    for message in messages {
        out.push_str("<|im_start|>");
        out.push_str(&message.role);
        out.push('\n');
        out.push_str(&message.content);
        out.push_str("<|im_end|>\n");
    }
    if show_thinking {
        out.push_str("<|im_start|>assistant\n<think>\n");
    } else {
        out.push_str("<|im_start|>assistant\n<think>\n\n</think>\n\n");
    }
    out
}

fn split_thinking(decoded: &str) -> io::Result<(Option<String>, &str)> {
    let text = decoded.trim();
    let text = text.strip_prefix("<think>").unwrap_or(text);
    let (thinking, answer) = text
        .split_once("</think>")
        .ok_or_else(|| invalid("model used its reply budget before answering"))?;
    Ok((Some(thinking.trim().to_owned()), answer.trim()))
}

fn visible_reply(decoded: &str) -> io::Result<&str> {
    let reply = decoded.trim();
    if let Some(thinking) = reply.strip_prefix("<think>") {
        let (_, answer) = thinking
            .split_once("</think>")
            .ok_or_else(|| invalid("model used its reply budget before answering"))?;
        return Ok(answer.trim());
    }
    Ok(reply)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        boundary_decision, boundary_messages, format_prompt, merge_output_head,
        obvious_knowledge_gap, reply_budget, split_thinking, visible_reply,
    };
    use crate::agent::Message;
    #[test]
    fn adapter_delta_is_folded_into_bf16_output_head() {
        let mut head = 0x3f80u16.to_le_bytes().to_vec(); // 1.0
        let mut down = [0.0f32; 8];
        let mut up = [0.0f32; 8];
        down[0] = 1.0;
        up[0] = 0.25;
        merge_output_head(&mut head, 1, 1, &down, &up).unwrap();
        assert_eq!(u16::from_le_bytes([head[0], head[1]]), 0x3fa0); // 1.25
    }
    #[test]
    fn unanswered_public_question_queues_research_without_personal_lookup() {
        assert!(obvious_knowledge_gap(
            "Who was the first editor of The Journal of Hellenic Studies?",
            "Thanks for sharing that. What would you like to talk about?"
        ));
        assert!(!obvious_knowledge_gap(
            "Where do I live?",
            "I don't know where you live."
        ));
    }
    #[test]
    fn qwen_template() {
        assert_eq!(
            format_prompt(
                &[Message {
                    role: "user".into(),
                    content: "Hi".into()
                }],
                false
            ),
            "<|im_start|>user\nHi<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"
        );
    }

    #[test]
    fn debug_template_leaves_thinking_open_and_splits_it_from_answer() {
        assert!(format_prompt(&[], true).ends_with("<|im_start|>assistant\n<think>\n"));
        assert_eq!(
            split_thinking("Let me reason.\n</think>\nAnswer").unwrap(),
            (Some("Let me reason.".into()), "Answer")
        );
    }

    #[test]
    fn continuity_judge_uses_recent_meaning_and_only_accepts_clear_reset_label() {
        let history = [
            Message {
                role: "user".into(),
                content: "We were discussing my name.".into(),
            },
            Message {
                role: "assistant".into(),
                content: "You said Ryan.".into(),
            },
        ];
        let messages = boundary_messages(
            &history,
            "I mean generally, about your world understanding?",
        );
        assert!(
            messages[0]
                .content
                .contains("Judge meaning, intent, and discourse function")
        );
        assert!(messages[1].content.contains("We were discussing my name."));
        assert!(messages[1].content.contains("Latest user message:"));
        assert!(boundary_decision("NEW_CONVERSATION"));
        assert!(!boundary_decision("CONTINUE"));
        assert!(!boundary_decision("I think this is a new conversation"));
    }

    #[test]
    fn hides_qwen_thinking_wrapper() {
        assert_eq!(
            visible_reply("<think>\n\n</think>\n\nHello!").unwrap(),
            "Hello!"
        );
    }

    #[test]
    fn reply_budget_grows_with_added_observation_context_and_respects_model_limit() {
        assert_eq!(reply_budget(12_000, 3_500, 40_960, 4096), 7_596);
        assert_eq!(reply_budget(39_000, 3_500, 40_960, 4096), 1_960);
    }
}
