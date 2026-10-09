use super::Cli;
use crate::{curmudgeon, inference::assignments::AgentRole, maintenance, scholar, scout, storage};

impl Cli {
    pub(super) fn curate_once(&mut self, args: Vec<String>) -> Result<(), String> {
        let mut args = args.into_iter();
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
        let summary =
            maintenance::run_pass(database_path, knowledge_path, &self.model_runtime, mode)
                .map_err(|e| format!("maintenance failed: {e}"))?;
        if mode == "serve" {
            println!(
                "{}",
                serde_json::to_string(&summary).map_err(|error| error.to_string())?
            );
        } else {
            println!(
                "Curated {} observation(s); audited {} assistant reply/replies; queued {} Scout investigation(s); processed {} Scholar job(s); {} role error(s)",
                summary.curated,
                summary.audited,
                summary.scouted,
                summary.researched,
                summary.errors
            );
        }
        Ok(())
    }

    pub(super) fn scout_once(&mut self, args: Vec<String>) -> Result<(), String> {
        let mut args = args.into_iter();
        let database_path = &self.database_path;
        let knowledge_path = &self.knowledge_path;
        if args.next().is_some() {
            return Err("scout-once takes no arguments".into());
        }
        let mut backend = self.model_runtime.model(AgentRole::Scout);
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
        for probe in store.model_probes().map_err(|e| e.to_string())? {
            println!(
                "Model probe #{} [{}] {}",
                probe.id,
                probe.status,
                probe.topic.as_deref().unwrap_or(&probe.seed)
            );
            if let Some(question) = &probe.question {
                println!("  Question: {question}");
            }
            println!("  Reason: {}", probe.selection_reason);
            if let Some(job_id) = &probe.scholar_job_id {
                println!("  Scholar job: {job_id}");
            }
        }
        Ok(())
    }

    pub(super) fn scholar_once(&mut self, args: Vec<String>) -> Result<(), String> {
        let mut args = args.into_iter();
        let database_path = &self.database_path;
        if args.next().is_some() {
            return Err("scholar-once takes no arguments".into());
        }
        let mut backend = self.model_runtime.model(AgentRole::Scholar);
        let processed = scholar::process_once(database_path, &mut backend)
            .map_err(|e| format!("scholar failed: {e}"))?;
        println!("Processed {processed} scholar job(s)");
        Ok(())
    }

