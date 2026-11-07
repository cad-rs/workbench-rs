//! The app's own state: settings, the workspace layout, autosaves and the
//! clean-exit marker.
//!
//! Unlike user files ([`crate::storage`]) this state needs no picker and is
//! small, so the store is synchronous: files under the config directory on the
//! desktop, the browser's localStorage on the web. Keys are `/`-separated
//! paths such as `settings.json` or `autosave/part_.rcad`.

pub trait StateStore: Send + Sync {
    /// The value at `key`, if there is one.
    fn read(&self, key: &str) -> Option<Vec<u8>>;
    /// Create or replace the value at `key`. Errors are user-readable.
    fn write(&self, key: &str, bytes: &[u8]) -> Result<(), String>;
    /// Remove the value at `key`, if there is one.
    fn remove(&self, key: &str);
    /// The keys in the directory `prefix` ends in (e.g. `autosave/`) that
    /// start with `prefix`, sorted. Subdirectories are not listed.
    fn keys(&self, prefix: &str) -> Vec<String>;
}
