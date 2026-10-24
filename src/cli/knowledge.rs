use super::{Cli, ensure_no_arguments};
use crate::{knowledge::KnowledgeMap, storage};

impl Cli {
    pub(super) fn learn(&mut self, args: Vec<String>) -> Result<(), String> {
        self.ensure_map()?;
        let map = self.map.as_mut().expect("knowledge map initialized");
        let knowledge_path = &self.knowledge_path;
        let values = args;
        if values.len() != 4 {
            return Err("learn needs SUBJECT RELATION OBJECT SOURCE".into());
        }
        let id = map
            .add(&values[0], &values[1], &values[2], &values[3])
            .map_err(|e| e.to_string())?;
        map.save(knowledge_path).map_err(|e| e.to_string())?;
        println!("Added claim #{id}");
        Ok(())
    }

    pub(super) fn revise(&mut self, args: Vec<String>) -> Result<(), String> {
        self.ensure_map()?;
        let map = self.map.as_mut().expect("knowledge map initialized");
        let knowledge_path = &self.knowledge_path;
        let values = args;
        if values.len() != 5 {
            return Err("revise needs ID SUBJECT RELATION OBJECT SOURCE".into());
        }
        let id: u64 = values[0].parse().map_err(|_| "ID must be an integer")?;
        let replacement = map
            .revise(id, &values[1], &values[2], &values[3], &values[4])
            .map_err(|e| e.to_string())?;
        map.save(knowledge_path).map_err(|e| e.to_string())?;
        println!("Revised claim #{id} as #{replacement}");
        Ok(())
    }

    pub(super) fn retract(&mut self, args: Vec<String>) -> Result<(), String> {
        self.ensure_map()?;
        let map = self.map.as_mut().expect("knowledge map initialized");
        let knowledge_path = &self.knowledge_path;
        let values = args;
        if values.len() != 2 {
            return Err("retract needs ID SOURCE".into());
        }
        let id: u64 = values[0].parse().map_err(|_| "ID must be an integer")?;
        map.retract(id, &values[1]).map_err(|e| e.to_string())?;
        map.save(knowledge_path).map_err(|e| e.to_string())?;
        println!("Retracted claim #{id}");
        Ok(())
    }

