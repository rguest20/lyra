use super::*;
use std::io::Write;
use std::process::{Child, Stdio};

pub(super) use crate::storage::ScholarSearchCandidateInput as SearchCandidate;
pub(super) use run_serpapi_discovery as discover_serpapi;
pub(super) use run_tavily_discovery as discover_tavily;

pub(super) fn discover_searxng(
    store: &mut LearningStore,
    job_id: &str,
    query: &str,
    reason: &str,
    endpoint: &str,
) -> bool {
    run_searxng_discovery(store, job_id, query, reason, endpoint)
}

#[derive(Deserialize)]
pub(super) struct SerpApiResponse {
    #[serde(default)]
    organic_results: Vec<SerpApiResult>,
    error: Option<String>,
    #[serde(flatten)]
    pub(super) structured_results: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct SerpApiResult {
    title: Option<String>,
    link: Option<String>,
    snippet: Option<String>,
}

#[derive(Clone, Copy)]
struct SearchAttempt<'a> {
    job_id: &'a str,
    provider: &'a str,
    trigger_reason: &'a str,
    query: &'a str,
}

impl SearchAttempt<'_> {
    fn record(
        self,
        store: &mut LearningStore,
        outcome: &str,
        result_count: Option<usize>,
        error: Option<&str>,
    ) {
        let _ = store.record_scholar_search_attempt_for_provider(
            crate::storage::ScholarSearchAttemptInput {
                job_id: self.job_id,
                provider: self.provider,
                trigger_reason: self.trigger_reason,
                query: self.query,
                status: outcome,
                result_count,
                error,
            },
        );
    }

    fn record_results(
        self,
        store: &mut LearningStore,
        result_count: Option<usize>,
        error: Option<&str>,
    ) {
        let outcome = if error.is_some() {
            "error"
        } else if result_count == Some(0) {
            "no_result"
        } else {
            "results"
        };
        self.record(store, outcome, result_count, error);
    }
}

pub(super) fn is_serpapi_metadata_field(name: &str) -> bool {
    matches!(
        name,
        "search_metadata"
            | "search_parameters"
            | "search_information"
            | "serpapi_pagination"
            | "pagination"
            | "error"
            | "organic_results"
    )
}

pub(super) fn linked_serpapi_results(
    result_type: &str,
    value: &serde_json::Value,
) -> Vec<SearchCandidate> {
    fn visit(
        result_type: &str,
        value: &serde_json::Value,
        depth: usize,
        output: &mut Vec<SearchCandidate>,
    ) {
        if depth > 4 || output.len() >= 20 {
            return;
        }
        match value {
            serde_json::Value::Array(items) => {
                for item in items.iter().take(20) {
                    visit(result_type, item, depth + 1, output);
                }
            }
            serde_json::Value::Object(fields) => {
                let title = ["title", "name", "product_title", "headline"]
                    .iter()
                    .find_map(|key| fields.get(*key).and_then(serde_json::Value::as_str));
                let link = ["link", "url", "website"]
                    .iter()
                    .find_map(|key| fields.get(*key).and_then(serde_json::Value::as_str));
                if let (Some(title), Some(link)) =
                    (title.map(str::trim).filter(|title| !title.is_empty()), link)
                    && let Ok(url) = url::Url::parse(link)
                    && matches!(url.scheme(), "http" | "https")
                    && let Some(url) = clean_serpapi_result_url(url)
                {
                    let snippet = ["snippet", "description", "description_snippet"]
                        .iter()
                        .find_map(|key| fields.get(*key).and_then(serde_json::Value::as_str))
                        .map(str::to_owned)
                        .or_else(|| {
                            fields
                                .get("ingredients")
                                .and_then(serde_json::Value::as_array)
                                .map(|items| {
                                    items
                                        .iter()
                                        .take(30)
                                        .filter_map(serde_json::Value::as_str)
                                        .collect::<Vec<_>>()
                                        .join("; ")
                                })
                        })
                        .unwrap_or_default();
                    let structured = compact_serp_json(value);
                    output.push(SearchCandidate {
                        result_type: result_type.to_owned(),
                        title: title.chars().take(500).collect(),
                        url,
                        snippet: snippet.chars().take(2_000).collect(),
                        structured_data_json: structured,
                    });
                    return;
                }
                for child in fields.values() {
                    visit(result_type, child, depth + 1, output);
                    if output.len() >= 20 {
                        break;
                    }
                }
            }
            _ => {}
        }
    }

    let mut output = Vec::new();
    visit(result_type, value, 0, &mut output);
    output
}

