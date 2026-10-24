//! Structured operational events for following a maintenance decision.
//!
//! Events deliberately contain identifiers, outcomes, and reasons rather than
//! raw conversation text, which is already retained in the local SQLite store.

use std::io::{self, Write};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

static RESEARCH_LOG_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub(crate) fn text_excerpt(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

pub(crate) fn event(name: &str, fields: serde_json::Value) {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let record = serde_json::json!({
        "ts_unix_ms": timestamp,
        "event": name,
        "details": fields,
    });
    let stderr = io::stderr();
    let mut output = stderr.lock();
    let _ = serde_json::to_writer(&mut output, &record);
    let _ = output.write_all(b"\n");
}

/// Persist the small set of model decisions needed to diagnose missed lookups.
/// The regular event stream remains on stderr; this file is the durable audit
/// trail for live grounding tests.
pub(crate) fn research_decision(name: &str, fields: serde_json::Value) {
    event(name, fields.clone());
    #[cfg(not(test))]
    persist_research_decision(name, fields);
}

#[cfg(not(test))]
fn persist_research_decision(name: &str, fields: serde_json::Value) {
    let record = serde_json::json!({
        "ts_unix_ms": SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        "event": name,
        "details": fields,
    });
    if let Err(error) = append_jsonl(Path::new("state/research-decisions.jsonl"), &record) {
        eprintln!("could not persist research decision: {error}");
    }
}

fn append_jsonl(path: &Path, record: &serde_json::Value) -> io::Result<()> {
    let _guard = RESEARCH_LOG_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| io::Error::other("research decision log lock is poisoned"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    serde_json::to_writer(&mut file, record)?;
    file.write_all(b"\n")
}

pub(crate) fn safe_public_url(value: &str) -> String {
    let Ok(mut url) = url::Url::parse(value) else {
        return "[invalid URL]".into();
    };
    if !matches!(url.scheme(), "http" | "https") {
        return "[non-web URL]".into();
    }
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_excerpt_limits_unicode_characters_without_splitting_utf8() {
        assert_eq!(text_excerpt("a🌱é", 2), "a🌱");
        assert_eq!(text_excerpt("short", 10), "short");
    }

    #[test]
    fn appends_structured_research_decisions_as_json_lines() {
        let path = crate::test_support::unique_temp_path("lyra-research-decisions", "jsonl");
        let first = serde_json::json!({"event":"intent","details":{"lookup":true}});
        let second = serde_json::json!({"event":"plan","details":{"queued":false}});
        append_jsonl(&path, &first).unwrap();
        append_jsonl(&path, &second).unwrap();
        let lines = std::fs::read_to_string(&path).unwrap();
        let records = lines
            .lines()
            .map(serde_json::from_str::<serde_json::Value>)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(records, [first, second]);
        std::fs::remove_file(path).unwrap();
    }
}