    pub(super) fn maintenance(&mut self, args: Vec<String>) -> Result<(), String> {
        let database_path = &self.database_path;
        let mut store = storage::LearningStore::open(database_path).map_err(|e| e.to_string())?;
        let values = args;
        if values.first().is_some_and(|value| value == "runs") {
            if values.len() > 2 {
                return Err("maintenance runs accepts one optional LIMIT".into());
            }
            let limit = values.get(1).map_or(Ok(10usize), |value| {
                value.parse().map_err(|_| "LIMIT must be an integer")
            })?;
            for pass in store
                .maintenance_recent_passes(limit)
                .map_err(|e| e.to_string())?
            {
                println!(
                    "Pass #{} [{}; {}] started {}; duration {:?} ms",
                    pass.id, pass.mode, pass.status, pass.started_at, pass.duration_ms
                );
                for role in pass.roles {
                    println!(
                        "  {}: {} ({} item(s), {:?} ms){}",
                        role.role,
                        role.status,
                        role.processed,
                        role.duration_ms,
                        role.detail
                            .map(|value| format!(": {value}"))
                            .unwrap_or_default()
                    );
                }
            }
            return Ok(());
        }
        if values.first().is_some_and(|value| value == "history") {
            if values.len() > 2 {
                return Err("maintenance history accepts one optional LIMIT".into());
            }
            let limit = values.get(1).map_or(Ok(30usize), |value| {
                value.parse().map_err(|_| "LIMIT must be an integer")
            })?;
            for event in store
                .recent_maintenance_decisions(limit)
                .map_err(|e| e.to_string())?
            {
                println!(
                    "#{} {} proposed {} from {} {} → {} ({}){}",
                    event.id,
                    event.role,
                    event.suggested_action,
                    event.source_type,
                    event.source_id,
                    event.outcome,
                    event.reason,
                    event
                        .resulting_claim_id
                        .map(|id| format!(
                            "; claim #{id} is {}",
                            event.resulting_state.as_deref().unwrap_or("unchanged")
                        ))
                        .unwrap_or_default(),
                );
            }
            return Ok(());
        }
        if let [action, id, resolution, reason @ ..] = values.as_slice()
            && action == "resolve"
            && !reason.is_empty()
        {
            let claim_id: i64 = id.parse().map_err(|_| "claim ID must be an integer")?;
            store
                .resolve_semantic_claim(claim_id, resolution, &reason.join(" "))
                .map_err(|e| e.to_string())?;
            println!("Applied {resolution} review to semantic claim #{claim_id}");
            return Ok(());
        }
        if let [action, old_id, new_id, reason @ ..] = values.as_slice()
            && action == "supersede"
            && !reason.is_empty()
        {
            let old_claim_id: i64 = old_id
                .parse()
                .map_err(|_| "old claim ID must be an integer")?;
            let new_claim_id: i64 = new_id
                .parse()
                .map_err(|_| "new claim ID must be an integer")?;
            store
                .supersede_semantic_claim(old_claim_id, new_claim_id, &reason.join(" "))
                .map_err(|e| e.to_string())?;
            println!("Semantic claim #{old_claim_id} was superseded by #{new_claim_id}");
            return Ok(());
        }
        if let [action, id, volatility, dates @ ..] = values.as_slice()
            && action == "schedule"
        {
            if dates.len() > 1 {
                return Err("maintenance schedule accepts one optional NEXT_REVIEW_AT".into());
            }
            let claim_id: i64 = id.parse().map_err(|_| "claim ID must be an integer")?;
            store
                .set_claim_review_schedule(claim_id, volatility, dates.first().map(String::as_str))
                .map_err(|e| e.to_string())?;
            println!("Updated review schedule for semantic claim #{claim_id}");
            return Ok(());
        }
        if let [action, id, status, dates @ ..] = values.as_slice()
            && action == "review"
        {
            if dates.len() > 2 {
                return Err(
                    "maintenance review accepts optional CHECKED_AT and NEXT_REVIEW_AT".into(),
                );
            }
            let claim_id: i64 = id.parse().map_err(|_| "claim ID must be an integer")?;
            store
                .record_claim_review(
                    claim_id,
                    status,
                    dates.first().map(String::as_str),
                    dates.get(1).map(String::as_str),
                )
                .map_err(|e| e.to_string())?;
            println!("Recorded review for semantic claim #{claim_id}");
            return Ok(());
        }
        if let [action, id, valid_from, valid_until] = values.as_slice()
            && action == "scope"
        {
            let claim_id: i64 = id.parse().map_err(|_| "claim ID must be an integer")?;
            store
                .set_claim_temporal_scope(
                    claim_id,
                    (*valid_from != "-").then_some(valid_from.as_str()),
                    (*valid_until != "-").then_some(valid_until.as_str()),
                )
                .map_err(|e| e.to_string())?;
            println!("Updated temporal scope for semantic claim #{claim_id}");
            return Ok(());
        }
        if !values.is_empty() {
            return Err("usage: maintenance [runs [LIMIT] | history [LIMIT] | resolve CLAIM_ID confirm|challenge|qualify|retract REASON | supersede OLD_CLAIM_ID NEW_CLAIM_ID REASON | review CLAIM_ID active|uncertain|challenged [CHECKED_AT] [NEXT_REVIEW_AT] | schedule CLAIM_ID stable|slow_changing|fast_changing [NEXT_REVIEW_AT] | scope CLAIM_ID VALID_FROM|- VALID_UNTIL|-]".into());
        }
        let status = store.maintenance_status().map_err(|e| e.to_string())?;
        if let Some(pass) = status.last_pass {
            println!(
                "Latest maintenance pass #{} [{}; {}] started {}; duration {:?} ms",
                pass.id, pass.mode, pass.status, pass.started_at, pass.duration_ms
            );
            for role in pass.roles {
                println!(
                    "  {}: {} ({} item(s)){}",
                    role.role,
                    role.status,
                    role.processed,
                    role.detail
                        .map(|value| format!(": {value}"))
                        .unwrap_or_default()
                );
            }
        } else {
            println!("Maintenance has not run yet");
        }
        println!(
            "Queues: Curator {} (oldest {:?}s), Scout {} claim review(s) (oldest {:?}s), {} bucket question(s) waiting, Scholar {} (oldest {:?}s)",
            status.curator.pending,
            status.curator.oldest_age_seconds,
            status.scout.pending,
            status.scout.oldest_age_seconds,
            status.scout_bucket_questions_pending,
            status.scholar.pending,
            status.scholar.oldest_age_seconds,
        );
        println!("Scout scan cursor: db#{}", status.scan_cursor);
        println!(
            "Scout priority: {}",
            status.scout_priority_order.join(" → ")
        );
        if let Some(reason) = status.latest_scout_reason {
            println!("Latest Scout selection: {reason}");
        }
        let work = store.maintenance_work_counts().map_err(|e| e.to_string())?;
        println!(
            "Open work: {} Curator review(s), {} Scholar finding(s), {} research job(s), {} challenged claim(s), {} due claim(s)",
            work.curator_reviews,
            work.scholar_reviews,
            work.pending_research,
            work.challenged_claims,
            work.due_claims
        );
        let due: std::collections::HashSet<i64> = store
            .due_claim_ids(10_000)
            .map_err(|e| e.to_string())?
            .into_iter()
            .collect();
        let claims = store.active_semantic_claims().map_err(|e| e.to_string())?;
        let claim_count = claims.len();
        for detail in &claims {
            let claim = &detail.claim;
            let evidence = store.claim_evidence(claim.id).map_err(|e| e.to_string())?;
            println!(
                "db#{} [{}{}; {}; evidence={}]: {} — {} — {}",
                claim.id,
                claim.status,
                if due.contains(&claim.id) { "; due" } else { "" },
                claim.volatility,
                evidence.len(),
                claim.subject,
                claim.relation,
                claim.object
            );
            println!(
                "  temporal: valid {:?} to {:?}; observed {:?}; checked {:?}; next review {:?}",
                claim.valid_from,
                claim.valid_until,
                claim.observed_at,
                claim.checked_at,
                claim.next_review_at
            );
            println!(
                "  source assessment: quality {}; freshness {}; extraction confidence {} (extractor only)",
                claim.source_quality.as_deref().unwrap_or("unknown"),
                claim.freshness.as_deref().unwrap_or("unknown"),
                claim
                    .extraction_confidence
                    .map_or_else(|| "unknown".into(), |value| format!("{value:.2}")),
            );
            for proposal in store
                .claim_open_proposals(claim.id)
                .map_err(|e| e.to_string())?
            {
                println!(
                    "  unresolved proposal: {} suggests {} [{}]: {}",
                    proposal.role, proposal.action, proposal.outcome, proposal.reason
                );
            }
            println!(
                "  next action: maintenance resolve {} confirm|challenge|qualify|retract REASON",
                claim.id
            );
            if due.contains(&claim.id) {
                println!(
                    "  scheduled action: maintenance review {} active|uncertain|challenged",
                    claim.id
                );
            }
            for item in evidence {
                println!(
                    "  evidence #{} [{}]: {}{} [published: {:?}; checked: {:?}]",
                    item.id,
                    item.stance,
                    item.source_kind,
                    item.source_uri
                        .as_deref()
                        .or(item.observation_id.as_deref())
                        .map(|value| format!(" ({value})"))
                        .unwrap_or_default(),
                    item.published_at,
                    item.retrieved_at
                );
                println!("    {}", item.excerpt);
            }
        }
        println!("{} active SQLite semantic claims", claim_count);
        Ok(())
    }

