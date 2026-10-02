use super::Cli;
use crate::{curmudgeon, inference::LocalModel, maintenance, scholar, scout, storage};

impl Cli {
    pub(super) fn curate_once(&mut self, args: Vec<String>) -> Result<(), String> {
        let mut args = args.into_iter();
        let model_dir = &self.model_dir;
        let adapter_path = &self.adapter_path;
        let cpu = self.cpu;
        let database_path = &self.database_path;
        let knowledge_path = &self.knowledge_path;
        if args.next().is_some() {
            return Err("curate-once takes no arguments".into());
        }
        let mode = if std::env::var("COMMUNITY_MAINTENANCE_MODE").as_deref() == Ok("serve") {
            "serve"
        } else {
            "once"
        };
        let mut backend = LocalModel::load(model_dir, adapter_path, cpu).map_err(|error| {
            let reason = format!("could not load local model for maintenance: {error}");
            if let Err(record_error) =
                maintenance::record_startup_failure(database_path, mode, &reason)
            {
                return format!("{reason}; could not record failed pass: {record_error}");
            }
            reason
        })?;
        let summary = maintenance::run_pass(database_path, knowledge_path, &mut backend, mode)
            .map_err(|e| format!("maintenance failed: {e}"))?;
        println!(
            "Curated {} observation(s); audited {} assistant reply/replies; queued {} Scout investigation(s); processed {} Scholar job(s); {} role error(s)",
            summary.curated, summary.audited, summary.scouted, summary.researched, summary.errors
        );
        Ok(())
    }

    pub(super) fn scout_once(&mut self, args: Vec<String>) -> Result<(), String> {
        let mut args = args.into_iter();
        let database_path = &self.database_path;
        let knowledge_path = &self.knowledge_path;
        if args.next().is_some() {
            return Err("scout-once takes no arguments".into());
        }
        let mut backend = LocalModel::load(&self.model_dir, &self.adapter_path, self.cpu)
            .map_err(|e| format!("could not load local model for Scout: {e}"))?;
        let queued = scout::process_once(database_path, knowledge_path, &mut backend)
            .map_err(|e| format!("Scout failed: {e}"))?;
        println!("Queued {queued} Scout investigation(s)");
        Ok(())
    }

    pub(super) fn scout(&mut self, args: Vec<String>) -> Result<(), String> {
        let mut args = args.into_iter();
        let database_path = &self.database_path;
        if args.next().is_some() {
            return Err("scout takes no arguments".into());
        }
        let store = storage::LearningStore::open(database_path).map_err(|e| e.to_string())?;
        for investigation in store.scout_investigations().map_err(|e| e.to_string())? {
            println!(
                "[{}] {}",
                investigation.trigger_kind, investigation.question
            );
            println!(
                "  Trigger: {} ({})",
                investigation.trigger_ref, investigation.topic_key
            );
            println!("  Scholar job: {}", investigation.scholar_job_id);
        }
        Ok(())
    }

    pub(super) fn scholar_once(&mut self, args: Vec<String>) -> Result<(), String> {
        let mut args = args.into_iter();
        let model_dir = &self.model_dir;
        let adapter_path = &self.adapter_path;
        let cpu = self.cpu;
        let database_path = &self.database_path;
        if args.next().is_some() {
            return Err("scholar-once takes no arguments".into());
        }
        let mut backend = LocalModel::load(model_dir, adapter_path, cpu)
            .map_err(|e| format!("could not load local model for scholar: {e}"))?;
        let processed = scholar::process_once(database_path, &mut backend)
            .map_err(|e| format!("scholar failed: {e}"))?;
        println!("Processed {processed} scholar job(s)");
        Ok(())
    }

    pub(super) fn curmudgeon_once(&mut self, args: Vec<String>) -> Result<(), String> {
        let mut args = args.into_iter();
        let model_dir = &self.model_dir;
        let adapter_path = &self.adapter_path;
        let cpu = self.cpu;
        let database_path = &self.database_path;
        let knowledge_path = &self.knowledge_path;
        if args.next().is_some() {
            return Err("curmudgeon-once takes no arguments".into());
        }
        let mut backend = LocalModel::load(model_dir, adapter_path, cpu)
            .map_err(|e| format!("could not load local model for Curmudgeon: {e}"))?;
        let processed = curmudgeon::process_once(database_path, knowledge_path, &mut backend)
            .map_err(|e| format!("Curmudgeon failed: {e}"))?;
        println!("Audited {processed} assistant reply/replies");
        Ok(())
    }

