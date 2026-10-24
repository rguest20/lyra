//! Syntax-only model JSON parsing; never infer missing fields or values.
use super::*;
use serde::de::DeserializeOwned;

#[derive(serde::Deserialize)]
pub(crate) struct DecisionRationale {
    pub decision: String,
    pub rationale: String,
}

pub(crate) fn json_object_slice<'a>(
    raw: &'a str,
    missing_object: &str,
    incomplete_object: &str,
) -> io::Result<&'a str> {
    let start = raw.find('{').ok_or_else(|| invalid(missing_object))?;
    let end = raw.rfind('}').ok_or_else(|| invalid(incomplete_object))?;
    Ok(&raw[start..=end])
}

pub(crate) fn json_object(raw: &str) -> io::Result<serde_json::Value> {
    let envelope = json_object_slice(
        raw,
        "model omitted JSON object",
        "model returned incomplete JSON",
    )?;
    serde_json::from_str(envelope).map_err(|_| {
        crate::diagnostics::event(
            "research.invalid_json",
            serde_json::json!({"output":crate::diagnostics::text_excerpt(raw, 2000)}),
        );
        invalid("model returned invalid JSON")
    })
}

pub(crate) fn json_object_as<T>(raw: &str) -> io::Result<T>
where
    T: DeserializeOwned,
{
    serde_json::from_value(json_object(raw)?).map_err(crate::errors::invalid_json)
}

pub(crate) fn deserialize_json_object<T>(
    raw: &str,
    missing_object: &str,
    incomplete_object: &str,
    invalid_json: impl FnOnce(serde_json::Error) -> io::Error,
) -> io::Result<T>
where
    T: DeserializeOwned,
{
    let envelope = json_object_slice(raw, missing_object, incomplete_object)?;
    serde_json::from_str(envelope).map_err(invalid_json)
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

pub(crate) fn json_object_with_delimiter_recovery(
    raw: &str,
) -> io::Result<(serde_json::Value, bool)> {
    match json_object(raw) {
        Ok(value) => Ok((value, false)),
        Err(error) => close_json_delimiters(raw)
            .map(|value| (value, true))
            .ok_or(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delimiter_recovery_reports_repair_without_supplying_values() {
        assert_eq!(
            json_object_with_delimiter_recovery(r#"{"verdict":"accept"}"#).unwrap(),
            (serde_json::json!({"verdict":"accept"}), false)
        );
        assert_eq!(
            json_object_with_delimiter_recovery(r#"{"verdict":"accept""#).unwrap(),
            (serde_json::json!({"verdict":"accept"}), true)
        );
        assert!(json_object_with_delimiter_recovery(r#"{"verdict":]"#).is_err());
    }
}
