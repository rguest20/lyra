//! Conservative policy checks and bounded, policy-aware source-page retrieval.

use std::io;
use std::net::{IpAddr, ToSocketAddrs};
use std::process::{Command, Stdio};

use crate::storage::{LearningStore, ScholarSearchCandidate};

use super::{html_to_text, passages::coherent_passages, provenance::extract_page_provenance};

const USER_AGENT: &str = "CommunityLabScholar/0.1 (local Rust chatbot; educational use)";
const PRODUCT_TOKEN: &str = "CommunityLabScholar";
const MAX_POLICY_BYTES: usize = 512 * 1024;
const MAX_PAGE_BYTES: usize = 1_000_000;
const STATUS_MARKER: &str = "\nCOMMUNITY_SITE_STATUS:";
const POLICY_CACHE_TTL_MS: i64 = 6 * 60 * 60 * 1_000;
type RobotsRule = (bool, String);
type RobotsGroup = (Vec<String>, Vec<RobotsRule>);

pub(super) struct CandidateReview {
    pub status: &'static str,
    pub policy_status: &'static str,
    pub policy_reason: String,
    pub fetch_status: &'static str,
    pub fetched_url: Option<String>,
    pub excerpt: Option<String>,
    pub passages: Vec<String>,
    pub provenance_json: Option<String>,
}

struct FetchTarget {
    host: String,
    ip: IpAddr,
}

struct HttpResponse {
    status: u16,
    content_type: String,
    body: String,
}

struct SiteManifest {
    disallow_agents: Vec<String>,
    requests_per_minute: Option<u32>,
    require_auth: bool,
    llms_txt: Option<String>,
}

impl CandidateReview {
    fn skipped(policy_status: &'static str, reason: impl Into<String>) -> Self {
        Self {
            status: "skipped",
            policy_status,
            policy_reason: reason.into(),
            fetch_status: "skipped",
            fetched_url: None,
            excerpt: None,
            passages: Vec::new(),
            provenance_json: None,
        }
    }

    fn failed(reason: impl Into<String>) -> Self {
        Self {
            status: "failed",
            policy_status: "unavailable",
            policy_reason: reason.into(),
            fetch_status: "failed",
            fetched_url: None,
            excerpt: None,
            passages: Vec::new(),
            provenance_json: None,
        }
    }

    fn fetch_failed(reason: impl Into<String>) -> Self {
        let mut review = Self {
            status: "failed",
            policy_status: "allowed",
            policy_reason: "site policy checks allowed the candidate fetch".into(),
            fetch_status: "failed",
            fetched_url: None,
            excerpt: None,
            passages: Vec::new(),
            provenance_json: None,
        };
        review.policy_reason = format!("{}; {}", review.policy_reason, reason.into());
        review
    }
}

pub(super) fn inspect_and_fetch(
    store: &mut LearningStore,
    candidate: &ScholarSearchCandidate,
) -> CandidateReview {
    match inspect_and_fetch_inner(store, candidate) {
        Ok(review) => review,
        Err(error) => CandidateReview::failed(error.to_string()),
    }
}

enum PolicyOutcome<T> {
    Proceed(T),
    Stop(CandidateReview),
}

fn stop<T>(review: CandidateReview) -> io::Result<PolicyOutcome<T>> {
    Ok(PolicyOutcome::Stop(review))
}

fn inspect_and_fetch_inner(
    store: &mut LearningStore,
    candidate: &ScholarSearchCandidate,
) -> io::Result<CandidateReview> {
    let candidate_url =
        url::Url::parse(&candidate.url).map_err(|_| invalid("candidate URL is malformed"))?;
    let candidate_host = candidate_url
        .host_str()
        .ok_or_else(|| invalid("candidate URL has no host"))?
        .to_ascii_lowercase();
    let candidate_path = url_path_query(&candidate_url);
    if store.robots_blocked(&candidate_host, &candidate_path)? {
        return Ok(CandidateReview::skipped(
            "disallowed",
            "recent robots.txt denial is cached for this source",
        ));
    }
    let target = prepare_target(&candidate_url)?;
    let origin = candidate_url.origin().ascii_serialization();
    let rules = match check_robots(store, &candidate_url, &candidate_path, &target, &origin)? {
        PolicyOutcome::Proceed(rules) => rules,
        PolicyOutcome::Stop(review) => return Ok(review),
    };
    let (page_url, interval_ms) = match inspect_site_guidance(
        store,
        candidate,
        &candidate_url,
        &candidate_path,
        &target,
        &origin,
        &rules,
    )? {
        PolicyOutcome::Proceed(selection) => selection,
        PolicyOutcome::Stop(review) => return Ok(review),
    };
    fetch_source_page(store, candidate, &target, page_url, interval_ms)
}

