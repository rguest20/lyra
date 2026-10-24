//! Wikipedia API access and page-to-passage processing.

use super::*;
use sha2::{Digest, Sha256};

fn is_non_content_section(heading: &str) -> bool {
    matches!(
        heading,
        "references" | "notes" | "external links" | "see also" | "further reading" | "bibliography"
    )
}

pub(super) fn wikipedia_json_request(
    store: &mut LearningStore,
    request_kind: &str,
    max_bytes: usize,
    parameters: &[(&str, String)],
) -> io::Result<serde_json::Value> {
    let mut key_parameters = parameters.to_vec();
    key_parameters.sort();
    let key = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(request_kind, &key_parameters)).map_err(io::Error::other)?
        )
    );
    if let Some(response) = store.cached_wikipedia_response(&key)? {
        crate::diagnostics::event(
            "internet.cache_hit",
            serde_json::json!({"provider":"wikipedia","request_kind":request_kind}),
        );
        return Ok(response);
    }
    let reservation = store.reserve_host_request_wait(
        "en.wikipedia.org",
        request_kind,
        crate::storage::wikipedia_interval_ms(),
        std::time::Duration::from_secs(30),
    )?;
    super::wait_for_host_request(reservation.scheduled_at_ms)?;
    let mut command = super::limited_curl(15, max_bytes);
    command.args(["--user-agent", USER_AGENT, "--get", SEARCH_API]);
    for (name, value) in parameters {
        command
            .arg("--data-urlencode")
            .arg(format!("{name}={value}"));
    }
    command.args(["--data-urlencode", "maxlag=5"]);
    command
        .arg("--write-out")
        .arg(super::http_response_write_out(HTTP_STATUS_MARKER));
    let (output, metadata) = super::run_reserved_http_response(
        store,
        reservation.id,
        &mut command,
        HTTP_STATUS_MARKER,
        "Wikipedia response omitted HTTP status",
        "Wikipedia response omitted its HTTP status",
    )?;
    let body = String::from_utf8_lossy(&output.stdout);
    let response_status = super::require_http_status(
        store,
        reservation.id,
        &metadata,
        "Wikipedia response omitted HTTP status",
        "Wikipedia response omitted its HTTP status",
    )?;
    let parsed = serde_json::from_slice::<serde_json::Value>(body.as_bytes());
    if let Ok(json) = &parsed
        && let Some(code) = json["error"]["code"].as_str()
    {
        let transient = mediawiki_transient_error(code);
        super::complete_reserved_http_request(
            store,
            reservation.id,
            &metadata,
            Some(if transient {
                crate::storage::MEDIAWIKI_TRANSIENT_ERROR
            } else {
                "MediaWiki API returned an error"
            }),
        )?;
        crate::diagnostics::event(
            "internet.request_failed",
            serde_json::json!({"provider":"wikipedia","request_kind":request_kind,"http_status":response_status,"api_error_code":code.chars().take(100).collect::<String>(),"retryable":transient}),
        );
        return Err(io::Error::new(
            if transient {
                io::ErrorKind::WouldBlock
            } else {
                io::ErrorKind::Other
            },
            format!(
                "Wikipedia API error: {}",
                code.chars().take(100).collect::<String>()
            ),
        ));
    }
    let request_error = (!output.status.success())
        .then(|| wikipedia_request_failure(output.status.code(), response_status, max_bytes));
    super::complete_reserved_http_request(
        store,
        reservation.id,
        &metadata,
        request_error.as_deref(),
    )?;
    if let Some(error) = request_error {
        crate::diagnostics::event(
            "internet.request_failed",
            serde_json::json!({
                "provider": "wikipedia",
                "request_kind": request_kind,
                "http_status": response_status,
                "curl_exit_code": output.status.code(),
                "max_bytes": max_bytes,
                "received_bytes": body.len(),
                "error": error,
            }),
        );
        return Err(io::Error::new(
            if matches!(response_status, 429 | 503) {
                io::ErrorKind::WouldBlock
            } else {
                io::ErrorKind::Other
            },
            error,
        ));
    }
    if body.len() > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Wikipedia response exceeded the configured size limit",
        ));
    }
    let parsed = parsed.map_err(crate::errors::invalid_json)?;
    store.cache_wikipedia_response(&key, request_kind, &parsed)?;
    Ok(parsed)
}

fn mediawiki_transient_error(code: &str) -> bool {
    matches!(
        code,
        "ratelimited" | "maxlag" | "readonly" | "internal_api_error_DBConnectionError"
    )
}

