//! Loopback-only browser interface. The CLI handles each reply in a short
//! process, then a second short process curates staged observations.

use std::fs;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;

use crate::maintenance;
use crate::storage::LearningStore;

const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_IMMEDIATE_MAINTENANCE_PASSES: usize = 3;
const PAGE: &str = include_str!("../web/index.html");
static SHUTDOWN: AtomicBool = AtomicBool::new(false);
static SIGNAL_HANDLER: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();

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
    let registration = SIGNAL_HANDLER.get_or_init(|| {
        ctrlc::set_handler(|| SHUTDOWN.store(true, Ordering::SeqCst))
            .map_err(|error| error.to_string())
    });
    if let Err(error) = registration {
        return Err(io::Error::other(format!(
            "could not install shutdown handler: {error}"
        )));
    }
    SHUTDOWN.store(false, Ordering::SeqCst);
    let listener = TcpListener::bind(address)?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    println!("Community web UI: http://{address}");
    let executable = std::env::current_exe()?;
    let session_state_directory = Path::new(database_path)
        .parent()
        .unwrap_or_else(|| Path::new("state"))
        .join("web-sessions");
    fs::create_dir_all(&session_state_directory)?;
    let mut curator: Option<Child> = None;
    let mut productive_passes = 0;
    // Scan durable queues once at startup, then recover on the periodic tick.
    let mut pending_maintenance = true;
    let interval = maintenance::interval();
    let mut next_tick = Instant::now() + interval;

    while !SHUTDOWN.load(Ordering::SeqCst) {
        if let Some(child) = curator.as_mut() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let mut output = Vec::new();
                    if let Some(mut stdout) = child.stdout.take() {
                        stdout.read_to_end(&mut output)?;
                    }
                    if !status.success() {
                        eprintln!("background maintenance process exited with {status}");
                        let store = LearningStore::open(Path::new(database_path))?;
                        store.mark_maintenance_worker_interrupted(child.id())?;
                        productive_passes = 0;
                    } else if maintenance_summary_did_work(&output)
                        && productive_passes < MAX_IMMEDIATE_MAINTENANCE_PASSES
                    {
                        productive_passes += 1;
                        wake_maintenance(&mut pending_maintenance);
                    } else {
                        productive_passes = 0;
                    }
                    curator = None;
                    next_tick = Instant::now() + interval;
                }
                Ok(None) => {}
                Err(error) => {
                    eprintln!("could not poll background maintenance process: {error}");
                    let _ = child.kill();
                    let _ = child.wait();
                    let store = LearningStore::open(Path::new(database_path))?;
                    store.mark_maintenance_worker_interrupted(child.id())?;
                    curator = None;
                    next_tick = Instant::now() + interval;
                }
            }
        }

        if tick_due(Instant::now(), &mut next_tick, interval) {
            wake_maintenance(&mut pending_maintenance);
        }
        if pending_maintenance && curator.is_none() {
            match spawn_maintenance(&executable, database_path) {
                Ok(child) => curator = Some(child),
                Err(error) => eprintln!("could not start scheduled maintenance: {error}"),
            }
            pending_maintenance = false;
        }

        let (mut stream, _) = match listener.accept() {
            Ok(stream) => stream,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
                continue;
            }
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
                wake_maintenance(&mut pending_maintenance);
                next_tick = Instant::now() + interval;
            }
            Ok(_) => {}
            Err(error) => eprintln!("could not handle web request: {error}"),
        }
    }
    if let Some(mut child) = curator {
        let status = child.wait()?;
        if !status.success() {
            let store = LearningStore::open(Path::new(database_path))?;
            store.mark_maintenance_worker_interrupted(child.id())?;
        }
    }
    Ok(())
}

fn tick_due(now: Instant, next_tick: &mut Instant, interval: Duration) -> bool {
    if now < *next_tick {
        return false;
    }
    *next_tick = now + interval;
    true
}

fn wake_maintenance(pending: &mut bool) {
    *pending = true;
}

fn spawn_maintenance(executable: &Path, database_path: &str) -> io::Result<Child> {
    Command::new(executable)
        .arg("curate-once")
        .env("COMMUNITY_DB", database_path)
        .env("COMMUNITY_MAINTENANCE_MODE", "serve")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
}

fn maintenance_summary_did_work(output: &[u8]) -> bool {
    output
        .split(|byte| *byte == b'\n')
        .rev()
        .find_map(|line| serde_json::from_slice::<maintenance::PassSummary>(line).ok())
        .is_some_and(|summary| summary.did_work())
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
    if method == b"GET" && (path == b"/api/status" || path == b"/status") {
        let store = LearningStore::open(database_path)?;
        let status = store.maintenance_status()?;
        respond(
            stream,
            "200 OK",
            "application/json; charset=utf-8",
            &serde_json::to_vec(&status)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
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
    use super::{
        handle_request, maintenance_summary_did_work, read_request, serve, tick_due,
        valid_session_id, wake_maintenance,
    };
    use crate::storage::{AssistantClaim, ClaimJudgment, LearningStore};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};

    #[test]
    fn browser_maintenance_tick_fires_without_chat_activity() {
        let now = std::time::Instant::now();
        let interval = std::time::Duration::from_millis(10);
        let mut next = now + interval;
        assert!(!tick_due(now, &mut next, interval));
        assert!(tick_due(now + interval, &mut next, interval));
        assert!(!tick_due(now + interval, &mut next, interval));
    }

    #[test]
    fn repeated_chat_events_coalesce_until_maintenance_starts() {
        let mut pending = false;
        wake_maintenance(&mut pending);
        wake_maintenance(&mut pending);
        assert!(pending);
        pending = false;
        assert!(!pending);
    }

    #[test]
    fn productive_maintenance_pass_requests_one_bounded_follow_up() {
        assert!(maintenance_summary_did_work(
            br#"{"curated":1,"audited":0,"scouted":0,"researched":0,"errors":0}"#
        ));
        assert!(!maintenance_summary_did_work(
            br#"{"curated":0,"audited":0,"scouted":0,"researched":0,"errors":0}"#
        ));
        assert!(!maintenance_summary_did_work(
            b"worker failed before writing a summary"
        ));
    }

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
    fn status_endpoint_distinguishes_no_pass_from_completed_idle_pass() {
        let directory = std::env::temp_dir().join(format!(
            "community-web-status-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let database_path = directory.join("learning.db");
        let mut store = LearningStore::open(&database_path).unwrap();
        let status_before = store.maintenance_status().unwrap();
        assert!(status_before.last_pass.is_none());
        let pass_id = store.start_maintenance_pass("serve").unwrap();
        for role in ["curator", "curmudgeon", "scout", "scholar"] {
            store
                .record_maintenance_role(pass_id, role, "idle", 0, None, 0)
                .unwrap();
        }
        store.finish_maintenance_pass(pass_id).unwrap();
        drop(store);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn({
            let database_path = database_path.clone();
            move || {
                let (mut stream, _) = listener.accept().unwrap();
                handle_request(
                    &mut stream,
                    std::path::Path::new("unused"),
                    address,
                    std::path::Path::new("/tmp"),
                    &database_path,
                )
                .unwrap();
            }
        });
        let mut client = TcpStream::connect(address).unwrap();
        write!(
            client,
            "GET /api/status HTTP/1.1\r\nHost: {address}\r\n\r\n"
        )
        .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert!(response.contains("\"status\":\"completed\""));
        assert!(response.contains("\"role\":\"scout\""));
        assert!(response.contains("\"pending\":0"));
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(directory);
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