fn check_robots(
    store: &mut LearningStore,
    candidate_url: &url::Url,
    candidate_path: &str,
    target: &FetchTarget,
    origin: &str,
) -> io::Result<PolicyOutcome<Vec<RobotsRule>>> {
    let robots_url = url::Url::parse(&format!("{origin}/robots.txt"))
        .map_err(|_| invalid("could not construct robots.txt URL"))?;
    let robots_response =
        match fetch_site_policy(store, target, &robots_url, "external_robots_txt", 10_000) {
            Ok(response) => response,
            Err(error) => {
                return stop(CandidateReview::fetch_failed(format!(
                    "robots.txt unavailable: {error}"
                )));
            }
        };
    let robots_text = match robots_response.status {
        200 => Some(robots_response.body),
        404 | 410 => None,
        401 | 403 => {
            store.remember_robots_block(
                &target.host,
                &candidate_path,
                candidate_url.path(),
                "robots.txt is access denied",
                true,
            )?;
            return stop(CandidateReview::skipped(
                "disallowed",
                format!("robots.txt returned HTTP {}", robots_response.status),
            ));
        }
        status => {
            return stop(CandidateReview::fetch_failed(format!(
                "robots.txt returned HTTP {status}"
            )));
        }
    };
    let rules = match robots_text.as_deref().map(parse_robots).transpose() {
        Ok(Some(rules)) => rules,
        Ok(None) => Vec::new(),
        Err(reason) => return stop(CandidateReview::skipped("policy_ambiguous", reason)),
    };
    if !robots_allowed(&rules, &candidate_path) {
        store.remember_robots_block(
            &target.host,
            &candidate_path,
            candidate_url.path(),
            "robots.txt disallows this source path",
            false,
        )?;
        return stop(CandidateReview::skipped(
            "disallowed",
            "robots.txt disallows the candidate path for CommunityLabScholar",
        ));
    }

    Ok(PolicyOutcome::Proceed(rules))
}

fn inspect_site_guidance(
    store: &mut LearningStore,
    candidate: &ScholarSearchCandidate,
    candidate_url: &url::Url,
    candidate_path: &str,
    target: &FetchTarget,
    origin: &str,
    rules: &[RobotsRule],
) -> io::Result<PolicyOutcome<(url::Url, i64)>> {
    let mut interval_ms = 10_000_i64;
    let mut llms_index = None;
    let default_llms = origin_url(&origin, "/llms.txt")?;
    if robots_allowed(&rules, default_llms.path()) {
        match fetch_site_policy(
            store,
            target,
            &default_llms,
            "external_llms_txt",
            interval_ms,
        ) {
            Ok(response) if response.status == 200 => {
                llms_index = Some((default_llms.clone(), response.body))
            }
            Ok(response) if matches!(response.status, 404 | 410) => {}
            Ok(response) if matches!(response.status, 401 | 403) => {}
            Ok(response) => {
                return stop(CandidateReview::failed(format!(
                    "llms.txt returned HTTP {}",
                    response.status
                )));
            }
            Err(error) => {
                return stop(CandidateReview::failed(format!(
                    "llms.txt unavailable: {error}"
                )));
            }
        }
    }

    let manifest_url = origin_url(&origin, "/agent-manifest.txt")?;
    let mut manifest = SiteManifest {
        disallow_agents: Vec::new(),
        requests_per_minute: None,
        require_auth: false,
        llms_txt: None,
    };
    if robots_allowed(&rules, manifest_url.path()) {
        match fetch_url(
            store,
            target,
            &manifest_url,
            "external_manifest_txt",
            interval_ms,
            MAX_POLICY_BYTES,
        ) {
            Ok(response) if matches!(response.status, 404 | 410) => {}
            Ok(response) if response.status == 200 => {
                manifest = match parse_manifest(&response.body) {
                    Ok(manifest) => manifest,
                    Err(reason) => {
                        return stop(CandidateReview::skipped("policy_ambiguous", reason));
                    }
                };
            }
            Ok(response) => {
                return stop(CandidateReview::skipped(
                    "policy_ambiguous",
                    format!(
                        "agent-manifest.txt could not be assessed (HTTP {})",
                        response.status
                    ),
                ));
            }
            Err(error) => {
                return stop(CandidateReview::skipped(
                    "policy_ambiguous",
                    format!("agent-manifest.txt could not be assessed: {error}"),
                ));
            }
        }
    }
    if let Some(reason) = manifest_access_denial(&manifest) {
        return stop(CandidateReview::skipped("disallowed", reason));
    }
    interval_ms = manifest_request_interval(interval_ms, manifest.requests_per_minute);

    if let Some(path) = manifest.llms_txt.as_deref() {
        let custom_llms = origin_url(&origin, path)?;
        if custom_llms != default_llms && robots_allowed(&rules, custom_llms.path()) {
            match fetch_site_policy(
                store,
                target,
                &custom_llms,
                "external_llms_txt",
                interval_ms,
            ) {
                Ok(response) if response.status == 200 => {
                    llms_index = Some((custom_llms, response.body));
                }
                Ok(response) if matches!(response.status, 404 | 410) => {}
                Ok(response) => {
                    return stop(CandidateReview::failed(format!(
                        "declared llms.txt returned HTTP {}",
                        response.status
                    )));
                }
                Err(error) => {
                    return stop(CandidateReview::failed(format!(
                        "declared llms.txt unavailable: {error}"
                    )));
                }
            }
        }
    }

    let page_url = select_page_url(
        candidate_url,
        &candidate.query,
        &candidate.title,
        llms_index,
        rules,
    );
    if !robots_allowed(&rules, &url_path_query(&page_url)) {
        store.remember_robots_block(
            &target.host,
            &candidate_path,
            candidate_url.path(),
            "robots.txt disallows the selected source path",
            false,
        )?;
        return stop(CandidateReview::skipped(
            "disallowed",
            "robots.txt disallows the selected source path",
        ));
    }
    Ok(PolicyOutcome::Proceed((page_url, interval_ms)))
}