fn wikipedia_request_failure(exit_code: Option<i32>, http_status: u16, max_bytes: usize) -> String {
    let reason = match exit_code {
        Some(6) => "host could not be resolved".to_owned(),
        Some(7) => "connection failed".to_owned(),
        Some(18) => "response was incomplete".to_owned(),
        Some(22) => "server returned an HTTP error".to_owned(),
        Some(28) => "request timed out".to_owned(),
        Some(35) | Some(60) => "TLS connection or certificate verification failed".to_owned(),
        Some(63) => format!("response exceeded the {max_bytes}-byte limit"),
        _ => "curl transport failed".to_owned(),
    };
    let exit = exit_code.map_or_else(|| "terminated".into(), |code| code.to_string());
    format!("Wikipedia request failed: {reason} (curl exit {exit}; HTTP status {http_status})")
}

#[cfg(test)]
mod tests {
    use super::wikipedia_request_failure;
    use crate::scholar::ParsedSection;

    #[test]
    fn mediawiki_json_rate_limits_are_retryable_even_with_http_200() {
        assert!(super::mediawiki_transient_error("ratelimited"));
        assert!(super::mediawiki_transient_error("maxlag"));
        assert!(!super::mediawiki_transient_error("badvalue"));
    }

    #[test]
    fn successful_http_status_can_still_have_a_transport_or_size_failure() {
        let timeout = wikipedia_request_failure(Some(28), 200, 1200);
        assert!(timeout.contains("request timed out"));
        assert!(timeout.contains("curl exit 28; HTTP status 200"));
        let oversized = wikipedia_request_failure(Some(63), 200, 1200);
        assert!(oversized.contains("exceeded the 1200-byte limit"));
        let server_error = wikipedia_request_failure(Some(22), 503, 1200);
        assert!(server_error.contains("server returned an HTTP error"));
        assert!(server_error.contains("HTTP status 503"));
    }

    #[test]
    fn relevant_passages_excludes_the_same_non_content_sections_as_ranking() {
        let sections = [
            ParsedSection {
                index: serde_json::json!("1"),
                line: "Ingredients".into(),
            },
            ParsedSection {
                index: serde_json::json!("2"),
                line: "References".into(),
            },
        ];
        let ranked = super::ranked_sections("ingredients", &sections);
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].title, "Ingredients");

        let article = "# Ingredients\nChicken soup contains carrots.\n# References\nChicken soup contains unrelated source notes.";
        let excerpt = super::relevant_passages("What ingredients are in chicken soup?", article);
        assert!(excerpt.contains("Chicken soup contains carrots."));
        assert!(!excerpt.contains("unrelated source notes"));
    }
}

pub(super) fn text_property(value: &serde_json::Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.get("*").and_then(serde_json::Value::as_str))
}

pub(super) fn html_to_text(html: &str) -> io::Result<String> {
    let without_references = remove_classed_html_element(html, "sup", "reference");
    let without_reference_list =
        remove_classed_html_element(&without_references, "ol", "references");
    let without_reference_wrap =
        remove_classed_html_element(&without_reference_list, "div", "mw-references-wrap");
    let without_edit_links =
        remove_classed_html_element(&without_reference_wrap, "span", "mw-editsection");
    let text = html2text::config::plain()
        .link_footnotes(false)
        .string_from_read(without_edit_links.as_bytes(), 2_000)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(normalize_wikipedia_text(&text))
}

pub(super) fn remove_classed_html_element(html: &str, tag: &str, class_name: &str) -> String {
    let lowered = html.to_ascii_lowercase();
    let opening = format!("<{tag}");
    let closing = format!("</{tag}");
    let mut output = String::with_capacity(html.len());
    let mut copied_until = 0;
    let mut search_from = 0;
    while let Some(relative_start) = lowered[search_from..].find(&opening) {
        let start = search_from + relative_start;
        let after_name = start + opening.len();
        if lowered
            .as_bytes()
            .get(after_name)
            .is_some_and(|byte| !byte.is_ascii_whitespace() && *byte != b'>')
        {
            search_from = after_name;
            continue;
        }
        let Some(relative_open_end) = lowered[after_name..].find('>') else {
            break;
        };
        let open_end = after_name + relative_open_end;
        let attributes = &lowered[after_name..open_end];
        if !attributes.contains(class_name) {
            search_from = open_end + 1;
            continue;
        }
        let Some(relative_close_start) = lowered[open_end + 1..].find(&closing) else {
            break;
        };
        let close_start = open_end + 1 + relative_close_start;
        let Some(relative_close_end) = lowered[close_start..].find('>') else {
            break;
        };
        let close_end = close_start + relative_close_end + 1;
        output.push_str(&html[copied_until..start]);
        copied_until = close_end;
        search_from = close_end;
    }
    output.push_str(&html[copied_until..]);
    output
}

