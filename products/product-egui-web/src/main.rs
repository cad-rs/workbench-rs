//! Example product: the egui frontend compiled to wasm32 and served by trunk
//! (design.md acceptance item #1: exactly one GUI per product).
//!
//! The web slice mirrors the desktop product but without the Python plugin
//! stage (pyo3 does not target wasm32-unknown-unknown): no local file
//! dialogs, no background thread tasks, and layout persistence stays in
//! memory. See workbench_ui_egui::run_web for the full difference list.
//!
//! Run: `trunk serve` in this directory (needs `rustup target add
//! wasm32-unknown-unknown` and `cargo install trunk`).

#[cfg(target_arch = "wasm32")]
fn main() {
    // 内嵌/无头环境拿不到控制台历史：panic 与入口错误同时写入 DOM 便于诊断
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("PANIC: {info}");
        eframe::web_sys::console::error_1(&msg.clone().into());
        web_dom_append(&msg);
    }));

    let builder = workbench_core::WorkbenchAppBuilder::new(
        "com.example.workbench-egui-web",
        "Workbench Demo - egui (Web)",
    )
    .with_backend_label("egui-web")
    .with_workbench(wb_example::ExampleWorkbench)
    .with_assets(wb_example::example_assets())
    .create_default_document(true)
    .open_on_start("example.welcome", false)
    .open_on_start("example.canvas", true);

    wasm_bindgen_futures::spawn_local(async move {
        if let Err(e) = workbench_ui_egui::run_web(builder, "wb-canvas").await {
            let msg = format!("run_web failed: {e:?}");
            eframe::web_sys::console::error_1(&msg.clone().into());
            web_dom_append(&msg);
        }
    });
}

#[cfg(target_arch = "wasm32")]
fn web_dom_append(message: &str) {
    let Some(document) = eframe::web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let Some(body) = document.body() else { return };
    let Ok(pre) = document.create_element("pre") else { return };
    pre.set_text_content(Some(message));
    let _ = body.append_child(&pre);
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
        "product-egui-web is a wasm32 product: run `trunk serve` in this \
         directory (see README \"Running in the browser\")."
    );
    std::process::exit(1);
}