fn manifest_access_denial(manifest: &SiteManifest) -> Option<&'static str> {
    if manifest
        .disallow_agents
        .iter()
        .any(|agent| agent == "*" || agent.eq_ignore_ascii_case(PRODUCT_TOKEN))
    {
        Some("agent-manifest.txt explicitly disallows CommunityLabScholar")
    } else if manifest.require_auth {
        Some("agent-manifest.txt marks the site endpoints as requiring authentication")
    } else {
        None
    }
}

fn manifest_request_interval(interval_ms: i64, requests_per_minute: Option<u32>) -> i64 {
    requests_per_minute.map_or(interval_ms, |requests_per_minute| {
        interval_ms.max((60_000_i64 / i64::from(requests_per_minute)).max(1))
    })
}

fn select_page_url(
    candidate_url: &url::Url,
    query: &str,
    title: &str,
    llms_index: Option<(url::Url, String)>,
    rules: &[RobotsRule],
) -> url::Url {
    if let Some((index_url, index_text)) = llms_index
        && let Some(markdown_url) = select_llms_markdown(&index_text, &index_url, query, title)
        && robots_allowed(rules, &url_path_query(&markdown_url))
    {
        return markdown_url;
    }
    candidate_url.clone()
}

fn fetch_source_page(
    store: &mut LearningStore,
    candidate: &ScholarSearchCandidate,
    target: &FetchTarget,
    page_url: url::Url,
    interval_ms: i64,
) -> io::Result<CandidateReview> {
    let page = fetch_url(
        store,
        target,
        &page_url,
        "external_page_content",
        interval_ms,
        MAX_PAGE_BYTES,
    );
    let page = match page {
        Ok(page) if page.status == 200 => page,
        Ok(page) => {
            return Ok(CandidateReview::failed(format!(
                "source page returned HTTP {}",
                page.status
            )));
        }
        Err(error) => {
            return Ok(CandidateReview::failed(format!(
                "source page fetch failed: {error}"
            )));
        }
    };
    let provenance_json = if page
        .content_type
        .to_ascii_lowercase()
        .starts_with("text/html")
        || page
            .content_type
            .eq_ignore_ascii_case("application/xhtml+xml")
    {
        extract_page_provenance(&page.body, &page_url)
    } else {
        None
    };
    let text = match page.content_type.to_ascii_lowercase().as_str() {
        content_type if content_type.starts_with("text/html") => html_to_text(&page.body)?,
        "application/xhtml+xml" => html_to_text(&page.body)?,
        content_type if content_type.starts_with("text/") || content_type.is_empty() => page.body,
        _ => {
            return Ok(CandidateReview::fetch_failed(
                "source page is not HTML or text",
            ));
        }
    };
    store.retain_fetched_candidate_source(
        candidate.id,
        &candidate.title,
        page_url.as_str(),
        &text,
    )?;
    let query_words = crate::storage::relevant_words(&candidate.query);
    let page_words = crate::storage::relevant_words(&text);
    if crate::storage::lexical_overlap(&query_words, &page_words) == 0 {
        return Ok(CandidateReview::fetch_failed(
            "source page had no usable text passage",
        ));
    }
    let passages = coherent_passages(&candidate.query, &text);
    if passages.is_empty() {
        return Ok(CandidateReview::fetch_failed(
            "source page had no usable text passage",
        ));
    }
    let excerpt = passages.join("\n\n");
    Ok(CandidateReview {
        status: "fetched",
        policy_status: "allowed",
        policy_reason: "robots.txt and available site manifest checks allowed this fetch; no MCP endpoint was invoked".into(),
        fetch_status: "fetched",
        fetched_url: Some(page_url.to_string()),
        excerpt: Some(excerpt),
        passages,
        provenance_json,
    })
}

