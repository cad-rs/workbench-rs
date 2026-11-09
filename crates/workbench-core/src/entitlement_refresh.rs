//! When to ask the product for the latest entitlements, and what a change
//! means for the session already running.
//!
//! The schedule only decides; `AppRuntime` runs the checks. Time comes in
//! through [`RefreshSchedule::tick`], so the rules can be tested without
//! waiting.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use workbench_api::{BoxFuture, Entitlements};

/// Routine checks catch expiries and changes made elsewhere.
const ROUTINE_EVERY: f32 = 30.0 * 60.0;
/// Coming back to the window checks at most this often.
const ACTIVATION_GAP: f32 = 30.0;
/// How long after a purchase page opens a return to the window counts as
/// coming back from it.
const PURCHASE_WAIT: f32 = 30.0 * 60.0;
/// Back from a purchase page, the payment may not have reached the product's
/// server yet, so check this often...
const POLL_EVERY: f32 = 4.0;
/// ...for this long.
const POLL_FOR: f32 = 120.0;

/// The product's answers, except that what the user could use stays usable
/// until the app closes: a plan lapsing mid-session must not close the view
/// they are working in. The next launch starts from the product's answers.
pub(crate) struct SessionEntitlements {
    product: Arc<dyn Entitlements>,
    kept: Mutex<Kept>,
}

#[derive(Default)]
struct Kept {
    /// False until a check has finished: the product may start from a cached
    /// plan that the first check finds has lapsed.
    settled: bool,
    features: HashSet<String>,
}

impl SessionEntitlements {
    pub(crate) fn new(product: Arc<dyn Entitlements>) -> Self {
        Self { product, kept: Mutex::default() }
    }

    fn kept(&self) -> std::sync::MutexGuard<'_, Kept> {
        self.kept.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The latest answers are in; from here on what is allowed is kept.
    pub(crate) fn settle(&self) {
        self.kept().settled = true;
    }

    /// Someone else is signed in: nothing carries over, and their answers
    /// are not known until the next check finishes.
    pub(crate) fn forget(&self) {
        *self.kept() = Kept::default();
    }
}

impl Entitlements for SessionEntitlements {
    fn allows(&self, feature: &str) -> bool {
        let mut kept = self.kept();
        if kept.features.contains(feature) {
            return true;
        }
        let allowed = self.product.allows(feature);
        if allowed && kept.settled {
            kept.features.insert(feature.to_string());
        }
        allowed
    }

    fn refusal(&self, feature: &str) -> String {
        self.product.refusal(feature)
    }

    fn refresh(self: Arc<Self>) -> BoxFuture<Result<(), String>> {
        self.product.clone().refresh()
    }
}

/// Each method that returns `bool` answers whether a check should start now.
/// Only one runs at a time.
#[derive(Default)]
pub(crate) struct RefreshSchedule {
    running: bool,
    /// Asked for during a running check, which may not have seen what changed.
    queued: bool,
    /// The user asked, so the last check of this round reports its outcome.
    report: bool,
    /// None until the first check, which is due at once.
    since_start: Option<f32>,
    purchase_wait: f32,
    polling: f32,
}

impl RefreshSchedule {
    fn elapsed(&self) -> f32 {
        self.since_start.unwrap_or(f32::INFINITY)
    }

    fn start(&mut self) -> bool {
        if self.running {
            return false;
        }
        self.running = true;
        self.since_start = Some(0.0);
        true
    }

    pub(crate) fn tick(&mut self, dt: f32) -> bool {
        let dt = dt.max(0.0);
        self.since_start = self.since_start.map(|elapsed| elapsed + dt);
        self.purchase_wait = (self.purchase_wait - dt).max(0.0);
        let poll_due = self.polling > 0.0 && self.elapsed() >= POLL_EVERY;
        self.polling = (self.polling - dt).max(0.0);
        (poll_due || self.elapsed() >= ROUTINE_EVERY) && self.start()
    }

    /// The window came back to the foreground.
    pub(crate) fn activated(&mut self) -> bool {
        if self.purchase_wait > 0.0 {
            self.polling = POLL_FOR;
            return self.start();
        }
        self.elapsed() >= ACTIVATION_GAP && self.start()
    }

    /// Something the answers depend on changed, such as who is signed in.
    pub(crate) fn asked(&mut self) -> bool {
        self.queued = self.running;
        self.start()
    }

    pub(crate) fn asked_by_user(&mut self) -> bool {
        self.report = true;
        self.asked()
    }

    pub(crate) fn purchase_page_opened(&mut self) {
        self.purchase_wait = PURCHASE_WAIT;
    }

    /// What the user was waiting for arrived.
    pub(crate) fn answers_changed(&mut self) {
        self.purchase_wait = 0.0;
        self.polling = 0.0;
    }

    /// The running check ended.
    pub(crate) fn finished(&mut self) -> bool {
        self.running = false;
        std::mem::take(&mut self.queued) && self.start()
    }

