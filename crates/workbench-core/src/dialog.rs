//! The OS's own open/save dialogs, for files on the local file system
//! (desktop builds).
//!
//! The dialogs are **blocking** (an OS modal loop) and must not run on the UI
//! thread: there they re-enter gpui's window procedure and wedge the render
//! loop. [`pick`] therefore runs one on `blocking`'s thread pool, and the
//! runtime awaits it as I/O like any other.

use std::path::PathBuf;

/// Show an open dialog, or a save dialog suggesting `default_name`, filtered
/// by `extensions` (empty: no filter). Resolves to the picked path, or None.
pub(crate) async fn pick(extensions: Vec<String>, default_name: Option<String>) -> Option<PathBuf> {
    blocking::unblock(move || {
        let mut dialog = rfd::FileDialog::new();
        if !extensions.is_empty() {
            dialog = dialog.add_filter("Supported files", &extensions);
        }
        match default_name {
            Some(name) => dialog.set_title("Save Document").set_file_name(name).save_file(),
            None => dialog.set_title("Open Document").pick_file(),
        }
    })
    .await
}