pub(super) fn normalize_wikipedia_text(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut previous_blank = false;
    for line in text.lines() {
        let line = line.replace('\u{00a0}', " ");
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !previous_blank && !normalized.is_empty() {
                normalized.push('\n');
            }
            previous_blank = true;
            continue;
        }
        if trimmed == "[edit]" || trimmed.starts_with("[[edit]") || trimmed.starts_with("**[^]") {
            continue;
        }
        normalized.push_str(&trimmed.replace(['[', ']'], ""));
        normalized.push('\n');
        previous_blank = false;
    }
    normalized.trim().to_owned()
}

pub(super) fn ranked_sections(question: &str, sections: &[ParsedSection]) -> Vec<SectionChoice> {
    let query_words = crate::storage::relevant_words(question);
    let mut ranked = sections
        .iter()
        .filter(|section| !is_non_content_section(&section.line.trim().to_ascii_lowercase()))
        .filter_map(|section| {
            let words = crate::storage::relevant_words(&section.line);
            let index = section
                .index
                .as_str()
                .map(str::to_owned)
                .or_else(|| section.index.as_i64().map(|value| value.to_string()))?;
            Some((
                crate::storage::lexical_overlap(&query_words, &words),
                index,
                section.line.trim().to_owned(),
            ))
        })
        .filter(|(score, _, _)| *score > 0)
        .collect::<Vec<_>>();
    ranked.sort_by(
        |(left_score, _, left_title), (right_score, _, right_title)| {
            right_score
                .cmp(left_score)
                .then_with(|| left_title.cmp(right_title))
        },
    );
    ranked
        .into_iter()
        .take(2)
        .map(|(_, index, title)| SectionChoice { index, title })
        .collect()
}

pub(super) fn page_relevance_score(question: &str, page: &SearchPage) -> usize {
    let query_words = crate::storage::relevant_words(question);
    let page_words = crate::storage::relevant_words(&format!(
        "{} {}",
        page.title,
        page.extract.as_deref().unwrap_or_default()
    ));
    crate::storage::lexical_overlap(&query_words, &page_words)
}

pub(super) fn relevant_passages(question: &str, article: &str) -> String {
    let query_words = crate::storage::relevant_words(question);
    let mut skip_section = false;
    let mut article_without_reference_sections = String::new();
    for line in article.lines() {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix('#') {
            let heading = heading.trim_matches('#').trim().to_ascii_lowercase();
            skip_section = is_non_content_section(&heading);
        }
        if !skip_section {
            article_without_reference_sections.push_str(line);
            article_without_reference_sections.push('\n');
        }
    }
    let sentences = article_without_reference_sections
        .split_inclusive(". ")
        .map(str::trim)
        .filter(|sentence| !sentence.is_empty())
        .collect::<Vec<_>>();
    let mut ranked = sentences
        .iter()
        .enumerate()
        .map(|(index, sentence)| {
            let words = crate::storage::relevant_words(sentence);
            (crate::storage::lexical_overlap(&query_words, &words), index)
        })
        .filter(|(score, _)| *score > 0)
        .collect::<Vec<_>>();
    ranked.sort_by(|(left_score, left_index), (right_score, right_index)| {
        right_score
            .cmp(left_score)
            .then_with(|| left_index.cmp(right_index))
    });
    let mut selected = Vec::new();
    let mut used_chars = 0;
    for (_, index) in ranked {
        if selected.contains(&index) || selected.len() >= 8 {
            continue;
        }
        let sentence = sentences[index];
        let added_chars = sentence.chars().count() + usize::from(!selected.is_empty());
        if used_chars + added_chars > MAX_SELECTED_PASSAGE_CHARS {
            continue;
        }
        used_chars += added_chars;
        selected.push(index);
    }
    if selected.is_empty() {
        return article.chars().take(MAX_SELECTED_PASSAGE_CHARS).collect();
    }
    selected.sort_unstable();
    selected
        .into_iter()
        .map(|index| sentences[index])
        .collect::<Vec<_>>()
        .join("\n")
}
