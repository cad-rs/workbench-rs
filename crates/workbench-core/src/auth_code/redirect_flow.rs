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
enum Phase {
    #[default]
    Idle,
    Waiting(Login),
    Exchanging,
    Exchanged(Result<(String, AccountUser), String>),
}

#[derive(Default)]
pub(super) struct RedirectFlow {
    phase: Phase,
    session: Option<(String, AccountUser)>,
}

impl RedirectFlow {
    pub(super) fn begin(
        &mut self,
        redirect_uri: String,
        config: &AuthCodeConfig,
    ) -> Result<DeviceAuthRequest, String> {
        let login = Login::new(redirect_uri)?;
        let request = login.sign_in_request(config);
        self.phase = Phase::Waiting(login);
        Ok(request)
    }

    /// The state of the sign-in still waiting for its redirect.
    pub(super) fn waiting_for(&self) -> Option<&str> {
        match &self.phase {
            Phase::Waiting(login) => Some(&login.state),
            _ => None,
        }
    }

    /// One poll; `callback` is the redirect's query, if it has arrived. Any
    /// phase not put back here is over, which leaves the flow idle.
    pub(super) fn advance(
        &mut self,
        config: &AuthCodeConfig,
        callback: Option<&str>,
    ) -> Result<Step, String> {
        let pending = Ok(Step::Answer(PollResult::Pending));
        match std::mem::take(&mut self.phase) {
            Phase::Idle => Err("Sign-in has not started".to_string()),
            Phase::Exchanging => {
                self.phase = Phase::Exchanging;
                pending
            }
            Phase::Exchanged(outcome) => {
                let (token, user) = outcome?;
                self.session = Some((token, user.clone()));
                Ok(Step::Answer(PollResult::Success(user)))
            }
            Phase::Waiting(login) => match callback.map(|q| read_callback(q, &login.state)) {
                None | Some(Callback::Forged) => {
                    self.phase = Phase::Waiting(login);
                    pending
                }
                Some(Callback::Over(answer)) => answer.map(Step::Answer),
                Some(Callback::Code(code)) => {
                    let form = form_urlencoded::Serializer::new(String::new())
                        .extend_pairs(login.token_form(config, &code))
                        .finish();
                    self.phase = Phase::Exchanging;
                    Ok(Step::Exchange(form))
                }
            },
        }
    }

    /// The answer to [`Step::Exchange`]. One from an attempt given up since
    /// finds the flow doing something else and is dropped.
    pub(super) fn exchanged(&mut self, outcome: Result<(String, AccountUser), String>) {
        if matches!(self.phase, Phase::Exchanging) {
            self.phase = Phase::Exchanged(outcome);
        }
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
    fn the_exchange_of_an_abandoned_attempt_is_dropped() {
        let config = config();
        let (mut flow, first) = started(&config);
        let query = format!("code=old&state={}", first["state"]);
        assert!(matches!(flow.advance(&config, Some(&query)), Ok(Step::Exchange(_))));

        flow.begin(REDIRECT.to_string(), &config).unwrap();
        flow.exchanged(Ok(("old".to_string(), user())));
        assert_eq!(flow.advance(&config, None), pending());
        assert!(flow.session().is_none());
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
