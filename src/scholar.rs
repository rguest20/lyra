//! Separate research queue for public facts. Search results are evidence, not instructions.

mod claims;
mod config;
mod discovery;
pub(crate) mod evidence;
mod extraction;
pub(crate) mod passages;
mod provenance;
mod retention;
pub(crate) use retention::probe_baskets;
mod site_policy;
mod wikipedia;

use std::io;
use std::path::Path;

use serde::Deserialize;

use crate::agent::Message;
use crate::inference::LocalModel;
use crate::storage::{
    CorroborationTask, LearningStore, ScholarEvidence, ScholarFinding, ScholarSourceProfile,
};

use wikipedia::{
    html_to_text, page_relevance_score, ranked_sections, relevant_passages, text_property,
    wikipedia_json_request,
};

use claims::{
    calibrate_research_relevance, claim_grounding_failure, explicit_list_finding, select_finding,
    structured_list_finding,
};
use config::ProviderConfig;

use extraction::extract_scholar_claim_guided;
#[cfg(test)]
use extraction::{
    evaluate_extraction, evaluate_extraction_semantically, extraction_evidence_diagnostics,
    parse_extraction, should_retry_grounding,
};
pub(crate) use extraction::{extract_scholar_claim_with, probe_extraction};

use discovery::{
    discover_searxng, discover_serpapi, discover_tavily, serp_interval_seconds,
    tavily_interval_seconds,
};

const SEARCH_API: &str = "https://en.wikipedia.org/w/api.php";
const USER_AGENT: &str = "CommunityLabScholar/0.1 (local Rust chatbot; educational use)";
const MAX_SEARCH_EXTRACT_CHARS: usize = 1_200;
const MAX_PARSED_PAGE_BYTES: usize = 524_288;
const MAX_PARSED_ARTICLE_CHARS: usize = 100_000;
const MAX_SELECTED_PASSAGE_CHARS: usize = 4_500;
const MAX_SERPAPI_CANDIDATES: usize = 50;
const HTTP_STATUS_MARKER: &str = "\nCOMMUNITY_HTTP_STATUS:";
const SERPAPI_SEARCH_API: &str = "https://serpapi.com/search.json";
const TAVILY_SEARCH_API: &str = "https://api.tavily.com/search";

struct ResearchResult {
    page: SearchPage,
    excerpt: String,
    finding: Option<ScholarFinding>,
    research_relevance: String,
    candidate_count: usize,
}

#[derive(Deserialize)]
struct SearchResponse {
    query: Option<SearchResults>,
}

#[derive(Deserialize)]
struct TavilyResponse {
    #[serde(default)]
    results: Vec<TavilyResult>,
}

#[derive(Deserialize)]
struct TavilyResult {
    title: Option<String>,
    url: Option<String>,
    content: Option<String>,
}

#[derive(Deserialize)]
struct SearchResults {
    pages: Vec<SearchPage>,
}

#[derive(Deserialize)]
struct SearchPage {
    index: usize,
    pageid: u64,
    title: String,
    extract: Option<String>,
    fullurl: Option<String>,
    revisions: Option<Vec<Revision>>,
}

#[derive(Deserialize)]
struct ParsedPageResponse {
    parse: Option<ParsedPage>,
}

#[derive(Deserialize)]
struct ParsedPage {
    #[serde(default)]
    text: Option<serde_json::Value>,
    #[serde(default)]
    sections: Vec<ParsedSection>,
}

#[derive(Deserialize)]
struct ParsedSection {
    index: serde_json::Value,
    line: String,
}

struct SectionChoice {
    index: String,
    title: String,
}

#[derive(Deserialize)]
struct Revision {
    timestamp: Option<String>,
}

#[derive(Deserialize)]
struct Extraction {
    claim: Option<ScholarFindingJson>,
}

#[derive(Deserialize)]
struct ScholarFindingJson {
    subject: String,
    relation: String,
    object: String,
    evidence_text: String,
    #[serde(default)]
    confidence: f64,
    #[serde(default)]
    question_relevance: String,
    #[serde(default)]
    valid_from: Option<String>,
    #[serde(default)]
    valid_until: Option<String>,
    #[serde(default)]
    attribution: Option<String>,
    #[serde(default)]
    geographic_scope: Option<String>,
    #[serde(default)]
    context_scope: Option<String>,
    #[serde(default)]
    polarity: String,
    #[serde(default)]
    modality: String,
}

impl ScholarFindingJson {
    fn proposition(&self, evidence: &str) -> crate::storage::PropositionMetadata {
        use crate::storage::{ClaimModality, ClaimPolarity, PropositionMetadata};
        let attribution = self
            .attribution
            .as_deref()
            .map(str::trim)
            .filter(|value| {
                !value.is_empty()
                    && value.chars().count() <= 240
                    && evidence.contains(value)
                    && !value.chars().any(char::is_control)
            })
            .map(str::to_owned);
        PropositionMetadata {
            attribution,
            geographic_scope: exact_scope(&self.geographic_scope, evidence),
            context_scope: exact_scope(&self.context_scope, evidence),
            polarity: match self.polarity.trim().to_ascii_lowercase().as_str() {
                "asserted" => ClaimPolarity::Asserted,
                "negated" => ClaimPolarity::Negated,
                _ => ClaimPolarity::Unknown,
            },
            modality: match self.modality.trim().to_ascii_lowercase().as_str() {
                "factual" => ClaimModality::Factual,
                "possible" => ClaimModality::Possible,
                "necessary" => ClaimModality::Necessary,
                "conditional" => ClaimModality::Conditional,
                _ => ClaimModality::Unknown,
            },
        }
    }
}