    pub(super) fn knowledge(&mut self, args: Vec<String>) -> Result<(), String> {
        self.ensure_map()?;
        let database_path = &self.database_path;
        let map = self.map.as_mut().expect("knowledge map initialized");
        ensure_no_arguments(&args, "knowledge takes no arguments")?;
        let store = storage::LearningStore::open(database_path).map_err(|e| e.to_string())?;
        for entry in map
            .entries()
            .iter()
            .filter(|entry| entry.retracted_by.is_none())
        {
            println!(
                "#{}: {} — {} — {} [source: {}]",
                entry.id, entry.subject, entry.relation, entry.object, entry.source
            );
        }
        let curated = store.active_semantic_claims().map_err(|e| e.to_string())?;
        let mut curated_count = 0;
        for detail in curated {
            let claim = detail.claim;
            let duplicate_in_json = map.entries().iter().any(|entry| {
                entry.retracted_by.is_none()
                    && entry
                        .subject
                        .trim()
                        .eq_ignore_ascii_case(claim.subject.trim())
                    && entry
                        .relation
                        .trim()
                        .eq_ignore_ascii_case(claim.relation.trim())
                    && entry
                        .object
                        .trim()
                        .eq_ignore_ascii_case(claim.object.trim())
            });
            if duplicate_in_json {
                continue;
            }
            curated_count += 1;
            println!(
                "db#{}: {} — {} — {} [source: {}]",
                claim.id, claim.subject, claim.relation, claim.object, claim.source
            );
            for source in detail.sources {
                println!("  provenance: {source}");
            }
        }
        println!(
            "{} active JSON map claims; {curated_count} curated SQLite claims",
            map.active_count()
        );
        Ok(())
    }

