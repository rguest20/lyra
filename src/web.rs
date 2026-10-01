//! Loopback-only browser interface. The CLI handles each reply in a short
//! process, then a second short process curates staged observations.

use std::fs;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::storage::LearningStore;

const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const PAGE: &str = include_str!("../web/index.html");

#[derive(Deserialize)]
struct ChatRequest {
    message: String,
    session_id: String,
    #[serde(default)]
    show_thinking: bool,
}

pub fn serve(address: SocketAddr, database_path: &str) -> io::Result<()> {
    if !address.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "web server must bind to a loopback address",
        ));
    }
    let listener = TcpListener::bind(address)?;
    let address = listener.local_addr()?;
    println!("Community web UI: http://{address}");
    let executable = std::env::current_exe()?;
    let session_state_directory = Path::new(database_path)
        .parent()
        .unwrap_or_else(|| Path::new("state"))
        .join("web-sessions");
    fs::create_dir_all(&session_state_directory)?;
    let mut curator: Option<Child> = None;

    for incoming in listener.incoming() {
        if let Some(mut child) = curator.take()
            && let Err(error) = child.wait()
        {
            eprintln!("background curation process failed: {error}");
        }
        let mut stream = match incoming {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("web request failed: {error}");
                continue;
            }
        };
        stream.set_read_timeout(Some(Duration::from_secs(15)))?;
        match handle_request(
            &mut stream,
            &executable,
            address,
            &session_state_directory,
            Path::new(database_path),
        ) {
            Ok(should_curate) if should_curate => {
                match Command::new(&executable)
                    .arg("curate-once")
                    .env("COMMUNITY_DB", database_path)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::inherit())
                    .spawn()
                {
                    Ok(child) => curator = Some(child),
                    Err(error) => eprintln!("could not start background curation: {error}"),
                }
            }
            Ok(_) => {}
            Err(error) => eprintln!("could not handle web request: {error}"),
        }
    }
    Ok(())
}

