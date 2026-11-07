//! The local file system as a storage provider (desktop builds).

use std::path::PathBuf;

use workbench_api as api;

/// Keys are paths. Each operation runs on `blocking`'s thread pool, so a slow
/// disk holds up neither the UI thread nor the async executor's workers.
pub(crate) struct LocalFiles;

impl api::StorageProvider for LocalFiles {
    fn label(&self) -> &str {
        "本地文件"
    }

    /// The OS's own open/save dialogs browse the local file system.
    fn native_picker(&self) -> bool {
        true
    }

    fn read(&self, key: &str) -> api::StorageFuture<Vec<u8>> {
        let path = PathBuf::from(key);
        Box::pin(blocking::unblock(move || {
            std::fs::read(path).map_err(|e| e.to_string())
        }))
    }

    fn write(&self, key: &str, bytes: Vec<u8>) -> api::StorageFuture<()> {
        let path = PathBuf::from(key);
        Box::pin(blocking::unblock(move || {
            std::fs::write(path, bytes).map_err(|e| e.to_string())
        }))
    }
}
