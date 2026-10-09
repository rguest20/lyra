use std::io::{self, Write};
use std::process::{Command, Stdio};

use crate::agent::Message;

pub(super) fn review(key: &str, messages: &[Message]) -> io::Result<String> {
    if key.chars().any(char::is_control) {
        return Err(io::Error::other(
            "OPENAI_API_KEY contains invalid characters",
        ));
    }
    let model = std::env::var("COMMUNITY_CURMUDGEON_OPENAI_MODEL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "gpt-4.1-mini".into());
    let payload = serde_json::json!({
        "model": model,
        "messages": messages.iter().map(|message| serde_json::json!({
            "role": message.role,
            "content": message.content.strip_prefix("/no_think\n").unwrap_or(&message.content),
        })).collect::<Vec<_>>(),
        "response_format": {"type": "json_object"},
        "max_completion_tokens": 1536,
        "store": false,
    });
    let config = format!(
        "url = \"https://api.openai.com/v1/chat/completions\"\nrequest = \"POST\"\nheader = {}\nheader = \"Content-Type: application/json\"\ndata = {}\n",
        serde_json::to_string(&format!("Authorization: Bearer {key}")).map_err(io::Error::other)?,
        serde_json::to_string(&payload.to_string()).map_err(io::Error::other)?,
    );
    let mut child = Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--connect-timeout",
            "10",
            "--max-time",
            "45",
            "--max-filesize",
            "1048576",
            "--write-out",
            "\n%{http_code}",
            "--config",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let write_result = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("OpenAI request stdin is unavailable"))
        .and_then(|mut stdin| stdin.write_all(config.as_bytes()));
    if write_result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return Err(io::Error::other("could not write OpenAI review request"));
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            "OpenAI review request failed or timed out; review remains pending",
        ));
    }
    let response = std::str::from_utf8(&output.stdout)
        .map_err(|_| io::Error::other("OpenAI returned invalid response encoding"))?;
    let (body, status) = response
        .rsplit_once('\n')
        .ok_or_else(|| io::Error::other("OpenAI response is missing HTTP status"))?;
    let result = parse_response(status, body);
    crate::diagnostics::event(
        "curmudgeon.provider_review",
        serde_json::json!({
            "provider": "openai", "model": model, "http_status": status,
            "status": if result.is_ok() { "completed" } else { "deferred" },
        }),
    );
    result
}

fn parse_response(status: &str, body: &str) -> io::Result<String> {
    if status != "200" {
        let error: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
        let guidance = match error["error"]["code"].as_str() {
            Some("insufficient_quota") => "; check OpenAI API billing and project quota",
            Some("rate_limit_exceeded") => "; OpenAI rate limit exceeded, retry later",
            Some("invalid_api_key") => "; check OPENAI_API_KEY",
            Some("model_not_found") => "; check COMMUNITY_CURMUDGEON_OPENAI_MODEL and model access",
            _ => "",
        };
        return Err(io::Error::other(format!(
            "OpenAI review returned HTTP {status}{guidance}; review remains pending"
        )));
    }
    let response: serde_json::Value =
        serde_json::from_str(body).map_err(|_| io::Error::other("OpenAI returned invalid JSON"))?;
    let choice = &response["choices"][0];
    if choice["finish_reason"] != "stop" || !choice["message"]["refusal"].is_null() {
        return Err(io::Error::other(
            "OpenAI review was refused or incomplete; review remains pending",
        ));
    }
    let content = choice["message"]["content"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| io::Error::other("OpenAI review returned no assessment"))?;
    let assessment: serde_json::Value = serde_json::from_str(content)
        .map_err(|_| io::Error::other("OpenAI review returned invalid assessment JSON"))?;
    if !assessment.is_object() {
        return Err(io::Error::other("OpenAI review must return a JSON object"));
    }
    Ok(content.to_owned())
}

#[cfg(test)]
mod tests {
    use super::parse_response;

    #[test]
    fn accepts_complete_json_assessment() {
        let content = r#"{"decision":"accept","rationale":"Faithful paraphrase."}"#;
        let response = serde_json::json!({"choices": [{"finish_reason": "stop", "message": {"content": content}}]});
        assert_eq!(
            parse_response("200", &response.to_string()).unwrap(),
            content
        );
    }

    #[test]
    fn provider_errors_do_not_expose_response_or_become_rejections() {
        let error = parse_response("401", r#"{"error":{"message":"secret-key"}}"#).unwrap_err();
        assert!(error.to_string().contains("review remains pending"));
        assert!(!error.to_string().contains("secret-key"));
        assert!(parse_response("429", "rate limited").is_err());
        assert!(parse_response("500", "provider unavailable").is_err());
        let quota = parse_response(
            "429",
            r#"{"error":{"code":"insufficient_quota","message":"secret-key"}}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(quota.contains("billing and project quota"));
        assert!(!quota.contains("secret-key"));
    }

    #[test]
    fn refuses_truncated_refused_or_malformed_assessments() {
        for (finish, refusal, content) in [
            (
                "length",
                serde_json::Value::Null,
                r#"{"decision":"accept"}"#,
            ),
            (
                "stop",
                serde_json::json!("refused"),
                r#"{"decision":"accept"}"#,
            ),
            ("stop", serde_json::Value::Null, "not JSON"),
            ("stop", serde_json::Value::Null, "[]"),
            ("stop", serde_json::Value::Null, ""),
        ] {
            let response = serde_json::json!({"choices": [{"finish_reason": finish, "message": {"content": content, "refusal": refusal}}]});
            assert!(parse_response("200", &response.to_string()).is_err());
        }
    }
}
