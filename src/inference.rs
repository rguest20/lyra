//! BF16 Qwen3 inference with a small, trainable output adapter.
use crate::agent::Message;
use candle_core::{DType, Device, Tensor};
use candle_nn::{AdamW, Init, Optimizer, ParamsAdamW, VarBuilder, VarMap};
use candle_transformers::{
    generation::{LogitsProcessor, Sampling},
    models::qwen3::{Config, Model},
};
use safetensors::{
    Dtype,
    tensor::{TensorView, serialize_to_file},
};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tokenizers::Tokenizer;

const MAX_REPLY: usize = 4096;
// Treat this as a ceiling, not a target. `reply_budget` still limits the
// generated output to the model's remaining context window. A high ceiling
// prevents structured extraction replies from being truncated at 768 tokens.
// Structured extraction should be compact. A huge cap lets a malformed or
// unbounded model response consume the entire context window before the
// caller can validate it and apply its bounded repair policy.
const MAX_EXTRACTION_REPLY: usize = 1_024;
const MAX_RESEARCH_REPLY: usize = 96;
const MIN_REPLY: usize = 256;
const MAX_TRAIN: usize = 128;
const RANK: usize = 8;
const PREFILL_CHUNK: usize = 256;

fn training_target_ids(
    tokenizer: &Tokenizer,
    answer: &str,
    prompt_ids: &[u32],
    max_context: usize,
) -> io::Result<Vec<u32>> {
    let mut targets = tokenizer
        .encode(answer, false)
        .map_err(other)?
        .get_ids()
        .to_vec();
    if targets.is_empty() || targets.len() > MAX_TRAIN {
        return Err(invalid("training answer exceeds 128 tokens"));
    }
    targets.push(tokenizer.token_to_id("<|im_end|>").unwrap());
    if prompt_ids.len() + targets.len() > max_context {
        return Err(invalid("training example exceeds context limit"));
    }
    Ok(targets)
}

pub mod assignments;
mod mlx;

pub enum LocalModel {
    Candle(Box<CandleModel>),
    Mlx(mlx::MlxModel),
    Shared(assignments::RoutedModel),
}

impl LocalModel {
    pub fn model_id(&self) -> io::Result<String> {
        match self {
            Self::Shared(model) => model.model_id(),
            _ => Ok(std::env::var("COMMUNITY_MODEL_ID").unwrap_or_else(|_| "local".into())),
        }
    }
    pub fn load(dir: &Path, adapter: &Path, cpu: bool) -> io::Result<Self> {
        Self::load_checkpoint(dir, &checkpoint_path(dir)?, adapter, cpu)
    }

    pub fn load_for_chat(dir: &Path, adapter: &Path, cpu: bool) -> io::Result<Self> {
        let shared = model_lease_path();
        let lease = std::env::var_os("COMMUNITY_CHAT_MODEL_LOCK")
            .map_or_else(|| chat_lease_path(&shared), PathBuf::from);
        Self::load_checkpoint_with_lease(dir, &checkpoint_path(dir)?, adapter, cpu, &lease)
    }

    pub fn load_checkpoint(
        dir: &Path,
        checkpoint: &Path,
        adapter: &Path,
        cpu: bool,
    ) -> io::Result<Self> {
        Self::load_checkpoint_with_lease(dir, checkpoint, adapter, cpu, &model_lease_path())
    }