fn exact_scope(value: &Option<String>, evidence: &str) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|scope| {
            !scope.is_empty()
                && scope.chars().count() <= 240
                && !scope.chars().any(char::is_control)
                && evidence.contains(scope)
        })
        .map(str::to_owned)
}

pub(crate) struct ExtractionAttempt {
    pub(crate) finding: Option<ScholarFinding>,
    pub(crate) relevance: String,
    pub(crate) status: &'static str,
    pub(crate) reason: String,
    pub(crate) model_output: Option<String>,
}

pub(super) fn retry_after_delay_ms(value: &str, now_ms: i64) -> Option<i64> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<i64>() {
        return (seconds >= 0).then(|| seconds.saturating_mul(1_000).max(1_000));
    }
    let (_, date) = value.split_once(',')?;
    let mut parts = date.split_whitespace();
    let day = parts.next()?.parse::<i64>().ok()?;
    let month = match parts.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year = parts.next()?.parse::<i64>().ok()?;
    let time = parts.next()?;
    if parts.next()? != "GMT" || parts.next().is_some() {
        return None;
    }
    let mut clock = time.split(':').map(str::parse::<i64>);
    let hour = clock.next()?.ok()?;
    let minute = clock.next()?.ok()?;
    let second = clock.next()?.ok()?;
    if clock.next().is_some() || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60
    {
        return None;
    }
    let adjusted_year = year - i64::from(month <= 2);
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let adjusted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days_since_epoch = era * 146_097 + day_of_era - 719_468;
    let response_ms = days_since_epoch
        .saturating_mul(86_400)
        .saturating_add(hour * 3_600 + minute * 60 + second)
        .saturating_mul(1_000);
    Some(response_ms.saturating_sub(now_ms).max(1_000))
}

pub fn process_once(database_path: &Path, model: &mut LocalModel) -> io::Result<usize> {
    let mut store = LearningStore::open(database_path)?;
    let imported = store.bootstrap_research_sources()?;
    if imported > 0 {
        crate::diagnostics::event(
            "research.sources_bootstrapped",
            serde_json::json!({"sources":imported}),
        );
    }
    store.recover_finished_scholar_feedback()?;
    let provider_config = ProviderConfig::load().unwrap_or_default();
    let user_job = store.claim_user_scholar_job()?;
    if user_job.is_none() {
        if let Some(task) = store.claim_scholar_feedback("repair")? {
            return process_feedback_task_with(&mut store, &task, |task| {
                let guidance = format!(
                    "{}\nPrevious rejected claim: {}\nPreferred exact evidence from reviewer: {}",
                    task.guidance,
                    task.rejected_claim,
                    task.suggested_evidence.as_deref().unwrap_or("none")
                );
                extract_scholar_claim_guided(
                    &task.question,
                    &task.source_title,
                    &task.source_excerpt,
                    &task.source_quality,
                    task.source_updated_at.clone(),
                    Some(&guidance),
                    model,
                )
            });
        }
        let corroboration = store.claim_corroboration_task(
            provider_config.serp_api_key.is_some(),
            provider_config.tavily_api_key.is_some(),
            serp_interval_seconds().saturating_mul(1_000),
            tavily_interval_seconds().saturating_mul(1_000),
        )?;
        if let Some(task) = corroboration {
            return process_corroboration_task(&mut store, &task, model);
        }
        if let Some(source) = store.claim_research_source()? {
            return retention::harvest(&mut store, source, model);
        }
    }
    let job = match user_job {
        Some(job) => Some(job),
        None => store.claim_scholar_job()?,
    };
    let Some(job) = job else {
        return store.automatically_review_pending_scholar_evidence();
    };
    process_job(&mut store, job, model, false)
}

pub fn process_conversation(
    database_path: &Path,
    conversation: &str,
    turn: usize,
    model: &mut LocalModel,
) -> io::Result<usize> {
    let mut store = LearningStore::open(database_path)?;
    let Some(job) = store.claim_conversation_research(conversation, turn)? else {
        return Ok(0);
    };
    process_job(&mut store, job, model, true)
}

