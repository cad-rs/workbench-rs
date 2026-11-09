//! The web client: the sign-in page opens in another tab and redirects to a
//! same-origin callback page, so the app's own page, with any unsaved work,
//! stays where it is. The callback page leaves the redirect in `localStorage`
//! for [`AccountService::poll`] to find.
//!
//! The token and userinfo endpoints have to allow this page's origin (CORS).

use std::sync::{Arc, Mutex};

use serde_json::Value;
use wasm_bindgen::JsValue;
use web_sys::{Headers, RequestInit, Response};
use workbench_api::{AccountService, AccountUser, DeviceAuthRequest, PollResult};

use super::redirect_flow::{RedirectFlow, Step};
use super::{AuthCodeConfig, access_token, lock};
use crate::account::user_from_claims;
use crate::js::{js_error, resolve};

/// `auth-callback.html` stores the redirect's query under this plus its
/// `state`, which makes the entry this sign-in's alone.
const CALLBACK_KEY: &str = "workbench.sign_in.";

pub struct WebAuthCodeClient {
    config: AuthCodeConfig,
    flow: Arc<Mutex<RedirectFlow>>,
}

impl WebAuthCodeClient {
    pub fn new(config: AuthCodeConfig) -> Self {
        Self { config, flow: Arc::default() }
    }

    /// The token for calling the product's own services while signed in.
    pub fn access_token(&self) -> Option<String> {
        lock(&self.flow).session().map(|(token, _)| token.clone())
    }
}

impl AccountService for WebAuthCodeClient {
    fn begin_login(&self) -> Result<DeviceAuthRequest, String> {
        let origin = web_sys::window()
            .and_then(|window| window.location().origin().ok())
            .ok_or("No page to sign in from")?;
        let redirect_uri = format!("{origin}{}", self.config.redirect_path);
        lock(&self.flow).begin(redirect_uri, &self.config)
    }

    fn poll(&self) -> Result<PollResult, String> {
        let mut flow = lock(&self.flow);
        let callback = flow.waiting_for().and_then(take_callback);
        match flow.advance(&self.config, callback.as_deref())? {
            Step::Answer(answer) => Ok(answer),
            Step::Exchange(form) => {
                let config = self.config.clone();
                let flow = self.flow.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    let outcome = exchange(&config, &form).await;
                    lock(&flow).exchanged(outcome);
                });
                Ok(PollResult::Pending)
            }
        }
    }

    fn logout(&self) {
        lock(&self.flow).logout();
    }

    fn current_user(&self) -> Option<AccountUser> {
        lock(&self.flow).session().map(|(_, user)| user.clone())
    }
}

fn take_callback(state: &str) -> Option<String> {
    let storage = web_sys::window()?.local_storage().ok()??;
    let key = format!("{CALLBACK_KEY}{state}");
    let query = storage.get_item(&key).ok()??;
    let _ = storage.remove_item(&key);
    Some(query)
}

/// Trade the authorization code for a token and the user it belongs to.
async fn exchange(config: &AuthCodeConfig, form: &str) -> Result<(String, AccountUser), String> {
    let headers = Headers::new().map_err(js_error)?;
    headers.set("Content-Type", "application/x-www-form-urlencoded").map_err(js_error)?;
    let init = RequestInit::new();
    init.set_method("POST");
    init.set_headers(&headers);
    init.set_body(&JsValue::from_str(form));
    let body = fetch_json(&config.token_endpoint, &init).await?;
    let token = access_token(&body)?;

    let user = match &config.userinfo_endpoint {
        Some(url) => {
            let headers = Headers::new().map_err(js_error)?;
            headers.set("Authorization", &format!("Bearer {token}")).map_err(js_error)?;
            let init = RequestInit::new();
            init.set_headers(&headers);
            user_from_claims(&fetch_json(url, &init).await?)?
        }
        None => user_from_claims(&body)?,
    };
    Ok((token, user))
}

/// The parsed body of any response, error statuses included: a refused code
/// comes back as a JSON `error` with a 400.
async fn fetch_json(url: &str, init: &RequestInit) -> Result<Value, String> {
    let window = web_sys::window().ok_or("No page to sign in from")?;
    let response: Response = resolve(window.fetch_with_str_and_init(url, init))
        .await
        .map_err(|e| format!("Could not reach {url}: {e}"))?;
    let text: JsValue = resolve(response.text().map_err(js_error)?).await?;
    serde_json::from_str(&text.as_string().unwrap_or_default())
        .map_err(|e| format!("The response from {url} is not JSON: {e}"))
}
