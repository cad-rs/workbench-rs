//! 系统文件对话框（OS 层基础设施，与具体 GUI 框架无关）。
//!
//! Web（wasm32）构建没有同步本地对话框：返回 None（命令按“已取消”处理）；
//! 需要文件选择时应改用浏览器文件/拖拽等 Web 惯例（后续扩展）。

use std::path::PathBuf;

#[cfg(target_arch = "wasm32")]
pub fn pick_open_file(_extensions: &[String]) -> Option<PathBuf> {
    None
}

#[cfg(target_arch = "wasm32")]
pub fn pick_save_file(_default_name: &str, _extensions: &[String]) -> Option<PathBuf> {
    None
}

#[cfg(not(target_arch = "wasm32"))]
pub fn pick_open_file(extensions: &[String]) -> Option<PathBuf> {
    let mut dlg = rfd::FileDialog::new().set_title("打开文档");
    if !extensions.is_empty() {
        let exts: Vec<&str> = extensions.iter().map(|s| s.as_str()).collect();
        dlg = dlg.add_filter("支持的文件", &exts);
    }
    dlg.pick_file()
}

#[cfg(not(target_arch = "wasm32"))]
pub fn pick_save_file(default_name: &str, extensions: &[String]) -> Option<PathBuf> {
    let mut dlg = rfd::FileDialog::new()
        .set_title("保存文档")
        .set_file_name(default_name);
    if !extensions.is_empty() {
        let exts: Vec<&str> = extensions.iter().map(|s| s.as_str()).collect();
        dlg = dlg.add_filter("支持的文件", &exts);
    }
    dlg.save_file()
}