    pub(super) fn curmudgeon_once(&mut self, args: Vec<String>) -> Result<(), String> {
        let mut args = args.into_iter();
        let database_path = &self.database_path;
        let knowledge_path = &self.knowledge_path;
        if args.next().is_some() {
            return Err("curmudgeon-once takes no arguments".into());
        }
        let mut backend = self.model_runtime.model(AgentRole::Curmudgeon);
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
            [subcommand] if subcommand == "usage" => {
                println!(
                    "{}",
                    store
                        .research_network_usage()
                        .map_err(|error| error.to_string())?
                );
            }
            [subcommand] if subcommand == "baskets" => {
                println!(
                    "{}",
                    store
                        .research_basket_summary()
                        .map_err(|error| error.to_string())?
                );
            }
            [subcommand, kind] if subcommand == "baskets" && kind == "import" => {
                println!(
                    "Retained {} source excerpt(s); old findings are unchanged",
                    store
                        .import_retained_research_sources()
                        .map_err(|error| error.to_string())?
                );
            }
            [subcommand, kind] if subcommand == "baskets" => {
                println!(
                    "{}",
                    serde_json::to_string(
                        &store
                            .research_basket_items(kind)
                            .map_err(|error| error.to_string())?
                    )
                    .map_err(|error| error.to_string())?
                );
            }
            [subcommand, kind, id]
                if subcommand == "baskets"
                    && matches!(kind.as_str(), "claim" | "source" | "retry") =>
            {
                let id = id
                    .parse::<i64>()
                    .map_err(|_| "basket ID must be an integer")?;
                if kind == "retry" {
                    store
                        .retry_retained_source(id)
                        .map_err(|error| error.to_string())?;
                    println!("Queued source #{id} for re-harvesting");
                } else {
                    let detail = if kind == "claim" {
                        store.retained_claim_evidence(id)
                    } else {
                        store.retained_source_detail(id)
                    }
                    .map_err(|error| error.to_string())?;
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&detail).map_err(|error| error.to_string())?
                    );
                }
            }
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
            [subcommand] if subcommand == "sites" => {
                print_serpapi_sites(&store, 50)?;
            }
            [subcommand] if subcommand == "blocked" => {
                print_robots_blocks(&store, 50)?;
            }
            [subcommand] if subcommand == "queue" => {
                for task in store.corroboration_queue(50).map_err(|e| e.to_string())? {
                    println!(
                        "Finding #{} [{}] {} — SerpAPI {}/3, Tavily {}/3{}",
                        task.finding_id,
                        task.status,
                        task.claim,
                        task.serp_attempts,
                        task.tavily_attempts,
                        task.claimed_provider
                            .as_deref()
                            .map(|provider| format!("; running {provider}"))
                            .unwrap_or_default()
                    );
                    if let Some(outcome) = task.last_outcome {
                        println!("  Last outcome: {outcome}");
                    }
                }
            }
            [subcommand, limit] if subcommand == "blocked" => {
                let limit = limit
                    .parse::<usize>()
                    .map_err(|_| "scholar blocked limit must be a positive integer")?;
                print_robots_blocks(&store, limit)?;
            }
            [subcommand, limit] if subcommand == "sites" => {
                let limit = limit
                    .parse::<usize>()
                    .map_err(|_| "scholar sites limit must be a positive integer")?;
                print_serpapi_sites(&store, limit)?;
            }
            [subcommand, job_id] if subcommand == "show" => {
                for attempt in store
                    .scholar_search_attempts(job_id)
                    .map_err(|e| e.to_string())?
                {
                    println!(
                        "Search [{}; {}; {}]: {} ({} result(s))",
                        attempt.provider,
                        attempt.trigger_reason,
                        attempt.status,
                        attempt.query,
                        attempt.result_count.unwrap_or(0)
                    );
                    if let Some(error) = attempt.error {
                        println!("  {error}");
                    }
                }
                let candidates = store
                    .scholar_search_candidates(job_id)
                    .map_err(|e| e.to_string())?;
                if !candidates.is_empty() {
                    println!(
                        "Discovered source candidates (search snippets are leads, not evidence):"
                    );
                    for candidate in candidates {
                        println!(
                            "  [{} {}; {}] {}",
                            candidate.provider,
                            candidate.result_type,
                            candidate.status,
                            candidate.title
                        );
                        println!(
                            "    Policy: {}{}; fetch: {}",
                            candidate.policy_status,
                            candidate
                                .policy_reason
                                .as_deref()
                                .map(|reason| format!(" — {reason}"))
                                .unwrap_or_default(),
                            candidate.fetch_status
                        );
                        if !candidate.query.is_empty() {
                            println!("    Query: {}", candidate.query);
                        }
                        println!("    {}", candidate.url);
                        if !candidate.snippet.is_empty() {
                            println!("    {}", candidate.snippet);
                        }
                        if let Some(fetched_url) = &candidate.fetched_url {
                            println!("    Fetched source: {fetched_url}");
                        }
                        if let Some(excerpt) = &candidate.fetched_excerpt {
                            println!(
                                "    Stored page passages (not yet independently corroborated):\n{excerpt}"
                            );
                        }
                        if let Some(provenance) = &candidate.page_provenance_json {
                            println!("    Page-declared provenance (unverified): {provenance}");
                        }
                        for attempt in store
                            .scholar_extraction_attempts(candidate.id)
                            .map_err(|e| e.to_string())?
                        {
                            println!(
                                "    Extraction #{} [{}]: {}",
                                attempt.attempt_index, attempt.status, attempt.reason
                            );
                            if let Some(output) = attempt.model_output {
                                println!("      Model output (untrusted): {output}");
                            }
                        }
                        if let Ok(details) = serde_json::from_str::<serde_json::Value>(
                            &candidate.structured_data_json,
                        ) && details != serde_json::json!({})
                        {
                            let pretty = serde_json::to_string_pretty(&details)
                                .unwrap_or_else(|_| candidate.structured_data_json.clone());
                            println!("    Structured search data (unverified): {pretty}");
                        }
                    }
                }
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
                    if let Some(assessment) = store
                        .scholar_source_assessment(finding.id)
                        .map_err(|e| e.to_string())?
                    {
                        println!(
                            "  Initial assessment: {} [{}; {}; expertise: {}; direct support: {:?}; independence verified: {}; published: {:?}; research relevance: {} (retrieval weight {:.2})]",
                            assessment.gate_status,
                            assessment.topic_class,
                            assessment.source_kind,
                            assessment.topical_expertise,
                            assessment.direct_support,
                            assessment.independence_verified,
                            assessment.published_at,
                            assessment.research_relevance,
                            match assessment.research_relevance.as_str() {
                                "related" => 0.75,
                                "tangential" => 0.40,
                                _ => 1.0,
                            },
                        );
                        println!("  Assessment reason: {}", assessment.reason);
                    }
                    for review in store
                        .scholar_source_expertise_reviews(finding.id)
                        .map_err(|e| e.to_string())?
                    {
                        println!(
                            "  Source review #{} ({}): {} source; suitable for this topic: {}; publication date: {:?}",
                            review.id,
                            review.review_method,
                            review.source_kind,
                            review.suitable_for_topic,
                            review.published_at
                        );
                        println!("    Evidence reviewed: {}", review.evidence_text);
                        println!("    Reason: {}", review.rationale);
                    }
                    for review in store
                        .scholar_source_origin_reviews(finding.id)
                        .map_err(|e| e.to_string())?
                    {
                        println!(
                            "  Verified underlying source #{} ({}): {}",
                            review.id, review.review_method, review.underlying_source_id
                        );
                        println!("    Evidence: {}", review.evidence_text);
                        println!("    Reason: {}", review.rationale);
                    }
                    for review in store
                        .scholar_provenance_reviews(finding.id)
                        .map_err(|e| e.to_string())?
                    {
                        println!(
                            "  Curmudgeon provenance review #{} [{}]: {}",
                            review.id,
                            review.verdict.as_deref().unwrap_or(&review.status),
                            review.rationale.as_deref().unwrap_or(&review.signal)
                        );
                        println!(
                            "    Compared with finding #{}: {}",
                            review.related_finding_id, review.related_source_url
                        );
                    }
                    if let Some(reason) = finding.decision_reason {
                        println!("  Decision: {reason}");
                    }
                }
            }
            [
                subcommand,
                id,
                source_kind,
                suitability,
                published_at,
                reason @ ..,
            ] if subcommand == "source-review"
                && !reason.is_empty()
                && matches!(source_kind.as_str(), "primary" | "secondary")
                && matches!(suitability.as_str(), "suitable" | "unsuitable") =>
            {
                let id: i64 = id.parse().map_err(|_| "finding ID must be an integer")?;
                let published_at = (published_at != "-").then_some(published_at.as_str());
                let reason = reason.join(" ");
                store
                    .review_scholar_source_expertise(
                        id,
                        source_kind,
                        suitability == "suitable",
                        published_at,
                        &reason,
                    )
                    .map_err(|e| e.to_string())?;
                let assessment = store
                    .scholar_source_assessment(id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "source assessment was not saved".to_owned())?;
                println!(
                    "Scholar source review recorded for finding #{id}: {} [{}]",
                    assessment.gate_status, assessment.reason
                );
            }
            [subcommand, id, origin_id, reason @ ..]
                if subcommand == "origin-review" && !reason.is_empty() =>
            {
                let id: i64 = id.parse().map_err(|_| "finding ID must be an integer")?;
                let reason = reason.join(" ");
                store
                    .review_scholar_source_origin(id, origin_id, &reason)
                    .map_err(|e| e.to_string())?;
                let assessment = store
                    .scholar_source_assessment(id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "source assessment was not saved".to_owned())?;
                println!(
                    "Scholar source origin recorded for finding #{id}: {} [{}]",
                    assessment.gate_status, assessment.reason
                );
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
                        "usage: scholar [ask QUESTION | show JOB_ID | source-review FINDING_ID primary|secondary suitable|unsuitable YYYY-MM-DD|- REASON | origin-review FINDING_ID UNDERLYING_SOURCE_ID REASON | review FINDING_ID accept|reject REASON]"
                            .into(),
                    );
            }
        }
        Ok(())
    }
}

fn print_robots_blocks(store: &storage::LearningStore, limit: usize) -> Result<(), String> {
    for block in store
        .scholar_robots_blocks(limit)
        .map_err(|e| e.to_string())?
    {
        println!(
            "{}{} — {} (expires {})",
            block.host, block.display_path, block.reason, block.expires_at
        );
    }
    Ok(())
}

fn print_serpapi_sites(store: &storage::LearningStore, limit: usize) -> Result<(), String> {
    let sites = store
        .serpapi_domain_history(limit)
        .map_err(|error| error.to_string())?;
    if sites.is_empty() {
        println!("No SerpAPI domain history yet");
        return Ok(());
    }
    println!(
        "SerpAPI domains by search (search-result appearance is discovery provenance, not a trust score):"
    );
    for site in sites {
        println!(
            "{} — {} Google result(s) for: {}",
            site.domain, site.result_count, site.query
        );
    }
    Ok(())
}
