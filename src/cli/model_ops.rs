use super::Cli;
use crate::inference::assignments::AgentRole;
use crate::{agent, inference::LocalModel, storage};
use sha2::{Digest, Sha256};
use std::fs;

impl Cli {
    pub(super) fn model(&mut self, args: Vec<String>) -> Result<(), String> {
        let values = args;
        if !matches!(
            values.first().map(String::as_str),
            Some("history" | "approve")
        ) {
            self.ensure_runtime("model")?;
        }
        let model_dir = &self.model_dir;
        let adapter_path = &self.adapter_path;
        let cpu = self.cpu;
        let database_path = &self.database_path;
        let mut store = storage::LearningStore::open(database_path).map_err(|e| e.to_string())?;
        match values.as_slice() {
            [action] if action == "history" => {
                for run in store.model_consolidations().map_err(|e| e.to_string())? {
                    println!(
                        "#{} db#{} loss {:.3}: {}",
                        run.id, run.claim_id, run.training_loss, run.output_checkpoint
                    );
                    println!("  rollback: {}", run.source_checkpoint);
                    if let Some(manifest) = run.manifest_path {
                        println!(
                            "  base SHA-256: {}",
                            run.base_checkpoint_sha256.as_deref().unwrap_or("unknown")
                        );
                        println!(
                            "  included claims: {}",
                            run.included_claim_ids
                                .iter()
                                .map(|id| format!("db#{id}"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                        println!("  manifest: {manifest}");
                    } else {
                        println!("  lineage manifest: unavailable (legacy consolidation)");
                    }
                }
            }
            [action, title, passage] if action == "probe-baskets" => {
                let mut backend = self.model_runtime.as_ref().unwrap().model(AgentRole::Scout);
                let baskets = crate::scholar::probe_baskets(title, passage, &mut backend)
                    .map_err(|error| error.to_string())?;
                println!(
                    "{}",
                    serde_json::to_string(&baskets).map_err(|error| error.to_string())?
                );
            }
            [action, question, title, passage] if action == "probe-scholar" => {
                let mut backend = self
                    .model_runtime
                    .as_ref()
                    .unwrap()
                    .model(AgentRole::Scholar);
                println!(
                    "{}",
                    crate::scholar::probe_extraction(question, title, passage, &mut backend)
                );
            }
            [action, context] if action == "probe-support" => {
                let context: serde_json::Value =
                    serde_json::from_str(context).map_err(|error| error.to_string())?;
                let mut backend = self
                    .model_runtime
                    .as_ref()
                    .unwrap()
                    .model(AgentRole::Scholar);
                println!(
                    "{}",
                    crate::semantic::probe(&context, &mut backend)
                        .map_err(|error| error.to_string())?
                );
            }
            [action, subject, title, passage] if action == "probe-aliases" => {
                let mut backend = self
                    .model_runtime
                    .as_ref()
                    .unwrap()
                    .model(AgentRole::Curmudgeon);
                let aliases = crate::entities::resolve_with(subject, title, passage, |messages| {
                    backend.chat_for_extraction(messages)
                })
                .map_err(|error| error.to_string())?;
                println!(
                    "{}",
                    serde_json::json!({"aliases":aliases.iter().map(|a|serde_json::json!({"name":a.finding.subject,"alias":a.finding.object,"evidence":a.finding.evidence_text})).collect::<Vec<_>>()})
                );
            }
            [action, question, evidence] if action == "probe-lookup" => {
                let mut backend = self
                    .model_runtime
                    .as_ref()
                    .unwrap()
                    .model(AgentRole::Curmudgeon);
                let plan = crate::research::plan_with(question, evidence, |messages| {
                    backend.chat_for_extraction(messages)
                })
                .map_err(|error| error.to_string())?;
                println!(
                    "{}",
                    serde_json::to_string(&plan).map_err(|error| error.to_string())?
                );
            }
            [action, context] if action == "probe-review" => {
                let context: serde_json::Value =
                    serde_json::from_str(context).map_err(|error| error.to_string())?;
                let mut backend = self
                    .model_runtime
                    .as_ref()
                    .unwrap()
                    .model(AgentRole::Curmudgeon);
                println!(
                    "{}",
                    crate::curmudgeon::probe_source_review(&context, &mut backend)
                        .map_err(|error| error.to_string())?
                );
            }
            [action, prompt @ ..] if action == "probe-chat" && !prompt.is_empty() => {
                let mut backend = self
                    .model_runtime
                    .as_ref()
                    .unwrap()
                    .model(AgentRole::Interaction);
                println!(
                    "{}",
                    backend
                        .probe_chat(&prompt.join(" "))
                        .map_err(|e| e.to_string())?
                );
            }
            [action, prompt @ ..] if action == "probe" && !prompt.is_empty() => {
                let mut backend = LocalModel::load_for_chat(model_dir, adapter_path, cpu)
                    .map_err(|e| format!("could not load local model: {e}"))?;
                println!(
                    "{}",
                    backend
                        .probe(&prompt.join(" "))
                        .map_err(|e| e.to_string())?
                );
            }
            [action, id, reason @ ..] if action == "approve" && !reason.is_empty() => {
                let id: i64 = id.parse().map_err(|_| "claim ID must be an integer")?;
                store
                    .approve_model_claim(id, &reason.join(" "))
                    .map_err(|e| e.to_string())?;
                println!("Approved active SQLite claim db#{id} for model training");
            }
            [action, id] if action == "consolidate" => {
                require_training_readiness(
                    std::env::var("COMMUNITY_MODEL_TRAINING_READINESS")
                        .ok()
                        .as_deref(),
                )?;
                let id: i64 = id.parse().map_err(|_| "claim ID must be an integer")?;
                let claim = store.approved_model_claim(id).map_err(|e| e.to_string())?;
                let (approval_reason, approved_at) = store
                    .model_claim_approval_record(id)
                    .map_err(|e| e.to_string())?;
                let evidence = store.claim_evidence(id).map_err(|e| e.to_string())?;
                let source =
                    crate::inference::checkpoint_path(model_dir).map_err(|e| e.to_string())?;
                let source_text = source.to_string_lossy().into_owned();
                let parent_manifest = store
                    .model_manifest_for_checkpoint(&source_text)
                    .map_err(|e| e.to_string())?;
                if parent_manifest
                    .as_ref()
                    .is_some_and(|_| adapter_path.exists())
                {
                    return Err("this checkpoint already has lineage; an extra external adapter cannot be consolidated reproducibly".into());
                }
                let already_included = parent_manifest.as_ref().is_some_and(|manifest| {
                    manifest
                        .included_claims
                        .iter()
                        .any(|revision| revision.id == id)
                });
                if already_included {
                    return Err(format!(
                        "claim db#{id} is already included in this checkpoint; approve a new semantic claim revision instead"
                    ));
                }
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| e.to_string())?
                    .as_nanos();
                let name = model_dir
                    .file_name()
                    .ok_or("model directory has no name")?
                    .to_string_lossy();
                let output_dir = model_dir.with_file_name(format!("{name}-claim-{id}-{stamp}"));
                fs::create_dir(&output_dir)
                    .map_err(|e| format!("could not create checkpoint directory: {e}"))?;
                let mut manifest = if let Some(manifest) = parent_manifest {
                    manifest
                } else {
                    let base_adapter = if adapter_path.exists() {
                        let snapshot = output_dir.join("base-adapter.safetensors");
                        fs::copy(adapter_path, &snapshot).map_err(|e| e.to_string())?;
                        Some(snapshot.to_string_lossy().into_owned())
                    } else {
                        None
                    };
                    storage::ModelCheckpointManifest {
                        version: 1,
                        base_model_dir: model_dir.to_string_lossy().into_owned(),
                        base_checkpoint: source_text.clone(),
                        base_checkpoint_sha256: sha256_file(&source)?,
                        base_adapter_sha256: base_adapter
                            .as_deref()
                            .map(|path| sha256_file(std::path::Path::new(path)))
                            .transpose()?,
                        base_adapter_snapshot: base_adapter,
                        included_claims: Vec::new(),
                        system_prompt: agent::SYSTEM_PROMPT.into(),
                        max_training_passes: 24,
                        loss_threshold: 0.6,
                        probe_max_chars: 200,
                    }
                };
                if sha256_file(std::path::Path::new(&manifest.base_checkpoint))?
                    != manifest.base_checkpoint_sha256
                {
                    return Err("base checkpoint changed since its lineage was recorded".into());
                }
                if let (Some(adapter), Some(expected)) = (
                    &manifest.base_adapter_snapshot,
                    &manifest.base_adapter_sha256,
                ) && sha256_file(std::path::Path::new(adapter))? != *expected
                {
                    return Err(
                        "base adapter snapshot changed since its lineage was recorded".into(),
                    );
                }
                manifest
                    .included_claims
                    .push(storage::ConsolidatedClaimRevision {
                        id,
                        subject: claim.subject.clone(),
                        relation: claim.relation.clone(),
                        object: claim.object.clone(),
                        source: claim.source.clone(),
                        valid_from: claim.valid_from.clone(),
                        valid_until: claim.valid_until.clone(),
                        approval_reason,
                        approved_at,
                        evidence,
                    });
                let training_adapter = output_dir.join("training-adapter.safetensors");
                if adapter_path.exists() {
                    fs::copy(adapter_path, &training_adapter).map_err(|e| e.to_string())?;
                }
                let mut backend = LocalModel::load(model_dir, &training_adapter, cpu)
                    .map_err(|e| format!("could not load source model: {e}"))?;
                let prompt = format!("What is known about {}?", claim.subject);
                let answer = format!("{} {} {}.", claim.subject, claim.relation, claim.object);
                let mut result = None;
                for pass in 1..=24 {
                    let (tokens, loss) = backend
                        .train_with_system(agent::SYSTEM_PROMPT, &prompt, &answer)
                        .map_err(|e| format!("model training failed on pass {pass}: {e}"))?;
                    if loss <= 0.6 {
                        let probe = backend
                            .probe_chat(&prompt)
                            .map_err(|e| format!("model validation failed: {e}"))?;
                        if probe.to_lowercase().contains(&claim.object.to_lowercase())
                            && probe.chars().count() <= 200
                        {
                            result = Some((tokens, loss, pass));
                            break;
                        }
                    }
                }
                let Some((tokens, loss, passes)) = result else {
                    drop(backend);
                    fs::remove_dir_all(&output_dir).map_err(|e| e.to_string())?;
                    return Err("model did not recall the claim after 24 passes; no checkpoint was published".into());
                };
                let output = output_dir.join("model.safetensors");
                backend
                    .save_merged_checkpoint(&source, &output)
                    .map_err(|e| format!("checkpoint consolidation failed: {e}"))?;
                drop(backend);
                fs::remove_file(&training_adapter).map_err(|e| e.to_string())?;
                let config_bytes =
                    fs::read(model_dir.join("config.json")).map_err(|e| e.to_string())?;
                let mut config: serde_json::Value =
                    serde_json::from_slice(&config_bytes).map_err(|e| e.to_string())?;
                config["tie_word_embeddings"] = serde_json::Value::Bool(false);
                if config["text_config"].is_object() {
                    config["text_config"]["tie_word_embeddings"] = serde_json::Value::Bool(false);
                }
                fs::write(
                    output_dir.join("config.json"),
                    serde_json::to_vec_pretty(&config).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
                fs::copy(
                    model_dir.join("tokenizer.json"),
                    output_dir.join("tokenizer.json"),
                )
                .map_err(|e| e.to_string())?;
                copy_tokenizer_metadata(model_dir, &output_dir)?;
                let mut check =
                    LocalModel::load(&output_dir, &output_dir.join("adapter.safetensors"), cpu)
                        .map_err(|e| format!("new checkpoint failed validation: {e}"))?;
                let merged_probe = check
                    .probe_chat(&prompt)
                    .map_err(|e| format!("new checkpoint failed recall probe: {e}"))?;
                drop(check);
                if !merged_probe
                    .to_lowercase()
                    .contains(&claim.object.to_lowercase())
                    || merged_probe.chars().count() > 200
                {
                    fs::remove_dir_all(&output_dir).map_err(|e| e.to_string())?;
                    return Err(
                        "merged checkpoint did not retain the claim; no checkpoint was published"
                            .into(),
                    );
                }
                let manifest_path = output_dir.join("consolidation-manifest.json");
                let manifest_json =
                    serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
                fs::write(&manifest_path, &manifest_json).map_err(|e| e.to_string())?;
                store
                    .record_model_consolidation_with_lineage(
                        id,
                        &source_text,
                        &output.to_string_lossy(),
                        loss,
                        &manifest_path.to_string_lossy(),
                        &manifest_json,
                    )
                    .map_err(|e| e.to_string())?;
                println!(
                    "Trained on approved db#{id}: {passes} passes, {tokens} target tokens, mean loss {loss:.3}"
                );
                println!("New model: {}", output_dir.display());
                println!(
                    "Use COMMUNITY_MODEL_DIR={} to select it; unset that variable to return to the original checkpoint.",
                    output_dir.display()
                );
            }
            [action, run_id] if action == "rebuild" => {
                require_training_readiness(
                    std::env::var("COMMUNITY_MODEL_TRAINING_READINESS")
                        .ok()
                        .as_deref(),
                )?;
                let run_id: i64 = run_id.parse().map_err(|_| "run ID must be an integer")?;
                rebuild_model_checkpoint(&mut store, run_id, model_dir, cpu)?;
            }
            _ => {
                return Err(
                        "usage: model approve CLAIM_ID REASON | model consolidate CLAIM_ID | model history | model rebuild RUN_ID | model probe PROMPT | model probe-chat PROMPT | model probe-scholar QUESTION TITLE PASSAGE | model probe-review CONTEXT_JSON | model probe-support CONTEXT_JSON | model probe-lookup QUESTION SOURCE_EVIDENCE | model probe-aliases SUBJECT TITLE PASSAGE".into(),
                    );
            }
        }
        Ok(())
    }
}

const TRAINING_READINESS_TOKEN: &str = "learning-quality-v1-passed";

fn training_readiness_is_enabled(value: Option<&str>) -> bool {
    value == Some(TRAINING_READINESS_TOKEN)
}

fn require_training_readiness(value: Option<&str>) -> Result<(), String> {
    if training_readiness_is_enabled(value) {
        Ok(())
    } else {
        Err("model consolidation and rebuild are paused until the blind-adjudicated learning-quality v1 benchmark passes; set COMMUNITY_MODEL_TRAINING_READINESS=learning-quality-v1-passed only after reviewing that result".into())
    }
}

fn sha256_file(path: &std::path::Path) -> Result<String, String> {
    use std::io::Read;
    let hash = |path: &std::path::Path| -> Result<Vec<u8>, String> {
        let mut file = fs::File::open(path)
            .map_err(|error| format!("could not open {} for hashing: {error}", path.display()))?;
        let mut digest = Sha256::new();
        let mut buffer = [0_u8; 65536];
        loop {
            let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        Ok(digest.finalize().to_vec())
    };
    if path
        .file_name()
        .is_some_and(|name| name == "model.safetensors.index.json")
    {
        let mut digest = Sha256::new();
        digest.update(b"community-lab-sharded-checkpoint-v1");
        digest.update(hash(path)?);
        for shard in crate::inference::checkpoint_files(path).map_err(|error| error.to_string())? {
            let name = shard.file_name().unwrap().to_string_lossy();
            digest.update((name.len() as u64).to_le_bytes());
            digest.update(name.as_bytes());
            digest.update(hash(&shard)?);
        }
        Ok(format!("{:x}", digest.finalize()))
    } else {
        Ok(hash(path)?
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }
}

fn copy_tokenizer_metadata(
    source_dir: &std::path::Path,
    output_dir: &std::path::Path,
) -> Result<(), String> {
    let metadata = source_dir.join("tokenizer_config.json");
    if metadata.is_file() {
        fs::copy(metadata, output_dir.join("tokenizer_config.json"))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn rebuild_model_checkpoint(
    store: &mut storage::LearningStore,
    run_id: i64,
    model_dir: &std::path::Path,
    cpu: bool,
) -> Result<(), String> {
    let mut manifest = store
        .model_manifest_for_run(run_id)
        .map_err(|e| e.to_string())?;
    let base_checkpoint = std::path::PathBuf::from(&manifest.base_checkpoint);
    if sha256_file(&base_checkpoint)? != manifest.base_checkpoint_sha256 {
        return Err("recorded base checkpoint is missing or has changed; rebuild is unsafe".into());
    }
    if let (Some(adapter), Some(expected)) = (
        &manifest.base_adapter_snapshot,
        &manifest.base_adapter_sha256,
    ) && sha256_file(std::path::Path::new(adapter))? != *expected
    {
        return Err("recorded base adapter snapshot is missing or has changed".into());
    }
    let mut included = Vec::new();
    for revision in &manifest.included_claims {
        if store
            .model_revision_matches(revision)
            .map_err(|e| e.to_string())?
        {
            included.push(revision.clone());
        }
    }
    if included.is_empty() {
        return Err("none of this checkpoint's claims remain active and approved".into());
    }
    manifest.included_claims = included;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let name = model_dir
        .file_name()
        .ok_or("model directory has no name")?
        .to_string_lossy();
    let output_dir = model_dir.with_file_name(format!("{name}-rebuild-{run_id}-{stamp}"));
    fs::create_dir(&output_dir).map_err(|e| format!("could not create rebuild directory: {e}"))?;
    let training_adapter = output_dir.join("rebuild-adapter.safetensors");
    if let Some(adapter) = &manifest.base_adapter_snapshot {
        fs::copy(adapter, &training_adapter).map_err(|e| e.to_string())?;
    }
    let mut backend = LocalModel::load_checkpoint(
        std::path::Path::new(&manifest.base_model_dir),
        &base_checkpoint,
        &training_adapter,
        cpu,
    )
    .map_err(|e| format!("could not load recorded base checkpoint: {e}"))?;
    let mut losses = Vec::new();
    let mut total_tokens = 0usize;
    for revision in &manifest.included_claims {
        let prompt = format!("What is known about {}?", revision.subject);
        let answer = format!(
            "{} {} {}.",
            revision.subject, revision.relation, revision.object
        );
        let mut trained = false;
        for pass in 1..=manifest.max_training_passes {
            let (tokens, loss) = backend
                .train_with_system(&manifest.system_prompt, &prompt, &answer)
                .map_err(|e| {
                    format!(
                        "rebuild training failed for db#{} on pass {pass}: {e}",
                        revision.id
                    )
                })?;
            total_tokens += tokens;
            if loss <= manifest.loss_threshold {
                let probe = backend
                    .probe_chat(&prompt)
                    .map_err(|e| format!("rebuild probe failed: {e}"))?;
                if probe
                    .to_lowercase()
                    .contains(&revision.object.to_lowercase())
                    && probe.chars().count() <= manifest.probe_max_chars
                {
                    losses.push(loss);
                    trained = true;
                    break;
                }
            }
        }
        if !trained {
            return Err(format!(
                "rebuilt model failed recall for still-approved claim db#{}",
                revision.id
            ));
        }
    }
    let output = output_dir.join("model.safetensors");
    backend
        .save_merged_checkpoint(&base_checkpoint, &output)
        .map_err(|e| format!("could not save rebuilt checkpoint: {e}"))?;
    drop(backend);
    if training_adapter.exists() {
        fs::remove_file(&training_adapter).map_err(|e| e.to_string())?;
    }
    let config_bytes = fs::read(std::path::Path::new(&manifest.base_model_dir).join("config.json"))
        .map_err(|e| e.to_string())?;
    let mut config: serde_json::Value =
        serde_json::from_slice(&config_bytes).map_err(|e| e.to_string())?;
    config["tie_word_embeddings"] = serde_json::Value::Bool(false);
    if config["text_config"].is_object() {
        config["text_config"]["tie_word_embeddings"] = serde_json::Value::Bool(false);
    }
    fs::write(
        output_dir.join("config.json"),
        serde_json::to_vec_pretty(&config).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::copy(
        std::path::Path::new(&manifest.base_model_dir).join("tokenizer.json"),
        output_dir.join("tokenizer.json"),
    )
    .map_err(|e| e.to_string())?;
    copy_tokenizer_metadata(std::path::Path::new(&manifest.base_model_dir), &output_dir)?;
    let mut check = LocalModel::load(&output_dir, &output_dir.join("adapter.safetensors"), cpu)
        .map_err(|e| format!("rebuilt checkpoint failed to load: {e}"))?;
    for revision in &manifest.included_claims {
        let probe = check
            .probe_chat(&format!("What is known about {}?", revision.subject))
            .map_err(|e| format!("rebuilt checkpoint probe failed: {e}"))?;
        if !probe
            .to_lowercase()
            .contains(&revision.object.to_lowercase())
            || probe.chars().count() > manifest.probe_max_chars
        {
            return Err(format!(
                "rebuilt checkpoint does not recall db#{}",
                revision.id
            ));
        }
    }
    drop(check);
    let manifest_path = output_dir.join("consolidation-manifest.json");
    let manifest_json = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    fs::write(&manifest_path, &manifest_json).map_err(|e| e.to_string())?;
    let final_claim = manifest
        .included_claims
        .last()
        .expect("nonempty validated above");
    let average_loss = losses.iter().sum::<f32>() / losses.len() as f32;
    store
        .record_model_consolidation_with_lineage(
            final_claim.id,
            &manifest.base_checkpoint,
            &output.to_string_lossy(),
            average_loss,
            &manifest_path.to_string_lossy(),
            &manifest_json,
        )
        .map_err(|e| e.to_string())?;
    println!(
        "Rebuilt from {} still-active approved claim(s); {total_tokens} target tokens, mean loss {average_loss:.3}",
        manifest.included_claims.len()
    );
    println!("New model: {}", output_dir.display());
    println!("Manifest: {}", manifest_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn model_weight_changes_require_explicit_quality_gate_readiness() {
        assert!(!super::training_readiness_is_enabled(None));
        assert!(!super::training_readiness_is_enabled(Some("true")));
        assert!(super::training_readiness_is_enabled(Some(
            "learning-quality-v1-passed"
        )));
    }

    #[test]
    fn checkpoint_hash_detects_shard_changes_without_index_changes() {
        let dir = crate::test_support::unique_temp_path("community-shard-hash", "");
        std::fs::create_dir(&dir).unwrap();
        let shard = dir.join("weights.safetensors");
        let index = dir.join("model.safetensors.index.json");
        std::fs::write(&shard, b"original").unwrap();
        std::fs::write(&index, r#"{"weight_map":{"x":"weights.safetensors"}}"#).unwrap();
        let original = super::sha256_file(&index).unwrap();
        assert_eq!(original, super::sha256_file(&index).unwrap());
        std::fs::write(&shard, b"changed").unwrap();
        assert_ne!(original, super::sha256_file(&index).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
