//! Structured operational events for following a maintenance decision.
//!
//! Events deliberately contain identifiers, outcomes, and reasons rather than
//! raw conversation text, which is already retained in the local SQLite store.

use std::io::{self, Write};
use std::time::{SystemTime, UNIX_EPOCH};

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