fn process_job(
    store: &mut LearningStore,
    job: crate::storage::ScholarJob,
    model: &mut LocalModel,
    foreground: bool,
) -> io::Result<usize> {
    let provider_config = ProviderConfig::load().unwrap_or_default();
    let names = match crate::entities::names_with(&job.question, |messages| {
        model.chat_for_extraction(messages)
    }) {
        Ok(names) => names,
        Err(error) => {
            store.fail_scholar_job(
                &job.id,
                &format!("entity constraints could not be established: {error}"),
            )?;
            return Ok(1);
        }
    };
    // Persist only constraints extracted from the user's request. Search pages
    // can inform evidence but must not silently redefine the task target.
    store.set_scholar_task_constraints(&job.id, &names, &[])?;
    let aliases = store.entity_aliases()?;
    let feedback = store.scholar_feedback_for_job(&job.id)?;
    let excluded_sources = store.scholar_feedback_excluded_sources(&job.id)?;
    let search_request = feedback.as_ref().map_or_else(|| job.question.clone(), |task| {
        format!("Original research question: {}\nReviewer guidance for finding a better source: {}\nSearch for evidence answering the original question; avoid repeating the rejected source.", job.question, task.guidance)
    });
    let expansion = store.scholar_job_is_expansion(&job.id)?;
    let queries = match store.scholar_lookup_queries(&job.id)? {
        Some(queries) => Ok(queries),
        None => model.search_phrase(&search_request).map(|phrase| {
            let mut queries = vec![phrase.unwrap_or_else(|| job.question.clone())];
            if queries[0] != job.question {
                queries.push(job.question.clone());
            }
            queries
        }),
    };
    let mut result = Ok(None);
    let mut logged_query = job.question.clone();
    match queries {
        Err(error) => result = Err(error),
        Ok(mut queries) => {
            if names.len() == 1 && !queries.iter().any(|q| q.eq_ignore_ascii_case(&names[0])) {
                queries.insert(0, names[0].clone());
            }
            for query in queries.into_iter().take(2) {
                logged_query = query;
                result = research(
                    store,
                    &job.id,
                    &job.question,
                    &logged_query,
                    model,
                    expansion,
                    feedback.as_ref().map(|task| task.guidance.as_str()),
                    &excluded_sources,
                    &names,
                );
                if let Ok(Some(found)) = &mut result {
                    found.finding = found.finding.take().filter(|finding| {
                        crate::entities::matches_claim(
                            &names,
                            &finding.subject,
                            &finding.object,
                            &aliases,
                        )
                    });
                }
                if matches!(&result, Ok(Some(found)) if found.finding.is_none()) {
                    result = Ok(None);
                }
                if !matches!(&result, Ok(None)) {
                    break;
                }
                store.record_scholar_search_attempt_for_provider(
                    &job.id,
                    "wikipedia",
                    "lookup_query_no_result",
                    &logged_query,
                    "no_result",
                    Some(0),
                    None,
                )?;
            }
        }
    }
    let search_status = match &result {
        Ok(Some(_)) => "results",
        Ok(None) => "no_result",
        Err(_) => "error",
    };
    let wikipedia_weak = !matches!(
        &result,
        Ok(Some(found)) if found.finding.is_some() && found.research_relevance == "direct"
    );
    let now_ms = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| io::Error::other(error.to_string()))?
            .as_millis(),
    )
    .map_err(|_| io::Error::other("search timestamp too large"))?;
    let mut sources = Vec::new();
    if !foreground || wikipedia_weak {
        if let Some(trigger) = discovery_trigger(
            wikipedia_weak,
            store.last_scholar_provider_search_ms("serpapi")?,
            now_ms,
            serp_interval_seconds(),
        ) {
            discover_serpapi(store, &job.id, &logged_query, trigger);
        }
        if let Some(trigger) = discovery_trigger(
            wikipedia_weak,
            store.last_scholar_provider_search_ms("tavily")?,
            now_ms,
            tavily_interval_seconds(),
        ) {
            discover_tavily(store, &job.id, &logged_query, trigger);
        }
        discover_searxng_fallback(
            store,
            &job.id,
            &logged_query,
            provider_config.searxng_url.as_deref(),
        )?;
        sources = process_discovered_candidates(
            store,
            &job.id,
            &job.question,
            model,
            expansion,
            None,
            foreground,
        )?;
    }
    store.record_scholar_search_attempt(
        &job.id,
        &logged_query,
        search_status,
        result
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .map(|result| result.candidate_count)
            .or_else(|| Some(0)),
        result
            .as_ref()
            .err()
            .map(|error| error.to_string())
            .as_deref(),
    )?;
    match result {
        Ok(result) => {
            let note = if result.is_none() {
                Some("no relevant encyclopedia result")
            } else {
                None
            };
            if let Some(result) = result {
                sources.insert(
                    0,
                    ScholarEvidence {
                        source_title: result.page.title,
                        source_url: result.page.fullurl.unwrap_or_else(|| SEARCH_API.to_owned()),
                        source_excerpt: result.excerpt,
                        finding: result.finding,
                        profile: ScholarSourceProfile::default(),
                        research_relevance: result.research_relevance,
                    },
                );
            }
            store.finish_scholar_job_with_evidence(&job.id, &sources, note)?;
        }
        Err(error) if sources.iter().any(|source| source.finding.is_some()) => {
            store.finish_scholar_job_with_evidence(&job.id, &sources, None)?;
            eprintln!(
                "scholar completed job {} using fetched sources after encyclopedia research failed: {error}",
                job.id
            );
        }
        Err(error) => {
            if error.kind() == io::ErrorKind::WouldBlock {
                let retry_at_ms = store.next_research_retry_ms()?;
                store.defer_scholar_job(&job.id, &error.to_string(), retry_at_ms)?;
                crate::diagnostics::event(
                    "scholar.job_deferred",
                    serde_json::json!({"job_id":job.id,"reason":error.to_string(),"retry_at_ms":retry_at_ms}),
                );
                return Ok(1);
            }
            store.fail_scholar_job(&job.id, &error.to_string())?;
            crate::diagnostics::event(
                "scholar.job_failed",
                serde_json::json!({"job_id": job.id, "error": error.to_string()}),
            );
            eprintln!("scholar recorded no result for job {}: {error}", job.id);
        }
    }
    if let Some(task) = feedback {
        let found = !store.scholar_findings(&job.id)?.is_empty();
        store.finish_scholar_feedback(
            &task,
            found,
            if found {
                "new source produced a finding for AI review"
            } else {
                "no grounded finding from a new source"
            },
        )?;
    }
    Ok(1)
}

