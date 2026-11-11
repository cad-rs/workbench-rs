//! The browser sign-in flow on its own: no IdP and no backend. A scripted
//! transport stands in for the token and userinfo endpoints, and raw sockets
//! stand in for the browser coming back to the loopback redirect.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use workbench_api::{AccountService, PollResult};
use workbench_core::account::HttpTransport;
use workbench_core::auth_code::{AuthCodeClient, AuthCodeConfig};

const AUTHORIZE: &str = "https://idp.test/authorize";
const TOKEN: &str = "https://idp.test/token";
const USERINFO: &str = "https://idp.test/userinfo";

/// Answers like an IdP and records what it was sent.
#[derive(Default)]
struct ScriptedIdp {
    token_forms: Mutex<Vec<HashMap<String, String>>>,
    bearers: Mutex<Vec<String>>,
    /// Replaces the normal token response.
    token_reply: Mutex<Option<Value>>,
}

impl HttpTransport for &'static ScriptedIdp {
    fn get_json(&self, url: &str, bearer: Option<&str>) -> Result<Value, String> {
        assert_eq!(url, USERINFO);
        self.bearers.lock().unwrap().push(bearer.unwrap_or_default().to_string());
        Ok(json!({"sub": "user-1", "name": "Ada", "email": "ada@example.test"}))
    }

    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<Value, String> {
        assert_eq!(url, TOKEN);
        let form = form.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        self.token_forms.lock().unwrap().push(form);
        Ok(self
            .token_reply
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| json!({"access_token": "tok-1", "token_type": "Bearer"})))
    }
}

fn config() -> AuthCodeConfig {
    AuthCodeConfig {
        userinfo_endpoint: Some(USERINFO.to_string()),
        ..AuthCodeConfig::new(AUTHORIZE, TOKEN, "client-1")
    }
}

fn client_with(config: AuthCodeConfig) -> (AuthCodeClient<&'static ScriptedIdp>, &'static ScriptedIdp) {
    let idp: &'static ScriptedIdp = Box::leak(Box::default());
    (AuthCodeClient::new(config, idp), idp)
}

fn client() -> (AuthCodeClient<&'static ScriptedIdp>, &'static ScriptedIdp) {
    client_with(config())
}

/// The sign-in page's URL, with what the client put in its query.
fn begin(client: &AuthCodeClient<&'static ScriptedIdp>) -> HashMap<String, String> {
    let request = client.begin_login().expect("begin_login");
    assert_eq!(request.user_code, None, "there is nothing for the user to type");
    let (_, query) = request.verification_uri.split_once('?').expect("a query");
    form_urlencoded::parse(query.as_bytes()).into_owned().collect()
}

