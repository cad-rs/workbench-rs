//! 系统文件对话框（OS 层基础设施，与具体 GUI 框架无关）。
//!
//! 对话框是**阻塞式**的（OS 模态循环），只能在后台线程运行：UI 线程内
//! 调用会重入 gpui 的窗口过程、楔死渲染循环。[`spawn`] 因此开一个线程
//! 弹框、经通道回传结果；唯一调用方是
//! [`crate::app::AppRuntime::request_file_dialog`]（帧节拍排水）。
//!
//! Web（wasm32）没有线程，浏览器的文件选择框也给不出能存回去的文件：
//! 结果立即为 None（按取消处理）。

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

/// Show an open dialog, or a save dialog when `default_name` is given, on a
/// thread of its own. The receiver gets the picked path, or None.
pub(crate) fn spawn(
    extensions: Vec<String>,
    default_name: Option<String>,
) -> Receiver<Option<PathBuf>> {
    let (tx, rx) = channel();
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (extensions, default_name);
        let _ = tx.send(None);
    }
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let picked = match &default_name {
            Some(name) => pick_save_file(name, &extensions),
            None => pick_open_file(&extensions),
        };
        let _ = tx.send(picked);
    });
    rx
}

#[cfg(not(target_arch = "wasm32"))]
fn pick_open_file(extensions: &[String]) -> Option<PathBuf> {
    let mut dlg = rfd::FileDialog::new().set_title("打开文档");
    if !extensions.is_empty() {
        let exts: Vec<&str> = extensions.iter().map(|s| s.as_str()).collect();
        dlg = dlg.add_filter("支持的文件", &exts);
    }
    dlg.pick_file()
}

#[cfg(not(target_arch = "wasm32"))]
fn pick_save_file(default_name: &str, extensions: &[String]) -> Option<PathBuf> {
    let mut dlg = rfd::FileDialog::new()
        .set_title("保存文档")
        .set_file_name(default_name);
    if !extensions.is_empty() {
        let exts: Vec<&str> = extensions.iter().map(|s| s.as_str()).collect();
        dlg = dlg.add_filter("支持的文件", &exts);
    }
    dlg.save_file()
}
