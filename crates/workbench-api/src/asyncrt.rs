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

/// fetch 一个同源二进制资源（非 2xx / 不存在 → None）。
///
/// 两个 web 前端共用：egui 的 CJK 字体加载与 gpui 的首帧前字体注入。
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
    // ArrayBuffer → Uint8Array：需用构造包装，dyn_into 不跨类型
    let bytes = js_sys::Uint8Array::new(&buffer.into());
    Some(bytes.to_vec())
}