fn clean_serpapi_result_url(mut url: url::Url) -> Option<String> {
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let _ = url.set_username("");
    let _ = url.set_password(None);
    let sensitive = ["api_key", "key", "token", "access_token", "auth", "session"];
    let safe_pairs = url
        .query_pairs()
        .filter(|(name, _)| !sensitive.contains(&name.to_ascii_lowercase().as_str()))
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    url.set_query(None);
    if !safe_pairs.is_empty() {
        url.query_pairs_mut().extend_pairs(safe_pairs);
    }
    url.set_fragment(None);
    Some(url.to_string())
}

fn compact_serp_json(value: &serde_json::Value) -> String {
    fn compact(value: &serde_json::Value, depth: usize, budget: &mut usize) -> serde_json::Value {
        if depth > 4 || *budget == 0 {
            return serde_json::Value::Null;
        }
        match value {
            serde_json::Value::String(text) => {
                let max_chars = (*budget / 6).min(1_000);
                let text = text.chars().take(max_chars).collect::<String>();
                *budget = budget.saturating_sub(text.len());
                serde_json::Value::String(text)
            }
            serde_json::Value::Array(items) => serde_json::Value::Array(
                items
                    .iter()
                    .take(12)
                    .map(|item| compact(item, depth + 1, budget))
                    .collect(),
            ),
            serde_json::Value::Object(fields) => {
                let mut safe = serde_json::Map::new();
                for (key, item) in fields.iter().take(24) {
                    if matches!(
                        key.to_ascii_lowercase().as_str(),
                        "api_key" | "token" | "access_token" | "auth" | "session"
                    ) {
                        continue;
                    }
                    if *budget == 0 {
                        break;
                    }
                    *budget = budget.saturating_sub(key.len());
                    safe.insert(
                        key.chars().take(100).collect(),
                        compact(item, depth + 1, budget),
                    );
                }
                serde_json::Value::Object(safe)
            }
            _ => value.clone(),
        }
    }
    let mut budget = 5_000;
    let safe = compact(value, 0, &mut budget);
    let serialized = serde_json::to_string(&safe).unwrap_or_else(|_| "{}".into());
    if serialized.len() <= 8_000 {
        serialized
    } else {
        "{}".into()
    }
}

pub(super) fn serp_interval_seconds() -> i64 {
    configured_interval_seconds("SCHOLAR_SERP_INTERVAL_SECONDS", 1_800)
}

pub(super) fn tavily_interval_seconds() -> i64 {
    configured_interval_seconds("SCHOLAR_TAVILY_INTERVAL_SECONDS", 3_600)
}

fn configured_interval_seconds(name: &str, default: i64) -> i64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn spawn_search_curl() -> io::Result<Child> {
    super::limited_curl(20, 1_048_576)
        .args(["--config", "-"])
        .arg("--write-out")
        .arg(super::http_response_write_out(HTTP_STATUS_MARKER))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
}

fn response_metadata(output: &[u8]) -> super::HttpResponseMetadata {
    super::parse_http_response(output, HTTP_STATUS_MARKER)
        .map_or_else(super::HttpResponseMetadata::default, |(_, metadata)| {
            metadata
        })
}

struct SearchHttpResponse {
    body: String,
    metadata: super::HttpResponseMetadata,
}

fn parse_search_http_response(output: &[u8]) -> SearchHttpResponse {
    let (body, metadata) = super::parse_http_response(output, HTTP_STATUS_MARKER)
        .unwrap_or((output, super::HttpResponseMetadata::default()));
    SearchHttpResponse {
        body: String::from_utf8_lossy(body).into_owned(),
        metadata,
    }
}

fn reserve_search_http_slot(
    store: &mut LearningStore,
    attempt: SearchAttempt<'_>,
    host: &str,
    request_kind: &str,
) -> Option<crate::storage::HostRequestReservation> {
    match store.reserve_host_request_wait(
        host,
        request_kind,
        10_000,
        std::time::Duration::from_secs(30),
    ) {
        Ok(reservation) => Some(reservation),
        Err(error) => {
            attempt.record(
                store,
                "error",
                None,
                Some(&format!("discovery request deferred: {error}")),
            );
            None
        }
    }
}

