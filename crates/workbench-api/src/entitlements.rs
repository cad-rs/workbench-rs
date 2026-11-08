//! Which features the current user may use.
//!
//! How a product sells itself is the product's business, so the framework has
//! no tiers, plans or license formats. A feature is a name the product
//! chooses; a product that limits features injects an [`Entitlements`]
//! (`WorkbenchAppBuilder::with_entitlements`) that answers from a license
//! file, an account's plan, a dongle or anything else. Without one, everything
//! is allowed.
//!
//! The platform applies the answer to commands (`CommandDef::requires`) and
//! plugins (`requires` in the manifest); domain code can ask directly through
//! `ServiceHost::allows`.

use std::sync::Arc;

/// The product's answer to what the current user may use.
pub trait Entitlements: Send + Sync {
    /// Whether the user may use `feature`. Asked on the UI thread, as often as
    /// every frame while the ribbon is drawn, so answer from memory. The answer
    /// may change while the app runs (an activation, an expiry); plugins are
    /// only checked as they load.
    fn allows(&self, feature: &str) -> bool;

    /// What to tell the user when `feature` is refused.
    fn refusal(&self, feature: &str) -> String {
        format!("{feature} is not available with your license")
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
