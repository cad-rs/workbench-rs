//! The web client's logic with the browser left out, so it can be tested
//! anywhere: the caller reads the redirect from wherever it arrives and runs
//! the token exchange, which cannot block on the page's only thread.

use workbench_api::{AccountUser, DeviceAuthRequest, PollResult};

use super::{AuthCodeConfig, Callback, Login, read_callback};

/// What the caller should do after [`RedirectFlow::advance`].
#[derive(Debug, PartialEq)]
pub(super) enum Step {
    Answer(PollResult),
    /// POST this form to the token endpoint and report through
    /// [`RedirectFlow::exchanged`].
    Exchange(String),
}

#[derive(Default)]
pub(super) struct RedirectFlow {
    login: Option<Login>,
    exchanging: bool,
    outcome: Option<Result<(String, AccountUser), String>>,
    session: Option<(String, AccountUser)>,
}

impl RedirectFlow {
    pub(super) fn begin(
        &mut self,
        redirect_uri: String,
        config: &AuthCodeConfig,
    ) -> Result<DeviceAuthRequest, String> {
        let login = Login::new(redirect_uri, config)?;
        let request = login.sign_in_request(config);
        self.login = Some(login);
        self.exchanging = false;
        self.outcome = None;
        Ok(request)
    }

    /// The state of the sign-in still waiting for its redirect.
    pub(super) fn waiting_for(&self) -> Option<&str> {
        self.login.as_ref().filter(|_| !self.exchanging).map(|login| login.state.as_str())
    }

    /// One poll; `callback` is the redirect's query, if it has arrived.
    pub(super) fn advance(
        &mut self,
        config: &AuthCodeConfig,
        callback: Option<&str>,
    ) -> Result<Step, String> {
        if let Some(outcome) = self.outcome.take() {
            self.exchanging = false;
            self.login = None;
            let (token, user) = outcome?;
            self.session = Some((token, user.clone()));
            return Ok(Step::Answer(PollResult::Success(user)));
        }
        if self.exchanging {
            return Ok(Step::Answer(PollResult::Pending));
        }
        let login = self.login.as_ref().ok_or("Sign-in has not started")?;
        if login.expired() {
            self.login = None;
            return Ok(Step::Answer(PollResult::Expired));
        }
        let Some(query) = callback else {
            return Ok(Step::Answer(PollResult::Pending));
        };
        match read_callback(query, &login.state) {
            Callback::Forged => Ok(Step::Answer(PollResult::Pending)),
            Callback::Denied => {
                self.login = None;
                Ok(Step::Answer(PollResult::Denied))
            }
            Callback::Failed(error) => {
                self.login = None;
                Err(error)
            }
            Callback::Code(code) => {
                let form = form_urlencoded::Serializer::new(String::new())
                    .extend_pairs(login.token_form(config, &code))
                    .finish();
                self.exchanging = true;
                Ok(Step::Exchange(form))
            }
        }
    }

    pub(super) fn exchanged(&mut self, outcome: Result<(String, AccountUser), String>) {
        self.outcome = Some(outcome);
    }

    pub(super) fn session(&self) -> Option<&(String, AccountUser)> {
        self.session.as_ref()
    }