/// What a browser does when the sign-in page redirects: `query` is appended to
/// the redirect URI. Returns the raw response once the client has answered.
fn browser_returns(redirect_uri: &str, path_and_query: String) -> JoinHandle<String> {
    let authority = redirect_uri.strip_prefix("http://").unwrap().split('/').next().unwrap().to_string();
    std::thread::spawn(move || {
        let mut stream = TcpStream::connect(&authority).unwrap();
        write!(stream, "GET {path_and_query} HTTP/1.1\r\nHost: {authority}\r\nAccept: */*\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    })
}

fn callback(redirect_uri: &str, query: &str) -> JoinHandle<String> {
    let path = format!("/{}", redirect_uri.strip_prefix("http://").unwrap().split_once('/').unwrap().1);
    browser_returns(redirect_uri, format!("{path}?{query}"))
}

/// Polls until something other than Pending, or until the browser thread has
/// its answer (a request that leaves the flow waiting).
fn poll_until(
    client: &AuthCodeClient<&'static ScriptedIdp>,
    browser: JoinHandle<String>,
) -> (Result<PollResult, String>, String) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(Instant::now() < deadline, "the browser never got an answer");
        let result = client.poll();
        if result != Ok(PollResult::Pending) || browser.is_finished() {
            return (result, browser.join().unwrap());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn signing_in_exchanges_the_code_with_the_verifier_behind_the_challenge() {
    let (client, idp) = client();
    let params = begin(&client);
    assert_eq!(params["response_type"], "code");
    assert_eq!(params["client_id"], "client-1");
    assert_eq!(params["scope"], "openid profile email");
    assert_eq!(params["code_challenge_method"], "S256");
    let redirect_uri = &params["redirect_uri"];
    assert!(redirect_uri.starts_with("http://127.0.0.1:"), "{redirect_uri}");
    assert!(redirect_uri.ends_with("/callback"), "{redirect_uri}");

    let query = format!("code=abc%2F1&state={}", params["state"]);
    let (result, page) = poll_until(&client, callback(redirect_uri, &query));
    let Ok(PollResult::Success(user)) = result else {
        panic!("expected success, got {result:?}");
    };
    assert_eq!((user.id.as_str(), user.name.as_str()), ("user-1", "Ada"));
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains("Signed in"), "{page}");

    let forms = idp.token_forms.lock().unwrap();
    assert_eq!(forms.len(), 1);
    assert_eq!(forms[0]["grant_type"], "authorization_code");
    assert_eq!(forms[0]["code"], "abc/1", "the code is percent-decoded");
    assert_eq!(forms[0]["client_id"], "client-1");
    assert_eq!(&forms[0]["redirect_uri"], redirect_uri);
    assert_eq!(
        product_kit::auth_code::code_challenge(forms[0]["code_verifier"].as_str()),
        params["code_challenge"]
    );

    assert_eq!(*idp.bearers.lock().unwrap(), ["tok-1"]);
    assert_eq!(client.access_token().as_deref(), Some("tok-1"));
    assert_eq!(client.current_user().map(|u| u.name), Some("Ada".to_string()));
    client.logout();
    assert!(client.current_user().is_none());
    assert!(client.access_token().is_none());
}

#[test]
fn every_sign_in_gets_its_own_state_and_challenge() {
    let (client, _) = client();
    let first = begin(&client);
    let second = begin(&client);
    assert_ne!(first["state"], second["state"]);
    assert_ne!(first["code_challenge"], second["code_challenge"]);
}

#[test]
fn a_callback_with_the_wrong_state_does_not_end_the_wait() {
    let (client, idp) = client();
    let params = begin(&client);
    let redirect_uri = &params["redirect_uri"];

    let (result, page) = poll_until(&client, callback(redirect_uri, "code=stolen&state=forged"));
    assert_eq!(result, Ok(PollResult::Pending));
    assert!(page.starts_with("HTTP/1.1 400"), "{page}");
    assert!(idp.token_forms.lock().unwrap().is_empty(), "the code was never exchanged");

    let (no_state, _) = poll_until(&client, callback(redirect_uri, "code=stolen"));
    assert_eq!(no_state, Ok(PollResult::Pending));

    let query = format!("code=real&state={}", params["state"]);
    let (result, _) = poll_until(&client, callback(redirect_uri, &query));
    assert!(matches!(result, Ok(PollResult::Success(_))), "{result:?}");
}

#[test]
fn other_requests_to_the_port_are_turned_away() {
    let (client, idp) = client();
    let params = begin(&client);
    let redirect_uri = &params["redirect_uri"];
    let attempts = [
        "/favicon.ico?x=1".to_string(),
        "/favicon.ico".to_string(),
        format!("/callback/extra?code=c&state={}", params["state"]),
    ];
    for attempt in attempts {
        let (result, page) = poll_until(&client, browser_returns(redirect_uri, attempt));
        assert_eq!(result, Ok(PollResult::Pending));
        assert!(page.starts_with("HTTP/1.1 404"), "{page}");
    }
    assert!(idp.token_forms.lock().unwrap().is_empty());
}

#[test]
fn the_user_refusing_is_a_denial() {
    let (client, idp) = client();
    let params = begin(&client);
    let query = format!("error=access_denied&state={}", params["state"]);
    let (result, page) = poll_until(&client, callback(&params["redirect_uri"], &query));
    assert_eq!(result, Ok(PollResult::Denied));
    assert!(page.contains("did not complete"), "{page}");
    assert!(idp.token_forms.lock().unwrap().is_empty());
}

#[test]
fn a_refused_code_fails_and_says_so_in_the_browser() {
    let (client, idp) = client();
    *idp.token_reply.lock().unwrap() = Some(json!({"error": "invalid_grant"}));
    let params = begin(&client);
    let query = format!("code=old&state={}", params["state"]);
    let (result, page) = poll_until(&client, callback(&params["redirect_uri"], &query));
    assert_eq!(result, Err("The token endpoint refused the code: invalid_grant".to_string()));
    assert!(page.contains("Sign-in failed"), "{page}");
    assert!(client.current_user().is_none());
}

#[test]
fn nothing_from_the_request_reaches_the_page() {
    let (client, _) = client();
    let params = begin(&client);
    let query = format!("error=%3Cscript%3Ealert(1)%3C%2Fscript%3E&state={}", params["state"]);
    let (result, page) = poll_until(&client, callback(&params["redirect_uri"], &query));
    assert_eq!(
        result,
        Err("The sign-in page returned an error: <script>alert(1)</script>".to_string())
    );
    assert!(!page.contains("script"), "{page}");
}

#[test]
fn the_platform_is_told_how_long_to_wait() {
    let (client, _) = client_with(AuthCodeConfig { timeout_secs: 7, ..config() });
    assert_eq!(client.begin_login().unwrap().expires_in, 7);
}

#[test]
fn polling_before_starting_fails() {
    let (client, _) = client();
    assert!(client.poll().is_err());
}

#[test]
fn signing_out_abandons_a_sign_in_in_progress() {
    let (client, _) = client();
    begin(&client);
    client.logout();
    assert!(client.poll().is_err());
}

/// A free port that this test then asks the client to use.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

#[test]
fn the_redirect_can_be_pinned_for_a_backend_that_registers_it() {
    let port = free_port();
    let (client, _) = client_with(AuthCodeConfig {
        redirect_port: port,
        redirect_path: "/oauth/done".to_string(),
        scope: "read write".to_string(),
        ..config()
    });
    let first = begin(&client);
    assert_eq!(first["redirect_uri"], format!("http://127.0.0.1:{port}/oauth/done"));
    assert_eq!(first["scope"], "read write");

    // Starting over must not trip over the port the last attempt held.
    let params = begin(&client);
    let query = format!("code=c&state={}", params["state"]);
    let (result, _) = poll_until(&client, callback(&params["redirect_uri"], &query));
    assert!(matches!(result, Ok(PollResult::Success(_))), "{result:?}");
}

#[test]
fn a_port_already_taken_is_reported() {
    let holder = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = holder.local_addr().unwrap().port();
    let (client, _) = client_with(AuthCodeConfig { redirect_port: port, ..config() });
    let error = client.begin_login().unwrap_err();
    assert!(error.contains("Could not listen for the browser"), "{error}");
}

#[test]
fn an_endpoint_with_its_own_query_keeps_it() {
    let (client, _) = client_with(AuthCodeConfig {
        authorization_endpoint: format!("{AUTHORIZE}?tenant=acme"),
        ..config()
    });
    let request = client.begin_login().unwrap();
    assert!(request.verification_uri.starts_with(&format!("{AUTHORIZE}?tenant=acme&")));
}

#[test]
fn without_a_userinfo_endpoint_the_token_response_names_the_user() {
    let (client, idp) = client_with(AuthCodeConfig { userinfo_endpoint: None, ..config() });
    *idp.token_reply.lock().unwrap() =
        Some(json!({"access_token": "tok-2", "sub": "user-2", "name": "Grace"}));
    let params = begin(&client);
    let query = format!("code=c&state={}", params["state"]);
    let (result, _) = poll_until(&client, callback(&params["redirect_uri"], &query));
    let Ok(PollResult::Success(user)) = result else {
        panic!("expected success, got {result:?}");
    };
    assert_eq!(user.name, "Grace");
    assert!(idp.bearers.lock().unwrap().is_empty());
}
