//! OAuth 2.0 device-flow client ([RFC 8628]) — the platform's reference
//! [`AccountService`](api::AccountService) implementation (Phase 3).
//!
//! IdP-agnostic by construction: all deployment-specific input is an issuer
//! URL and a client id. Endpoints come from OIDC discovery
//! (`{issuer}/.well-known/openid-configuration`); tokens are cached in
//! memory for the session. Every call performs blocking network I/O — the
//! platform invokes them from background threads, never the UI thread.
//!
//! The HTTP layer is injectable ([`HttpTransport`]): desktop uses
//! [`UreqTransport`]; tests drive the client with a scripted transport or a
//! local mock server, with no external IdP involved.

use std::sync::{Mutex, MutexGuard};

use product_kit::oauth::{
    classify_poll_error, device_authorization_form, token_poll_form, PollError,
};
use serde_json::Value;
use workbench_api as api;
use api::{AccountService, AccountUser, DeviceAuthRequest, PollResult};

/// Borrow an owned form for the transport.
fn pairs(form: &[(String, String)]) -> Vec<(&str, &str)> {
    form.iter().map(|(key, value)| (key.as_str(), value.as_str())).collect()
}

/// One HTTP round trip: JSON in, JSON out. Status codes are surfaced in the
/// body handling — a `400` with an `error` member is a normal RFC 8628
/// answer (`authorization_pending` / `slow_down` / `access_denied` /
/// `expired_token`), not a transport failure.
pub trait HttpTransport: Send + Sync {
    /// GET `url` with an optional Bearer token; returns the parsed JSON body.
    fn get_json(&self, url: &str, bearer: Option<&str>) -> Result<Value, String>;
    /// POST `url` as `application/x-www-form-urlencoded`; returns the parsed
    /// JSON body (including 4xx error bodies).
    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<Value, String>;
}

/// A lock that survives a panic elsewhere: everything it guards stays valid.
pub(crate) fn lock<V>(mutex: &Mutex<V>) -> MutexGuard<'_, V> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

/// Device-flow client over any [`HttpTransport`].
pub struct DeviceFlowClient<T: HttpTransport> {
    issuer: String,
    client_id: String,
    transport: T,
    /// Cached session (access token + user) between login and logout.
    session: Mutex<Option<(String, AccountUser)>>,
    /// The device code minted by `begin_login`, remembered for `poll`
    /// ([RFC 8628]: the token endpoint wants the device code, never the
    /// user-visible user code).
    device_code: Mutex<Option<String>>,
}

impl<T: HttpTransport> DeviceFlowClient<T> {
    pub fn new(issuer: impl Into<String>, client_id: impl Into<String>, transport: T) -> Self {
        Self {
            // Trailing slash breaks the well-known join; normalize.
            issuer: issuer.into().trim_end_matches('/').to_string(),
            client_id: client_id.into(),
            transport,
            session: Mutex::new(None),
            device_code: Mutex::new(None),
        }
    }

    fn endpoints(&self) -> Result<DeviceEndpoints, String> {
        let url = format!("{}/.well-known/openid-configuration", self.issuer);
        let doc = self.transport.get_json(&url, None)?;
        DeviceEndpoints::from_discovery(&doc)
    }
}

/// The user an OIDC claims object describes.
pub(crate) fn user_from_claims(claims: &Value) -> Result<AccountUser, String> {
    let id = claims
        .get("sub")
        .and_then(Value::as_str)
        .ok_or("The userinfo response is missing sub")?
        .to_string();
    let name = claims
        .get("name")
        .or_else(|| claims.get("preferred_username"))
        .and_then(Value::as_str)
        .unwrap_or(&id)
        .to_string();
    let email = claims
        .get("email")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok(AccountUser { id, name, email })
}

/// OIDC endpoints the device flow needs, from the discovery document.
struct DeviceEndpoints {
    device_authorization: String,
    token: String,
    userinfo: Option<String>,
}