fn fetch_url(
    store: &mut LearningStore,
    target: &FetchTarget,
    url: &url::Url,
    request_kind: &str,
    interval_ms: i64,
    max_bytes: usize,
) -> io::Result<HttpResponse> {
    if request_kind == "external_page_content" {
        if let Some((content_type, body)) = store.cached_source_page(url.as_str())? {
            if body.len() <= max_bytes {
                crate::diagnostics::event(
                    "internet.cache_hit",
                    serde_json::json!({"provider":"web","request_kind":request_kind}),
                );
                return Ok(HttpResponse {
                    status: 200,
                    content_type,
                    body,
                });
            }
        }
    }
    let reservation = store.reserve_host_request_wait(
        &target.host,
        request_kind,
        interval_ms.max(10_000),
        std::time::Duration::from_secs(30),
    )?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| io::Error::other(error.to_string()))?
        .as_millis();
    if reservation.scheduled_at_ms > now_ms {
        std::thread::sleep(std::time::Duration::from_millis(
            u64::try_from(reservation.scheduled_at_ms - now_ms).unwrap_or(u64::MAX),
        ));
    }
    let port = url.port_or_known_default().unwrap_or(443);
    let pinned_ip = match target.ip {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    };
    let output = Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--fail",
            "--max-time",
            "15",
            "--max-filesize",
        ])
        .arg(max_bytes.to_string())
        .args(["--proto", "=https", "--user-agent", USER_AGENT, "--resolve"])
        .arg(format!("{}:{port}:{pinned_ip}", target.host))
        .arg("--write-out")
        .arg(format!(
            "{STATUS_MARKER}%{{http_code}}\t%{{content_type}}\t%header{{retry-after}}"
        ))
        .arg(url.as_str())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            store.complete_host_request(reservation.id, None, Some("curl could not run"))?;
            return Err(error);
        }
    };
    let combined = String::from_utf8_lossy(&output.stdout);
    let Some((body, metadata)) = combined.rsplit_once(STATUS_MARKER) else {
        store.complete_host_request(
            reservation.id,
            None,
            Some("source response omitted HTTP metadata"),
        )?;
        return Err(io::Error::other("source response omitted HTTP metadata"));
    };
    let mut metadata_parts = metadata.trim_end_matches(['\r', '\n']).split('\t');
    let (Some(status), Some(content_type)) = (metadata_parts.next(), metadata_parts.next()) else {
        store.complete_host_request(
            reservation.id,
            None,
            Some("source response omitted HTTP status"),
        )?;
        return Err(io::Error::other("source response omitted HTTP status"));
    };
    let status = match status.parse::<u16>() {
        Ok(status) => status,
        Err(_) => {
            store.complete_host_request(
                reservation.id,
                None,
                Some("source returned invalid HTTP status"),
            )?;
            return Err(io::Error::other("source returned an invalid HTTP status"));
        }
    };
    let retry_after = metadata_parts.next().and_then(|value| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_millis();
        super::retry_after_delay_ms(value, i64::try_from(now).ok()?)
    });
    let request_error = (!output.status.success()).then_some("curl reported a source fetch error");
    store.complete_host_request_with_retry_after(
        reservation.id,
        Some(status),
        retry_after,
        request_error,
    )?;
    if !output.status.success() && status == 200 {
        return Err(io::Error::other(
            "source transfer failed or exceeded the response-size limit",
        ));
    }
    if body.len() > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source response exceeded the size limit",
        ));
    }
    if status == 200 && output.status.success() && request_kind == "external_page_content" {
        store.cache_source_page(url.as_str(), content_type, body)?;
    }
    Ok(HttpResponse {
        status,
        content_type: content_type.to_owned(),
        body: body.to_owned(),
    })
}

fn fetch_site_policy(
    store: &mut LearningStore,
    target: &FetchTarget,
    url: &url::Url,
    request_kind: &str,
    interval_ms: i64,
) -> io::Result<HttpResponse> {
    let origin = url.origin().ascii_serialization();
    if let Some((status, content_type, body)) = store.cached_site_policy(&origin, request_kind)? {
        return Ok(HttpResponse {
            status,
            content_type,
            body,
        });
    }
    let response = fetch_url(
        store,
        target,
        url,
        request_kind,
        interval_ms,
        MAX_POLICY_BYTES,
    )?;
    if matches!(response.status, 200 | 401 | 403 | 404 | 410) {
        store.cache_site_policy(
            &origin,
            request_kind,
            response.status,
            &response.content_type,
            &response.body,
            POLICY_CACHE_TTL_MS,
        )?;
    }
    Ok(response)
}