    fn load_checkpoint_with_lease(
        dir: &Path,
        checkpoint: &Path,
        adapter: &Path,
        cpu: bool,
        lease: &Path,
    ) -> io::Result<Self> {
        let config: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.join("config.json"))?).map_err(other)?;
        match config["model_type"].as_str() {
            Some("qwen3") => Ok(Self::Candle(Box::new(CandleModel::load_checkpoint(
                dir, checkpoint, adapter, cpu, lease,
            )?))),
            Some("qwen3_5") => Ok(Self::Mlx(mlx::MlxModel::load(
                dir, checkpoint, adapter, cpu, lease,
            )?)),
            _ => Err(invalid(
                "unsupported model architecture; expected qwen3 or qwen3_5",
            )),
        }
    }

    fn is_consolidated(&self) -> io::Result<bool> {
        match self {
            Self::Candle(model) => Ok(model.consolidated_head),
            Self::Mlx(model) => Ok(model.consolidated),
            Self::Shared(model) => model.with_model(|model| model.is_consolidated()),
        }
    }

    pub fn chat(
        &mut self,
        messages: &[Message],
        extra_prompt_tokens: usize,
        show_thinking: bool,
    ) -> io::Result<String> {
        let temperature = if self.is_consolidated()? { 0.1 } else { 0.7 };
        self.chat_with_reply_cap(
            messages,
            extra_prompt_tokens,
            MAX_REPLY,
            temperature,
            show_thinking,
        )
    }

    pub fn chat_for_extraction_batch(
        &mut self,
        conversations: &[Vec<Message>],
    ) -> Vec<io::Result<String>> {
        self.chat_batch(conversations, 0, MAX_EXTRACTION_REPLY, 0.1, false)
    }

    /// Generate independent conversations in bounded, same-model batches.
    /// Return values retain input order; dependent follow-ups require a later call.
    pub fn chat_batch(
        &mut self,
        conversations: &[Vec<Message>],
        extra_tokens: usize,
        cap: usize,
        temperature: f64,
        thinking: bool,
    ) -> Vec<io::Result<String>> {
        if conversations.is_empty() {
            return Vec::new();
        }
        match self {
            Self::Mlx(model) => {
                model.chat_batch(conversations, extra_tokens, cap, temperature, thinking)
            }
            Self::Shared(model) => {
                model.generate_batch(conversations, extra_tokens, cap, temperature, thinking)
            }
            Self::Candle(_) => conversations
                .iter()
                .map(|messages| {
                    self.chat_with_reply_cap(messages, extra_tokens, cap, temperature, thinking)
                })
                .collect(),
        }
    }

    pub fn chat_for_extraction(&mut self, messages: &[Message]) -> io::Result<String> {
        self.chat_for_extraction_with_budget(messages, MAX_EXTRACTION_REPLY)
    }

    pub fn chat_for_extraction_with_budget(
        &mut self,
        messages: &[Message],
        max_tokens: usize,
    ) -> io::Result<String> {
        self.chat_with_reply_cap(messages, 0, max_tokens.clamp(1, MAX_REPLY), 0.1, false)
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

    pub fn search_phrase(&mut self, question: &str) -> io::Result<Option<String>> {
        let messages = [
            Message {
                role: "system".into(),
                content: "/no_think\nTurn the research request into a short encyclopedia search phrase. Preserve the named subject and the requested property, comparison, time, and scope needed to find evidence answering it. Do not reduce a question about a specific attribute to the subject alone. Remove grammatical question wording and omit private personal details. Return exactly SEARCH: followed by 2 to 8 words, with no explanation. Example: `What was the approximate age of the Trapezium stars?` -> `SEARCH: Trapezium stars approximate age`.".into(),
            },
            Message { role: "user".into(), content: question.into() },
        ];
        let result = self.chat_with_reply_cap(&messages, 0, MAX_RESEARCH_REPLY, 0.1, false)?;
        Ok(result
            .trim()
            .strip_prefix("SEARCH:")
            .map(str::trim)
            .filter(|phrase| {
                !phrase.is_empty() && phrase.chars().count() <= 120 && !phrase.contains('\n')
            })
            .map(str::to_owned))
    }

    pub fn scout_question(
        &mut self,
        subject: &str,
        known_claims: &[String],
        previous_questions: &[String],
    ) -> io::Result<String> {
        let messages = [
            Message {
                role: "system".into(),
                content: "/no_think\nYou are checking encyclopedia claims for one concrete missing detail. Write one focused factual research question whose answer would add a new fact about the named subject. Use the known claims as clues, but do not assume unstated details are true. Do not ask about a detail already answered in the known claims or repeat previous questions. Avoid broad questions such as 'What are the key facts?' or 'What is it?'. The question must name the subject. Return only QUESTION: followed by the question, with no explanation. Example: subject 'Chicken soup', known summary 'usually with various other ingredients' -> QUESTION: What other ingredients are commonly used in chicken soup?".into(),
            },
            Message {
                role: "user".into(),
                content: format!(
                    "Subject: {subject}\nKnown claims: {}\nPrevious research questions: {}",
                    known_claims.join(" | "),
                    if previous_questions.is_empty() {
                        "none".into()
                    } else {
                        previous_questions.join(" | ")
                    }
                ),
            },
        ];
        self.chat_with_reply_cap(&messages, 0, MAX_RESEARCH_REPLY, 0.1, false)
    }

    /// Ask for a public research lead from pretrained knowledge. The reply is
    /// only a question for Scholar; it is never evidence for a claim.
    pub fn scout_model_lead(&mut self, seed: &str) -> io::Result<String> {
        let subject = seed
            .split_once(':')
            .map_or(seed, |(_, subject)| subject.trim());
        // The scheduler chooses a concrete starting subject; the small model
        // formulates its factual question instead of copying a category label.
        let question = self.scout_question(subject, &[], &[])?;
        let question = question
            .trim()
            .strip_prefix("QUESTION:")
            .unwrap_or(question.trim())
            .trim();
        Ok(format!("TOPIC: {subject}\nQUESTION: {question}"))
    }

    fn chat_with_reply_cap(
        &mut self,
        messages: &[Message],
        extra_tokens: usize,
        cap: usize,
        temperature: f64,
        thinking: bool,
    ) -> io::Result<String> {
        match self {
            Self::Candle(model) => {
                model.chat_with_reply_cap(messages, extra_tokens, cap, temperature, thinking)
            }
            Self::Mlx(model) => model.chat(messages, extra_tokens, cap, temperature, thinking),
            Self::Shared(model) => {
                model.generate(messages, extra_tokens, cap, temperature, thinking)
            }
        }
    }
    pub fn take_thinking(&mut self) -> Option<String> {
        match self {
            Self::Candle(model) => model.take_thinking(),
            Self::Mlx(model) => model.last_thinking.take(),
            Self::Shared(model) => model.last_thinking.take(),
        }
    }
    pub fn additional_prompt_tokens(
        &self,
        baseline: &[Message],
        expanded: &[Message],
    ) -> io::Result<usize> {
        match self {
            Self::Candle(model) => model.additional_prompt_tokens(baseline, expanded),
            Self::Mlx(model) => model.additional_prompt_tokens(baseline, expanded),
            Self::Shared(model) => {
                model.with_model(|model| model.additional_prompt_tokens(baseline, expanded))
            }
        }
    }
    pub fn train_with_system(
        &mut self,
        system: &str,
        prompt: &str,
        answer: &str,
    ) -> io::Result<(usize, f32)> {
        match self {
            Self::Candle(model) => model.train_with_system(system, prompt, answer),
            Self::Mlx(model) => model.train(system, prompt, answer),
            Self::Shared(model) => {
                model.with_model(|model| model.train_with_system(system, prompt, answer))
            }
        }
    }
    pub fn save_merged_checkpoint(&self, source: &Path, destination: &Path) -> io::Result<()> {
        match self {
            Self::Candle(model) => model.save_merged_checkpoint(source, destination),
            Self::Mlx(model) => model.save_merged_checkpoint(source, destination),
            Self::Shared(model) => {
                model.with_model(|model| model.save_merged_checkpoint(source, destination))
            }
        }
    }
}