fn record_provider_unavailable(
    store: &mut LearningStore,
    attempt: SearchAttempt<'_>,
    reason: &str,
) -> bool {
    attempt.record(store, "error", None, Some(reason));
    false
}

/// Tavily structured results are also leads only. Its persistent ledger
/// enforces the provider's configured call and credit ceilings.
pub(super) fn run_tavily_discovery(
    store: &mut LearningStore,
    job_id: &str,
    query: &str,
    trigger_reason: &str,
) -> bool {
    let attempt = SearchAttempt {
        job_id,
        provider: "tavily",
        trigger_reason,
        query,
    };
    let config = ProviderConfig::load().unwrap_or_default();
    let Some(api_key) = config.tavily_api_key else {
        return record_provider_unavailable(
            store,
            attempt,
            config
                .config_error
                .as_deref()
                .unwrap_or("provider_unavailable: Tavily credentials are not configured"),
        );
    };
    let Some(http_slot) =
        reserve_search_http_slot(store, attempt, "api.tavily.com", "tavily_search")
    else {
        return false;
    };
    let reservation = match store.reserve_tavily_search_with_interval(
        "basic",
        tavily_interval_seconds().saturating_mul(1_000),
    ) {
        Ok(reservation) => reservation,
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
            let _ = store.complete_host_request(
                http_slot.id,
                None,
                Some("Tavily request was not dispatched"),
            );
            attempt.record(
                store,
                "error",
                None,
                Some(&format!("Tavily request not dispatched: {error}")),
            );
            return false;
        }
        Err(error) => {
            let _ = store.complete_host_request(
                http_slot.id,
                None,
                Some("Tavily request was not dispatched"),
            );
            attempt.record(
                store,
                "error",
                None,
                Some(&format!("could not reserve Tavily search: {error}")),
            );
            return false;
        }
    };
    let body = serde_json::json!({
        "query": query,
        "search_depth": "basic",
        "max_results": 5,
        "include_answer": false
    });
    let body = body.to_string();
    let mut child = match spawn_search_curl() {
        Ok(child) => child,
        Err(error) => {
            let _ = store.complete_host_request(http_slot.id, None, Some("curl could not run"));
            let _ = store.complete_tavily_search(reservation.id, None, Some("curl could not run"));
            attempt.record(
                store,
                "error",
                None,
                Some(&format!("curl could not run: {error}")),
            );
            return true;
        }
    };
    write_tavily_request(&mut child, &api_key, &body);
    let output = child.wait_with_output();
    let output = match output {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            let metadata = response_metadata(&output.stdout);
            let _ = super::complete_reserved_http_request(
                store,
                http_slot.id,
                &metadata,
                Some("Tavily request failed"),
            );
            let _ = store.complete_tavily_search(
                reservation.id,
                metadata.status,
                Some("Tavily request failed"),
            );
            attempt.record(store, "error", None, Some("Tavily request failed"));
            return true;
        }
        Err(error) => {
            let _ = store.complete_host_request(http_slot.id, None, Some("curl could not run"));
            let _ = store.complete_tavily_search(reservation.id, None, Some("curl could not run"));
            attempt.record(
                store,
                "error",
                None,
                Some(&format!("Tavily request failed: {error}")),
            );
            return true;
        }
    };
    let http_response = parse_search_http_response(&output.stdout);
    let _ =
        super::complete_reserved_http_request(store, http_slot.id, &http_response.metadata, None);
    match parse_tavily_candidates(http_response.body.as_bytes()) {
        Ok(candidates) => {
            let result = persist_search_candidates(store, job_id, "tavily", query, &candidates);
            let error = result.as_ref().err().map(std::string::ToString::to_string);
            let _ = store.complete_tavily_search(
                reservation.id,
                http_response.metadata.status,
                error.as_deref(),
            );
            attempt.record_results(store, Some(candidates.len()), error.as_deref());
            if should_retry_tavily_advanced(
                http_response.metadata.status,
                candidates.len(),
                error.is_some(),
            ) {
                run_tavily_advanced_retry(store, job_id, query, &api_key);
            }
        }
        Err(_) => {
            let _ = store.complete_tavily_search(
                reservation.id,
                http_response.metadata.status,
                Some("Tavily returned invalid JSON"),
            );
            attempt.record(store, "error", None, Some("Tavily returned invalid JSON"));
        }
    }
    true
}

fn should_retry_tavily_advanced(
    response_status: Option<u16>,
    usable_candidates: usize,
    basic_error: bool,
) -> bool {
    response_status == Some(200) && usable_candidates == 0 && !basic_error
}

