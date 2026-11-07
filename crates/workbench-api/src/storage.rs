//! Where user files live.
//!
//! A [`Location`] names a file by the storage provider that holds it and that
//! provider's key for it: a path on the local file system, a path in the
//! browser's origin-private file system, an object id in a cloud store.
//! Documents remember a `Location`, never an OS path, so opening and saving
//! work the same whichever provider holds the file.
//!
//! Providers are asynchronous (the origin-private file system has only async
//! APIs on the page's thread, and a cloud store is network I/O). The platform
//! runs their futures off the UI thread and hands the result back on a later
//! frame tick; see [`crate::AppServices::read_file`].

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

/// The provider id of the local file system (desktop builds register it).
pub const LOCAL: &str = "local";

/// A file in some storage provider.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Location {
    /// The id the provider was registered under.
    pub provider: String,
    /// The provider's key for the file.
    pub key: String,
}

impl Location {
    pub fn new(provider: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            key: key.into(),
        }
    }

    /// A file on the local file system.
    pub fn local(path: impl AsRef<Path>) -> Self {
        Self::new(LOCAL, path.as_ref().to_string_lossy())
    }

    /// The key's last segment: the file name for providers whose keys are
    /// paths.
    pub fn file_name(&self) -> &str {
        self.key.rsplit(['/', '\\']).next().unwrap_or(&self.key)
    }

    /// The file name without its extension.
    pub fn file_stem(&self) -> &str {
        split_file_name(self.file_name()).0
    }

    /// The file name's extension, without the dot.
    pub fn extension(&self) -> Option<&str> {
        split_file_name(self.file_name()).1
    }
}

/// A file name's stem and extension (without the dot). Like `Path`, a
/// leading dot starts a name, not an extension.
pub fn split_file_name(name: &str) -> (&str, Option<&str>) {
    match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], Some(&name[dot + 1..])),
        _ => (name, None),
    }
}

/// One item of a folder listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The provider's key for it.
    pub key: String,
    /// What the user sees.
    pub name: String,
    pub is_dir: bool,
}

impl fmt::Display for Location {
    /// Local files show as their path, other files as `provider:key`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.provider == LOCAL {
            f.write_str(&self.key)
        } else {
            write!(f, "{}:{}", self.provider, self.key)
        }
    }
}

/// A storage operation's future. It is `Send` on the desktop, where the
/// platform runs it on its executor threads; on the web it runs on the
/// page's only thread, and the JS promises it awaits are not `Send`.
#[cfg(not(target_arch = "wasm32"))]
pub type StorageFuture<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;
#[cfg(target_arch = "wasm32")]
pub type StorageFuture<T> = Pin<Box<dyn Future<Output = Result<T, String>>>>;

/// One place files can live. Errors are user-readable messages; the platform
/// adds which file they are about.
pub trait StorageProvider: Send + Sync {
    /// What the file picker calls this place.
    fn label(&self) -> &str;
    /// Whether files here are picked with the OS's own dialog (the local file
    /// system) rather than the in-app picker, which browses with
    /// [`Self::list`].
    fn native_picker(&self) -> bool {
        false
    }
    /// The whole content of the file at `key`.
    fn read(&self, key: &str) -> StorageFuture<Vec<u8>>;
    /// Create the file at `key`, or replace its content, with `bytes`.
    fn write(&self, key: &str, bytes: Vec<u8>) -> StorageFuture<()>;
    /// The folders and files in the folder `dir` (the root is `""`).
    fn list(&self, dir: &str) -> StorageFuture<Vec<Entry>> {
        let _ = dir;
        unsupported("Browsing")
    }
    /// Create the folder `key`.
    fn create_dir(&self, key: &str) -> StorageFuture<()> {
        let _ = key;
        unsupported("Creating folders")
    }
    /// The key of `name` in the folder `dir`: paths by default.
    fn child_key(&self, dir: &str, name: &str) -> String {
        if dir.is_empty() {
            name.to_string()
        } else {
            format!("{dir}/{name}")
        }
    }
}

fn unsupported<T: Send + 'static>(what: &str) -> StorageFuture<T> {
    Box::pin(std::future::ready(Err(format!("{what} is not supported here"))))
}

/// The product's storage providers, by id.
#[derive(Clone, Default)]
pub struct Storage {
    providers: BTreeMap<String, Arc<dyn StorageProvider>>,
}

impl Storage {
    /// Register `provider` under `id`, replacing any provider already there.
    pub fn register(&mut self, id: impl Into<String>, provider: Arc<dyn StorageProvider>) {
        self.providers.insert(id.into(), provider);
    }

    /// The provider registered under `id`.
    pub fn get(&self, id: &str) -> Option<&Arc<dyn StorageProvider>> {
        self.providers.get(id)
    }

    /// The places the in-app picker offers, as (id, label): every provider
    /// without a native dialog.
    pub fn in_app_sources(&self) -> Vec<(String, String)> {
        self.providers
            .iter()
            .filter(|(_, provider)| !provider.native_picker())
            .map(|(id, provider)| (id.clone(), provider.label().to_string()))
            .collect()
    }

    /// List the folder at `location`.
    pub fn list(&self, location: &Location) -> StorageFuture<Vec<Entry>> {
        let what = format!("read folder {location}");
        match self.providers.get(&location.provider) {
            Some(provider) => with_context(provider.list(&location.key), what),
            None => missing(location, what),
        }
    }

    /// Create the folder at `location`.
    pub fn create_dir(&self, location: &Location) -> StorageFuture<()> {
        let what = format!("create folder {location}");
        match self.providers.get(&location.provider) {
            Some(provider) => with_context(provider.create_dir(&location.key), what),
            None => missing(location, what),
        }
    }

    /// Read the file at `location`.
    pub fn read(&self, location: &Location) -> StorageFuture<Vec<u8>> {
        let what = format!("read {location}");
        match self.providers.get(&location.provider) {
            Some(provider) => with_context(provider.read(&location.key), what),
            None => missing(location, what),
        }
    }

    /// Write `bytes` to the file at `location`.
    pub fn write(&self, location: &Location, bytes: Vec<u8>) -> StorageFuture<()> {
        let what = format!("write {location}");
        match self.providers.get(&location.provider) {
            Some(provider) => with_context(provider.write(&location.key, bytes), what),
            None => missing(location, what),
        }
    }
}

fn missing<T: Send + 'static>(location: &Location, what: String) -> StorageFuture<T> {
    let error = format!("Failed to {what}: no `{}` storage", location.provider);
    Box::pin(std::future::ready(Err(error)))
}

fn with_context<T: 'static>(future: StorageFuture<T>, what: String) -> StorageFuture<T> {
    Box::pin(async move { future.await.map_err(|e| format!("Failed to {what}: {e}")) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_come_from_the_last_key_segment() {
        let local = Location::local(r"C:\work\part.v2.rcad");
        assert_eq!(local.file_name(), "part.v2.rcad");
        assert_eq!(local.file_stem(), "part.v2");
        assert_eq!(local.extension(), Some("rcad"));
        assert_eq!(local.to_string(), r"C:\work\part.v2.rcad");

        let opfs = Location::new("opfs", "docs/.hidden");
        assert_eq!(opfs.file_stem(), ".hidden");
        assert_eq!(opfs.extension(), None);
        assert_eq!(opfs.to_string(), "opfs:docs/.hidden");
    }
}
