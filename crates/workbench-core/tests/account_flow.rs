//! Device-flow integration tests against a local mock OIDC server
//! (RFC 8628): no external IdP, no cloud account — the server is a std TCP
//! listener speaking just enough HTTP for the four endpoints the client
//! touches.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use workbench_api::{AccountService, PollResult};
use workbench_core::account::{DeviceFlowClient, UreqTransport};

/// Mock IdP state shared with the test and the server threads.
struct MockState {
    /// Token polls served so far: the first N return `authorization_pending`,
    /// after which the flow is "approved" (or denied, per `deny`).
    polls: AtomicUsize,
    pending_polls: usize,
    deny: std::sync::atomic::AtomicBool,
    /// Emulate `slow_down` on the first token poll.
    slow_down_once: std::sync::atomic::AtomicBool,
    /// The issuer URL this mock answers as (for discovery/URI building).
    issuer: String,
}

impl MockState {
    fn new(pending_polls: usize, issuer: String) -> Self {
        Self {
            polls: AtomicUsize::new(0),
            pending_polls,
            deny: std::sync::atomic::AtomicBool::new(false),
            slow_down_once: std::sync::atomic::AtomicBool::new(false),
            issuer,
        }
    }
}

/// Serve one HTTP/1.1 request on `stream` (connection-per-request; ureq is
/// fine with `Connection: close`).
fn serve_one(mut stream: TcpStream, state: &MockState) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();

    // Headers: only content-length matters for the POST bodies.
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body).ok();
    }
    let body = String::from_utf8_lossy(&body).to_string();

    let (status, response) = route(&method, &path, &body, state);
    let reason = if status == 200 { "OK" } else { "Bad Request" };
    let reply = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
        response.len()
    );
    let _ = stream.write_all(reply.as_bytes());
}

fn route(method: &str, path: &str, body: &str, state: &MockState) -> (u16, String) {
    let issuer = &state.issuer;
    match (method, path) {
        ("GET", "/.well-known/openid-configuration") => (
            200,
            format!(
                r#"{{"device_authorization_endpoint":"{issuer}/device-authorize","token_endpoint":"{issuer}/token","userinfo_endpoint":"{issuer}/userinfo"}}"#
            ),
        ),
        ("POST", "/device-authorize") => {
            let device_code = format!("dev-{}", state.polls.load(Ordering::Relaxed));
            (
                200,
                format!(
                    r#"{{"device_code":"{device_code}","user_code":"ABCD-EFGH","verification_uri":"{issuer}/activate","expires_in":600,"interval":1}}"#
                ),
            )
        }
        ("POST", "/token") => {
            // RFC 8628: the token endpoint wants the *device* code.
            if !body.contains("device_code=dev-") || body.contains("user_code=") {
                return (400, r#"{"error":"invalid_grant"}"#.to_string());
            }
            let poll = state.polls.fetch_add(1, Ordering::Relaxed);
            if poll == 0 && state.slow_down_once.load(Ordering::Relaxed) {
                return (400, r#"{"error":"slow_down"}"#.to_string());
            }
            if poll < state.pending_polls {
                return (
                    400,
                    r#"{"error":"authorization_pending"}"#.to_string(),
                );
            }
            if state.deny.load(Ordering::Relaxed) {
                return (400, r#"{"error":"access_denied"}"#.to_string());
            }
            (
                200,
                r#"{"access_token":"tok-123","token_type":"Bearer","expires_in":3600}"#.to_string(),
            )
        }
        ("GET", "/userinfo") => (
            200,
            r#"{"sub":"user-1","name":"Mock User","email":"mock@example.test"}"#.to_string(),
        ),
        _ => (404, r#"{"error":"not_found"}"#.to_string()),
    }
}

#[test]
fn device_flow_success_after_pending_polls() {
    let mock = spawn_mock(2);
    let client = DeviceFlowClient::new(&mock.issuer, "test-client", UreqTransport::new());
    let req = client.begin_login().expect("begin_login");
    assert_eq!(req.user_code.as_deref(), Some("ABCD-EFGH"));
    assert!(req.verification_uri.contains("/activate"));
    assert!(req.interval >= 1);

    // Polls 1-2: pending (server-side pending_polls = 2).
    assert_eq!(client.poll().expect("poll 1"), PollResult::Pending);
    assert_eq!(client.poll().expect("poll 2"), PollResult::Pending);
    // Poll 3: approved; identity comes from userinfo.
    match client.poll().expect("poll 3") {
        PollResult::Success(user) => {
            assert_eq!(user.id, "user-1");
            assert_eq!(user.name, "Mock User");
            assert_eq!(user.email.as_deref(), Some("mock@example.test"));
        }
        other => panic!("expected Success, got {other:?}"),
    }
    assert_eq!(
        client.current_user().map(|u| u.name),
        Some("Mock User".into())
    );

    // Logout clears the cached session.
    client.logout();
    assert!(client.current_user().is_none());
}

/// Mock harness: bound listener + state in one bundle.
struct MockIdP {
    issuer: String,
    state: Arc<MockState>,
}

fn spawn_mock(pending_polls: usize) -> MockIdP {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let issuer = format!("http://{}", listener.local_addr().unwrap());
    let state = Arc::new(MockState::new(pending_polls, issuer.clone()));
    std::thread::spawn({
        let state = state.clone();
        move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let state = state.clone();
                std::thread::spawn(move || serve_one(stream, &state));
            }
        }
    });
    MockIdP { issuer, state }
}

#[test]
fn device_flow_denied() {
    let mock = spawn_mock(0);
    mock.state.deny.store(true, Ordering::Relaxed);
    let client = DeviceFlowClient::new(&mock.issuer, "test-client", UreqTransport::new());
    client.begin_login().expect("begin_login");
    assert_eq!(client.poll().expect("poll"), PollResult::Denied);
}

#[test]
fn device_flow_slow_down_then_success() {
    let mock = spawn_mock(0);
    mock.state.slow_down_once.store(true, Ordering::Relaxed);
    let client = DeviceFlowClient::new(&mock.issuer, "test-client", UreqTransport::new());
    client.begin_login().expect("begin_login");
    assert_eq!(client.poll().expect("poll 1"), PollResult::SlowDown);
    assert!(matches!(client.poll().expect("poll 2"), PollResult::Success(_)));
}

#[test]
fn poll_without_begin_fails() {
    let mock = spawn_mock(0);
    let client = DeviceFlowClient::new(&mock.issuer, "test-client", UreqTransport::new());
    assert!(client.poll().is_err(), "no device code yet");
}

#[test]
fn token_endpoint_rejects_user_code() {
    // The mock's token endpoint refuses `user_code=` in the form: a client
    // that sent the user-visible code instead of the device code fails.
    let mock = spawn_mock(0);
    let client = DeviceFlowClient::new(&mock.issuer, "test-client", UreqTransport::new());
    client.begin_login().expect("begin_login");
    // Reach past the client and post a user_code directly via transport.
    use workbench_core::account::HttpTransport;
    let transport = UreqTransport::new();
    let body = transport
        .post_form(
            &format!("{}/token", mock.issuer),
            &[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", "ABCD-EFGH"),
                ("client_id", "x"),
            ],
        )
        .expect("400 body decodes");
    assert_eq!(body.get("error").and_then(|v| v.as_str()), Some("invalid_grant"));
}
