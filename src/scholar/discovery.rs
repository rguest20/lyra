use super::*;
use std::io::Write;
use std::process::{Child, Stdio};

pub(super) use crate::storage::ScholarSearchCandidateInput as SearchCandidate;

pub(super) fn discover_searxng(
    store: &mut LearningStore,
    job_id: &str,
    query: &str,
    reason: &str,
    endpoint: &str,
) -> bool {
    run_searxng_discovery(store, job_id, query, reason, endpoint)
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

pub(super) fn searxng_corroboration_interval_seconds() -> i64 {
    std::env::var("SCHOLAR_SEARXNG_CORROBORATION_INTERVAL_SECONDS")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(1_800)
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

#[cfg(test)]
mod tests {
    use super::{
        SearchAttempt, local_searxng_url, parse_search_http_response, parse_searxng_candidates,
    };

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
    fn curl_response_fixture_preserves_retry_after_and_redirect_status() {
        let response = parse_search_http_response(
            b"response body\nCOMMUNITY_HTTP_STATUS:429\tapplication/json\t120",
        );
        assert_eq!(response.metadata.status, Some(429));
        assert_eq!(response.metadata.retry_after_ms, Some(120_000));
        assert_eq!(response.body, "response body");

        let response = parse_search_http_response(
            b"response body\nCOMMUNITY_HTTP_STATUS:200\tapplication/json\t",
        );
        assert_eq!(response.metadata.status, Some(200));
        assert_eq!(response.body, "response body");

        let response = parse_search_http_response(b"body without metadata");
        assert_eq!(response.metadata.status, None);
        assert_eq!(response.body, "body without metadata");
    }

    #[test]
    fn search_attempt_records_provider_and_outcome() {
        let mut store = crate::test_support::in_memory_learning_store();
        store
            .enqueue_research("searxng-attempt", 1, "What are Mars's moons?")
            .unwrap();
        let job = store.claim_scholar_job().unwrap().unwrap();
        let attempt = SearchAttempt {
            job_id: &job.id,
            provider: "searxng",
            trigger_reason: "wikipedia_insufficient",
            query: &job.question,
        };
        attempt.record_results(&mut store, Some(0), None);
        let attempts = store.scholar_search_attempts(&job.id).unwrap();
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].provider, "searxng");
        assert_eq!(attempts[0].status, "no_result");
        assert_eq!(attempts[0].result_count, Some(0));
    }
}
