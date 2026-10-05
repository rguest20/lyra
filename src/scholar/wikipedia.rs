//! Wikipedia API access and page-to-passage processing.

use super::*;
use std::process::Command;

pub(super) fn wikipedia_json_request(
    store: &mut LearningStore,
    request_kind: &str,
    max_bytes: usize,
    parameters: &[(&str, String)],
) -> io::Result<serde_json::Value> {
    let reservation = store.reserve_host_request_wait(
        "en.wikipedia.org",
        request_kind,
        10_000,
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
    let mut command = Command::new("curl");
    command
        .args([
            "--silent",
            "--show-error",
            "--fail",
            "--max-time",
            "15",
            "--max-filesize",
        ])
        .arg(max_bytes.to_string())
        .args(["--user-agent", USER_AGENT, "--get", SEARCH_API]);
    for (name, value) in parameters {
        command
            .arg("--data-urlencode")
            .arg(format!("{name}={value}"));
    }
    let output = command
        .arg("--write-out")
        .arg(format!(
            "{HTTP_STATUS_MARKER}%{{http_code}}\t%header{{retry-after}}"
        ))
        .output();
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            store.complete_host_request(reservation.id, None, Some("curl could not run"))?;
            return Err(error);
        }
    };
    let combined = String::from_utf8_lossy(&output.stdout);
    let Some((body, metadata)) = combined.rsplit_once(HTTP_STATUS_MARKER) else {
        store.complete_host_request(
            reservation.id,
            None,
            Some("Wikipedia response omitted HTTP status"),
        )?;
        return Err(io::Error::other(
            "Wikipedia response omitted its HTTP status",
        ));
    };
    let mut metadata = metadata.trim_end_matches(['\r', '\n']).split('\t');
    let Some(response_status) = metadata
        .next()
        .and_then(|status| status.parse::<u16>().ok())
    else {
        store.complete_host_request(
            reservation.id,
            None,
            Some("Wikipedia response omitted HTTP status"),
        )?;
        return Err(io::Error::other(
            "Wikipedia response omitted its HTTP status",
        ));
    };
    let retry_after = metadata.next().and_then(|value| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_millis();
        retry_after_delay_ms(value, i64::try_from(now).ok()?)
    });
    let request_error = (!output.status.success()).then_some("Wikipedia request failed");
    store.complete_host_request_with_retry_after(
        reservation.id,
        Some(response_status),
        retry_after,
        request_error,
    )?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "Wikipedia request failed with HTTP status {response_status}"
        )));
    }
    if body.len() > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Wikipedia response exceeded the configured size limit",
        ));
    }
    serde_json::from_slice(body.as_bytes())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
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
        .filter(|section| {
            !matches!(
                section.line.trim().to_ascii_lowercase().as_str(),
                "references"
                    | "notes"
                    | "external links"
                    | "see also"
                    | "further reading"
                    | "bibliography"
            )
        })
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
            skip_section = matches!(
                heading.as_str(),
                "references"
                    | "notes"
                    | "external links"
                    | "see also"
                    | "further reading"
                    | "bibliography"
            );
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