fn process_feedback_task_with(
    store: &mut LearningStore,
    task: &crate::storage::ScholarFeedbackTask,
    mut extract: impl FnMut(&crate::storage::ScholarFeedbackTask) -> ExtractionAttempt,
) -> io::Result<usize> {
    crate::diagnostics::event(
        "scholar.guided_repair_started",
        serde_json::json!({
            "feedback_id": task.id, "finding_id": task.finding_id, "root_finding_id": task.root_finding_id,
            "attempt": task.attempt, "max_attempts": 3, "question": task.question, "guidance": task.guidance,
            "suggested_evidence": task.suggested_evidence,
        }),
    );
    let mut attempt = extract(task);
    let finding = attempt.finding.take();
    let found = finding.is_some();
    let relevance = finding
        .as_ref()
        .map(|finding| calibrate_research_relevance(&task.question, finding, &attempt.relevance))
        .unwrap_or_else(|| "unknown".into());
    store.finish_scholar_job_with_evidence(
        &task.job_id,
        &[ScholarEvidence {
            source_title: task.source_title.clone(),
            source_url: task.source_url.clone(),
            source_excerpt: task.source_excerpt.clone(),
            finding,
            profile: ScholarSourceProfile::default(),
            research_relevance: relevance,
        }],
        Some(&attempt.reason),
    )?;
    store.finish_scholar_feedback(task, found, &attempt.reason)?;
    Ok(1)
}

fn discover_searxng_fallback(
    store: &mut LearningStore,
    job_id: &str,
    query: &str,
    endpoint: Option<&str>,
) -> io::Result<bool> {
    if !store.scholar_search_candidates(job_id)?.is_empty() {
        return Ok(false);
    }
    Ok(endpoint.is_some_and(|endpoint| {
        discover_searxng(store, job_id, query, "provider_fallback", endpoint)
    }))
}

fn process_corroboration_task(
    store: &mut LearningStore,
    task: &CorroborationTask,
    model: &mut LocalModel,
) -> io::Result<usize> {
    let query = format!("{} {} {}", task.subject, task.relation, task.object);
    let started = match task.provider.as_str() {
        "serpapi" => discover_serpapi(store, &task.job_id, &query, "corroboration_queue"),
        "tavily" => discover_tavily(store, &task.job_id, &query, "corroboration_queue"),
        _ => return Err(io::Error::other("unsupported corroboration provider")),
    };
    let sources = if started {
        process_discovered_candidates(
            store,
            &task.job_id,
            &task.question,
            model,
            false,
            Some(task),
            false,
        )?
    } else {
        Vec::new()
    };
    let grounded = sources.iter().any(|source| source.finding.is_some());
    store.finish_scholar_job_with_evidence(
        &task.job_id,
        &sources,
        Some(if !started {
            "provider slot unavailable; queued for another eligible pass"
        } else if grounded {
            "corroboration evidence recorded"
        } else {
            "no matching independent evidence found"
        }),
    )?;
    store.finish_corroboration_task(
        task,
        if !started {
            "deferred"
        } else if grounded {
            "evidence_recorded"
        } else {
            "no_match"
        },
    )?;
    Ok(usize::from(started))
}

fn discovery_trigger(
    wikipedia_weak: bool,
    last_search_ms: Option<i64>,
    now_ms: i64,
    minimum_interval_seconds: i64,
) -> Option<&'static str> {
    let interval_ms = minimum_interval_seconds.saturating_mul(1_000);
    if wikipedia_weak {
        return last_search_ms
            .is_none_or(|last| now_ms >= last.saturating_add(interval_ms))
            .then_some("wikipedia_insufficient");
    }
    last_search_ms
        .is_none_or(|last| now_ms >= last.saturating_add(interval_ms.saturating_mul(2)))
        .then_some("exploration_elapsed")
}

fn finding_matches_target(finding: &ScholarFinding, target: &CorroborationTask) -> bool {
    finding
        .subject
        .trim()
        .eq_ignore_ascii_case(target.subject.trim())
        && finding
            .relation
            .trim()
            .eq_ignore_ascii_case(target.relation.trim())
        && finding
            .object
            .trim()
            .eq_ignore_ascii_case(target.object.trim())
        && finding.valid_from == target.valid_from
        && finding.valid_until == target.valid_until
}