impl DeviceEndpoints {
    fn from_discovery(doc: &Value) -> Result<Self, String> {
        let get = |key: &str| {
            doc.get(key)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("The discovery document is missing {key}"))
        };
        Ok(Self {
            device_authorization: get("device_authorization_endpoint")?,
            token: get("token_endpoint")?,
            userinfo: doc
                .get("userinfo_endpoint")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }
}

impl<T: HttpTransport> AccountService for DeviceFlowClient<T> {
    fn begin_login(&self) -> Result<DeviceAuthRequest, String> {
        let eps = self.endpoints()?;
        let form = device_authorization_form(&self.client_id, &[], None);
        let body = self.transport.post_form(&eps.device_authorization, &pairs(&form))?;
        let str_field = |key: &str| {
            body.get(key)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("The device_authorization response is missing {key}"))
        };
        let device_code = str_field("device_code")?;
        *lock(&self.device_code) = Some(device_code);
        Ok(DeviceAuthRequest {
            user_code: Some(str_field("user_code")?),
            verification_uri: str_field("verification_uri")?,
            expires_in: body.get("expires_in").and_then(Value::as_u64).unwrap_or(600),
            interval: body.get("interval").and_then(Value::as_u64).unwrap_or(5),
        })
    }

    fn poll(&self) -> Result<PollResult, String> {
        let device_code =
            lock(&self.device_code).clone().ok_or("Sign-in has not started (no device code)")?;
        let eps = self.endpoints()?;
        let form = token_poll_form(&device_code, &self.client_id);
        let body = self.transport.post_form(&eps.token, &pairs(&form))?;

        if let Some(access_token) = body.get("access_token").and_then(Value::as_str) {
            // Success: fetch identity from userinfo when available, else
            // fall back to the token response's own claims.
            let user = match &eps.userinfo {
                Some(url) => {
                    let claims = self.transport.get_json(url, Some(access_token))?;
                    user_from_claims(&claims)?
                }
                None => user_from_claims(&body)?,
            };
            *lock(&self.session) = Some((access_token.to_string(), user.clone()));
            return Ok(PollResult::Success(user));
        }

        // RFC 8628 §3.5 error responses ride on HTTP 400.
        match body.get("error").and_then(Value::as_str) {
            Some(error) => match classify_poll_error(error) {
                PollError::Pending => Ok(PollResult::Pending),
                PollError::SlowDown => Ok(PollResult::SlowDown),
                PollError::Denied => Ok(PollResult::Denied),
                PollError::Expired => Ok(PollResult::Expired),
                PollError::Unknown(other) => {
                    Err(format!("The token endpoint returned an unknown error: {other}"))
                }
            },
            None => Err("The token response has neither access_token nor error".to_string()),
        }
    }

    fn logout(&self) {
        *lock(&self.session) = None;
    }

    fn current_user(&self) -> Option<AccountUser> {
        lock(&self.session).as_ref().map(|(_, user)| user.clone())
    }
}

// ---- Desktop transport (ureq) ----

/// Blocking ureq-backed transport for desktop builds.
#[cfg(not(target_arch = "wasm32"))]
pub struct UreqTransport {
    agent: ureq::Agent,
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for UreqTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl UreqTransport {
    pub fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(15)))
            // The device flow *expects* 400 bodies (authorization_pending &
            // friends): keep them readable instead of ureq's StatusCode error.
            .http_status_as_error(false)
            .build()
            .new_agent();
        Self { agent }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl HttpTransport for UreqTransport {
    fn get_json(&self, url: &str, bearer: Option<&str>) -> Result<Value, String> {
        let mut req = self.agent.get(url);
        if let Some(token) = bearer {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        read_json(req.call())
    }

    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<Value, String> {
        read_json(self.agent.post(url).send_form(form.iter().copied()))
    }
}

/// Read a JSON body from a ureq response (any status: the caller interprets
/// RFC 8628 error members in 400 bodies).
#[cfg(not(target_arch = "wasm32"))]
fn read_json(
    result: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<Value, String> {
    let res = result.map_err(|e| format!("HTTP request failed: {e}"))?;
    let text = res
        .into_body()
        .read_to_string()
        .map_err(|e| format!("Failed to read the response: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("The response is not JSON: {e}; body: {text}"))
}
