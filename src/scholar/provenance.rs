//! Capture self-declared page origin clues. These are review aids, never an
//! automatic finding of expertise or independent authorship.

use std::collections::BTreeSet;

use scraper::{Html, Selector};
use serde::Serialize;
use serde_json::Value;
use url::Url;

#[derive(Default, Serialize)]
struct PageProvenance {
    canonical_url: Option<String>,
    publisher: Option<String>,
    author: Option<String>,
    published_at: Option<String>,
    cited_urls: Vec<String>,
    publication_ids: Vec<String>,
    publisher_ids: Vec<String>,
}

pub(super) fn extract_page_provenance(html: &str, page_url: &Url) -> Option<String> {
    let document = Html::parse_document(html);
    let mut found = PageProvenance::default();
    let mut cited = BTreeSet::new();
    let mut publication_ids = BTreeSet::new();
    let mut publisher_ids = BTreeSet::new();
    let meta = Selector::parse("meta").expect("static selector");
    for element in document.select(&meta).take(300) {
        let name = element
            .value()
            .attr("name")
            .or_else(|| element.value().attr("property"))
            .unwrap_or("")
            .to_ascii_lowercase();
        let Some(content) = element.value().attr("content").map(str::trim) else {
            continue;
        };
        match name.as_str() {
            "author" | "article:author" | "citation_author" => {
                set_first(&mut found.author, content)
            }
            "publisher" | "citation_publisher" | "og:site_name" => {
                set_first(&mut found.publisher, content)
            }
            "article:published_time" | "citation_publication_date" | "datepublished" => {
                set_first(&mut found.published_at, content)
            }
            "citation_reference" | "citation_pdf_url" => add_url(&mut cited, content, page_url),
            "citation_doi" | "dc.identifier" | "prism.doi" | "doi" => {
                add_doi(&mut publication_ids, content)
            }
            "citation_pmid" | "pmid" => add_pmid(&mut publication_ids, content),
            _ => {}
        }
    }
    let links = Selector::parse("link[rel][href]").expect("static selector");
    for element in document.select(&links).take(100) {
        let rel = element.value().attr("rel").unwrap_or("");
        if rel
            .split_whitespace()
            .any(|part| part.eq_ignore_ascii_case("canonical"))
        {
            if let Some(url) = element.value().attr("href") {
                found.canonical_url = normalized_url(url, page_url);
            }
            break;
        }
    }
    let scripts = Selector::parse("script[type='application/ld+json']").expect("static selector");
    for element in document.select(&scripts).take(12) {
        let raw = element.text().collect::<String>();
        if raw.len() > 100_000 {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(&raw) {
            collect_json_ld(
                &value,
                &mut found,
                &mut cited,
                &mut publication_ids,
                &mut publisher_ids,
                page_url,
                0,
            );
        }
    }
    found.cited_urls = cited.into_iter().take(8).collect();
    found.publication_ids = publication_ids.into_iter().take(8).collect();
    found.publisher_ids = publisher_ids.into_iter().take(8).collect();
    if found.canonical_url.is_none()
        && found.publisher.is_none()
        && found.author.is_none()
        && found.published_at.is_none()
        && found.cited_urls.is_empty()
        && found.publication_ids.is_empty()
        && found.publisher_ids.is_empty()
    {
        return None;
    }
    serde_json::to_string(&found).ok()
}

fn collect_json_ld(
    value: &Value,
    found: &mut PageProvenance,
    cited: &mut BTreeSet<String>,
    publication_ids: &mut BTreeSet<String>,
    publisher_ids: &mut BTreeSet<String>,
    page_url: &Url,
    depth: usize,
) {
    if depth > 3 {
        return;
    }
    if let Some(array) = value.as_array() {
        for item in array.iter().take(20) {
            collect_json_ld(
                item,
                found,
                cited,
                publication_ids,
                publisher_ids,
                page_url,
                depth + 1,
            );
        }
        return;
    }
    let Some(object) = value.as_object() else {
        return;
    };
    if let Some(graph) = object.get("@graph") {
        collect_json_ld(
            graph,
            found,
            cited,
            publication_ids,
            publisher_ids,
            page_url,
            depth + 1,
        );
    }
    for (field, target) in [
        ("publisher", &mut found.publisher),
        ("author", &mut found.author),
        ("datePublished", &mut found.published_at),
    ] {
        if let Some(value) = object.get(field).and_then(name_or_string) {
            set_first(target, value);
        }
    }
    if let Some(publisher) = object.get("publisher") {
        collect_publisher_ids(publisher, publisher_ids, page_url);
    }
    for field in ["citation", "isBasedOn"] {
        if let Some(value) = object.get(field) {
            collect_citations(value, cited, page_url);
        }
    }
    if let Some(identifier) = object.get("identifier") {
        collect_identifiers(identifier, publication_ids);
    }
}

fn collect_publisher_ids(value: &Value, identifiers: &mut BTreeSet<String>, page_url: &Url) {
    match value {
        Value::Array(items) => {
            for item in items.iter().take(12) {
                collect_publisher_ids(item, identifiers, page_url);
            }
        }
        Value::Object(object) => {
            if let Some(url) = object
                .get("@id")
                .or_else(|| object.get("url"))
                .and_then(Value::as_str)
            {
                add_url(identifiers, url, page_url);
            }
        }
        Value::String(url) if url.starts_with("https://") || url.starts_with("http://") => {
            add_url(identifiers, url, page_url)
        }
        _ => {}
    }
}

fn collect_identifiers(value: &Value, identifiers: &mut BTreeSet<String>) {
    match value {
        Value::Array(items) => {
            for item in items.iter().take(20) {
                collect_identifiers(item, identifiers);
            }
        }
        Value::Object(object) => {
            let kind = object
                .get("propertyID")
                .or_else(|| object.get("@type"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase();
            let raw = object
                .get("value")
                .or_else(|| object.get("@id"))
                .and_then(Value::as_str);
            if let Some(raw) = raw {
                if kind.contains("doi") {
                    add_doi(identifiers, raw);
                } else if kind.contains("pmid") || kind.contains("pubmed") {
                    add_pmid(identifiers, raw);
                }
            }
        }
        Value::String(raw) => add_doi(identifiers, raw),
        _ => {}
    }
}

fn add_doi(identifiers: &mut BTreeSet<String>, raw: &str) {
    let mut value = raw
        .trim()
        .trim_start_matches(|character: char| character.is_ascii_whitespace() || character == '(')
        .trim_end_matches(|character: char| {
            character.is_ascii_whitespace() || matches!(character, ')' | '.' | ',' | ';')
        });
    let lowercase = value.to_ascii_lowercase();
    for prefix in ["https://doi.org/", "http://doi.org/", "doi:"] {
        if lowercase.starts_with(prefix) {
            value = &value[prefix.len()..];
            break;
        }
    }
    let Some((registrant, suffix)) = value.split_once('/') else {
        return;
    };
    if value.len() <= 300
        && registrant.strip_prefix("10.").is_some_and(|digits| {
            (4..=9).contains(&digits.len()) && digits.bytes().all(|byte| byte.is_ascii_digit())
        })
        && !suffix.is_empty()
        && suffix.bytes().all(|byte| byte.is_ascii_graphic())
    {
        identifiers.insert(format!("doi:{}", value.to_ascii_lowercase()));
    }
}

fn add_pmid(identifiers: &mut BTreeSet<String>, raw: &str) {
    let value = raw.trim();
    let value = if value.to_ascii_lowercase().starts_with("pmid:") {
        &value[5..]
    } else {
        value
    }
    .trim();
    if !value.is_empty() && value.len() <= 32 && value.bytes().all(|byte| byte.is_ascii_digit()) {
        identifiers.insert(format!("pmid:{value}"));
    }
}

fn name_or_string(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.get("name").and_then(Value::as_str))
        .or_else(|| value.as_array()?.first().and_then(name_or_string))
}

fn collect_citations(value: &Value, cited: &mut BTreeSet<String>, page_url: &Url) {
    if let Some(items) = value.as_array() {
        for item in items.iter().take(20) {
            collect_citations(item, cited, page_url);
        }
    } else if let Some(url) = value
        .as_str()
        .or_else(|| value.get("url").and_then(Value::as_str))
        .or_else(|| value.get("@id").and_then(Value::as_str))
    {
        add_url(cited, url, page_url);
    }
}

fn set_first(target: &mut Option<String>, value: &str) {
    if target.is_none() && !value.is_empty() {
        *target = Some(value.chars().take(300).collect());
    }
}

fn add_url(cited: &mut BTreeSet<String>, value: &str, page_url: &Url) {
    if cited.len() < 8
        && let Some(url) = normalized_url(value, page_url)
    {
        cited.insert(url);
    }
}

fn normalized_url(value: &str, page_url: &Url) -> Option<String> {
    let parsed = page_url.join(value.trim()).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    let mut parsed = parsed;
    parsed.set_fragment(None);
    parsed.set_query(None);
    Some(parsed.as_str().chars().take(500).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_declared_origin_without_treating_common_ingredients_as_a_citation() {
        let page = Url::parse("https://cooking.example/recipes/soup").unwrap();
        let html = r#"<html><head>
            <link rel="canonical" href="/recipes/chicken-soup">
            <meta name="author" content="R. Cook">
            <meta property="article:published_time" content="2025-01-02">
            <script type="application/ld+json">{"@type":"Recipe","publisher":{"@type":"Organization","name":"The Kitchen","url":"https://publisher.example/about"},"citation":[{"url":"https://food.example/original"}]}</script>
            </head><body>Chicken soup contains chicken.</body></html>"#;
        let result = extract_page_provenance(html, &page).unwrap();
        let metadata: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            metadata["canonical_url"],
            "https://cooking.example/recipes/chicken-soup"
        );
        assert_eq!(metadata["publisher"], "The Kitchen");
        assert_eq!(metadata["author"], "R. Cook");
        assert_eq!(metadata["published_at"], "2025-01-02");
        assert_eq!(metadata["cited_urls"][0], "https://food.example/original");
        assert_eq!(
            metadata["publisher_ids"][0],
            "https://publisher.example/about"
        );
        assert_eq!(metadata["cited_urls"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn extracts_only_recognized_publication_identifiers() {
        let page = Url::parse("https://journal.example/article").unwrap();
        let html = r#"<meta name="citation_doi" content="HTTPS://DOI.ORG/10.1234/AbC.9">
            <meta name="citation_pmid" content="PMID: 123456">
            <meta name="dc.identifier" content="not-a-publication-id">
            <script type="application/ld+json">{"@type":"ScholarlyArticle","identifier":{"@type":"PropertyValue","propertyID":"DOI","value":"10.5555/xy-z"}}</script>"#;
        let metadata: Value =
            serde_json::from_str(&extract_page_provenance(html, &page).unwrap()).unwrap();
        assert_eq!(
            metadata["publication_ids"],
            serde_json::json!(["doi:10.1234/abc.9", "doi:10.5555/xy-z", "pmid:123456"])
        );
    }
}
