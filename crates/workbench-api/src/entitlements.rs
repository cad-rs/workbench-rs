//! Which features the current user may use.
//!
//! How a product sells itself is the product's business, so the framework has
//! no tiers, plans or license formats. A feature is a name the product
//! chooses; a product that limits features injects an [`Entitlements`]
//! (`WorkbenchAppBuilder::with_entitlements`) that answers from a license
//! file, an account's plan, a dongle or anything else. Without one, everything
//! is allowed.
//!
//! The platform applies the answer to commands, views and panels (`requires`
//! on their definitions) and plugins (`requires` in the manifest); domain code
//! can ask directly through `ServiceHost::allows`.
//!
//! A plan can change while the app runs, on the product's website for one. The
//! product says how to fetch the latest answers ([`Entitlements::refresh`]);
//! the platform decides when.

use std::sync::Arc;

use crate::asyncrt::BoxFuture;

/// The product's answer to what the current user may use.
pub trait Entitlements: Send + Sync {
    /// Whether the user may use `feature`. Asked on the UI thread many times a
    /// second, so answer from memory. The answer may change while the app
    /// runs: for the features named by `requires`, a feature gained applies
    /// within a tick. A feature lost stays usable until the app closes, and
    /// the next launch closes what is no longer allowed. That grace does not
    /// cover signing out, or a plan the check at startup finds has lapsed.
    /// Plugins are only checked as they load.
    fn allows(&self, feature: &str) -> bool;

    /// What to tell the user when `feature` is refused.
    fn refusal(&self, feature: &str) -> String {
        format!("{feature} is not available with your license")
    }

    /// Fetch the latest answers and update what [`Self::allows`] reads. The
    /// platform calls this at startup, when the window comes back to the
    /// foreground, after sign-in and sign-out, every half hour and from Help >
    /// Refresh License, one call at a time and off the UI thread.
    ///
    /// It must not block: on the desktop wrap a blocking network call in
    /// `blocking::unblock`, and give up after a timeout, since no other call
    /// starts while one is pending. On an error leave the answers as they are,
    /// so a lost connection takes nothing away; the user only hears about
    /// errors when they asked for the refresh.
    fn refresh(self: Arc<Self>) -> BoxFuture<Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
}

/// Why `entitlements` refuses `feature`; None when it is allowed.
pub(crate) fn refusal(
    entitlements: Option<&Arc<dyn Entitlements>>,
    feature: &str,
) -> Option<String> {
    let entitlements = entitlements?;
    (!entitlements.allows(feature)).then(|| entitlements.refusal(feature))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Only(&'static str);

    impl Entitlements for Only {
        fn allows(&self, feature: &str) -> bool {
            feature == self.0
        }
    }

    #[test]
    fn with_nothing_injected_everything_is_allowed() {
        assert_eq!(refusal(None, "anything"), None);
    }

    #[test]
    fn the_product_decides() {
        let only: Arc<dyn Entitlements> = Arc::new(Only("export"));
        assert_eq!(refusal(Some(&only), "export"), None);
        assert_eq!(
            refusal(Some(&only), "import").as_deref(),
            Some("import is not available with your license")
        );
    }
}