fn prepare_target(url: &url::Url) -> io::Result<FetchTarget> {
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Err(invalid(
            "only credential-free HTTPS source URLs are eligible",
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| invalid("candidate URL has no host"))?
        .to_ascii_lowercase();
    if host.parse::<IpAddr>().is_ok()
        || host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
    {
        return Err(invalid("local or literal-IP source hosts are not eligible"));
    }
    let port = url.port_or_known_default().unwrap_or(443);
    if port != 443 {
        return Err(invalid("source URLs on nonstandard ports are not eligible"));
    }
    let addresses = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|_| invalid("could not resolve source host"))?;
    let ip = addresses
        .map(|address| address.ip())
        .find(|ip| is_public_ip(*ip))
        .ok_or_else(|| invalid("source host did not resolve to a public address"))?;
    Ok(FetchTarget { host, ip })
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_broadcast()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && a != 0
                && a < 224
                && !(a == 100 && (64..=127).contains(&b))
                && !(a == 192 && b == 0 && c == 0)
                && !(a == 192 && b == 0 && c == 2)
                && !(a == 198 && (b == 18 || b == 19))
                && !(a == 198 && b == 51 && c == 100)
                && !(a == 203 && b == 0 && c == 113)
        }
        IpAddr::V6(ip) => {
            let segments = ip.segments();
            !ip.is_loopback()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && !ip.is_unique_local()
                && !ip.is_unicast_link_local()
                && (segments[0] & 0xe000) == 0x2000
                && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
        }
    }
}

fn origin_url(origin: &str, path: &str) -> io::Result<url::Url> {
    let base = url::Url::parse(origin).map_err(|_| invalid("site origin is malformed"))?;
    let url = base
        .join(path)
        .map_err(|_| invalid("site policy path is malformed"))?;
    if url.origin() != base.origin() {
        return Err(invalid(
            "site policy endpoint must remain on the source host",
        ));
    }
    Ok(url)
}

fn url_path_query(url: &url::Url) -> String {
    match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_owned(),
    }
}

fn parse_robots(text: &str) -> Result<Vec<RobotsRule>, String> {
    let mut groups: Vec<RobotsGroup> = Vec::new();
    for raw_line in text.lines() {
        let line = raw_line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err("robots.txt contains an unparseable directive".into());
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        match name.as_str() {
            "user-agent" => {
                if value.is_empty() {
                    return Err("robots.txt has an empty user-agent".into());
                }
                if groups.last().is_none_or(|(_, rules)| !rules.is_empty()) {
                    groups.push((Vec::new(), Vec::new()));
                }
                if let Some((agents, _)) = groups.last_mut() {
                    agents.push(value.to_ascii_lowercase());
                }
            }
            "allow" | "disallow" => {
                if value.is_empty() {
                    continue;
                }
                if !value.starts_with('/') {
                    return Err("robots.txt has an invalid allow/disallow path".into());
                }
                let Some((_, rules)) = groups.last_mut() else {
                    return Err("robots.txt has a rule before its user-agent group".into());
                };
                rules.push((name == "allow", value.to_owned()));
            }
            _ => {} // RFC 9309 permits other records such as Sitemap.
        }
    }
    let product = PRODUCT_TOKEN.to_ascii_lowercase();
    let specific_matches = groups
        .iter()
        .filter(|(agents, _)| {
            agents
                .iter()
                .any(|agent| agent != "*" && product.contains(agent))
        })
        .collect::<Vec<_>>();
    let selected = if specific_matches.is_empty() {
        groups
            .iter()
            .filter(|(agents, _)| agents.iter().any(|agent| agent == "*"))
            .collect::<Vec<_>>()
    } else {
        specific_matches
    };
    Ok(selected
        .into_iter()
        .flat_map(|(_, rules)| rules.iter().cloned())
        .collect())
}

fn robots_allowed(rules: &[RobotsRule], path: &str) -> bool {
    let matching = rules
        .iter()
        .filter_map(|(allow, pattern)| {
            glob_match(pattern, path).then_some((
                pattern.bytes().filter(|b| *b != b'*' && *b != b'$').count(),
                *allow,
            ))
        })
        .max_by(|(left_len, left_allow), (right_len, right_allow)| {
            left_len
                .cmp(right_len)
                .then_with(|| left_allow.cmp(right_allow))
        });
    matching.is_none_or(|(_, allow)| allow)
}

fn glob_match(pattern: &str, path: &str) -> bool {
    let anchored = pattern.ends_with('$');
    let pattern = pattern.strip_suffix('$').unwrap_or(pattern);
    let parts = pattern.split('*').collect::<Vec<_>>();
    let Some(first) = parts.first() else {
        return true;
    };
    if !path.starts_with(first) {
        return false;
    }
    let mut offset = first.len();
    for (index, part) in parts.iter().enumerate().skip(1) {
        if part.is_empty() {
            continue;
        }
        let Some(found) = path[offset..].find(part) else {
            return false;
        };
        offset += found + part.len();
        if anchored && index == parts.len() - 1 && offset != path.len() {
            return false;
        }
    }
    !anchored || pattern.ends_with('*') || offset == path.len()
}