fn normalized_source_host(host: &str) -> String {
    let lowered = host.to_ascii_lowercase();
    lowered.strip_prefix("www.").unwrap_or(&lowered).to_owned()
}

fn process_discovered_candidates(
    store: &mut LearningStore,
    job_id: &str,
    question: &str,
    model: &mut LocalModel,
    expansion: bool,
    target: Option<&CorroborationTask>,
    foreground: bool,
) -> io::Result<Vec<ScholarEvidence>> {
    let names =
        crate::entities::names_with(question, |messages| model.chat_for_extraction(messages))?;
    let aliases = store.entity_aliases()?;
    let feedback = store.scholar_feedback_for_job(job_id)?;
    let excluded_sources = store.scholar_feedback_excluded_sources(job_id)?;
    let existing_hosts = if let Some(target) = target {
        store
            .corroboration_source_urls(target)?
            .into_iter()
            .filter_map(|url| url::Url::parse(&url).ok())
            .filter_map(|url| url.host_str().map(normalized_source_host))
            .collect::<std::collections::HashSet<_>>()
    } else {
        std::collections::HashSet::new()
    };
    let candidates = store.scholar_search_candidates(job_id)?;
    let mut evidence = Vec::new();
    let mut grouped = std::collections::BTreeMap::<(String, String), Vec<_>>::new();
    for candidate in candidates
        .into_iter()
        .filter(|candidate| candidate.status == "discovered")
    {
        grouped
            .entry((candidate.provider.clone(), candidate.query.clone()))
            .or_default()
            .push(candidate);
    }
    for (_, mut candidates) in grouped {
        if foreground && !evidence.is_empty() {
            break;
        }
        candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate_relevance(candidate)));
        let mut inspected = 0;
        let mut page_fetches = 0;
        let mut visited_hosts = std::collections::BTreeSet::new();
        for candidate in candidates {
            if foreground && !evidence.is_empty() {
                break;
            }
            if feedback_source_blocked(&candidate.url, &excluded_sources) {
                store.record_scholar_candidate_fetch_result(
                    candidate.id,
                    "skipped",
                    "allowed",
                    Some("source excluded by Curmudgeon feedback"),
                    "skipped",
                    None,
                    None,
                )?;
                crate::diagnostics::event(
                    "internet.page_review",
                    serde_json::json!({
                        "job_id": job_id, "url": crate::diagnostics::safe_public_url(&candidate.url),
                        "fetch_status": "skipped", "policy_reason": "source excluded by Curmudgeon feedback",
                        "exclusion_scope": "root_finding_retry_chain",
                        "root_finding_id": feedback.as_ref().map(|task| task.root_finding_id),
                    }),
                );
                continue;
            }
            // Search engines can put blocked or broken hosts near the top.
            // Inspect a bounded shortlist, while allowing up to three actual
            // page fetch attempts across it.
            if inspected >= 12 || page_fetches >= 3 {
                break;
            }
            let host = url::Url::parse(&candidate.url)
                .ok()
                .and_then(|url| url.host_str().map(str::to_owned));
            if host
                .as_ref()
                .is_some_and(|host| existing_hosts.contains(&normalized_source_host(host)))
            {
                store.record_scholar_candidate_fetch_result(
                    candidate.id,
                    "skipped",
                    "allowed",
                    Some("host already supplied evidence for this claim"),
                    "skipped",
                    None,
                    None,
                )?;
                continue;
            }
            if host.is_some_and(|host| !visited_hosts.insert(host)) {
                continue;
            }
            inspected += 1;
            let review = site_policy::inspect_and_fetch(store, &candidate);
            if matches!(review.fetch_status, "fetched" | "failed") {
                page_fetches += 1;
            }
            if review
                .fetched_url
                .as_deref()
                .is_some_and(|url| feedback_source_blocked(url, &excluded_sources))
            {
                store.record_scholar_candidate_fetch_result(
                    candidate.id,
                    "skipped",
                    "allowed",
                    Some("redirected source excluded by Curmudgeon feedback"),
                    "skipped",
                    review.fetched_url.as_deref(),
                    None,
                )?;
                crate::diagnostics::event(
                    "internet.page_review",
                    serde_json::json!({
                        "job_id": job_id, "url": crate::diagnostics::safe_public_url(&candidate.url),
                        "fetched_url": review.fetched_url.as_deref().map(crate::diagnostics::safe_public_url),
                        "fetch_status": "skipped", "policy_reason": "redirected source excluded by Curmudgeon feedback",
                    }),
                );
                continue;
            }
            store.record_scholar_candidate_fetch_result(
                candidate.id,
                review.status,
                review.policy_status,
                Some(&review.policy_reason),
                review.fetch_status,
                review.fetched_url.as_deref(),
                review.excerpt.as_deref(),
            )?;
            crate::diagnostics::event(
                "internet.page_review",
                serde_json::json!({
                    "job_id": job_id,
                    "provider": candidate.provider,
                    "query": candidate.query,
                    "title": candidate.title,
                    "url": crate::diagnostics::safe_public_url(&candidate.url),
                    "policy_status": review.policy_status,
                    "policy_reason": review.policy_reason,
                    "fetch_status": review.fetch_status,
                    "fetched_url": review.fetched_url.as_deref().map(crate::diagnostics::safe_public_url),
                }),
            );
            if let Some(provenance_json) = review.provenance_json.as_deref() {
                store.record_scholar_candidate_provenance(candidate.id, provenance_json)?;
            }
            if review.status == "fetched" {
                if let (Some(source_url), Some(excerpt)) = (review.fetched_url, review.excerpt) {
                    let mut accepted = None;
                    for (index, passage) in review.passages.iter().enumerate() {
                        let mut attempt = extract_scholar_claim_guided(
                            question,
                            &candidate.title,
                            passage,
                            "unknown",
                            None,
                            feedback.as_ref().map(|task| task.guidance.as_str()),
                            model,
                        );
                        attempt.finding =
                            select_finding(attempt.finding, expansion).filter(|finding| {
                                crate::entities::matches_claim(
                                    &names,
                                    &finding.subject,
                                    &finding.object,
                                    &aliases,
                                )
                            });
                        if let Some(target) = target {
                            attempt.finding = attempt
                                .finding
                                .filter(|finding| finding_matches_target(finding, target));
                        }
                        if attempt.status == "accepted" && attempt.finding.is_none() {
                            attempt.status = "filtered_expansion";
                            attempt.reason = if target.is_some() {
                                "extracted claim did not match the queued corroboration target"
                                    .into()
                            } else {
                                "expansion question would repeat an encyclopedia summary".into()
                            };
                        }
                        store.record_scholar_extraction_attempt(
                            candidate.id,
                            index + 1,
                            passage,
                            attempt.status,
                            &attempt.reason,
                            attempt.model_output.as_deref(),
                        )?;
                        if let Some(finding) = attempt.finding {
                            let research_relevance = calibrate_research_relevance(
                                question,
                                &finding,
                                &attempt.relevance,
                            );
                            accepted = Some(ScholarEvidence {
                                source_title: candidate.title.clone(),
                                source_url: source_url.clone(),
                                source_excerpt: passage.clone(),
                                finding: Some(finding),
                                profile: ScholarSourceProfile::default(),
                                research_relevance,
                            });
                            break;
                        }
                    }
                    if accepted.is_none() {
                        for passage in &review.passages {
                            let Some(finding) = structured_list_finding(question, passage) else {
                                continue;
                            };
                            if !crate::entities::matches_claim(
                                &names,
                                &finding.subject,
                                &finding.object,
                                &aliases,
                            ) || target
                                .is_some_and(|target| !finding_matches_target(&finding, target))
                            {
                                continue;
                            }
                            store.record_scholar_extraction_attempt(
                                candidate.id,
                                3,
                                passage,
                                "structured_list",
                                "explicit heading and consecutive bullets formed a source-specific list; held for review",
                                None,
                            )?;
                            let research_relevance =
                                calibrate_research_relevance(question, &finding, "related");
                            accepted = Some(ScholarEvidence {
                                source_title: candidate.title.clone(),
                                source_url: source_url.clone(),
                                source_excerpt: passage.clone(),
                                finding: Some(finding),
                                profile: ScholarSourceProfile::default(),
                                research_relevance,
                            });
                            break;
                        }
                    }
                    let grounded = accepted.is_some();
                    evidence.push(accepted.unwrap_or_else(|| ScholarEvidence {
                        source_title: candidate.title,
                        source_url,
                        source_excerpt: excerpt,
                        finding: None,
                        profile: ScholarSourceProfile::default(),
                        research_relevance: "unknown".into(),
                    }));
                    if grounded {
                        break;
                    }
                }
            }
        }
    }
    Ok(evidence)
}