pub struct CandleModel {
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
    supports_thinking: bool,
    _device_lease: fs::File,
}

impl CandleModel {
    pub fn load_checkpoint(
        dir: &Path,
        checkpoint: &Path,
        adapter_path: &Path,
        cpu: bool,
        lease_path: &Path,
    ) -> io::Result<Self> {
        let device_lease = acquire_model_lease(lease_path)?;
        let device = if cpu {
            Device::Cpu
        } else {
            Device::new_metal(0).map_err(other)?
        };
        let raw_config: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.join("config.json"))?).map_err(other)?;
        if raw_config["model_type"] != "qwen3"
            || raw_config
                .get("quantization_config")
                .is_some_and(|value| !value.is_null())
        {
            return Err(invalid(
                "this runtime requires a non-quantized Qwen3 checkpoint",
            ));
        }
        let mut config: Config = serde_json::from_value(raw_config).map_err(other)?;
        let max_context = context_limit(
            config.max_position_embeddings,
            std::env::var("COMMUNITY_CONTEXT_TOKENS").ok().as_deref(),
        )?;
        config.max_position_embeddings = max_context;
        let supports_thinking = checkpoint_supports_thinking(dir)?;
        let checkpoints = checkpoint_files(checkpoint)?;
        let max_prompt = max_context
            .checked_sub(MIN_REPLY)
            .ok_or_else(|| invalid("model context is smaller than the minimum reply reserve"))?;
        // SAFETY: Candle keeps the mapped checkpoint alive through the builder.
        let weights =
            unsafe { VarBuilder::from_mmaped_safetensors(&checkpoints, DType::BF16, &device) }
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
        crate::diagnostics::event(
            "model.loaded",
            serde_json::json!({
                "model_dir": dir.to_string_lossy(), "precision": "BF16", "shard_count": checkpoints.len(),
                "context_tokens": max_context, "device": if cpu { "cpu" } else { "metal" },
                "chat_template": if supports_thinking { "qwen3_thinking" } else { "qwen3_instruct" },
                "adapter_path": adapter_path.to_string_lossy(), "adapter_loaded": adapter_path.exists(),
            }),
        );
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
            supports_thinking,
            _device_lease: device_lease,
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
            .encode(
                format_prompt_for_model(messages, show_thinking, self.supports_thinking),
                true,
            )
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
        if show_thinking && self.supports_thinking {
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
            .encode(
                format_prompt_for_model(messages, false, self.supports_thinking),
                true,
            )
            .map(|encoding| encoding.get_ids().to_vec())
            .map_err(other)
    }

    /// Train the output adapter on one reviewed prompt/answer pair and persist it.
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
            .encode(
                format_prompt_for_model(&messages, false, self.supports_thinking),
                true,
            )
            .map_err(other)?
            .get_ids()
            .to_vec();
        let targets = training_target_ids(&self.tokenizer, answer, &ids, self.max_context)?;
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
        write_merged_checkpoint(source, destination, &down, &up)
    }
}

