//! 平台无关的异步运行时小工具（Web 构建适配）。
//!
//! 桌面：基于 `async-io` 计时器（共享执行器线程池驱动）。
//! Web（wasm32）：基于 `performance.now` 截止时间 + JS `setTimeout` 唤醒；
//! future 自身只持有 `f64`，保持 `Send`（与 `AsyncJob` 的 Send 约束兼容）。

use std::time::Duration;

/// 异步睡眠：在异步命令体内等待一段时间，不阻塞线程。
///
/// 平台命令/视图/Workbench 应使用它而不是直接依赖 `async-io` 等
/// 平台相关的计时器，以保持 Web 构建可用。
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