fn parse_tavily_candidates(body: &[u8]) -> Result<Vec<SearchCandidate>, serde_json::Error> {
    let response = serde_json::from_slice::<TavilyResponse>(body)?;
    Ok(response
        .results
        .into_iter()
        .filter_map(|item| {
            let title = item.title?.trim().to_owned();
            let link = item.url?;
            let mut url = url::Url::parse(&link).ok()?;
            if !matches!(url.scheme(), "http" | "https") || title.is_empty() {
                return None;
            }
            url.set_fragment(None);
            Some(SearchCandidate {
                result_type: "search_result".into(),
                title,
                url: url.to_string(),
                snippet: item.content.unwrap_or_default(),
                structured_data_json: "{}".into(),
            })
        })
        .take(20)
        .collect())
}

fn persist_search_candidates(
    store: &mut LearningStore,
    job_id: &str,
    provider: &str,
    query: &str,
    candidates: &[SearchCandidate],
) -> io::Result<()> {
    candidates.iter().try_for_each(|candidate| {
        store.record_typed_search_candidate(job_id, provider, query, candidate)
    })
}

fn run_tavily_advanced_retry(store: &mut LearningStore, job_id: &str, query: &str, api_key: &str) {
    const RETRY_REASON: &str = "advanced_retry";
    let attempt = SearchAttempt {
        job_id,
        provider: "tavily",
        trigger_reason: RETRY_REASON,
        query,
    };
    let Some(slot) = reserve_search_http_slot(store, attempt, "api.tavily.com", "tavily_search")
    else {
        return;
    };
    let reservation = match store.reserve_tavily_search_with_interval("advanced", 0) {
        Ok(reservation) => reservation,
        Err(error) => {
            let _ = store.complete_host_request(
                slot.id,
                None,
                Some("Tavily advanced retry was not dispatched"),
            );
            attempt.record(
                store,
                "error",
                None,
                Some(&format!("Tavily advanced retry not dispatched: {error}")),
            );
            return;
        }
    };
    let body = serde_json::json!({
        "query": query,
        "search_depth": "advanced",
        "max_results": 5,
        "include_answer": false
    })
    .to_string();
    let mut child = match spawn_search_curl() {
        Ok(child) => child,
        Err(error) => {
            let _ = store.complete_host_request(slot.id, None, Some("curl could not run"));
            let _ = store.complete_tavily_search(reservation.id, None, Some("curl could not run"));
            attempt.record(
                store,
                "error",
                None,
                Some(&format!("Tavily advanced retry failed: {error}")),
            );
            return;
        }
    };
    write_tavily_request(&mut child, api_key, &body);
    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(error) => {
            let _ = store.complete_host_request(slot.id, None, Some("curl request failed"));
            let _ = store.complete_tavily_search(reservation.id, None, Some("curl request failed"));
            attempt.record(
                store,
                "error",
                None,
                Some(&format!("Tavily advanced retry failed: {error}")),
            );
            return;
        }
    };
    let response = parse_search_http_response(&output.stdout);
    let succeeded = output.status.success();
    let _ = super::complete_reserved_http_request(
        store,
        slot.id,
        &response.metadata,
        (!succeeded).then_some("Tavily advanced request failed"),
    );
    if !succeeded {
        let _ = store.complete_tavily_search(
            reservation.id,
            response.metadata.status,
            Some("Tavily request failed"),
        );
        attempt.record(store, "error", None, Some("Tavily advanced request failed"));
        return;
    }
    let (candidate_count, error) = match parse_tavily_candidates(response.body.as_bytes()) {
        Ok(candidates) => {
            let result = persist_search_candidates(store, job_id, "tavily", query, &candidates);
            (
                Some(candidates.len()),
                result.err().map(|error| error.to_string()),
            )
        }
        Err(_) => (None, Some("Tavily returned invalid JSON".into())),
    };
    let _ =
        store.complete_tavily_search(reservation.id, response.metadata.status, error.as_deref());
    attempt.record_results(store, candidate_count, error.as_deref());
}

fn curl_config_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(['\n', '\r'], " ")
}