fn write_merged_checkpoint(
    source: &Path,
    destination: &Path,
    down: &[f32],
    up: &[f32],
) -> io::Result<()> {
    if destination.exists() {
        return Err(invalid("destination checkpoint already exists"));
    }
    let checkpoints = checkpoint_files(source)?;
    // SAFETY: The mappings remain alive while serialization reads their views.
    let tensors = unsafe { candle_core::safetensors::MmapedSafetensors::multi(&checkpoints) }
        .map_err(other)?;
    let original_head = tensors
        .get("lm_head.weight")
        .or_else(|_| tensors.get("model.embed_tokens.weight"))
        .map_err(other)?;
    if original_head.dtype() != Dtype::BF16 {
        return Err(invalid("checkpoint output head must be BF16"));
    }
    let shape = original_head.shape().to_vec();
    if shape.len() != 2 {
        return Err(invalid("checkpoint output head must be a matrix"));
    }
    let mut merged = original_head.data().to_vec();
    merge_output_head(&mut merged, shape[0], shape[1], down, up)?;
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

fn model_lease_path() -> PathBuf {
    std::env::var_os("COMMUNITY_MODEL_LOCK").map_or_else(
        || PathBuf::from("state/model-inference.lock"),
        PathBuf::from,
    )
}

fn chat_lease_path(shared: &Path) -> PathBuf {
    let mut path = shared.as_os_str().to_os_string();
    path.push(".chat");
    PathBuf::from(path)
}

fn acquire_model_lease(path: &Path) -> io::Result<fs::File> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => {}
        Err(fs::TryLockError::WouldBlock) => {
            crate::diagnostics::event(
                "model.waiting_for_lease",
                serde_json::json!({"lock_path": path.to_string_lossy(), "reason": "another process is using the local model"}),
            );
            file.lock()?;
        }
        Err(fs::TryLockError::Error(error)) => return Err(error),
    }
    Ok(file)
}

