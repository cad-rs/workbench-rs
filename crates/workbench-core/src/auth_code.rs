//! OAuth 2.0 sign-in in the system browser: authorization code + PKCE
//! ([RFC 7636]) with a loopback redirect ([RFC 8252] §7.3). Desktop only.
//!
//! Every endpoint comes from [`AuthCodeConfig`], so the client needs no
//! discovery document. Nothing is persisted: the session lasts until the app
//! closes.

use std::io::{BufRead, BufReader, ErrorKind, Read as _, Write as _};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use workbench_api::{AccountService, AccountUser, DeviceAuthRequest, PollResult};

use crate::account::{HttpTransport, user_from_claims};

/// Where and how to sign in.
#[derive(Clone, Debug)]
pub struct AuthCodeConfig {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    /// Where the user's identity comes from; None reads it from the token response.
    pub userinfo_endpoint: Option<String>,
    pub client_id: String,
    pub scope: String,
    /// The loopback port of the redirect; 0 takes any free one.
    pub redirect_port: u16,
    pub redirect_path: String,
    /// How long to wait for the user to finish in the browser.
    pub timeout_secs: u64,
}

impl AuthCodeConfig {
    pub fn new(
        authorization_endpoint: impl Into<String>,
        token_endpoint: impl Into<String>,
        client_id: impl Into<String>,
    ) -> Self {
        Self {
            authorization_endpoint: authorization_endpoint.into(),
            token_endpoint: token_endpoint.into(),
            userinfo_endpoint: None,
            client_id: client_id.into(),
            scope: "openid profile email".to_string(),
            redirect_port: 0,
            redirect_path: "/callback".to_string(),
            timeout_secs: 300,
        }
    }
}

/// A sign-in waiting for the browser to come back.
struct PendingLogin {
    listener: TcpListener,
    redirect_uri: String,
    state: String,
    verifier: String,
    deadline: Instant,
}

pub struct AuthCodeClient<T: HttpTransport> {
    config: AuthCodeConfig,
    transport: T,
    pending: Mutex<Option<PendingLogin>>,
    /// The access token and the signed-in user.
    session: Mutex<Option<(String, AccountUser)>>,
}

impl<T: HttpTransport> AuthCodeClient<T> {
    pub fn new(config: AuthCodeConfig, transport: T) -> Self {
        Self { config, transport, pending: Mutex::new(None), session: Mutex::new(None) }
    }

    /// The token for calling the product's own services while signed in.
    pub fn access_token(&self) -> Option<String> {
        lock(&self.session).as_ref().map(|(token, _)| token.clone())
    }

