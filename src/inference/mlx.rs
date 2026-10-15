//! Persistent MLX worker for Qwen3.5's hybrid attention architecture.
use super::*;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;

struct Worker {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}
impl Worker {
    fn read(&mut self) -> io::Result<serde_json::Value> {
        let mut line = String::new();
        if self.output.read_line(&mut line)? == 0 {
            return Err(other("MLX worker exited before returning a response"));
        }
        let response: serde_json::Value = serde_json::from_str(&line).map_err(other)?;
        if let Some(error) = response["error"].as_str() {
            crate::diagnostics::event(
                "model.backend_error",
                serde_json::json!({"backend":"mlx","error":error,"memory":response["memory"]}),
            );
            return Err(other(format!("MLX worker: {error}")));
        }
        Ok(response)
    }
    fn request(&mut self, request: &serde_json::Value) -> io::Result<serde_json::Value> {
        serde_json::to_writer(&mut self.input, request)?;
        self.input.write_all(b"\n")?;
        self.input.flush()?;
        self.read()
    }

    fn request_with_restart(
        &mut self,
        request: &serde_json::Value,
        restart: impl FnOnce() -> io::Result<Worker>,
    ) -> io::Result<serde_json::Value> {
        match self.request(request) {
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {
                crate::diagnostics::event(
                    "model.worker_restarting",
                    serde_json::json!({"backend":"mlx","reason":error.to_string()}),
                );
                let mut replacement = restart()?;
                let ready = replacement.read()?;
                if ready["ready"] != true {
                    return Err(other("restarted MLX worker did not signal readiness"));
                }
                *self = replacement;
                self.request(request)
            }
            result => result,
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct MlxModel {
    worker: Mutex<Worker>,
    python: PathBuf,
    init: serde_json::Value,
    tokenizer: Tokenizer,
    supports_thinking: bool,
    max_context: usize,
    adapter: PathBuf,
    pub(super) consolidated: bool,
    pub(super) last_thinking: Option<String>,
    _device_lease: fs::File,
}
impl MlxModel {
    pub(super) fn load(
        dir: &Path,
        checkpoint: &Path,
        adapter: &Path,
        cpu: bool,
        lease: &Path,
    ) -> io::Result<Self> {
        if cpu {
            return Err(invalid(
                "the Qwen3.5 MLX backend currently requires Apple Silicon Metal",
            ));
        }
        let config: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.join("config.json"))?).map_err(other)?;
        let native = config["text_config"]["max_position_embeddings"]
            .as_u64()
            .ok_or_else(|| invalid("Qwen3.5 text context is missing"))?
            as usize;
        let max_context = context_limit(
            native,
            std::env::var("COMMUNITY_CONTEXT_TOKENS").ok().as_deref(),
        )?;
        let checkpoints = checkpoint_files(checkpoint)?;
        let device_lease = acquire_model_lease(lease)?;
        let python = std::env::var_os("COMMUNITY_MLX_PYTHON")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR")).join(".venv-qwen35/bin/python")
            });
        let init = serde_json::json!({"model_dir": dir, "checkpoints": checkpoints, "adapter": adapter, "context_tokens": max_context});
        let mut worker = spawn_worker(&python, &init)?;
        let ready = worker.read()?;
        if ready["ready"] != true {
            return Err(other("MLX worker did not signal readiness"));
        }
        let supports_thinking = checkpoint_supports_thinking(dir)?;
        let tokenizer = Tokenizer::from_file(dir.join("tokenizer.json")).map_err(other)?;
        for token in ["<|im_end|>", "<|endoftext|>"] {
            if tokenizer.token_to_id(token).is_none() {
                return Err(invalid("Qwen3.5 tokenizer is missing a stop token"));
            }
        }
        crate::diagnostics::event(
            "model.loaded",
            serde_json::json!({"model_dir":dir,"backend":"mlx","precision":ready["precision"],"quantization":ready["quantization"],"shard_count":checkpoints.len(),"context_tokens":max_context,"device":"metal","chat_template":"qwen3_5_no_thinking","adapter_path":adapter,"adapter_state": if ready["adapter_created"] == true {"initialized_untrained"} else if ready["adapter_active"] == true {"loaded_trained"} else {"loaded_untrained"},"adapter_loaded":ready["adapter_loaded"],"adapter_exists":adapter.exists(),"adapter_created":ready["adapter_created"],"adapter_active":ready["adapter_active"],"adapter_training_steps":ready["adapter_training_steps"],"memory":ready["memory"]}),
        );
        Ok(Self {
            worker: Mutex::new(worker),
            python,
            init,
            tokenizer,
            supports_thinking,
            max_context,
            adapter: adapter.into(),
            consolidated: config["text_config"]["tie_word_embeddings"] == false,
            last_thinking: None,
            _device_lease: device_lease,
        })
    }
    fn request(&self, value: serde_json::Value) -> io::Result<serde_json::Value> {
        let mut worker = self
            .worker
            .lock()
            .map_err(|_| other("MLX worker lock is poisoned"))?;
        worker.request_with_restart(&value, || spawn_worker(&self.python, &self.init))
    }
    fn ids(&self, messages: &[Message], thinking: bool) -> io::Result<Vec<u32>> {
        self.tokenizer
            .encode(
                format_prompt_for_model(messages, thinking, self.supports_thinking),
                true,
            )
            .map_err(other)
            .map(|value| value.get_ids().to_vec())
    }
    pub(super) fn additional_prompt_tokens(
        &self,
        baseline: &[Message],
        expanded: &[Message],
    ) -> io::Result<usize> {
        Ok(self
            .ids(expanded, false)?
            .len()
            .saturating_sub(self.ids(baseline, false)?.len()))
    }
    pub(super) fn chat(
        &mut self,
        messages: &[Message],
        extra: usize,
        cap: usize,
        temperature: f64,
        thinking: bool,
    ) -> io::Result<String> {
        self.last_thinking = None;
        for attempt in 0..3 {
            let use_thinking = thinking && attempt == 0 && self.supports_thinking;
            let ids = self.ids(messages, use_thinking)?;
            if ids.is_empty() || ids.len() > self.max_context - MIN_REPLY {
                return Err(invalid(
                    "full conversation exceeds the configured context limit; no history was discarded",
                ));
            }
            let requested_cap = cap.saturating_mul(1usize << attempt);
            let limit = reply_budget(ids.len(), extra, self.max_context, requested_cap);
            let initial_budget = thinking_budget().min(limit.saturating_sub(MIN_REPLY).max(1));
            let stops = [
                self.tokenizer.token_to_id("<|im_end|>").unwrap(),
                self.tokenizer.token_to_id("<|endoftext|>").unwrap(),
            ];
            let mut response = self.request(serde_json::json!({"op":"generate","ids":ids,"max_tokens":limit,"temperature":temperature,"stop_ids":stops,
                "thinking":use_thinking,"thinking_budget":initial_budget,"think_start_id":self.tokenizer.token_to_id("<think>"),"think_end_id":self.tokenizer.token_to_id("</think>")}))?;
            let mut tokens: Vec<u32> =
                serde_json::from_value(response["tokens"].clone()).map_err(other)?;
            if use_thinking && response["finish_reason"] == "thinking_budget" {
                let expanded_budget = initial_budget
                    .saturating_mul(4)
                    .min(2048)
                    .min(limit.saturating_sub(MIN_REPLY));
                let extra_budget = expanded_budget.saturating_sub(initial_budget);
                let mut continuation = ids.clone();
                continuation.extend_from_slice(&tokens);
                let remaining = limit
                    .saturating_sub(tokens.len())
                    .min(self.max_context.saturating_sub(continuation.len()));
                if extra_budget > 0 && remaining > MIN_REPLY {
                    let extended = self.request(serde_json::json!({"op":"generate",
                        "ids":continuation,"max_tokens":remaining,"temperature":temperature,
                        "stop_ids":stops,"thinking":true,
                        "thinking_budget":extra_budget.min(remaining.saturating_sub(MIN_REPLY)),
                        "think_start_id":self.tokenizer.token_to_id("<think>"),
                        "think_end_id":self.tokenizer.token_to_id("</think>")}))?;
                    tokens.extend(
                        serde_json::from_value::<Vec<u32>>(extended["tokens"].clone())
                            .map_err(other)?,
                    );
                    response["thinking_tokens"] = serde_json::json!(
                        response["thinking_tokens"].as_u64().unwrap_or(0)
                            + extended["thinking_tokens"].as_u64().unwrap_or(0)
                    );
                    response["finish_reason"] = extended["finish_reason"].clone();
                    crate::diagnostics::event(
                        "model.thinking_budget_extended",
                        serde_json::json!({"initial_budget":initial_budget,"expanded_budget":expanded_budget,
                            "reasoning_retained":true}),
                    );
                }
            }
            if use_thinking && response["finish_reason"] == "thinking_budget" {
                let close = self
                    .tokenizer
                    .token_to_id("</think>")
                    .ok_or_else(|| invalid("thinking model omitted its closing token"))?;
                tokens.push(close);
                tokens.extend(
                    self.tokenizer
                        .encode("\n\n", false)
                        .map_err(other)?
                        .get_ids(),
                );
                let mut continuation = ids.clone();
                continuation.extend_from_slice(&tokens);
                let remaining = limit
                    .saturating_sub(tokens.len())
                    .min(self.max_context.saturating_sub(continuation.len()));
                if remaining == 0 {
                    return Err(invalid(
                        "thinking left no context space for a visible answer",
                    ));
                }
                let completion = self.request(serde_json::json!({"op":"generate",
                    "ids":continuation,"max_tokens":remaining,"temperature":temperature,
                    "stop_ids":stops,"thinking":false,"thinking_budget":thinking_budget(),
                    "think_start_id":self.tokenizer.token_to_id("<think>"),"think_end_id":close}))?;
                tokens.extend(
                    serde_json::from_value::<Vec<u32>>(completion["tokens"].clone())
                        .map_err(other)?,
                );
                response["finish_reason"] = completion["finish_reason"].clone();
                crate::diagnostics::event(
                    "model.thinking_completed_at_budget",
                    serde_json::json!({"thinking_tokens":response["thinking_tokens"],"answer_budget":remaining,
                        "reasoning_retained":true}),
                );
            }
            let decoded = self.tokenizer.decode(&tokens, true).map_err(other)?;
            if response["finish_reason"] == "max_tokens" {
                if use_thinking && let Ok((Some(thought), _)) = generation_reply(&decoded, true) {
                    self.last_thinking = Some(format!(
                        "[Thinking retained from an output-cap retry.]\n{thought}"
                    ));
                }
                if attempt < 2 {
                    crate::diagnostics::event(
                        "model.output_budget_extended",
                        serde_json::json!({"attempt":attempt+1,"previous_budget":limit,
                            "next_budget":requested_cap.saturating_mul(2).min(self.max_context),
                            "reasoning_retained":self.last_thinking.is_some()}),
                    );
                    continue;
                }
                return Err(invalid(
                    "generation remained truncated after two output-budget increases",
                ));
            }
            crate::diagnostics::event(
                "model.generation",
                serde_json::json!({"backend":"mlx","attempt":attempt+1,"prompt_tokens":ids.len(),"reply_limit":limit,"generated_tokens":tokens.len(),"thinking_enabled":use_thinking,"thinking_tokens":response["thinking_tokens"],"finish_reason":response["finish_reason"],"memory":response["memory"]}),
            );
            match generation_reply(&decoded, use_thinking) {
                Ok((thought, answer)) => {
                    if thought.is_some() {
                        self.last_thinking = thought;
                    }
                    return Ok(answer);
                }
                Err(error) => {
                    crate::diagnostics::event(
                        "model.reply_recovery",
                        serde_json::json!({"attempt":attempt+1,"reason":error.to_string(),"finish_reason":response["finish_reason"],"generated_output_excerpt":decoded.chars().take(1200).collect::<String>(),"action":if attempt==0 {"retry_without_thinking"} else {"fail_after_retry"}}),
                    );
                    if attempt == 2 {
                        return Err(error);
                    }
                    if thinking {
                        self.last_thinking = Some(format!(
                            "[Debug trace from a discarded attempt; the final answer was generated with thinking disabled.]\n{}",
                            decoded
                                .trim()
                                .strip_prefix("<think>")
                                .unwrap_or(decoded.trim())
                                .trim()
                        ));
                    }
                }
            }
        }
        unreachable!()
    }
    pub(super) fn train(
        &mut self,
        system: &str,
        prompt: &str,
        answer: &str,
    ) -> io::Result<(usize, f32)> {
        if prompt.trim().is_empty() || answer.trim().is_empty() {
            return Err(invalid("training prompt and answer must be nonempty"));
        }
        let ids = self.ids(
            &[
                Message {
                    role: "system".into(),
                    content: system.into(),
                },
                Message {
                    role: "user".into(),
                    content: prompt.into(),
                },
            ],
            false,
        )?;
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
        let response = self.request(
            serde_json::json!({"op":"train","ids":ids,"targets":targets,"adapter":self.adapter}),
        )?;
        let count = response["tokens"]
            .as_u64()
            .ok_or_else(|| other("MLX returned no training token count"))?
            as usize;
        let loss = response["loss"]
            .as_f64()
            .filter(|loss| loss.is_finite())
            .ok_or_else(|| other("MLX returned invalid training loss"))? as f32;
        Ok((count, loss))
    }
    pub(super) fn save_merged_checkpoint(
        &self,
        source: &Path,
        destination: &Path,
    ) -> io::Result<()> {
        if destination.exists() {
            return Err(invalid("destination checkpoint already exists"));
        }
        self.request(serde_json::json!({"op":"merge","checkpoints":checkpoint_files(source)?,"destination":destination}))?;
        Ok(())
    }
}