/// Discover the canonical entry point without guessing shard filenames.
pub(crate) fn checkpoint_path(dir: &Path) -> io::Result<PathBuf> {
    let single = dir.join("model.safetensors");
    if single.is_file() {
        return Ok(single);
    }
    let index = dir.join("model.safetensors.index.json");
    if index.is_file() {
        return Ok(index);
    }
    Err(invalid(
        "model directory has neither model.safetensors nor model.safetensors.index.json",
    ))
}

pub(crate) fn checkpoint_files(checkpoint: &Path) -> io::Result<Vec<PathBuf>> {
    if checkpoint
        .extension()
        .is_some_and(|value| value == "safetensors")
    {
        if !checkpoint.is_file() {
            return Err(invalid("checkpoint file is missing"));
        }
        return Ok(vec![checkpoint.to_owned()]);
    }
    let index: serde_json::Value = serde_json::from_slice(&fs::read(checkpoint)?).map_err(other)?;
    let map = index["weight_map"]
        .as_object()
        .filter(|map| !map.is_empty())
        .ok_or_else(|| invalid("checkpoint index must have a nonempty weight_map"))?;
    let directory = checkpoint
        .parent()
        .ok_or_else(|| invalid("checkpoint index has no directory"))?;
    let canonical_directory = directory.canonicalize()?;
    let mut shards = std::collections::BTreeSet::new();
    for value in map.values() {
        let name = value
            .as_str()
            .ok_or_else(|| invalid("checkpoint shard name must be a string"))?;
        let path = Path::new(name);
        if path.components().count() != 1
            || !matches!(
                path.components().next(),
                Some(std::path::Component::Normal(_))
            )
            || path.extension().is_none_or(|ext| ext != "safetensors")
        {
            return Err(invalid(
                "checkpoint shard must be a safetensors filename within the model directory",
            ));
        }
        let shard = directory.join(path);
        if !shard.is_file() || shard.canonicalize()?.parent() != Some(canonical_directory.as_path())
        {
            return Err(invalid(
                "checkpoint shard is missing or escapes the model directory",
            ));
        }
        shards.insert(shard);
    }
    Ok(shards.into_iter().collect())
}

fn context_limit(native: usize, configured: Option<&str>) -> io::Result<usize> {
    let limit = match configured {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| invalid("COMMUNITY_CONTEXT_TOKENS must be a positive integer"))?,
        None if native > 65_536 => 8192,
        None => native,
    };
    if limit <= MIN_REPLY || limit > native {
        return Err(invalid(
            "context limit must exceed the reply reserve and not exceed the model's native context",
        ));
    }
    Ok(limit)
}

fn checkpoint_supports_thinking(dir: &Path) -> io::Result<bool> {
    let path = dir.join("tokenizer_config.json");
    let template = if path.exists() {
        let metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(&path)?).map_err(other)?;
        metadata["chat_template"].as_str().map(str::to_owned)
    } else {
        None
    };
    let template = match template {
        Some(template) => template,
        None => {
            let jinja = dir.join("chat_template.jinja");
            if jinja.exists() {
                fs::read_to_string(jinja)?
            } else if path.exists() {
                return Err(invalid(
                    "tokenizer metadata must provide chat_template or chat_template.jinja",
                ));
            } else {
                return Ok(true);
            }
        }
    };
    Ok(template.contains("<think>"))
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

#[cfg(test)]
fn format_prompt(messages: &[Message], show_thinking: bool) -> String {
    format_prompt_for_model(messages, show_thinking, true)
}

fn format_prompt_for_model(
    messages: &[Message],
    show_thinking: bool,
    supports_thinking: bool,
) -> String {
    let mut out = String::new();
    for message in messages {
        out.push_str("<|im_start|>");
        out.push_str(&message.role);
        out.push('\n');
        out.push_str(&message.content);
        out.push_str("<|im_end|>\n");
    }
    if !supports_thinking {
        out.push_str("<|im_start|>assistant\n");
    } else if show_thinking {
        out.push_str("<|im_start|>assistant\n<think>\n");
    } else {
        out.push_str("<|im_start|>assistant\n<think>\n\n</think>\n\n");
    }
    out
}

fn split_thinking(decoded: &str) -> io::Result<(Option<String>, &str)> {
    let text = decoded.trim();
    let text = text.strip_prefix("<think>").unwrap_or(text);
    let (thinking, answer) = text.split_once("</think>").ok_or_else(|| {
        invalid("model did not finish its thinking section; no visible answer was returned")
    })?;
    Ok((Some(thinking.trim().to_owned()), answer.trim()))
}