    pub(super) fn curmudgeon(&mut self, args: Vec<String>) -> Result<(), String> {
        let args = args.into_iter();
        let database_path = &self.database_path;
        let values = args.collect::<Vec<_>>();
        let store = storage::LearningStore::open(database_path).map_err(|e| e.to_string())?;
        match values.as_slice() {
            [subcommand, text @ ..] if subcommand == "audit" && !text.is_empty() => {
                let reply = text.join(" ");
                let nonce = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| e.to_string())?
                    .as_nanos();
                let turn = usize::try_from(nonce).map_err(|_| "audit timestamp too large")?;
                store
                    .stage_assistant_reply("manual-curmudgeon-audit", turn, &reply)
                    .map_err(|e| e.to_string())?;
                println!("Queued Curmudgeon audit: {reply}");
            }
            [] => {
                for claim in store.assistant_claims(30).map_err(|e| e.to_string())? {
                    println!(
                        "#{} [{}; {:.2} → {:.2}]: {} — {} — {}",
                        claim.id,
                        claim.verdict,
                        claim.confidence_before,
                        claim.confidence_after,
                        claim.subject,
                        claim.relation,
                        claim.object
                    );
                    println!("  Audited assertion: {}", claim.evidence_text);
                    println!("  {}", claim.reason);
                    if !claim.evidence_refs.is_empty() {
                        println!("  Compared with: {}", claim.evidence_refs.join(", "));
                    }
                }
                for (id, status, error) in store
                    .assistant_reply_attention()
                    .map_err(|e| e.to_string())?
                {
                    println!(
                        "Reply {id} [{status}]{}",
                        if error.is_empty() {
                            String::new()
                        } else {
                            format!(": {error}")
                        }
                    );
                }
            }
            _ => return Err("usage: curmudgeon [audit REPLY_TEXT]".into()),
        }
        Ok(())
    }

    pub(super) fn scholar(&mut self, args: Vec<String>) -> Result<(), String> {
        let args = args.into_iter();
        let database_path = &self.database_path;
        let values = args.collect::<Vec<_>>();
        let mut store = storage::LearningStore::open(database_path).map_err(|e| e.to_string())?;
        match values.as_slice() {
            [subcommand, query @ ..] if subcommand == "ask" && !query.is_empty() => {
                let question = query.join(" ");
                let nonce = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| e.to_string())?
                    .as_nanos();
                let turn =
                    usize::try_from(nonce).map_err(|_| "scholar job timestamp is too large")?;
                store
                    .enqueue_research("manual-scholar", turn, &question)
                    .map_err(|e| e.to_string())?;
                println!("Queued scholar research: {question}");
            }
            [] => {
                for (job, note) in store.scholar_jobs().map_err(|e| e.to_string())? {
                    println!("{} [{}] {}", job.id, job.status, job.question);
                    if let Some(note) = note {
                        println!("  {note}");
                    }
                }
            }
            [subcommand, job_id] if subcommand == "show" => {
                let findings = store.scholar_findings(job_id).map_err(|e| e.to_string())?;
                if findings.is_empty() {
                    println!("No findings for scholar job {job_id}");
                }
                for finding in findings {
                    println!(
                        "#{} [{}; extraction confidence {:.2} (extractor only)]: {} — {} — {}",
                        finding.id,
                        finding.status,
                        finding.confidence,
                        finding.subject,
                        finding.relation,
                        finding.object
                    );
                    println!("  Source: {}", finding.source_url);
                    println!(
                        "  Source quality: {}; freshness: {}; revised: {:?}",
                        finding.source_quality, finding.freshness, finding.source_updated_at
                    );
                    println!(
                        "  Validity: {:?} to {:?}",
                        finding.valid_from, finding.valid_until
                    );
                    println!("  Evidence: {}", finding.evidence_text);
                    if let Some(reason) = finding.decision_reason {
                        println!("  Decision: {reason}");
                    }
                }
            }
            [subcommand, id, decision, reason @ ..]
                if subcommand == "review"
                    && !reason.is_empty()
                    && matches!(decision.as_str(), "accept" | "reject") =>
            {
                let id: i64 = id.parse().map_err(|_| "finding ID must be an integer")?;
                let reason = reason.join(" ");
                store
                    .review_scholar_finding(id, decision == "accept", &reason)
                    .map_err(|e| e.to_string())?;
                println!("Scholar finding #{id} {decision}ed: {reason}");
            }
            _ => {
                return Err(
                        "usage: scholar [ask QUESTION | show JOB_ID | review FINDING_ID accept|reject REASON]"
                            .into(),
                    );
            }
        }
        Ok(())
    }
}
