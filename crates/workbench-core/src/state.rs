//! Where the app's own state lives ([`api::StateStore`]): files under the
//! config directory on the desktop, localStorage on the web.

use std::sync::Arc;

use workbench_api as api;

/// The state store for this product, and the layout's key in it.
///
/// `layout_path` is the builder's layout file, if it set one. On the desktop
/// its directory holds the rest of the state too (settings, autosaves); on the
/// web only its file name is used.
pub(crate) fn open(
    product_id: &str,
    layout_path: Option<&std::path::Path>,
) -> (Arc<dyn api::StateStore>, String) {
    let layout_key = layout_path
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "layout.json".to_string());
    #[cfg(not(target_arch = "wasm32"))]
    let store = desktop::Dir(dir(product_id, layout_path));
    #[cfg(target_arch = "wasm32")]
    let store = web::LocalStorage {
        prefix: format!("{product_id}/"),
    };
    (Arc::new(store), layout_key)
}

/// The directory the desktop state lives in.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn dir(product_id: &str, layout_path: Option<&std::path::Path>) -> std::path::PathBuf {
    match layout_path {
        Some(path) => path.parent().map(Into::into).unwrap_or_else(std::env::temp_dir),
        None => desktop::config_dir(product_id),
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod desktop {
    use std::path::PathBuf;

    use workbench_api as api;

    /// Windows: `%APPDATA%/<product_id>`; elsewhere `~/.config/<product_id>`.
    pub(super) fn config_dir(product_id: &str) -> PathBuf {
        crate::cli::user_config_home()
            .unwrap_or_else(std::env::temp_dir)
            .join(product_id)
    }

    /// Keys are paths under a directory.
    pub(super) struct Dir(pub(super) PathBuf);

    impl Dir {
        fn path(&self, key: &str) -> PathBuf {
            key.split('/').fold(self.0.clone(), |path, part| path.join(part))
        }
    }

    impl api::StateStore for Dir {
        fn read(&self, key: &str) -> Option<Vec<u8>> {
            std::fs::read(self.path(key)).ok()
        }

        fn write(&self, key: &str, bytes: &[u8]) -> Result<(), String> {
            let path = self.path(key);
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            product_kit::atomic_write(&path, bytes)
                .map_err(|e| format!("Failed to write {}: {e}", path.display()))
        }

        fn remove(&self, key: &str) {
            let _ = std::fs::remove_file(self.path(key));
        }

        fn keys(&self, prefix: &str) -> Vec<String> {
            let (dir, _) = prefix.rsplit_once('/').unwrap_or(("", prefix));
            let Ok(entries) = std::fs::read_dir(self.path(dir)) else {
                return Vec::new();
            };
            let mut keys: Vec<String> = entries
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|t| t.is_file()))
                .map(|entry| api::storage::join_key(dir, &entry.file_name().to_string_lossy()))
                .filter(|key| key.starts_with(prefix))
                .collect();
            keys.sort();
            keys
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use workbench_api as api;

    /// Keys live in the page's localStorage under `prefix` (the product id),
    /// so products served from one origin don't collide. localStorage holds
    /// text, which is all this state is on the web (autosaves are desktop
    /// only).
    pub(super) struct LocalStorage {
        pub(super) prefix: String,
    }

    fn storage() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok()?
    }

    impl api::StateStore for LocalStorage {
        fn read(&self, key: &str) -> Option<Vec<u8>> {
            let item = storage()?.get_item(&format!("{}{key}", self.prefix)).ok()??;
            Some(item.into_bytes())
        }

        fn write(&self, key: &str, bytes: &[u8]) -> Result<(), String> {
            let text = std::str::from_utf8(bytes)
                .map_err(|_| format!("Failed to write {key}: browser storage holds text only"))?;
            let storage = storage()
                .ok_or_else(|| format!("Failed to write {key}: browser storage is unavailable"))?;
            storage
                .set_item(&format!("{}{key}", self.prefix), text)
                .map_err(|e| format!("Failed to write {key}: {e:?}"))
        }

        fn remove(&self, key: &str) {
            if let Some(storage) = storage() {
                let _ = storage.remove_item(&format!("{}{key}", self.prefix));
            }
        }

        fn keys(&self, prefix: &str) -> Vec<String> {
            let Some(storage) = storage() else {
                return Vec::new();
            };
            let count = storage.length().unwrap_or(0);
            let mut keys: Vec<String> = (0..count)
                .filter_map(|i| storage.key(i).ok().flatten())
                .filter_map(|key| key.strip_prefix(&self.prefix).map(str::to_string))
                // In the listed directory: no `/` after the prefix.
                .filter(|key| key.strip_prefix(prefix).is_some_and(|rest| !rest.contains('/')))
                .collect();
            keys.sort();
            keys
        }
    }
}
