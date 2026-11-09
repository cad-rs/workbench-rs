//! Account contract (Phase 3): user identity behind a GUI-agnostic service.
//!
//! The platform owns the login *state machine* ([`crate::workspace::AccountPhase`]
//! on the workspace, driven by `app.account.*` commands); this crate owns the
//! *capability* contract. Products inject an implementation via
//! `WorkbenchAppBuilder::with_account` — the framework binds to no IdP. The
//! reference implementation is the OAuth2 device-flow client in workbench-core
//! ([RFC 8628], issuer/client-id configured per deployment).

/// A logged-in user (from the IdP's userinfo endpoint).
#[derive(Debug, Clone, PartialEq)]
pub struct AccountUser {
    /// Stable subject id (IdP `sub`).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Email address, when the IdP shares one.
    pub email: Option<String>,
}

/// What the user does in a browser to sign in: the device-authorization
/// response ([RFC 8628] §3.2), or a sign-in page with nothing to type.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceAuthRequest {
    /// Short code the user types at the verification URI (e.g. `ABCD-EFGH`).
    /// None: there is nothing to type, so the platform opens the URI itself.
    pub user_code: Option<String>,
    /// Where to enter it.
    pub verification_uri: String,
    /// Seconds the platform waits for the user before giving up.
    pub expires_in: u64,
    /// Minimum seconds between token polls (server-provided).
    pub interval: u64,
}

/// One token-endpoint poll ([RFC 8628] §3.5).
#[derive(Debug, Clone, PartialEq)]
pub enum PollResult {
    /// User has not approved yet; poll again after the interval.
    Pending,
    /// Polling too fast ([RFC 8628] `slow_down`): pending, and the client
    /// must add `SLOW_DOWN_INTERVAL_SECS` to its polling interval.
    SlowDown,
    /// The user approved; the access token is usable.
    Success(AccountUser),
    /// The user denied the request.
    Denied,
    /// The device/user code expired.
    Expired,
}

/// RFC 8628: seconds to add to the polling interval on `slow_down`.
pub const SLOW_DOWN_INTERVAL_SECS: u64 = 5;

/// The account capability a product may inject. On the desktop the platform
/// invokes these from a background thread, so they may block on the network.
/// On the web they run on the page's only thread and must not block.
pub trait AccountService: Send + Sync {
    /// Start signing in. The returned request is what the user acts on; the
    /// service remembers what it needs for [`AccountService::poll`].
    fn begin_login(&self) -> Result<DeviceAuthRequest, String>;

    /// One check on the flow started by [`AccountService::begin_login`].
    /// Pure network: no sleeping, no retry loops — pacing belongs to the
    /// caller.
    fn poll(&self) -> Result<PollResult, String>;

    /// End the session (clears cached credentials; best-effort server-side
    /// revocation when the IdP exposes a revocation endpoint).
    fn logout(&self);

    /// The cached logged-in user, if any (session restore).
    fn current_user(&self) -> Option<AccountUser>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slow_down_constant_matches_rfc() {
        assert_eq!(SLOW_DOWN_INTERVAL_SECS, 5);
    }
}