fn visible_reply(decoded: &str) -> io::Result<&str> {
    let reply = decoded.trim();
    if let Some(thinking) = reply.strip_prefix("<think>") {
        let (_, answer) = thinking.split_once("</think>").ok_or_else(|| {
            invalid("model did not finish its thinking section; no visible answer was returned")
        })?;
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
    #[test]
    fn chat_lease_does_not_wait_for_maintenance() {
        let directory = crate::test_support::unique_temp_path("community-chat-lease", "");
        let shared = directory.join("model-inference.lock");
        let chat = super::chat_lease_path(&shared);
        let maintenance_lease = super::acquire_model_lease(&shared).unwrap();
        let chat_file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&chat)
            .unwrap();
        chat_file.try_lock().unwrap();
        let second_chat = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&chat)
            .unwrap();
        assert!(matches!(
            second_chat.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(second_chat);
        drop(chat_file);
        drop(maintenance_lease);
        std::fs::remove_dir_all(directory).unwrap();
    }

    use super::{
        MAX_EXTRACTION_REPLY, format_prompt, merge_output_head, reply_budget, split_thinking,
        visible_reply,
    };
    use crate::agent::Message;
    fn fixture_dir() -> std::path::PathBuf {
        let path = crate::test_support::unique_temp_path("community-checkpoint-fixture", "");
        std::fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn model_lease_prevents_a_second_load_and_releases_on_drop() {
        let dir = fixture_dir();
        let path = dir.join("inference.lock");
        let lease = super::acquire_model_lease(&path).unwrap();
        let other = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        assert!(matches!(
            other.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(lease);
        other.try_lock().unwrap();
        drop(other);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn shard_index_is_sorted_deduplicated_and_single_file_still_works() {
        let dir = fixture_dir();
        for name in ["a.safetensors", "b.safetensors"] {
            std::fs::write(dir.join(name), b"fixture").unwrap();
        }
        let index = dir.join("model.safetensors.index.json");
        std::fs::write(
            &index,
            r#"{"weight_map":{"x":"b.safetensors","y":"a.safetensors","z":"b.safetensors"}}"#,
        )
        .unwrap();
        assert_eq!(super::checkpoint_path(&dir).unwrap(), index);
        assert_eq!(
            super::checkpoint_files(&index).unwrap(),
            vec![dir.join("a.safetensors"), dir.join("b.safetensors")]
        );
        let single = dir.join("model.safetensors");
        std::fs::write(&single, b"fixture").unwrap();
        assert_eq!(super::checkpoint_path(&dir).unwrap(), single);
        assert_eq!(super::checkpoint_files(&single).unwrap(), vec![single]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn shard_index_rejects_missing_unsafe_and_invalid_entries() {
        let dir = fixture_dir();
        let index = dir.join("model.safetensors.index.json");
        for name in [
            "missing.safetensors",
            "../outside.safetensors",
            "/tmp/outside.safetensors",
            "folder/weights.safetensors",
            "config.json",
        ] {
            std::fs::write(
                &index,
                serde_json::json!({"weight_map":{"x":name}}).to_string(),
            )
            .unwrap();
            assert!(super::checkpoint_files(&index).is_err(), "{name}");
        }
        for value in [
            r#"{"weight_map":{}}"#,
            r#"{"weight_map":{"x":5}}"#,
            "{}",
            "broken json",
        ] {
            std::fs::write(&index, value).unwrap();
            assert!(super::checkpoint_files(&index).is_err());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn shard_symlink_cannot_escape_model_directory() {
        let dir = fixture_dir();
        let outside = fixture_dir();
        std::fs::write(outside.join("weights.safetensors"), b"fixture").unwrap();
        std::os::unix::fs::symlink(
            outside.join("weights.safetensors"),
            dir.join("weights.safetensors"),
        )
        .unwrap();
        let index = dir.join("model.safetensors.index.json");
        std::fs::write(&index, r#"{"weight_map":{"x":"weights.safetensors"}}"#).unwrap();
        assert!(super::checkpoint_files(&index).is_err());
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn context_cap_is_bounded_and_large_models_default_to_8192() {
        assert_eq!(super::context_limit(262144, None).unwrap(), 8192);
        assert_eq!(super::context_limit(40960, None).unwrap(), 40960);
        assert_eq!(super::context_limit(262144, Some("4096")).unwrap(), 4096);
        for value in ["0", "256", "262145", "-1", "abc"] {
            assert!(super::context_limit(262144, Some(value)).is_err());
        }
    }

    #[test]
    fn instruction_template_has_no_thinking_prefill_even_in_debug_mode() {
        let messages = [Message {
            role: "user".into(),
            content: "Hello".into(),
        }];
        let expected = "<|im_start|>user\nHello<|im_end|>\n<|im_start|>assistant\n";
        assert_eq!(
            super::format_prompt_for_model(&messages, false, false),
            expected
        );
        assert_eq!(
            super::format_prompt_for_model(&messages, true, false),
            expected
        );
        let dir = fixture_dir();
        assert!(super::checkpoint_supports_thinking(&dir).unwrap());
        std::fs::write(
            dir.join("tokenizer_config.json"),
            r#"{"chat_template":"<|im_start|>assistant\n"}"#,
        )
        .unwrap();
        assert!(!super::checkpoint_supports_thinking(&dir).unwrap());
        std::fs::write(
            dir.join("tokenizer_config.json"),
            r#"{"chat_template":"<think>"}"#,
        )
        .unwrap();
        assert!(super::checkpoint_supports_thinking(&dir).unwrap());
        std::fs::write(
            dir.join("tokenizer_config.json"),
            r#"{"processor_class":"Qwen3VLProcessor"}"#,
        )
        .unwrap();
        std::fs::write(dir.join("chat_template.jinja"), "<think>").unwrap();
        assert!(super::checkpoint_supports_thinking(&dir).unwrap());
        std::fs::write(dir.join("chat_template.jinja"), "<|im_start|>assistant").unwrap();
        assert!(!super::checkpoint_supports_thinking(&dir).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sharded_checkpoint_merge_preserves_embeddings_and_other_weights() {
        use safetensors::{
            Dtype, SafeTensors,
            tensor::{TensorView, serialize_to_file},
        };
        let dir = fixture_dir();
        let embeddings = [0x80_u8, 0x3f, 0, 0x40]; // BF16 1 and 2
        let other = [0x40_u8, 0x40]; // BF16 3
        serialize_to_file(
            vec![(
                "model.embed_tokens.weight",
                TensorView::new(Dtype::BF16, vec![2, 1], &embeddings).unwrap(),
            )],
            None,
            &dir.join("a.safetensors"),
        )
        .unwrap();
        serialize_to_file(
            vec![(
                "other.weight",
                TensorView::new(Dtype::BF16, vec![1], &other).unwrap(),
            )],
            None,
            &dir.join("b.safetensors"),
        )
        .unwrap();
        let index = dir.join("model.safetensors.index.json");
        std::fs::write(&index, r#"{"weight_map":{"model.embed_tokens.weight":"a.safetensors","other.weight":"b.safetensors"}}"#).unwrap();
        let output = dir.join("model.safetensors");
        super::write_merged_checkpoint(&index, &output, &[1.0; 8], &[0.0; 16]).unwrap();
        let bytes = std::fs::read(&output).unwrap();
        let result = SafeTensors::deserialize(&bytes).unwrap();
        assert_eq!(result.tensor("lm_head.weight").unwrap().data(), embeddings);
        assert_eq!(
            result.tensor("model.embed_tokens.weight").unwrap().data(),
            embeddings
        );
        assert_eq!(result.tensor("other.weight").unwrap().data(), other);
        assert!(super::write_merged_checkpoint(&index, &output, &[1.0; 8], &[0.0; 16]).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

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

    #[test]
    fn extraction_reply_budget_stays_bounded_even_with_a_large_context_window() {
        assert_eq!(MAX_EXTRACTION_REPLY, 1_024);
        assert_eq!(reply_budget(119, 0, 16_000, MAX_EXTRACTION_REPLY), 1_024);
    }
}