fn write_tavily_request(child: &mut Child, api_key: &str, body: &str) {
    if let Some(mut stdin) = child.stdin.take() {
        let _ = writeln!(stdin, "url = \"{TAVILY_SEARCH_API}\"");
        let _ = writeln!(stdin, "request = \"POST\"");
        let _ = writeln!(
            stdin,
            "header = \"Authorization: Bearer {}\"",
            curl_config_escape(api_key)
        );
        let _ = writeln!(stdin, "header = \"Content-Type: application/json\"");
        let _ = writeln!(stdin, "data = \"{}\"", curl_config_escape(body));
    }
}

fn run_searxng_discovery(
    store: &mut LearningStore,
    job_id: &str,
    query: &str,
    trigger_reason: &str,
    endpoint: &str,
) -> bool {
    let attempt = SearchAttempt {
        job_id,
        provider: "searxng",
        trigger_reason,
        query,
    };
    let (request_url, host) = match local_searxng_url(endpoint, query) {
        Ok(value) => value,
        Err(error) => {
            attempt.record(store, "error", None, Some(&error.to_string()));
            return false;
        }
    };
    let Some(slot) = reserve_search_http_slot(store, attempt, &host, "searxng_search") else {
        return false;
    };
    let mut child = match spawn_search_curl() {
        Ok(child) => child,
        Err(error) => {
            let _ = store.complete_host_request(slot.id, None, Some("curl could not run"));
            attempt.record(
                store,
                "error",
                None,
                Some(&format!("SearXNG request failed: {error}")),
            );
            return true;
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = writeln!(stdin, "url = \"{request_url}\"");
    }
    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(error) => {
            let _ = store.complete_host_request(slot.id, None, Some("curl request failed"));
            attempt.record(
                store,
                "error",
                None,
                Some(&format!("SearXNG request failed: {error}")),
            );
            return true;
        }
    };
    let response = parse_search_http_response(&output.stdout);
    let succeeded = output.status.success();
    let _ = super::complete_reserved_http_request(
        store,
        slot.id,
        &response.metadata,
        (!succeeded).then_some("SearXNG request failed"),
    );
    if !succeeded {
        attempt.record(store, "error", None, Some("SearXNG request failed"));
        return true;
    }
    let results = match parse_searxng_candidates(response.body.as_bytes()) {
        Ok(results) => results,
        Err(error) => {
            attempt.record(store, "error", None, Some(&error.to_string()));
            return true;
        }
    };
    let persisted = persist_search_candidates(store, job_id, "searxng", query, &results);
    let error = persisted.err().map(|error| error.to_string());
    attempt.record_results(store, Some(results.len()), error.as_deref());
    true
}

fn local_searxng_url(endpoint: &str, query: &str) -> io::Result<(url::Url, String)> {
    let mut url = url::Url::parse(endpoint)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid SearXNG URL"))?;
    let host = url
        .host_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "SearXNG URL has no host"))?
        .to_ascii_lowercase();
    let local_host = host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || !local_host
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "SearXNG must use a local HTTP(S) endpoint without credentials or fragments",
        ));
    }
    if url.path().is_empty() || url.path() == "/" {
        url.set_path("/search");
    }
    url.set_query(None);
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("format", "json");
    Ok((url, host))
}

fn parse_searxng_candidates(body: &[u8]) -> io::Result<Vec<SearchCandidate>> {
    let value: serde_json::Value = serde_json::from_slice(body)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "SearXNG returned invalid JSON"))?;
    let Some(results) = value.get("results").and_then(serde_json::Value::as_array) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "SearXNG response is missing its results list",
        ));
    };
    Ok(results
        .iter()
        .take(20)
        .filter_map(|item| {
            let title = item.get("title")?.as_str()?.trim();
            let mut url = url::Url::parse(item.get("url")?.as_str()?).ok()?;
            if title.is_empty() || url.scheme() != "https" || url.host_str().is_none() {
                return None;
            }
            url.set_fragment(None);
            let category = item
                .get("category")
                .and_then(serde_json::Value::as_str)
                .filter(|category| category.len() <= 80)
                .unwrap_or("search_result");
            let snippet = item
                .get("content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            Some(SearchCandidate {
                result_type: category.chars().take(80).collect(),
                title: title.chars().take(500).collect(),
                url: url.to_string(),
                snippet: snippet.chars().take(2_000).collect(),
                structured_data_json: "{}".into(),
            })
        })
        .collect())
}

fn count_search_domain(domains: &mut std::collections::BTreeMap<String, usize>, url: &url::Url) {
    if let Some(host) = url.host_str() {
        *domains
            .entry(crate::storage::normalize_domain(host))
            .or_default() += 1;
    }
}

