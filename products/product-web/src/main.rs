//! Example product: workbench-rs compiled to wasm32 and served by trunk
//! (WebGPU rendering via gpui-ce's `gpui_web` platform backend).
//!
//! The web slice mirrors the desktop product but without the script plugin
//! stage (script loading from plugin directories is a desktop convenience).
//! `workbench_ui::run` is platform-agnostic: `gpui_platform::application()`
//! selects the WebPlatform on wasm, initializes WebGPU asynchronously and
//! then opens the window.
//!
//! Browser requirements: WebGPU (Chrome/Edge 113+; local file:// is not
//! supported — serve over http, e.g. `trunk serve`).
//!
//! Run: `trunk serve` in this directory (needs `rustup target add
//! wasm32-unknown-unknown` and `cargo install trunk`).

#[cfg(target_arch = "wasm32")]
fn main() {
    gpui_platform::web_init();

    let builder = workbench_core::WorkbenchAppBuilder::new(
        "com.example.workbench-web",
        "Workbench Demo (Web)",
    )
    .with_backend_label("gpui-web")
    .with_workbench(wb_example::ExampleWorkbench)
    .with_assets(wb_example::example_assets())
    .create_default_document(true)
    .open_on_start("example.welcome", false)
    .open_on_start("example.canvas", true);

    if let Err(e) = workbench_ui::run(builder, "Workbench Demo (Web)") {
        web_sys::console::error_1(&format!("run failed: {e}").into());
    }
}

// trunk 的 wasm-bindgen 加载器只调用 init()：start 标记让 main 在
// wasm 实例化完成后被自动执行（生成 __wbindgen_start 导出）。
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
fn wasm_main() {
    main();
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!(
        "product-web is a wasm32 product: run `trunk serve` in this \
         directory (see README \"Running in the browser\")."
    );
    std::process::exit(1);
}