/// Returns true after a chat request, which triggers one curation pass.
fn handle_request(
    stream: &mut TcpStream,
    executable: &std::path::Path,
    address: SocketAddr,
    session_state_directory: &Path,
    database_path: &Path,
) -> io::Result<bool> {
    let request = read_request(stream)?;
    let mut lines = request.split(|byte| *byte == b'\n');
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split(|byte| *byte == b' ');
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default();
    let headers_end = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing HTTP headers"))?;
    let headers = String::from_utf8_lossy(&request[..headers_end]);
    let origin = headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("origin"))
        .map(|(_, value)| value.trim());
    if let Some(origin) = origin {
        let accepted = [
            format!("http://{address}"),
            format!("http://localhost:{}", address.port()),
            format!("http://127.0.0.1:{}", address.port()),
            format!("http://[::1]:{}", address.port()),
        ];
        if !accepted.iter().any(|candidate| candidate == origin) {
            respond(
                stream,
                "403 Forbidden",
                "application/json",
                &json!({"error": "requests must come from this local web page"})
                    .to_string()
                    .into_bytes(),
            )?;
            return Ok(false);
        }
    }

    if method == b"GET" && path == b"/" {
        respond(
            stream,
            "200 OK",
            "text/html; charset=utf-8",
            PAGE.as_bytes(),
        )?;
        return Ok(false);
    }
    if method == b"GET" && (path == b"/api/ready" || path.starts_with(b"/api/ready?session_id=")) {
        let mut challenges = Vec::new();
        if let Some(session_id) = path.strip_prefix(b"/api/ready?session_id=") {
            let session_id = std::str::from_utf8(session_id).unwrap_or_default();
            if !valid_session_id(session_id) {
                respond(
                    stream,
                    "400 Bad Request",
                    "application/json",
                    br#"{"error":"invalid session ID"}"#,
                )?;
                return Ok(false);
            }
            let state_path = session_state_directory.join(format!("{session_id}.json"));
            if state_path.exists() {
                let state: serde_json::Value = serde_json::from_slice(&fs::read(&state_path)?)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                let conversation_id = state["conversation_id"]
                    .as_str()
                    .unwrap_or_else(|| state_path.to_str().unwrap_or_default());
                let store = LearningStore::open(database_path)?;
                challenges = store.latest_reply_challenges(conversation_id)?;
            }
        }
        let notices = challenges
            .iter()
            .map(|claim| {
                json!({
                    "subject": claim.subject,
                    "relation": claim.relation,
                    "object": claim.object,
                    "reason": claim.reason,
                    "evidence_refs": claim.evidence_refs,
                })
            })
            .collect::<Vec<_>>();
        respond(
            stream,
            "200 OK",
            "application/json; charset=utf-8",
            &json!({"ready": true, "challenges": notices})
                .to_string()
                .into_bytes(),
        )?;
        return Ok(false);
    }
    if method != b"POST" || path != b"/api/chat" {
        respond(
            stream,
            "404 Not Found",
            "application/json",
            &json!({"error": "route not found"}).to_string().into_bytes(),
        )?;
        return Ok(false);
    }

    let body = &request[headers_end + 4..];
    let parsed: ChatRequest = match serde_json::from_slice::<ChatRequest>(body) {
        Ok(parsed) if !parsed.message.trim().is_empty() && valid_session_id(&parsed.session_id) => {
            parsed
        }
        _ => {
            respond(
                stream,
                "400 Bad Request",
                "application/json",
                &json!({"error": "send a non-empty message"})
                    .to_string()
                    .into_bytes(),
            )?;
            return Ok(false);
        }
    };
    let output = Command::new(executable)
        .arg("say")
        .arg(parsed.message)
        .env(
            "COMMUNITY_STATE",
            session_state_directory.join(format!("{}.json", parsed.session_id)),
        )
        .env("COMMUNITY_WEB_REPLY", "1")
        .env(
            "COMMUNITY_DEBUG_THINKING",
            if parsed.show_thinking { "1" } else { "0" },
        )
        .stdin(Stdio::null())
        .output();
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            respond(
                stream,
                "500 Internal Server Error",
                "application/json; charset=utf-8",
                &json!({"error": format!("could not start reply process: {error}")})
                    .to_string()
                    .into_bytes(),
            )?;
            return Ok(false);
        }
    };
    if output.status.success() {
        let result: serde_json::Value =
            match serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                Ok(result) if result["reply"].as_str().is_some() => result,
                _ => {
                    respond(
                        stream,
                        "500 Internal Server Error",
                        "application/json; charset=utf-8",
                        br#"{"error":"reply process returned an invalid response"}"#,
                    )?;
                    return Ok(true);
                }
            };
        respond(
            stream,
            "200 OK",
            "application/json; charset=utf-8",
            &json!({
                "reply": result["reply"],
                "thinking": result["thinking"],
                "new_conversation": result["new_conversation"],
                "research_queued": result["research_queued"]
            })
            .to_string()
            .into_bytes(),
        )?;
    } else {
        let reason = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        respond(
            stream,
            "500 Internal Server Error",
            "application/json; charset=utf-8",
            &json!({"error": if reason.is_empty() { "reply process failed" } else { &reason }})
                .to_string()
                .into_bytes(),
        )?;
    }
    Ok(true)
}

fn valid_session_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn read_request(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..read]);
        if request.len() > MAX_REQUEST_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request exceeds 1 MiB",
            ));
        }
        if let Some(headers_end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&request[..headers_end]);
            let content_length = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            let expected = headers_end + 4 + content_length;
            if expected > MAX_REQUEST_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "request exceeds 1 MiB",
                ));
            }
            if request.len() >= expected {
                request.truncate(expected);
                return Ok(request);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "incomplete HTTP request",
    ))
}

