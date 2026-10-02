//! 系统文件对话框（OS 层基础设施，与具体 GUI 框架无关）。

use std::path::PathBuf;

pub fn pick_open_file(extensions: &[String]) -> Option<PathBuf> {
    let mut dlg = rfd::FileDialog::new().set_title("打开文档");
    if !extensions.is_empty() {
        let exts: Vec<&str> = extensions.iter().map(|s| s.as_str()).collect();
        dlg = dlg.add_filter("支持的文件", &exts);
    }
    dlg.pick_file()
}

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
