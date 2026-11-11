//! The desktop client: the redirect comes back to a port on 127.0.0.1.

use std::io::ErrorKind;
use std::net::{Ipv4Addr, TcpListener};
use std::sync::Mutex;

use product_kit::auth_code::{read_callback, read_callback_query, respond};
use workbench_api::{AccountService, AccountUser, DeviceAuthRequest, PollResult};

use super::{end_to_poll, AuthCodeConfig, Callback, Login, access_token};
use crate::account::{HttpTransport, lock, user_from_claims};

/// A sign-in waiting for the browser to come back.
struct PendingLogin {
    listener: TcpListener,
    login: Login,
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

    /// Trade the authorization code for a token and the user it belongs to.
    fn exchange(&self, code: &str, login: &Login) -> Result<AccountUser, String> {
        let form = login.token_form(&self.config, code);
        let body = self.transport.post_form(&self.config.token_endpoint, &form)?;
        let token = access_token(&body)?;
        let user = match &self.config.userinfo_endpoint {
            Some(url) => user_from_claims(&self.transport.get_json(url, Some(&token))?)?,
            None => user_from_claims(&body)?,
        };
        *lock(&self.session) = Some((token, user.clone()));
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
        let redirect_uri = format!("http://127.0.0.1:{port}{}", config.redirect_path);
        let login = Login::new(redirect_uri)?;
        let request = login.sign_in_request(config);
        *lock(&self.pending) = Some(PendingLogin { listener, login });
        Ok(request)
    }

    fn poll(&self) -> Result<PollResult, String> {
        let mut pending = lock(&self.pending);
        let current = pending.as_ref().ok_or("Sign-in has not started")?;
        let mut stream = match current.listener.accept() {
            Ok((stream, _)) => stream,
            Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(PollResult::Pending),
            Err(e) => return Err(format!("Could not read the browser's reply: {e}")),
        };
        let Some(query) = read_callback_query(&mut stream, &self.config.redirect_path) else {
            respond(&mut stream, "404 Not Found", "Not found.");
            return Ok(PollResult::Pending);
        };
        let code = match read_callback(&query, &current.login.state) {
            Callback::Forged => {
                respond(&mut stream, "400 Bad Request", "This sign-in request is not valid.");
                return Ok(PollResult::Pending);
            }
            Callback::Over(end) => {
                *pending = None;
                respond(&mut stream, "200 OK", "Sign-in did not complete. You can close this tab.");
                return end_to_poll(end);
            }
            Callback::Code(code) => code,
        };
        let login = pending.take().expect("checked above").login;
        drop(pending);

        match self.exchange(&code, &login) {
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