    pub(super) fn inbox(&mut self, args: Vec<String>) -> Result<(), String> {
        let database_path = &self.database_path;
        let values = args;
        let store = storage::LearningStore::open(database_path).map_err(|e| e.to_string())?;
        match values.as_slice() {
            [] => {
                let summary = store.inbox_summary().map_err(|e| e.to_string())?;
                println!(
                    "Pending: {}; processing: {}; needs review: {}",
                    summary.pending, summary.processing, summary.needs_review
                );
                for observation in summary.observations {
                    println!(
                        "{} [{}] turn {}: {}",
                        observation.id,
                        observation.status,
                        observation.turn_number,
                        observation.raw_text
                    );
                }
            }
            [subcommand, id] if subcommand == "show" => {
                let detail = store
                    .observation_detail(id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("no observation with ID {id}"))?;
                let observation = detail.observation;
                println!(
                    "Observation {} [{}] turn {}",
                    observation.id, observation.status, observation.turn_number
                );
                println!("Conversation: {}", observation.conversation_id);
                println!("Source: {}", observation.raw_text);
                if let Some(reason) = detail.last_error {
                    println!("Review reason: {reason}");
                }
                if detail.candidates.is_empty() {
                    println!("No candidate claims");
                }
                for candidate in detail.candidates {
                    println!(
                        "Candidate {} [{}; confidence {:.2}]: {} — {} — {}",
                        candidate.id,
                        candidate.status,
                        candidate.confidence,
                        candidate.subject,
                        candidate.relation,
                        candidate.object
                    );
                    if candidate.valid_from.is_some() || candidate.valid_until.is_some() {
                        println!(
                            "  Validity: {:?} to {:?}",
                            candidate.valid_from, candidate.valid_until
                        );
                    }
                    println!("  Evidence: {}", candidate.evidence_text);
                    if let Some(reason) = candidate.decision_reason {
                        println!("  Decision: {reason}");
                    }
                    if let Some(semantic_id) = candidate.semantic_claim_id {
                        println!("  Semantic claim: db#{semantic_id}");
                    }
                }
            }
            _ => return Err("usage: inbox [show OBSERVATION_ID]".into()),
        }
        Ok(())
    }

    pub(super) fn review(&mut self, args: Vec<String>) -> Result<(), String> {
        let database_path = &self.database_path;
        let values = args;
        if values.len() < 3 || !matches!(values[1].as_str(), "accept" | "reject") {
            return Err("usage: review CANDIDATE_ID accept|reject REASON".into());
        }
        let id = &values[0];
        let accept = values[1] == "accept";
        let reason = values[2..].join(" ");
        let mut store = storage::LearningStore::open(database_path).map_err(|e| e.to_string())?;
        store
            .resolve_candidate(id, accept, &reason)
            .map_err(|e| e.to_string())?;
        println!(
            "Candidate {id} {}: {}",
            if accept { "accepted" } else { "rejected" },
            reason
        );
        Ok(())
    }

    pub(super) fn migrate_knowledge(&mut self, args: Vec<String>) -> Result<(), String> {
        let database_path = &self.database_path;
        let knowledge_path = &self.knowledge_path;
        ensure_no_arguments(&args, "migrate-knowledge takes no arguments")?;
        let source = KnowledgeMap::load(knowledge_path).map_err(|e| {
            format!(
                "could not read source knowledge map {}: {e}",
                knowledge_path.display()
            )
        })?;
        let mut store = storage::LearningStore::open(database_path).map_err(|e| {
            format!(
                "could not open learning database {}: {e}",
                database_path.display()
            )
        })?;
        let summary = store.import_knowledge(&source).map_err(|e| e.to_string())?;
        println!(
            "Imported {} claims ({} already present) into {}. Source JSON remains unchanged.",
            summary.inserted,
            summary.already_present,
            database_path.display()
        );
        println!(
            "Database contains {} semantic claims.",
            store.claim_count().map_err(|e| e.to_string())?
        );
        Ok(())
    }

    pub(super) fn status(&mut self, args: Vec<String>) -> Result<(), String> {
        self.ensure_chat("status")?;
        let model = &self.model;
        let model_dir = &self.model_dir;
        let adapter_path = &self.adapter_path;
        let state_path = &self.state_path;
        let knowledge_path = &self.knowledge_path;
        let agent = self.agent.as_mut().expect("chat agent initialized");
        let map = self.map.as_mut().expect("knowledge map initialized");
        ensure_no_arguments(&args, "status takes no arguments")?;
        println!("Model: {model}");
        println!("Model directory: {}", model_dir.display());
        for role in [
            crate::inference::assignments::AgentRole::Scout,
            crate::inference::assignments::AgentRole::Scholar,
            crate::inference::assignments::AgentRole::Curmudgeon,
            crate::inference::assignments::AgentRole::Archivist,
            crate::inference::assignments::AgentRole::Interaction,
        ] {
            println!(
                "Agent {} model: {}",
                role.name(),
                self.model_runtime
                    .as_ref()
                    .expect("model runtime initialized")
                    .model_id(role)
                    .map_err(|error| error.to_string())?
            );
        }
        println!(
            "Adapter: {} ({})",
            adapter_path.display(),
            if adapter_path.exists() {
                "trained"
            } else {
                "fresh"
            }
        );
        println!("Conversation turns: {}", agent.turn_count());
        println!("State: {}", state_path.display());
        println!(
            "Knowledge: {} active claims ({})",
            map.active_count(),
            knowledge_path.display()
        );
        Ok(())
    }
}