fn spawn_worker(python: &std::path::Path, init: &serde_json::Value) -> io::Result<Worker> {
    let mut child = Command::new(python)
        .arg("-u")
        .arg("-c")
        .arg(include_str!("../../scripts/mlx_worker.py"))
        .arg(init.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| {
            other(format!(
                "could not start MLX worker; install scripts/requirements-mlx.txt in .venv-qwen35: {error}"
            ))
        })?;
    let input = child
        .stdin
        .take()
        .ok_or_else(|| other("MLX worker stdin is unavailable"))?;
    let output = BufReader::new(
        child
            .stdout
            .take()
            .ok_or_else(|| other("MLX worker stdout is unavailable"))?,
    );
    Ok(Worker {
        child,
        input,
        output,
    })
}

fn thinking_budget() -> usize {
    std::env::var("COMMUNITY_THINKING_TOKENS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| (1..=2048).contains(value))
        .unwrap_or(256)
}

fn generation_reply(decoded: &str, thinking: bool) -> io::Result<(Option<String>, String)> {
    let (thought, answer) = if thinking {
        split_thinking(decoded)?
    } else {
        (None, visible_reply(decoded)?)
    };
    if answer.trim().is_empty() {
        return Err(invalid("model returned no visible answer"));
    }
    if answer.contains("<think>") || answer.contains("</think>") {
        return Err(invalid(
            "model emitted another thinking boundary in its visible answer",
        ));
    }
    Ok((thought.filter(|value| !value.is_empty()), answer.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worker(script: &str) -> Worker {
        let mut child = Command::new("python3")
            .args(["-u", "-c", script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        Worker {
            input: child.stdin.take().unwrap(),
            output: BufReader::new(child.stdout.take().unwrap()),
            child,
        }
    }

    #[test]
    fn broken_worker_pipe_restarts_once_and_replays_request() {
        let mut dead_worker = worker(
            "import sys,json,time\nprint(json.dumps({'ready':True}),flush=True)\nsys.stdin.close()\ntime.sleep(.2)",
        );
        dead_worker.child.wait().unwrap();
        let request = serde_json::json!({"op":"generate"});
        let response = dead_worker
            .request_with_restart(&request, || {
                Ok(worker(
                    "import sys,json\nprint(json.dumps({'ready':True}),flush=True)\nfor line in sys.stdin: print(json.dumps({'replayed':json.loads(line)['op']}),flush=True)",
                ))
            })
            .unwrap();
        assert_eq!(response["replayed"], "generate");
    }

    #[test]
    fn generation_requires_visible_answer_and_recovers_by_disabling_thinking() {
        assert!(generation_reply("Reasoning without a closing tag", true).is_err());
        assert!(generation_reply("<think>unfinished reasoning", false).is_err());
        assert!(generation_reply("Reasoning</think>  ", true).is_err());
        assert!(generation_reply("Reasoning</think>More reasoning</think>Answer", true).is_err());
        assert!(generation_reply("", false).is_err());
        assert_eq!(
            generation_reply("Reasoning</think>Soup is ready.", true).unwrap(),
            (Some("Reasoning".into()), "Soup is ready.".into())
        );
        assert_eq!(
            generation_reply("Soup is ready.", false).unwrap(),
            (None, "Soup is ready.".into())
        );
    }

    #[test]
    fn protocol_preserves_requests_across_multiple_generations() {
        let mut worker = worker(
            "import sys,json\nfor line in sys.stdin: print(json.dumps(json.loads(line)),flush=True)",
        );
        for cap in [128, 256] {
            let request = serde_json::json!({"op":"generate", "ids":[1,2,3], "max_tokens":cap,"stop_ids":[4,5],"temperature":0.0});
            assert_eq!(worker.request(&request).unwrap(), request);
        }
        let pid = worker.child.id();
        drop(worker);
        let status = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(!status.success(), "dropping the model must reap its worker");
    }

    #[test]
    fn thinking_budget_expands_then_preserves_reasoning_into_the_answer() {
        let vocabulary = [
            ("<unk>".into(), 0),
            ("<|im_end|>".into(), 1),
            ("<|endoftext|>".into(), 2),
            ("<think>".into(), 3),
            ("unfinished".into(), 4),
            ("Soup.".into(), 5),
            ("</think>".into(), 6),
            ("\n\n".into(), 7),
        ]
        .into_iter()
        .collect();
        let tokenizer = Tokenizer::new(
            tokenizers::models::wordlevel::WordLevel::builder()
                .vocab(vocabulary)
                .unk_token("<unk>".into())
                .build()
                .unwrap(),
        );
        let path = std::env::temp_dir().join(format!(
            "community-reply-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let worker = worker(
            "import sys,json\nfor attempt,line in enumerate(sys.stdin):\n request=json.loads(line)\n assert attempt<3\n assert request['thinking'] == (attempt<2)\n if attempt==0: initial=request['thinking_budget']\n if attempt==1:\n  assert request['thinking_budget']>initial\n  assert request['ids'][-2:]==[3,4]\n if attempt==2: assert request['ids'][-5:]==[3,4,4,6,7]\n print(json.dumps({'tokens':[[3,4],[4],[5]][attempt],'thinking_tokens':initial if attempt==0 else request['thinking_budget'] if attempt==1 else 0,'finish_reason':'thinking_budget' if attempt<2 else 'stop_token'}),flush=True)",
        );
        let mut model = MlxModel {
            worker: Mutex::new(worker),
            python: PathBuf::new(),
            init: serde_json::Value::Null,
            tokenizer,
            supports_thinking: true,
            max_context: 8192,
            adapter: PathBuf::new(),
            consolidated: false,
            last_thinking: None,
            _device_lease: fs::File::create(&path).unwrap(),
        };
        let messages = [Message {
            role: "user".into(),
            content: "How do I make soup?".into(),
        }];
        assert_eq!(model.chat(&messages, 0, 4096, 0.1, true).unwrap(), "Soup.");
        assert_eq!(
            model.last_thinking.as_deref(),
            Some("unfinished unfinished")
        );
        drop(model);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn capped_generation_retries_without_thinking_at_a_larger_budget() {
        let vocabulary = [
            ("<unk>".into(), 0),
            ("<|im_end|>".into(), 1),
            ("<|endoftext|>".into(), 2),
            ("<think>".into(), 3),
            ("reasoning".into(), 4),
            ("</think>".into(), 5),
            ("partial".into(), 6),
            ("Soup.".into(), 7),
        ]
        .into_iter()
        .collect();
        let tokenizer = Tokenizer::new(
            tokenizers::models::wordlevel::WordLevel::builder()
                .vocab(vocabulary)
                .unk_token("<unk>".into())
                .build()
                .unwrap(),
        );
        let path = std::env::temp_dir().join(format!(
            "community-cap-retry-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let worker = worker(
            "import sys,json\nprevious=0\nfor attempt,line in enumerate(sys.stdin):\n request=json.loads(line)\n assert attempt<2\n if attempt==0:\n  assert request['thinking']\n  previous=request['max_tokens']\n  tokens=[3,4,5,6]\n  finish='max_tokens'\n else:\n  assert not request['thinking'] and request['max_tokens']>previous\n  tokens=[7]\n  finish='stop_token'\n print(json.dumps({'tokens':tokens,'thinking_tokens':1 if attempt==0 else 0,'finish_reason':finish}),flush=True)",
        );
        let mut model = MlxModel {
            worker: Mutex::new(worker),
            python: PathBuf::new(),
            init: serde_json::Value::Null,
            tokenizer,
            supports_thinking: true,
            max_context: 8192,
            adapter: PathBuf::new(),
            consolidated: false,
            last_thinking: None,
            _device_lease: fs::File::create(&path).unwrap(),
        };
        let messages = [Message {
            role: "user".into(),
            content: "How do I make soup?".into(),
        }];
        assert_eq!(model.chat(&messages, 0, 512, 0.1, true).unwrap(), "Soup.");
        assert!(
            model
                .last_thinking
                .as_deref()
                .is_some_and(|thought| thought.contains("reasoning"))
        );
        drop(model);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn persistently_capped_generation_stops_after_two_budget_increases() {
        let vocabulary = [
            ("<unk>".into(), 0),
            ("<|im_end|>".into(), 1),
            ("<|endoftext|>".into(), 2),
            ("<think>".into(), 3),
            ("reasoning".into(), 4),
            ("</think>".into(), 5),
            ("partial".into(), 6),
        ]
        .into_iter()
        .collect();
        let tokenizer = Tokenizer::new(
            tokenizers::models::wordlevel::WordLevel::builder()
                .vocab(vocabulary)
                .unk_token("<unk>".into())
                .build()
                .unwrap(),
        );
        let path = std::env::temp_dir().join(format!(
            "community-cap-stop-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let worker = worker(
            "import sys,json\nprevious=0\nfor attempt,line in enumerate(sys.stdin):\n request=json.loads(line)\n assert attempt<3\n if attempt:\n  assert not request['thinking'] and request['max_tokens']>previous\n previous=request['max_tokens']\n tokens=[3,4,5,6] if attempt==0 else [6]\n print(json.dumps({'tokens':tokens,'thinking_tokens':1 if attempt==0 else 0,'finish_reason':'max_tokens'}),flush=True)",
        );
        let mut model = MlxModel {
            worker: Mutex::new(worker),
            python: PathBuf::new(),
            init: serde_json::Value::Null,
            tokenizer,
            supports_thinking: true,
            max_context: 8192,
            adapter: PathBuf::new(),
            consolidated: false,
            last_thinking: None,
            _device_lease: fs::File::create(&path).unwrap(),
        };
        let messages = [Message {
            role: "user".into(),
            content: "How do I make soup?".into(),
        }];
        assert!(
            model
                .chat(&messages, 0, 512, 0.1, true)
                .unwrap_err()
                .to_string()
                .contains("after two output-budget increases")
        );
        assert!(model.last_thinking.is_some());
        drop(model);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn protocol_surfaces_backend_errors() {
        let mut worker = worker("print('{\"error\":\"invalid adapter shape\"}',flush=True)");
        assert!(
            worker
                .read()
                .unwrap_err()
                .to_string()
                .contains("invalid adapter shape")
        );
    }

    #[test]
    fn protocol_reports_early_exit_and_invalid_json() {
        let mut exited = worker("pass");
        assert!(
            exited
                .read()
                .unwrap_err()
                .to_string()
                .contains("exited before returning")
        );
        let mut malformed = worker("print('not json',flush=True)");
        assert!(malformed.read().is_err());
    }
}