    pub(super) fn logout(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    const REDIRECT: &str = "https://app.test/auth-callback.html";

    fn config() -> AuthCodeConfig {
        AuthCodeConfig::new("https://idp.test/authorize", "https://idp.test/token", "client-1")
    }

    fn params(query: &str) -> HashMap<String, String> {
        form_urlencoded::parse(query.as_bytes()).into_owned().collect()
    }

    /// A flow waiting for its redirect, and what the sign-in page was told.
    fn started(config: &AuthCodeConfig) -> (RedirectFlow, HashMap<String, String>) {
        let mut flow = RedirectFlow::default();
        let request = flow.begin(REDIRECT.to_string(), config).unwrap();
        assert_eq!(request.user_code, None);
        let query = request.verification_uri.split_once('?').unwrap().1.to_string();
        (flow, params(&query))
    }

    fn user() -> AccountUser {
        AccountUser { id: "u1".into(), name: "Ada".into(), email: None }
    }

    fn pending() -> Result<Step, String> {
        Ok(Step::Answer(PollResult::Pending))
    }

    #[test]
    fn it_waits_until_the_redirect_arrives() {
        let config = config();
        let (mut flow, sent) = started(&config);
        assert_eq!(sent["redirect_uri"], REDIRECT);
        assert_eq!(flow.waiting_for(), Some(sent["state"].as_str()));
        assert_eq!(flow.advance(&config, None), pending());
    }

    #[test]
    fn a_code_is_exchanged_once_and_ends_in_a_session() {
        let config = config();
        let (mut flow, sent) = started(&config);
        let query = format!("code=abc%2F1&state={}", sent["state"]);

        let Step::Exchange(form) = flow.advance(&config, Some(&query)).unwrap() else {
            panic!("expected the exchange to start");
        };
        let form = params(&form);
        assert_eq!(form["grant_type"], "authorization_code");
        assert_eq!(form["code"], "abc/1");
        assert_eq!(form["redirect_uri"], REDIRECT);
        assert_eq!(form["client_id"], "client-1");
        assert_eq!(super::super::code_challenge(&form["code_verifier"]), sent["code_challenge"]);

        assert_eq!(flow.waiting_for(), None, "no need to read the redirect again");
        assert_eq!(flow.advance(&config, None), pending(), "while the exchange runs");

        flow.exchanged(Ok(("tok-1".to_string(), user())));
        assert_eq!(flow.advance(&config, None), Ok(Step::Answer(PollResult::Success(user()))));
        assert_eq!(flow.session().map(|(token, _)| token.as_str()), Some("tok-1"));
        assert!(flow.advance(&config, None).is_err(), "the sign-in is over");
    }

    #[test]
    fn a_failed_exchange_is_reported_and_leaves_no_session() {
        let config = config();
        let (mut flow, sent) = started(&config);
        let query = format!("code=abc&state={}", sent["state"]);
        assert!(matches!(flow.advance(&config, Some(&query)), Ok(Step::Exchange(_))));
        flow.exchanged(Err("refused".to_string()));
        assert_eq!(flow.advance(&config, None), Err("refused".to_string()));
        assert!(flow.session().is_none());
    }

    #[test]
    fn a_redirect_with_the_wrong_state_does_not_end_the_wait() {
        let config = config();
        let (mut flow, sent) = started(&config);
        assert_eq!(flow.advance(&config, Some("code=stolen&state=forged")), pending());
        assert_eq!(flow.advance(&config, Some("code=stolen")), pending());
        let query = format!("code=real&state={}", sent["state"]);
        assert!(matches!(flow.advance(&config, Some(&query)), Ok(Step::Exchange(_))));
    }

    #[test]
    fn the_user_refusing_is_a_denial() {
        let config = config();
        let (mut flow, sent) = started(&config);
        let query = format!("error=access_denied&state={}", sent["state"]);
        assert_eq!(flow.advance(&config, Some(&query)), Ok(Step::Answer(PollResult::Denied)));
        assert_eq!(flow.waiting_for(), None);
    }

    #[test]
    fn another_error_from_the_sign_in_page_is_reported() {
        let config = config();
        let (mut flow, sent) = started(&config);
        let query = format!("error=server_error&state={}", sent["state"]);
        assert_eq!(
            flow.advance(&config, Some(&query)),
            Err("The sign-in page returned an error: server_error".to_string())
        );
    }

    #[test]
    fn a_redirect_without_a_code_is_reported() {
        let config = config();
        let (mut flow, sent) = started(&config);
        let query = format!("state={}", sent["state"]);
        assert!(flow.advance(&config, Some(&query)).is_err());
    }

    #[test]
    fn waiting_too_long_expires() {
        let config = AuthCodeConfig { timeout_secs: 0, ..config() };
        let (mut flow, _) = started(&config);
        assert_eq!(flow.advance(&config, None), Ok(Step::Answer(PollResult::Expired)));
        assert!(flow.advance(&config, None).is_err());
    }

    #[test]
    fn polling_before_starting_fails() {
        assert!(RedirectFlow::default().advance(&config(), None).is_err());
    }

    #[test]
    fn starting_over_forgets_the_last_attempt() {
        let config = config();
        let (mut flow, first) = started(&config);
        let query = format!("code=old&state={}", first["state"]);
        assert!(matches!(flow.advance(&config, Some(&query)), Ok(Step::Exchange(_))));
        flow.exchanged(Ok(("old".to_string(), user())));

        flow.begin(REDIRECT.to_string(), &config).unwrap();
        assert_eq!(flow.advance(&config, None), pending(), "the old answer is gone");
        assert_eq!(flow.advance(&config, Some(&query)), pending(), "and so is its state");
    }

    #[test]
    fn signing_out_clears_everything() {
        let config = config();
        let (mut flow, sent) = started(&config);
        let query = format!("code=abc&state={}", sent["state"]);
        assert!(matches!(flow.advance(&config, Some(&query)), Ok(Step::Exchange(_))));
        flow.exchanged(Ok(("tok".to_string(), user())));
        flow.advance(&config, None).unwrap();
        assert!(flow.session().is_some());

        flow.logout();
        assert!(flow.session().is_none());
        assert!(flow.advance(&config, None).is_err());
    }
}