/// SerpAPI is a paced discovery channel. Its snippets are retained as leads;
/// only later policy-checked page fetches may support a Scholar finding.
pub(super) fn run_serpapi_discovery(
    store: &mut LearningStore,
    job_id: &str,
    query: &str,
    trigger_reason: &str,
) -> bool {
    let attempt = SearchAttempt {
        job_id,
        provider: "serpapi",
        trigger_reason,
        query,
    };
    let Some(api_key) = ProviderConfig::load().unwrap_or_default().serp_api_key else {
        return record_provider_unavailable(
            store,
            attempt,
            "provider_unavailable: SERP_API_KEY is not configured",
        );
    };
    let Some(http_slot) = reserve_search_http_slot(store, attempt, "serpapi.com", "serpapi_search")
    else {
        return false;
    };
    let interval_seconds = serp_interval_seconds();
    let max_results = std::env::var("SCHOLAR_SERP_MAX_RESULTS")
        .ok()
        .and_then(|value| value.parse::<u8>().ok())
        .filter(|value| (1..=10).contains(value))
        .unwrap_or(5);
    let reservation =
        match store.reserve_serpapi_search(interval_seconds.saturating_mul(1_000), job_id, query) {
            Ok(reservation) => reservation,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                let _ = store.complete_host_request(
                    http_slot.id,
                    None,
                    Some("SerpAPI request was not dispatched"),
                );
                return false;
            }
            Err(error) => {
                let _ = store.complete_host_request(
                    http_slot.id,
                    None,
                    Some("SerpAPI request was not dispatched"),
                );
                attempt.record(
                    store,
                    "error",
                    None,
                    Some(&format!("could not reserve SerpAPI search: {error}")),
                );
                return false;
            }
        };
    let mut request_url = match url::Url::parse(SERPAPI_SEARCH_API) {
        Ok(url) => url,
        Err(error) => {
            let _ =
                store.complete_host_request(http_slot.id, None, Some("invalid SerpAPI endpoint"));
            let _ = store.complete_serpapi_search(
                reservation.id,
                None,
                Some("invalid SerpAPI endpoint"),
            );
            attempt.record(
                store,
                "error",
                None,
                Some(&format!("invalid SerpAPI endpoint: {error}")),
            );
            return true;
        }
    };
    request_url
        .query_pairs_mut()
        .append_pair("engine", "google")
        .append_pair("num", &max_results.to_string())
        .append_pair("q", query)
        .append_pair("api_key", &api_key);
    let mut child = match spawn_search_curl() {
        Ok(child) => child,
        Err(error) => {
            let _ = store.complete_host_request(http_slot.id, None, Some("curl could not run"));
            let _ = store.complete_serpapi_search(reservation.id, None, Some("curl could not run"));
            attempt.record(
                store,
                "error",
                None,
                Some(&format!("curl could not run: {error}")),
            );
            return true;
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        // The credential is kept out of curl's process arguments and persisted records.
        let _ = writeln!(stdin, "url = \"{}\"", request_url);
    }
    let output = child.wait_with_output();
    let output = match output {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            let metadata = response_metadata(&output.stdout);
            let _ = super::complete_reserved_http_request(
                store,
                http_slot.id,
                &metadata,
                Some("SerpAPI request failed"),
            );
            let _ = store.complete_serpapi_search(
                reservation.id,
                metadata.status,
                Some("SerpAPI request failed"),
            );
            attempt.record(store, "error", None, Some("SerpAPI request failed"));
            return true;
        }
        Err(error) => {
            let _ = store.complete_host_request(
                http_slot.id,
                None,
                Some("curl could not run for SerpAPI"),
            );
            let _ = store.complete_serpapi_search(reservation.id, None, Some(&error.to_string()));
            attempt.record(store, "error", None, Some("curl could not run for SerpAPI"));
            return true;
        }
    };
    let http_response = parse_search_http_response(&output.stdout);
    let _ =
        super::complete_reserved_http_request(store, http_slot.id, &http_response.metadata, None);
    let parsed = serde_json::from_slice::<SerpApiResponse>(http_response.body.as_bytes());
    match parsed {
        Ok(response) if response.error.is_none() => {
            let mut domains = std::collections::BTreeMap::<String, usize>::new();
            let mut candidates = Vec::new();
            for item in response.organic_results {
                let Some(link) = item.link else { continue };
                let Ok(url) = url::Url::parse(&link) else {
                    continue;
                };
                if !matches!(url.scheme(), "http" | "https") {
                    continue;
                }
                count_search_domain(&mut domains, &url);
                let Some(title) = item
                    .title
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty())
                else {
                    continue;
                };
                let Some(clean_url) = clean_serpapi_result_url(url) else {
                    continue;
                };
                candidates.push(SearchCandidate {
                    result_type: "organic_result".into(),
                    title,
                    url: clean_url,
                    snippet: item.snippet.unwrap_or_default(),
                    structured_data_json: "{}".into(),
                });
            }
            for (result_type, results) in response.structured_results {
                if is_serpapi_metadata_field(&result_type) {
                    continue;
                }
                for candidate in linked_serpapi_results(&result_type, &results) {
                    if candidates.len() >= MAX_SERPAPI_CANDIDATES {
                        break;
                    }
                    let Ok(url) = url::Url::parse(&candidate.url) else {
                        continue;
                    };
                    count_search_domain(&mut domains, &url);
                    candidates.push(candidate);
                }
            }
            let domain_rows = domains.into_iter().collect::<Vec<_>>();
            let domain_result = store.record_serpapi_search_domains(reservation.id, &domain_rows);
            let result = persist_search_candidates(store, job_id, "serpapi", query, &candidates);
            let result = domain_result.and(result);
            let _ = store.complete_serpapi_search(
                reservation.id,
                http_response.metadata.status,
                result
                    .as_ref()
                    .err()
                    .map(std::string::ToString::to_string)
                    .as_deref(),
            );
            attempt.record_results(
                store,
                Some(candidates.len()),
                result
                    .as_ref()
                    .err()
                    .map(std::string::ToString::to_string)
                    .as_deref(),
            );
        }
        Ok(_response) => {
            let _ = store.complete_serpapi_search(
                reservation.id,
                http_response.metadata.status,
                Some("SerpAPI returned an API error"),
            );
            attempt.record(store, "error", None, Some("SerpAPI returned an API error"));
        }
        Err(error) => {
            let _ = store.complete_serpapi_search(
                reservation.id,
                http_response.metadata.status,
                Some(&error.to_string()),
            );
            attempt.record(store, "error", None, Some("SerpAPI returned invalid JSON"));
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{
        SearchAttempt, linked_serpapi_results, local_searxng_url, parse_search_http_response,
        parse_searxng_candidates, parse_tavily_candidates, record_provider_unavailable,
        response_metadata, should_retry_tavily_advanced,
    };

    #[test]
    fn nested_serpapi_results_keep_valid_candidates_and_clean_their_urls() {
        let candidates = linked_serpapi_results(
            "recipes_results",
            &serde_json::json!({
                "items": [
                    {
                        "title": " Soup ",
                        "link": "https://food.example/soup?token=private#recipe",
                        "ingredients": ["chicken", "carrots"]
                    },
                    {"title": " ", "link": "https://food.example/empty"}
                ]
            }),
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].result_type, "recipes_results");
        assert_eq!(candidates[0].title, "Soup");
        assert_eq!(candidates[0].url, "https://food.example/soup");
        assert_eq!(candidates[0].snippet, "chicken; carrots");
    }

    #[test]
    fn missing_provider_credentials_are_recorded_without_stopping_the_queue() {
        let path = crate::test_support::unique_temp_path("community-provider-unavailable", "db");
        let mut store = crate::storage::LearningStore::open(&path).unwrap();
        store
            .enqueue_research("provider-unavailable", 1, "What are Mars's moons?")
            .unwrap();
        let job = store.claim_scholar_job().unwrap().unwrap();
        assert!(!record_provider_unavailable(
            &mut store,
            SearchAttempt {
                job_id: &job.id,
                provider: "tavily",
                trigger_reason: "wikipedia_insufficient",
                query: &job.question,
            },
            "provider_unavailable: Tavily credentials are not configured",
        ));
        let attempts = store.scholar_search_attempts(&job.id).unwrap();
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].provider, "tavily");
        assert_eq!(attempts[0].status, "error");
        assert!(
            attempts[0]
                .error
                .as_deref()
                .unwrap()
                .contains("provider_unavailable")
        );
        drop(store);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn search_attempt_maps_result_counts_and_errors_to_ledger_outcomes() {
        let mut store = crate::test_support::in_memory_learning_store();
        store
            .enqueue_research("search-outcomes", 1, "What are Mars's moons?")
            .unwrap();
        let job = store.claim_scholar_job().unwrap().unwrap();
        let attempt = SearchAttempt {
            job_id: &job.id,
            provider: "fixture",
            trigger_reason: "test",
            query: &job.question,
        };

        attempt.record_results(&mut store, Some(0), None);
        attempt.record_results(&mut store, Some(2), None);
        attempt.record_results(&mut store, Some(3), Some("candidate persistence failed"));

        let attempts = store.scholar_search_attempts(&job.id).unwrap();
        assert_eq!(
            attempts
                .iter()
                .map(|attempt| (attempt.status.as_str(), attempt.result_count))
                .collect::<Vec<_>>(),
            [
                ("no_result", Some(0)),
                ("results", Some(2)),
                ("error", Some(3))
            ]
        );
        assert_eq!(
            attempts[2].error.as_deref(),
            Some("candidate persistence failed")
        );
    }

    #[test]
    fn searxng_accepts_local_endpoints_and_encodes_json_search_parameters() {
        let (url, host) = local_searxng_url("http://searxng.local:8080", "chicken soup").unwrap();
        assert_eq!(host, "searxng.local");
        assert_eq!(url.path(), "/search");
        let query = url.query_pairs().collect::<Vec<_>>();
        assert!(query.contains(&("q".into(), "chicken soup".into())));
        assert!(query.contains(&("format".into(), "json".into())));
        assert!(local_searxng_url("https://localhost:8888/search", "topic").is_ok());
    }

    #[test]
    fn searxng_rejects_remote_or_credential_bearing_endpoints() {
        for endpoint in [
            "https://search.example/search",
            "http://user:pass@localhost:8080/search",
            "file:///tmp/search",
        ] {
            assert!(local_searxng_url(endpoint, "topic").is_err());
        }
    }

    #[test]
    fn searxng_parses_bounded_structured_results_as_leads() {
        let results = parse_searxng_candidates(
            br#"{"results":[{"title":"Soup","url":"https://food.example/soup","content":"A snippet","category":"recipes"},{"title":"Unsafe","url":"http://food.example/"},{"title":"Bad","url":"javascript:alert(1)"}]}"#,
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].result_type, "recipes");
        assert_eq!(results[0].snippet, "A snippet");
        assert!(parse_searxng_candidates(br#"{"oops":[]}"#).is_err());
    }

    #[test]
    fn tavily_results_use_the_shared_search_candidate_shape() {
        let candidates = parse_tavily_candidates(
            br#"{"results":[{"title":" Soup ","url":"https://food.example/soup#recipe","content":"A snippet"}]}"#,
        )
        .unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].result_type, "search_result");
        assert_eq!(candidates[0].title, "Soup");
        assert_eq!(candidates[0].url, "https://food.example/soup");
        assert_eq!(candidates[0].snippet, "A snippet");
        assert_eq!(candidates[0].structured_data_json, "{}");
        assert!(parse_tavily_candidates(b"not JSON").is_err());
    }

    #[test]
    fn curl_response_fixture_preserves_retry_after_and_redirect_status() {
        let response = parse_search_http_response(
            b"response body\nCOMMUNITY_HTTP_STATUS:429\tapplication/json\t120",
        );
        assert_eq!(response.metadata.status, Some(429));
        assert_eq!(response.metadata.retry_after_ms, Some(120_000));
        assert_eq!(response.body, "response body");

        let metadata = response_metadata(b"\nCOMMUNITY_HTTP_STATUS:302\ttext/html\t");
        assert_eq!(metadata.status, Some(302));
        assert_eq!(metadata.retry_after_ms, None);

        let response = parse_search_http_response(
            b"response body\nCOMMUNITY_HTTP_STATUS:200\tapplication/json\t",
        );
        assert_eq!(response.metadata.status, Some(200));
        assert_eq!(response.body, "response body");

        let response = parse_search_http_response(b"body without metadata");
        assert_eq!(response.metadata.status, None);
        assert_eq!(response.metadata.retry_after_ms, None);
        assert_eq!(response.body, "body without metadata");
    }

    #[test]
    fn tavily_uses_advanced_only_after_a_successful_empty_basic_response() {
        assert!(should_retry_tavily_advanced(Some(200), 0, false));
        assert!(!should_retry_tavily_advanced(Some(200), 1, false));
        assert!(!should_retry_tavily_advanced(Some(429), 0, false));
        assert!(!should_retry_tavily_advanced(Some(200), 0, true));
    }
}