fn candidate_relevance(candidate: &crate::storage::ScholarSearchCandidate) -> (usize, usize) {
    let query_words = crate::storage::relevant_words(&candidate.query);
    let page_words =
        crate::storage::relevant_words(&format!("{} {}", candidate.title, candidate.snippet));
    (
        crate::storage::lexical_overlap(&query_words, &page_words),
        usize::from(matches!(
            candidate.result_type.as_str(),
            "organic_result" | "search_result"
        )),
    )
}

fn feedback_source_blocked(value: &str, excluded: &[String]) -> bool {
    let Ok(mut url) = url::Url::parse(value) else {
        return false;
    };
    url.set_query(None);
    url.set_fragment(None);
    excluded.iter().any(|source| {
        let Ok(mut source) = url::Url::parse(source) else {
            return false;
        };
        source.set_query(None);
        source.set_fragment(None);
        url.host_str().map(normalized_source_host) == source.host_str().map(normalized_source_host)
            && url.path() == source.path()
    })
}

fn research(
    store: &mut LearningStore,
    job_id: &str,
    question: &str,
    search_phrase: &str,
    model: &mut LocalModel,
    expansion: bool,
    guidance: Option<&str>,
    excluded_sources: &[String],
    names: &[String],
) -> io::Result<Option<ResearchResult>> {
    let response: SearchResponse = wikipedia_json_request(
        store,
        "wikipedia_search",
        262_144,
        &[
            ("action", "query".into()),
            ("generator", "search".into()),
            ("gsrlimit", "3".into()),
            ("prop", "extracts|info|revisions".into()),
            ("rvprop", "timestamp".into()),
            ("explaintext", "1".into()),
            ("exintro", "1".into()),
            ("exchars", MAX_SEARCH_EXTRACT_CHARS.to_string()),
            ("inprop", "url".into()),
            ("format", "json".into()),
            ("formatversion", "2".into()),
            ("gsrsearch", search_phrase.to_owned()),
        ],
    )
    .and_then(|json| {
        serde_json::from_value(json)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    })?;
    let Some(mut pages) = response.query.map(|query| query.pages) else {
        return Ok(None);
    };
    pages.sort_by_key(|page| page.index);
    for page in &pages {
        if let (Some(url), Some(excerpt)) = (page.fullurl.as_deref(), page.extract.as_deref()) {
            if url.starts_with("https://en.wikipedia.org/wiki/") {
                store.retain_research_source_with_metadata(
                    job_id,
                    &page.title,
                    url,
                    excerpt,
                    "wikipedia",
                    "lead_extract_excerpt",
                    Some(search_phrase),
                    page.revisions
                        .as_ref()
                        .and_then(|revisions| revisions.first())
                        .and_then(|revision| revision.timestamp.as_deref()),
                )?;
            }
        }
    }
    let mut candidates = pages
        .into_iter()
        .filter(|page| {
            !page.title.to_lowercase().contains("(disambiguation)")
                && !page
                    .fullurl
                    .as_deref()
                    .is_some_and(|url| feedback_source_blocked(url, excluded_sources))
                && page
                    .fullurl
                    .as_deref()
                    .is_some_and(|url| url.starts_with("https://en.wikipedia.org/wiki/"))
                && page
                    .extract
                    .as_deref()
                    .is_some_and(|text| !text.trim().is_empty())
                && (names.is_empty()
                    || names.iter().any(|name| {
                        page.title.eq_ignore_ascii_case(name)
                            || page
                                .title
                                .to_lowercase()
                                .starts_with(&format!("{} (", name.to_lowercase()))
                            || page.extract.as_deref().is_some_and(|excerpt| {
                                excerpt.to_lowercase().contains(&name.to_lowercase())
                            })
                    }))
                && (relevant_page(question, page) || relevant_page(search_phrase, page))
        })
        .map(|page| {
            let named_title = names.iter().any(|name| {
                page.title.eq_ignore_ascii_case(name)
                    || page
                        .title
                        .to_lowercase()
                        .starts_with(&format!("{} (", name.to_lowercase()))
            });
            let score = page_relevance_score(question, &page) + if named_title { 1000 } else { 0 };
            let excerpt = relevant_passages(question, page.extract.as_deref().unwrap_or_default());
            (score, page, excerpt)
        })
        .collect::<Vec<_>>();
    let candidate_count = candidates.len();
    candidates.sort_by(|(left_score, left, _), (right_score, right, _)| {
        right_score
            .cmp(left_score)
            .then_with(|| left.index.cmp(&right.index))
    });
    let mut candidates = candidates.into_iter();
    let Some((_, mut page, lead_excerpt)) = candidates.next() else {
        return Ok(None);
    };
    let section_response: ParsedPageResponse = wikipedia_json_request(
        store,
        "wikipedia_sections",
        262_144,
        &[
            ("action", "parse".into()),
            ("pageid", page.pageid.to_string()),
            ("prop", "sections".into()),
            ("format", "json".into()),
            ("formatversion", "2".into()),
        ],
    )
    .and_then(|json| {
        serde_json::from_value(json)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    })?;
    let available_sections = section_response
        .parse
        .as_ref()
        .map(|parsed| &parsed.sections[..])
        .unwrap_or(&[]);
    let section_titles = available_sections
        .iter()
        .take(60)
        .filter_map(|section| {
            let index = section
                .index
                .as_str()
                .map(str::to_owned)
                .or_else(|| section.index.as_i64().map(|i| i.to_string()))?;
            Some((index, section.line.clone()))
        })
        .collect::<Vec<_>>();
    let sections =
        match crate::research::select_sections_with(question, &section_titles, |messages| {
            model.chat_for_extraction(messages)
        }) {
            Ok(indices) => indices
                .into_iter()
                .filter_map(|index| {
                    section_titles
                        .iter()
                        .find(|s| s.0 == index)
                        .map(|(_, title)| SectionChoice {
                            index,
                            title: title.clone(),
                        })
                })
                .collect::<Vec<_>>(),
            Err(error) => {
                crate::diagnostics::event(
                    "scholar.section_selection_failed",
                    serde_json::json!({"reason":error.to_string()}),
                );
                ranked_sections(question, available_sections)
            }
        };
    let mut section_passages = Vec::new();
    for section in sections.iter().take(2) {
        let parsed: ParsedPageResponse = wikipedia_json_request(
            store,
            "wikipedia_page_content",
            MAX_PARSED_PAGE_BYTES,
            &[
                ("action", "parse".into()),
                ("pageid", page.pageid.to_string()),
                ("prop", "text".into()),
                ("section", section.index.clone()),
                ("disabletoc", "1".into()),
                ("format", "json".into()),
                ("formatversion", "2".into()),
            ],
        )
        .and_then(|json| {
            serde_json::from_value(json)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
        })?;
        let section_text = parsed
            .parse
            .as_ref()
            .and_then(|parsed| parsed.text.as_ref())
            .and_then(text_property)
            .map(html_to_text)
            .transpose()?
            .filter(|text| !text.trim().is_empty());
        let Some(section_text) = section_text else {
            continue;
        };
        if let Some(url) = page.fullurl.as_deref() {
            store.retain_research_source_with_metadata(
                job_id,
                &page.title,
                url,
                &section_text,
                "wikipedia",
                "selected_section",
                Some(question),
                page.revisions
                    .as_ref()
                    .and_then(|revisions| revisions.first())
                    .and_then(|revision| revision.timestamp.as_deref()),
            )?;
        }
        let section_text = section_text
            .chars()
            .take(MAX_PARSED_ARTICLE_CHARS)
            .collect::<String>();
        if section_text.trim().is_empty() {
            continue;
        }
        let passage = relevant_passages(question, &section_text);
        section_passages.push(format!("Section: {}\n\n{passage}", section.title));
    }
    let mut excerpt = if section_passages.is_empty() {
        lead_excerpt.clone()
    } else {
        section_passages.join("\n\n")
    };
    let source_updated_at = page
        .revisions
        .as_ref()
        .and_then(|revisions| revisions.first())
        .and_then(|revision| revision.timestamp.clone());
    let mut attempt = extract_scholar_claim_guided(
        question,
        &page.title,
        &excerpt,
        "reference",
        source_updated_at.clone(),
        guidance,
        model,
    );
    let mut extracted = attempt.finding.take().or_else(|| {
        explicit_list_finding(question, &page.title, &excerpt, source_updated_at.clone())
    });
    if extracted.is_none() && !section_passages.is_empty() {
        let mut intro_attempt = extract_scholar_claim_guided(
            question,
            &page.title,
            &lead_excerpt,
            "reference",
            source_updated_at.clone(),
            guidance,
            model,
        );
        let intro_finding = intro_attempt.finding.take().or_else(|| {
            explicit_list_finding(question, &page.title, &lead_excerpt, source_updated_at)
        });
        if intro_finding.is_some() {
            excerpt = lead_excerpt;
            attempt = intro_attempt;
            extracted = intro_finding;
        }
    }
    let mut finding = select_finding(extracted, expansion);
    let mut research_relevance = finding
        .as_ref()
        .map(|claim| calibrate_research_relevance(question, claim, &attempt.relevance))
        .unwrap_or(attempt.relevance);
    if finding.is_none() {
        for (_, alternative_page, alternative_excerpt) in candidates.take(2) {
            let alternative_updated_at = alternative_page
                .revisions
                .as_ref()
                .and_then(|revisions| revisions.first())
                .and_then(|revision| revision.timestamp.clone());
            let mut alternative_attempt = extract_scholar_claim_guided(
                question,
                &alternative_page.title,
                &alternative_excerpt,
                "reference",
                alternative_updated_at.clone(),
                guidance,
                model,
            );
            let alternative_extracted = alternative_attempt.finding.take().or_else(|| {
                explicit_list_finding(
                    question,
                    &alternative_page.title,
                    &alternative_excerpt,
                    alternative_updated_at,
                )
            });
            if let Some(alternative_finding) = select_finding(alternative_extracted, expansion) {
                research_relevance = calibrate_research_relevance(
                    question,
                    &alternative_finding,
                    &alternative_attempt.relevance,
                );
                page = alternative_page;
                excerpt = alternative_excerpt;
                finding = Some(alternative_finding);
                break;
            }
        }
    }
    Ok(Some(ResearchResult {
        page,
        excerpt,
        finding,
        research_relevance,
        candidate_count,
    }))
}

/// Exercise the real extraction and repair path without searches or stored findings.
fn relevant_page(question: &str, page: &SearchPage) -> bool {
    const STOP: &[&str] = &[
        "about", "does", "first", "from", "have", "into", "many", "much", "some", "that", "their",
        "there", "these", "this", "what", "when", "where", "which", "while", "with", "would",
    ];
    let lower = question.to_lowercase();
    let terms = lower
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| word.len() >= 4 && !STOP.contains(word))
        .collect::<std::collections::BTreeSet<_>>();
    if terms.is_empty() {
        return false;
    }
    let text = format!(
        "{} {}",
        page.title,
        page.extract.as_deref().unwrap_or_default()
    )
    .to_lowercase();
    let hits = terms.iter().filter(|word| text.contains(**word)).count();
    hits >= terms.len().min(2)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests;
