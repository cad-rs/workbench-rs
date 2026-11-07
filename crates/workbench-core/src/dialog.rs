//! Native file dialogs (OS-level infrastructure, independent of any GUI framework).
//!
//! The dialogs are **blocking** (an OS modal loop) and may only run on a background thread:
//! calling one on the UI thread re-enters gpui's window procedure and wedges the render loop.
//! [`spawn`] therefore opens the dialog on a thread of its own and sends the result back over
//! a channel; the only caller is [`crate::app::AppRuntime::request_file_dialog`] (drained on
//! the frame tick).
//!
//! The web (wasm32) has no threads, and the browser's file chooser cannot give back a file
//! that can be saved to: the result is None immediately (treated as cancelled).

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
    let mut dlg = rfd::FileDialog::new().set_title("Open Document");
    if !extensions.is_empty() {
        let exts: Vec<&str> = extensions.iter().map(|s| s.as_str()).collect();
        dlg = dlg.add_filter("Supported files", &exts);
    }
    dlg.pick_file()
}

#[cfg(not(target_arch = "wasm32"))]
fn pick_save_file(default_name: &str, extensions: &[String]) -> Option<PathBuf> {
    let mut dlg = rfd::FileDialog::new()
        .set_title("Save Document")
        .set_file_name(default_name);
    if !extensions.is_empty() {
        let exts: Vec<&str> = extensions.iter().map(|s| s.as_str()).collect();
        dlg = dlg.add_filter("Supported files", &exts);
    }
    dlg.save_file()
}