    fn authorization_url(&self, redirect_uri: &str, state: &str, verifier: &str) -> String {
        let config = &self.config;
        let query = form_urlencoded::Serializer::new(String::new())
            .append_pair("response_type", "code")
            .append_pair("client_id", &config.client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("scope", &config.scope)
            .append_pair("state", state)
            .append_pair("code_challenge", &code_challenge(verifier))
            .append_pair("code_challenge_method", "S256")
            .finish();
        let separator = if config.authorization_endpoint.contains('?') { '&' } else { '?' };
        format!("{}{separator}{query}", config.authorization_endpoint)
    }

    /// Trade the authorization code for a token and the user it belongs to.
    fn exchange(&self, code: &str, login: &PendingLogin) -> Result<AccountUser, String> {
        let body = self.transport.post_form(
            &self.config.token_endpoint,
            &[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", &login.redirect_uri),
                ("client_id", &self.config.client_id),
                ("code_verifier", &login.verifier),
            ],
        )?;
        let Some(token) = body.get("access_token").and_then(Value::as_str) else {
            let error = body.get("error").and_then(Value::as_str).unwrap_or("no access_token");
            return Err(format!("The token endpoint refused the code: {error}"));
        };
        let user = match &self.config.userinfo_endpoint {
            Some(url) => user_from_claims(&self.transport.get_json(url, Some(token))?)?,
            None => user_from_claims(&body)?,
        };
        *lock(&self.session) = Some((token.to_string(), user.clone()));
        Ok(user)
    }
}

impl<T: HttpTransport> AccountService for AuthCodeClient<T> {
    fn begin_login(&self) -> Result<DeviceAuthRequest, String> {
        // Let go of a fixed port an abandoned sign-in still holds.
        *lock(&self.pending) = None;
        let config = &self.config;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, config.redirect_port))
            .and_then(|listener| listener.set_nonblocking(true).map(|()| listener))
            .map_err(|e| format!("Could not listen for the browser: {e}"))?;
        let port = listener
            .local_addr()
            .map_err(|e| format!("Could not listen for the browser: {e}"))?
            .port();
        let login = PendingLogin {
            listener,
            redirect_uri: format!("http://127.0.0.1:{port}{}", config.redirect_path),
            state: random_token()?,
            verifier: random_token()?,
            deadline: Instant::now() + Duration::from_secs(config.timeout_secs),
        };
        let url = self.authorization_url(&login.redirect_uri, &login.state, &login.verifier);
        *lock(&self.pending) = Some(login);
        Ok(DeviceAuthRequest {
            user_code: None,
            verification_uri: url,
            verification_uri_complete: None,
            expires_in: config.timeout_secs,
            interval: 1,
        })
    }

    fn poll(&self) -> Result<PollResult, String> {
        let mut pending = lock(&self.pending);
        let login = pending.as_ref().ok_or("Sign-in has not started")?;
        if Instant::now() >= login.deadline {
            *pending = None;
            return Ok(PollResult::Expired);
        }
        let mut stream = match login.listener.accept() {
            Ok((stream, _)) => stream,
            Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(PollResult::Pending),
            Err(e) => return Err(format!("Could not read the browser's reply: {e}")),
        };
        let Some(query) = read_callback_query(&mut stream, &self.config.redirect_path) else {
            respond(&mut stream, "404 Not Found", "Not found.");
            return Ok(PollResult::Pending);
        };
        let param = |name: &str| {
            form_urlencoded::parse(query.as_bytes())
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
        };
        // Anyone on this machine can reach the port: only the browser we sent knows the state.
        if param("state").as_deref() != Some(login.state.as_str()) {
            respond(&mut stream, "400 Bad Request", "This sign-in request is not valid.");
            return Ok(PollResult::Pending);
        }
        let login = pending.take().expect("checked above");
        drop(pending);

        if let Some(error) = param("error") {
            respond(&mut stream, "200 OK", "Sign-in did not complete. You can close this tab.");
            return match error.as_str() {
                "access_denied" => Ok(PollResult::Denied),
                _ => Err(format!("The sign-in page returned an error: {error}")),
            };
        }
        let exchanged = param("code")
            .ok_or_else(|| "The sign-in page returned no code".to_string())
            .and_then(|code| self.exchange(&code, &login));
        match exchanged {
            Ok(user) => {
                respond(&mut stream, "200 OK", "Signed in. You can close this tab.");
                Ok(PollResult::Success(user))
            }
            Err(e) => {
                respond(&mut stream, "200 OK", "Sign-in failed. Return to the app and try again.");
                Err(e)
            }
        }
    }

    fn logout(&self) {
        *lock(&self.pending) = None;
        *lock(&self.session) = None;
    }

    fn current_user(&self) -> Option<AccountUser> {
        lock(&self.session).as_ref().map(|(_, user)| user.clone())
    }
}

fn lock<V>(mutex: &Mutex<V>) -> MutexGuard<'_, V> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

/// 256 random bits, URL-safe.
fn random_token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| format!("No secure randomness available: {e}"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn code_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// The query string of a `GET` for `path`; None for any other request. The
/// whole header is read, because closing a socket with unread data can reset
/// the connection before the browser sees the response.
fn read_callback_query(stream: &mut TcpStream, path: &str) -> Option<String> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut reader = BufReader::new(stream.take(16 * 1024));
    let mut request_line = String::new();
    reader.read_line(&mut request_line).ok()?;
    let mut line = String::new();
    while matches!(reader.read_line(&mut line), Ok(n) if n > 0) && line.trim_end() != "" {
        line.clear();
    }
    let target = request_line.strip_prefix("GET ")?.split_whitespace().next()?;
    let (target_path, query) = target.split_once('?')?;
    (target_path == path).then(|| query.to_string())
}

/// `message` is always fixed text: nothing from the request reaches the page.
fn respond(stream: &mut TcpStream, status: &str, message: &str) {
    let body = format!("<!doctype html><meta charset=utf-8><title>Sign-in</title><p>{message}</p>");
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 7636 Appendix B.
    #[test]
    fn the_challenge_matches_the_rfc_example() {
        assert_eq!(
            code_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn random_tokens_are_long_enough_for_pkce_and_differ() {
        let first = random_token().unwrap();
        assert!((43..=128).contains(&first.len()));
        assert_ne!(first, random_token().unwrap());
    }
}
