//! Platform-independent async runtime helpers (for the web build).
//!
//! Desktop: based on `async-io` timers (driven by the shared executor thread pool).
//! Web (wasm32): a `performance.now` deadline plus a JS `setTimeout` wake-up; the future itself
//! only holds an `f64`, so it stays `Send` (compatible with `AsyncJob`'s Send bound).

use std::time::Duration;

/// A boxed future the platform runs for you. It is `Send` on the desktop,
/// where it runs on the platform's executor threads; on the web it runs on
/// the page's only thread, and the JS promises it awaits are not `Send`.
#[cfg(not(target_arch = "wasm32"))]
pub type BoxFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>;
#[cfg(target_arch = "wasm32")]
pub type BoxFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T>>>;

/// Async sleep: wait for a while inside an async command body without blocking the thread.
///
/// Platform commands/views/Workbenches should use this rather than depending directly on a
/// platform-specific timer such as `async-io`, so the web build keeps working.
pub async fn sleep(dur: Duration) {
    #[cfg(not(target_arch = "wasm32"))]
    async_io::Timer::after(dur).await;

    #[cfg(target_arch = "wasm32")]
    SleepAfter {
        deadline_ms: js_sys::Date::now() + dur.as_millis() as f64,
    }
    .await;
}

#[cfg(target_arch = "wasm32")]
struct SleepAfter {
    deadline_ms: f64,
}

#[cfg(target_arch = "wasm32")]
impl std::future::Future for SleepAfter {
    type Output = ();

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<()> {
        use wasm_bindgen::JsCast;

        let now = js_sys::Date::now();
        if now >= self.deadline_ms {
            return std::task::Poll::Ready(());
        }
        let remaining = (self.deadline_ms - now) as i32;
        let waker = cx.waker().clone();
        let callback = wasm_bindgen::prelude::Closure::once_into_js(move || waker.wake());
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                callback.unchecked_ref(),
                remaining,
            );
        }
        std::task::Poll::Pending
    }
}

/// Fetch a same-origin binary resource (non-2xx / missing → None).
///
/// Shared by both web frontends: egui's CJK font loading and gpui's font injection before the
/// first frame.
#[cfg(target_arch = "wasm32")]
pub async fn fetch_bytes(url: &str) -> Option<Vec<u8>> {
    use wasm_bindgen::JsCast;
    let window = web_sys::window()?;
    let response = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(url))
        .await
        .ok()?;
    let response: web_sys::Response = response.dyn_into().ok()?;
    if !response.ok() {
        return None;
    }
    let buffer =
        wasm_bindgen_futures::JsFuture::from(response.array_buffer().ok()?).await.ok()?;
    // ArrayBuffer → Uint8Array: needs a constructor wrap; dyn_into does not cross types
    let bytes = js_sys::Uint8Array::new(&buffer.into());
    Some(bytes.to_vec())
}
