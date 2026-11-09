//! OAuth 2.0 sign-in in a browser: authorization code + PKCE ([RFC 7636]).
//! The redirect comes back to a loopback port on the desktop
//! ([RFC 8252] §7.3) and to a same-origin callback page on the web.
//!
//! Every endpoint comes from [`AuthCodeConfig`], so the clients need no
//! discovery document. Nothing is persisted: the session lasts until the app
//! or the page closes.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use workbench_api::{DeviceAuthRequest, PollResult};

#[cfg(not(target_arch = "wasm32"))]
mod loopback;
#[cfg(any(target_arch = "wasm32", test))]
mod redirect_flow;
#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(not(target_arch = "wasm32"))]
pub use loopback::AuthCodeClient;
#[cfg(target_arch = "wasm32")]
pub use web::WebAuthCodeClient;

/// Where and how to sign in.
#[derive(Clone, Debug)]
pub struct AuthCodeConfig {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    /// Where the user's identity comes from; None reads it from the token response.
    pub userinfo_endpoint: Option<String>,
    pub client_id: String,
    pub scope: String,
    /// The loopback port of the redirect, desktop only; 0 takes any free one.
    pub redirect_port: u16,
    /// The path of the redirect: on the loopback port on the desktop, on the
    /// page's own origin on the web (where the callback page has to be served).
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
            redirect_path: if cfg!(target_arch = "wasm32") {
                "/auth-callback.html".to_string()
            } else {
                "/callback".to_string()
            },
            timeout_secs: 300,
        }
    }
}

/// One sign-in until the browser comes back.
struct Login {
    redirect_uri: String,
    state: String,
    verifier: String,
}

impl Login {
    fn new(redirect_uri: String) -> Result<Self, String> {
        Ok(Self { redirect_uri, state: random_token()?, verifier: random_token()? })
    }

    /// What the platform opens: the sign-in page, with nothing for the user to type.
    fn sign_in_request(&self, config: &AuthCodeConfig) -> DeviceAuthRequest {
        let query = form_urlencoded::Serializer::new(String::new())
            .append_pair("response_type", "code")
            .append_pair("client_id", &config.client_id)
            .append_pair("redirect_uri", &self.redirect_uri)
            .append_pair("scope", &config.scope)
            .append_pair("state", &self.state)
            .append_pair("code_challenge", &code_challenge(&self.verifier))
            .append_pair("code_challenge_method", "S256")
            .finish();
        let separator = if config.authorization_endpoint.contains('?') { '&' } else { '?' };
        DeviceAuthRequest {
            user_code: None,
            verification_uri: format!("{}{separator}{query}", config.authorization_endpoint),
            expires_in: config.timeout_secs,
            interval: 1,
        }
    }

    fn token_form<'a>(
        &'a self,
        config: &'a AuthCodeConfig,
        code: &'a str,
    ) -> [(&'static str, &'a str); 5] {
        [
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &self.redirect_uri),
            ("client_id", &config.client_id),
            ("code_verifier", &self.verifier),
        ]
    }
}

/// What the query of a redirect says about the sign-in with `state`.
enum Callback {
    /// Not this sign-in's: only the browser it was sent to knows the state.
    Forged,
    /// The sign-in ended at the page; what `poll` answers.
    Over(Result<PollResult, String>),
    Code(String),
}

fn read_callback(query: &str, state: &str) -> Callback {
    let param = |name: &str| {
        form_urlencoded::parse(query.as_bytes())
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    if param("state").as_deref() != Some(state) {
        return Callback::Forged;
    }
    match (param("error"), param("code")) {
        (Some(error), _) if error == "access_denied" => Callback::Over(Ok(PollResult::Denied)),
        (Some(error), _) => {
            Callback::Over(Err(format!("The sign-in page returned an error: {error}")))
        }
        (None, Some(code)) => Callback::Code(code),
        (None, None) => Callback::Over(Err("The sign-in page returned no code".to_string())),
    }
}

fn access_token(token_response: &Value) -> Result<String, String> {
    match token_response.get("access_token").and_then(Value::as_str) {
        Some(token) => Ok(token.to_string()),
        None => {
            let error =
                token_response.get("error").and_then(Value::as_str).unwrap_or("no access_token");
            Err(format!("The token endpoint refused the code: {error}"))
        }
    }
}

/// 256 random bits, URL-safe.
fn random_token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    random_bytes(&mut bytes)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

#[cfg(not(target_arch = "wasm32"))]
fn random_bytes(bytes: &mut [u8]) -> Result<(), String> {
    getrandom::fill(bytes).map_err(|e| format!("No secure randomness available: {e}"))
}

#[cfg(target_arch = "wasm32")]
fn random_bytes(bytes: &mut [u8]) -> Result<(), String> {
    web_sys::window()
        .and_then(|window| window.crypto().ok())
        .and_then(|crypto| crypto.get_random_values_with_u8_array(bytes).ok())
        .map(|_| ())
        .ok_or_else(|| "No secure randomness available".to_string())
}

fn code_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn over(query: &str) -> Result<PollResult, String> {
        match read_callback(query, "s") {
            Callback::Over(answer) => answer,
            _ => panic!("{query} should end the sign-in"),
        }
    }

    #[test]
    fn a_redirect_is_read_only_if_it_carries_this_sign_ins_state() {
        assert!(matches!(read_callback("code=c&state=other", "s"), Callback::Forged));
        assert!(matches!(read_callback("code=c", "s"), Callback::Forged));
        assert!(matches!(read_callback("code=a%2Fb&state=s", "s"), Callback::Code(c) if c == "a/b"));
        assert_eq!(over("error=access_denied&state=s"), Ok(PollResult::Denied));
        assert_eq!(
            over("error=server_error&state=s"),
            Err("The sign-in page returned an error: server_error".to_string())
        );
        assert_eq!(over("state=s"), Err("The sign-in page returned no code".to_string()));
    }

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
