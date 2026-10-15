//! Syntax-only model JSON parsing; never infer missing fields or values.
use super::*;

pub(crate) fn json_object(raw: &str) -> io::Result<serde_json::Value> {
    let start = raw
        .find('{')
        .ok_or_else(|| invalid("model omitted JSON object"))?;
    let end = raw
        .rfind('}')
        .ok_or_else(|| invalid("model returned incomplete JSON"))?;
    serde_json::from_str(&raw[start..=end]).map_err(|_| {
        crate::diagnostics::event(
            "research.invalid_json",
            serde_json::json!({"output":raw.chars().take(2000).collect::<String>()}),
        );
        invalid("model returned invalid JSON")
    })
}

// Recover only missing closing delimiters from otherwise complete JSON. This
// repairs transport truncation without inventing a verdict or field value.
pub(crate) fn close_json_delimiters(raw: &str) -> Option<serde_json::Value> {
    let start = raw.find('{')?;
    let mut stack = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    for ch in raw[start..].chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '}' | ']' if stack.last() == Some(&ch) => {
                stack.pop();
            }
            '}' | ']' => return None,
            _ => {}
        }
    }
    if in_string {
        return None;
    }
    let mut completed = raw[start..].to_owned();
    for closing in stack.into_iter().rev() {
        completed.push(closing);
    }
    serde_json::from_str(&completed).ok()
}