    /// Whether the check that just ended should tell the user how it went.
    /// Call after [`Self::finished`]: a queued check reports instead.
    pub(crate) fn take_report(&mut self) -> bool {
        !self.running && std::mem::take(&mut self.report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A schedule whose startup check has come and gone.
    fn started() -> RefreshSchedule {
        let mut schedule = RefreshSchedule::default();
        assert!(schedule.tick(0.0), "the first tick checks");
        assert!(!schedule.finished());
        schedule
    }

    /// Ticks one second at a time; how many checks started (each ends at once).
    fn run_for(schedule: &mut RefreshSchedule, seconds: u32) -> u32 {
        let mut checks = 0;
        for _ in 0..seconds {
            if schedule.tick(1.0) {
                checks += 1;
                schedule.finished();
            }
        }
        checks
    }

    #[test]
    fn routine_checks_are_half_an_hour_apart() {
        let mut schedule = started();
        assert_eq!(run_for(&mut schedule, 1799), 0);
        assert_eq!(run_for(&mut schedule, 1), 1);
    }

    #[test]
    fn only_one_check_runs_at_a_time() {
        let mut schedule = RefreshSchedule::default();
        assert!(schedule.tick(0.0));
        assert!(!schedule.tick(ROUTINE_EVERY), "the first is still running");
        assert!(!schedule.activated());
        schedule.finished();
        assert!(schedule.tick(ROUTINE_EVERY));
    }

    #[test]
    fn coming_back_to_the_window_checks_at_most_every_half_minute() {
        let mut schedule = started();
        assert!(!schedule.activated(), "just checked");
        run_for(&mut schedule, 30);
        assert!(schedule.activated());
        schedule.finished();
        assert!(!schedule.activated());
    }

    #[test]
    fn back_from_a_purchase_page_it_keeps_checking_for_two_minutes() {
        let mut schedule = started();
        schedule.purchase_page_opened();
        assert_eq!(run_for(&mut schedule, 60), 0, "nothing while the user is away");
        assert!(schedule.activated(), "a return is not held back by the gap");
        schedule.finished();
        assert_eq!(run_for(&mut schedule, 8), 2);
        assert_eq!(run_for(&mut schedule, 112), 28);
        assert_eq!(run_for(&mut schedule, 60), 0, "it gives up");
    }

    #[test]
    fn a_change_in_the_answers_ends_the_wait() {
        let mut schedule = started();
        schedule.purchase_page_opened();
        assert!(schedule.activated());
        schedule.finished();
        schedule.answers_changed();
        assert_eq!(run_for(&mut schedule, 60), 0);
        run_for(&mut schedule, 1);
        assert!(!schedule.purchase_wait_is_on());
    }

    #[test]
    fn a_later_return_within_the_wait_checks_again() {
        let mut schedule = started();
        schedule.purchase_page_opened();
        assert!(schedule.activated());
        schedule.finished();
        run_for(&mut schedule, 180);
        assert!(schedule.activated(), "they went back and finished paying");
    }

    #[test]
    fn the_wait_for_a_purchase_runs_out() {
        let mut schedule = started();
        schedule.purchase_page_opened();
        run_for(&mut schedule, 1800);
        assert!(!schedule.purchase_wait_is_on());
    }

    #[test]
    fn a_change_during_a_check_gets_a_check_of_its_own() {
        let mut schedule = RefreshSchedule::default();
        assert!(schedule.tick(0.0));
        assert!(!schedule.asked(), "one is running");
        assert!(schedule.finished(), "and another follows it");
        assert!(!schedule.finished(), "only one");
    }

    #[test]
    fn the_outcome_is_reported_once_by_the_last_check() {
        let mut schedule = RefreshSchedule::default();
        assert!(schedule.tick(0.0));
        assert!(!schedule.asked_by_user());
        assert!(schedule.finished());
        assert!(!schedule.take_report(), "the queued check saw the latest state");
        assert!(!schedule.finished());
        assert!(schedule.take_report());
        assert!(!schedule.take_report());
    }

    #[derive(Default)]
    struct Product(Mutex<Vec<&'static str>>);

    impl Product {
        fn sells(&self, features: &[&'static str]) {
            *self.0.lock().unwrap() = features.to_vec();
        }
    }

    impl Entitlements for Product {
        fn allows(&self, feature: &str) -> bool {
            self.0.lock().unwrap().contains(&feature)
        }
    }

    fn session(features: &[&'static str]) -> (Arc<Product>, SessionEntitlements) {
        let product = Arc::new(Product::default());
        product.sells(features);
        (product.clone(), SessionEntitlements::new(product))
    }

    #[test]
    fn until_a_check_finishes_the_product_decides() {
        let (product, session) = session(&["pro"]);
        assert!(session.allows("pro"));
        product.sells(&[]);
        assert!(!session.allows("pro"), "the cached plan had lapsed");
    }

    #[test]
    fn once_settled_a_lost_feature_stays() {
        let (product, session) = session(&["pro"]);
        session.settle();
        assert!(session.allows("pro"));
        product.sells(&[]);
        assert!(session.allows("pro"));
    }

    #[test]
    fn a_feature_gained_later_applies_and_stays() {
        let (product, session) = session(&[]);
        session.settle();
        assert!(!session.allows("pro"));
        product.sells(&["pro"]);
        assert!(session.allows("pro"));
        product.sells(&[]);
        assert!(session.allows("pro"));
    }

    #[test]
    fn a_feature_never_asked_about_while_allowed_is_not_kept() {
        let (product, session) = session(&["pro"]);
        session.settle();
        product.sells(&[]);
        assert!(!session.allows("pro"));
    }

    #[test]
    fn the_next_user_starts_over() {
        let (product, session) = session(&["pro"]);
        session.settle();
        assert!(session.allows("pro"));
        product.sells(&[]);
        session.forget();
        assert!(!session.allows("pro"), "nothing carries over");
        product.sells(&["pro"]);
        assert!(session.allows("pro"));
        product.sells(&[]);
        assert!(!session.allows("pro"), "not kept until their check finishes");
    }

    impl RefreshSchedule {
        fn purchase_wait_is_on(&self) -> bool {
            self.purchase_wait > 0.0
        }
    }
}
