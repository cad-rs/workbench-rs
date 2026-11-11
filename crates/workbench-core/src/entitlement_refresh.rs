//! When to ask the product for the latest entitlements, and what a change
//! means for the session already running.
//!
//! The schedule and the mid-session grace live in `product_kit`, shared with
//! the nanobot harness. This module keeps the workbench-facing wrapper: it
//! answers the [`Entitlements`] question through the grace and delegates the
//! product-specific questions (the refusal text, the refresh future).

use std::sync::Arc;

use product_kit::{EntitlementSource, SessionGrace};
use workbench_api::{BoxFuture, Entitlements};

/// The product's answers, wrapped so [`EntitlementSource`] can be
/// implemented: the trait is product-kit's and `dyn Entitlements` belongs to
/// workbench-api, so the orphan rule needs a local type in between.
struct GraceProduct(Arc<dyn Entitlements>);

impl EntitlementSource for GraceProduct {
    type Answer = bool;

    fn check(&self, feature: &str) -> bool {
        self.0.allows(feature)
    }
}

/// The product's answers, except that what the user could use stays usable
/// until the app closes: a plan lapsing mid-session must not close the view
/// they are working in. The next launch starts from the product's answers.
pub(crate) struct SessionEntitlements {
    grace: SessionGrace<GraceProduct>,
}

impl SessionEntitlements {
    pub(crate) fn new(product: Arc<dyn Entitlements>) -> Self {
        Self {
            grace: SessionGrace::new(Arc::new(GraceProduct(product))),
        }
    }

    /// The latest answers are in; from here on what is allowed is kept.
    pub(crate) fn settle(&self) {
        self.grace.settle();
    }

    /// Someone else is signed in: nothing carries over, and their answers
    /// are not known until the next check finishes.
    pub(crate) fn forget(&self) {
        self.grace.forget();
    }
}

impl Entitlements for SessionEntitlements {
    fn allows(&self, feature: &str) -> bool {
        self.grace.check(feature)
    }

    fn refusal(&self, feature: &str) -> String {
        self.grace.product().0.refusal(feature)
    }

    fn refresh(self: Arc<Self>) -> BoxFuture<Result<(), String>> {
        self.grace.product().0.clone().refresh()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Product {
        allows: bool,
        refusal: &'static str,
        refreshes: std::sync::atomic::AtomicUsize,
    }

    impl Entitlements for Product {
        fn allows(&self, _feature: &str) -> bool {
            self.allows
        }

        fn refusal(&self, feature: &str) -> String {
            format!("{}: {}", self.refusal, feature)
        }

        fn refresh(self: Arc<Self>) -> BoxFuture<Result<(), String>> {
            self.refreshes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }
    }

    fn session(product: Arc<Product>) -> Arc<SessionEntitlements> {
        Arc::new(SessionEntitlements::new(product))
    }

    #[test]
    fn refusal_and_refresh_reach_the_product() {
        let product = Arc::new(Product {
            allows: false,
            refusal: "needs Pro",
            refreshes: Default::default(),
        });
        let session = session(product.clone());
        assert_eq!(session.refusal("export"), "needs Pro: export");
        let _ = futures_lite::future::block_on(session.clone().refresh()).unwrap();
        assert_eq!(product.refreshes.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn the_grace_comes_from_product_kit() {
        let product = Arc::new(Product {
            allows: true,
            refusal: "no",
            refreshes: Default::default(),
        });
        let session = session(product);
        session.settle();
        assert!(session.allows("export"));
        session.forget();
        assert!(session.allows("export"), "the product still allows it");
    }
}