fn parse_manifest(text: &str) -> Result<SiteManifest, String> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        return parse_manifest_json(&value);
    }
    parse_manifest_yaml_subset(text)
}

fn parse_manifest_json(value: &serde_json::Value) -> Result<SiteManifest, String> {
    let object = value.as_object().ok_or("manifest root must be an object")?;
    reject_unknown_keys(
        object.keys().map(String::as_str),
        &["version", "governance_policies", "endpoints"],
    )?;
    if object
        .get("version")
        .and_then(serde_json::Value::as_str)
        .is_none_or(|version| version.trim().is_empty())
    {
        return Err("manifest requires a non-empty string version".into());
    }
    let mut manifest = SiteManifest {
        disallow_agents: Vec::new(),
        requests_per_minute: None,
        require_auth: false,
        llms_txt: None,
    };
    if let Some(policies) = object.get("governance_policies") {
        let policies = policies
            .as_object()
            .ok_or("governance_policies must be an object")?;
        reject_unknown_keys(
            policies.keys().map(String::as_str),
            &["disallow_agents", "rate_limit_req_per_min", "require_auth"],
        )?;
        if let Some(agents) = policies.get("disallow_agents") {
            manifest.disallow_agents = agents
                .as_array()
                .ok_or("disallow_agents must be an array")?
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(str::to_owned)
                        .ok_or("disallow_agents entries must be strings")
                })
                .collect::<Result<Vec<_>, _>>()?;
        }
        if let Some(limit) = policies.get("rate_limit_req_per_min") {
            manifest.requests_per_minute = Some(
                limit
                    .as_u64()
                    .and_then(|v| u32::try_from(v).ok())
                    .filter(|v| *v > 0)
                    .ok_or("rate_limit_req_per_min must be a positive integer")?,
            );
        }
        if let Some(require_auth) = policies.get("require_auth") {
            manifest.require_auth = require_auth
                .as_bool()
                .ok_or("require_auth must be boolean")?;
        }
    }
    if let Some(endpoints) = object.get("endpoints") {
        let endpoints = endpoints.as_object().ok_or("endpoints must be an object")?;
        reject_unknown_keys(
            endpoints.keys().map(String::as_str),
            &["llms_txt", "mcp_server"],
        )?;
        if let Some(path) = endpoints.get("llms_txt") {
            manifest.llms_txt = Some(validate_manifest_path(
                path.as_str().ok_or("llms_txt must be a string")?,
            )?);
        }
        if endpoints
            .get("mcp_server")
            .is_some_and(|value| value.as_str().is_none_or(str::is_empty))
        {
            return Err("mcp_server must be a non-empty string".into());
        }
    }
    Ok(manifest)
}

fn parse_manifest_yaml_subset(text: &str) -> Result<SiteManifest, String> {
    let mut manifest = SiteManifest {
        disallow_agents: Vec::new(),
        requests_per_minute: None,
        require_auth: false,
        llms_txt: None,
    };
    let mut section = "";
    let mut list_property = false;
    let mut has_version = false;
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or_default().trim_end();
        if line.trim().is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let content = line.trim();
        if indent == 4 && list_property && content.starts_with('-') {
            manifest.disallow_agents.push(unquote(content[1..].trim())?);
            continue;
        }
        let (key, value) = content
            .split_once(':')
            .ok_or("manifest YAML has an unparseable line")?;
        let key = key.trim();
        let value = value.trim();
        match (indent, section, key) {
            (0, _, "version") => {
                if unquote(value)?.trim().is_empty() {
                    return Err("manifest version cannot be empty".into());
                }
                has_version = true;
                section = "";
                list_property = false;
            }
            (0, _, "governance_policies") if value.is_empty() => {
                section = "governance_policies";
                list_property = false;
            }
            (0, _, "endpoints") if value.is_empty() => {
                section = "endpoints";
                list_property = false;
            }
            (2, "governance_policies", "disallow_agents") => {
                list_property = true;
                if !value.is_empty() {
                    manifest.disallow_agents = parse_yaml_string_list(value)?;
                    list_property = false;
                }
            }
            (2, "governance_policies", "rate_limit_req_per_min") => {
                let limit = value
                    .parse::<u32>()
                    .ok()
                    .filter(|v| *v > 0)
                    .ok_or("rate_limit_req_per_min must be a positive integer")?;
                manifest.requests_per_minute = Some(limit);
                list_property = false;
            }
            (2, "governance_policies", "require_auth") => {
                manifest.require_auth = match value {
                    "true" => true,
                    "false" => false,
                    _ => return Err("require_auth must be true or false".into()),
                };
                list_property = false;
            }
            (2, "endpoints", "llms_txt") => {
                manifest.llms_txt = Some(validate_manifest_path(&unquote(value)?)?);
                list_property = false;
            }
            (2, "endpoints", "mcp_server") => {
                unquote(value)?;
                list_property = false;
            }
            _ => {
                return Err(format!(
                    "manifest contains unsupported field or syntax: {key}"
                ));
            }
        }
    }
    if !has_version {
        return Err("manifest requires a version field".into());
    }
    Ok(manifest)
}