fn respond(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::{handle_request, read_request, serve, valid_session_id};
    use crate::storage::{AssistantClaim, ClaimJudgment, LearningStore};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};

    #[test]
    fn reads_a_json_post_body() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let sender = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            let body = br#"{"message":"hello","session_id":"0123456789abcdef0123456789abcdef"}"#;
            write!(
                stream,
                "POST /api/chat HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
        });
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_request(&mut stream).unwrap();
        assert!(
            request.ends_with(
                br#"{"message":"hello","session_id":"0123456789abcdef0123456789abcdef"}"#
            )
        );
        let parsed: super::ChatRequest = serde_json::from_slice(
            br#"{"message":"hello","session_id":"0123456789abcdef0123456789abcdef"}"#,
        )
        .unwrap();
        assert!(!parsed.show_thinking);
        let parsed: super::ChatRequest =
            serde_json::from_slice(br#"{"message":"hello","session_id":"0123456789abcdef0123456789abcdef","show_thinking":true}"#).unwrap();
        assert!(parsed.show_thinking);
        sender.join().unwrap();
    }

    #[test]
    fn serves_the_chat_page_on_a_loopback_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handle_request(
                &mut stream,
                std::path::Path::new("unused"),
                address,
                std::path::Path::new("/tmp"),
                std::path::Path::new("/tmp/unused-learning.db"),
            )
            .unwrap();
        });
        let mut client = TcpStream::connect(address).unwrap();
        write!(client, "GET / HTTP/1.1\r\nHost: {address}\r\n\r\n").unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("<title>Community Lab</title>"));
        assert!(response.contains("Show model thinking (debug)"));
        server.join().unwrap();
    }

    #[test]
    fn ready_endpoint_reports_latest_challenge_for_browser_session() {
        let unique = format!(
            "community-web-audit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let directory = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&directory).unwrap();
        let session_id = "0123456789abcdef0123456789abcdef";
        let state_path = directory.join(format!("{session_id}.json"));
        let conversation_id = format!("{}::conversation:changed", state_path.to_string_lossy());
        std::fs::write(
            &state_path,
            serde_json::json!({"conversation_id": conversation_id}).to_string(),
        )
        .unwrap();
        let db_path = directory.join("learning.db");
        let mut store = LearningStore::open(&db_path).unwrap();
        store
            .stage_assistant_reply(&conversation_id, 1, "A cricket bat is 400m long.")
            .unwrap();
        let job = store.claim_assistant_reply().unwrap().unwrap();
        store
            .finish_assistant_reply(
                &job.id,
                &[ClaimJudgment {
                    claim: AssistantClaim {
                        subject: "cricket bat".into(),
                        relation: "length".into(),
                        object: "400m".into(),
                        evidence_text: "A cricket bat is 400m long.".into(),
                        extraction_confidence: 0.8,
                    },
                    verdict: "challenged".into(),
                    challenge_type: Some("magnitude_inconsistency".into()),
                    reason: "Conflicts with known bat length".into(),
                    evidence_refs: vec!["json:1".into()],
                }],
            )
            .unwrap();
        drop(store);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let directory_for_server = directory.clone();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handle_request(
                &mut stream,
                std::path::Path::new("unused"),
                address,
                &directory_for_server,
                &directory_for_server.join("learning.db"),
            )
            .unwrap();
        });
        let mut client = TcpStream::connect(address).unwrap();
        write!(
            client,
            "GET /api/ready?session_id={session_id} HTTP/1.1\r\nHost: {address}\r\n\r\n"
        )
        .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert!(response.contains("\"ready\":true"));
        assert!(response.contains("cricket bat"));
        assert!(response.contains("json:1"));
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn refuses_non_loopback_bind_addresses() {
        let error = serve("0.0.0.0:0".parse().unwrap(), "/unused.db").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn browser_session_ids_are_safe_path_components() {
        assert!(valid_session_id("0123456789abcdef0123456789ABCDEF"));
        assert!(!valid_session_id("../../agent-state"));
        assert!(!valid_session_id("short"));
    }
}