fn parse_yaml_string_list(value: &str) -> Result<Vec<String>, String> {
    if !value.starts_with('[') || !value.ends_with(']') {
        return Err("disallow_agents must be an inline list or a YAML block list".into());
    }
    value[1..value.len() - 1]
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(unquote)
        .collect()
}

fn unquote(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("manifest scalar is empty".into());
    }
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        Ok(value[1..value.len() - 1].to_owned())
    } else if value.starts_with(['"', '\'']) || value.ends_with(['"', '\'']) {
        Err("manifest scalar has mismatched quotes".into())
    } else {
        Ok(value.to_owned())
    }
}

fn validate_manifest_path(path: &str) -> Result<String, String> {
    if !path.starts_with('/')
        || path.starts_with("//")
        || path.contains("..")
        || path.contains(['?', '#'])
    {
        return Err("llms_txt must be a same-site absolute path".into());
    }
    Ok(path.to_owned())
}

fn reject_unknown_keys<'a>(
    keys: impl Iterator<Item = &'a str>,
    allowed: &[&str],
) -> Result<(), String> {
    if let Some(key) = keys.into_iter().find(|key| !allowed.contains(key)) {
        return Err(format!("manifest contains unsupported field: {key}"));
    }
    Ok(())
}

fn select_llms_markdown(
    index: &str,
    index_url: &url::Url,
    query: &str,
    candidate_title: &str,
) -> Option<url::Url> {
    let query_words = crate::storage::relevant_words(&format!("{query} {candidate_title}"));
    let mut best: Option<(usize, url::Url)> = None;
    for line in index.lines() {
        let Some(open) = line.find("](") else {
            continue;
        };
        let label = line[..open].trim_start_matches('[').trim();
        let tail = &line[open + 2..];
        let Some(close) = tail.find(')') else {
            continue;
        };
        let link = tail[..close].trim();
        let Ok(url) = index_url.join(link) else {
            continue;
        };
        if url.origin() != index_url.origin()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query_pairs().any(|(name, _)| {
                matches!(
                    name.to_ascii_lowercase().as_str(),
                    "key" | "api_key" | "access_token" | "token" | "auth" | "session"
                )
            })
            || !url.path().to_ascii_lowercase().ends_with(".md")
            || url.scheme() != "https"
        {
            continue;
        }
        let words = crate::storage::relevant_words(&format!("{label} {}", url.path()));
        let score = crate::storage::lexical_overlap(&query_words, &words);
        if score > 0 && best.as_ref().is_none_or(|(current, _)| score > *current) {
            best = Some((score, url));
        }
    }
    best.map(|(_, url)| url)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::{
        FetchTarget, SiteManifest, fetch_site_policy, inspect_and_fetch, manifest_access_denial,
        manifest_request_interval, parse_manifest, parse_robots, robots_allowed,
        select_llms_markdown, select_page_url,
    };

    #[test]
    fn cached_site_policy_is_used_without_a_network_request() {
        let path = std::env::temp_dir().join(format!(
            "community-policy-cache-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut store = crate::storage::LearningStore::open(&path).unwrap();
        store
            .cache_site_policy(
                "https://example.test",
                "external_robots_txt",
                200,
                "text/plain",
                "User-agent: *\nDisallow: /private",
                60_000,
            )
            .unwrap();
        let target = FetchTarget {
            host: "example.test".into(),
            ip: "203.0.113.12".parse().unwrap(),
        };
        let url = url::Url::parse("https://example.test/robots.txt").unwrap();
        let response =
            fetch_site_policy(&mut store, &target, &url, "external_robots_txt", 10_000).unwrap();
        assert_eq!(response.status, 200);
        assert!(response.body.contains("Disallow: /private"));
        drop(store);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn cached_robots_denial_skips_a_candidate_before_dns() {
        let path = std::env::temp_dir().join(format!(
            "community-robots-skip-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut store = crate::storage::LearningStore::open(&path).unwrap();
        store
            .remember_robots_block(
                "unresolvable.example.invalid",
                "/private",
                "/private",
                "robots.txt disallows this source path",
                false,
            )
            .unwrap();
        let candidate = crate::storage::ScholarSearchCandidate {
            id: 1,
            provider: "test".into(),
            query: "topic".into(),
            result_type: "search_result".into(),
            title: "Private article".into(),
            url: "https://unresolvable.example.invalid/private".into(),
            snippet: String::new(),
            structured_data_json: "{}".into(),
            status: "discovered".into(),
            policy_status: "not_checked".into(),
            policy_reason: None,
            fetch_status: "not_attempted".into(),
            fetched_url: None,
            fetched_excerpt: None,
            fetched_at: None,
            page_provenance_json: None,
        };
        let review = inspect_and_fetch(&mut store, &candidate);
        assert_eq!(review.policy_status, "disallowed");
        assert!(review.policy_reason.contains("cached"));
        drop(store);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn robots_merges_matching_groups_and_uses_longest_allow_rule() {
        let rules = parse_robots(
            "User-agent: *\nDisallow: /global\n\nUser-agent: CommunityLabScholar\nDisallow: /private\n\nUser-agent: communitylabscholar\nDisallow: /articles\nAllow: /articles/public\n",
        )
        .unwrap();
        assert!(!robots_allowed(&rules, "/private/data"));
        assert!(!robots_allowed(&rules, "/articles/draft"));
        assert!(robots_allowed(&rules, "/articles/public/page"));
        assert!(robots_allowed(&rules, "/global"));
    }

    #[test]
    fn robots_supports_wildcard_and_end_anchor() {
        let rules = parse_robots("User-agent: *\nDisallow: /*.pdf$\n").unwrap();
        assert!(!robots_allowed(&rules, "/files/report.pdf"));
        assert!(robots_allowed(&rules, "/files/report.pdf/preview"));
    }

    #[test]
    fn manifest_accepts_supported_yaml_and_holds_unknown_terms() {
        let manifest = parse_manifest(
            "version: '1.0'\ngovernance_policies:\n  disallow_agents: ['CommunityLabScholar']\n  rate_limit_req_per_min: 3\n  require_auth: false\nendpoints:\n  llms_txt: '/knowledge/llms.txt'\n  mcp_server: 'https://example.test/mcp'\n",
        )
        .unwrap();
        assert_eq!(manifest.disallow_agents, ["CommunityLabScholar"]);
        assert_eq!(manifest.requests_per_minute, Some(3));
        assert_eq!(manifest.llms_txt.as_deref(), Some("/knowledge/llms.txt"));
        assert!(parse_manifest("please do not crawl this site").is_err());
        assert!(parse_manifest("version: '1'\nunknown_policy: maybe\n").is_err());
    }

    #[test]
    fn manifest_disallow_auth_and_declared_rate_are_enforced_conservatively() {
        let blocked =
            parse_manifest("version: '1'\ngovernance_policies:\n  disallow_agents: ['*']\n")
                .unwrap();
        assert!(
            manifest_access_denial(&blocked)
                .unwrap()
                .contains("disallows")
        );

        let gated = SiteManifest {
            disallow_agents: Vec::new(),
            requests_per_minute: Some(3),
            require_auth: true,
            llms_txt: None,
        };
        assert!(
            manifest_access_denial(&gated)
                .unwrap()
                .contains("authentication")
        );
        assert_eq!(
            manifest_request_interval(10_000, gated.requests_per_minute),
            20_000
        );
        assert_eq!(
            manifest_request_interval(30_000, gated.requests_per_minute),
            30_000
        );

        let per_agent = parse_manifest(
            "version: '1'\ngovernance_policies:\n  disallow_agents: ['CommunityLabScholar']\n",
        )
        .unwrap();
        assert!(manifest_access_denial(&per_agent).is_some());
    }

    #[test]
    fn llms_index_prefers_relevant_same_host_markdown() {
        let index_url = url::Url::parse("https://example.test/llms.txt").unwrap();
        let selected = select_llms_markdown(
            "# Index\n- [Chicken soup ingredients](https://example.test/soup.md)\n- [External](https://other.test/soup.md)",
            &index_url,
            "chicken soup",
            "Chicken Soup",
        )
        .unwrap();
        assert_eq!(selected.as_str(), "https://example.test/soup.md");
    }

    #[test]
    fn llms_markdown_is_selected_only_when_robots_allows_its_path() {
        let candidate = url::Url::parse("https://example.test/soup").unwrap();
        let index = url::Url::parse("https://example.test/llms.txt").unwrap();
        let listing = "- [Chicken soup ingredients](https://example.test/soup.md)";
        let denied = parse_robots("User-agent: *\nDisallow: /soup.md\n").unwrap();
        assert_eq!(
            select_page_url(
                &candidate,
                "chicken soup",
                "Chicken Soup",
                Some((index.clone(), listing.into())),
                &denied,
            ),
            candidate
        );
        assert_eq!(
            select_page_url(
                &candidate,
                "chicken soup",
                "Chicken Soup",
                Some((index, listing.into())),
                &[],
            )
            .as_str(),
            "https://example.test/soup.md"
        );
    }

    #[test]
    fn llms_index_rejects_credential_bearing_links() {
        let index_url = url::Url::parse("https://example.test/llms.txt").unwrap();
        let selected = select_llms_markdown(
            "- [Chicken soup ingredients](https://example.test/soup.md?api_key=secret)",
            &index_url,
            "chicken soup",
            "Chicken Soup",
        );
        assert!(selected.is_none());
    }
}
