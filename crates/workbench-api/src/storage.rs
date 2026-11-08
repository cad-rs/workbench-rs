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
//! frame tick; see [`crate::CommandCtx::read_file`].

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

use crate::asyncrt::BoxFuture;

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

    /// The file or folder `name` in this folder (keys are `/`-separated
    /// paths; the root folder's key is `""`).
    pub fn child(&self, name: &str) -> Location {
        Location::new(self.provider.as_str(), join_key(&self.key, name))
    }

    /// The file beside this one with `.{tag}` before the extension:
    /// `a/part.rcad` tagged `v1` is `a/part.v1.rcad` (for keys that are paths).
    pub fn tagged(&self, tag: &str) -> Location {
        let stem_end = self.key.len() - self.file_name().len() + self.file_stem().len();
        let mut key = self.key.clone();
        key.insert_str(stem_end, &format!(".{tag}"));
        Location::new(self.provider.as_str(), key)
    }
}

/// The key of `name` in the folder `dir` (the root is `""`).
pub fn join_key(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
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

/// Whether `name` has one of `extensions` (lowercase, no dot); any name
/// passes an empty list.
pub fn has_extension(name: &str, extensions: &[String]) -> bool {
    let ext = split_file_name(name).1.unwrap_or_default().to_lowercase();
    extensions.is_empty() || extensions.contains(&ext)
}

/// The file name to save `name` under: the first of `extensions` is added
/// when it has none of them.
pub fn with_extension(name: &str, extensions: &[String]) -> String {
    let name = name.trim();
    match extensions.first() {
        Some(ext) if !has_extension(name, extensions) => format!("{name}.{ext}"),
        _ => name.to_string(),
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

/// A storage operation's future (`Send` on the desktop, see [`BoxFuture`]).
pub type StorageFuture<T> = BoxFuture<Result<T, String>>;

/// One place files can live. Errors are user-readable messages; the platform
/// adds which file they are about.
pub trait StorageProvider: Send + Sync {
    /// What the file picker calls this place.
    fn label(&self) -> &str;
    /// The whole content of the file at `key`.
    fn read(&self, key: &str) -> StorageFuture<Vec<u8>>;
    /// Create the file at `key`, or replace its content, with `bytes`. All or
    /// nothing: a write that fails or is cut off must leave the old content.
    fn write(&self, key: &str, bytes: Vec<u8>) -> StorageFuture<()>;
    /// The folders and files in the folder `dir` (the root is `""`); the
    /// in-app picker browses with this.
    fn list(&self, dir: &str) -> StorageFuture<Vec<Entry>> {
        let _ = dir;
        failed("Browsing is not supported here".to_string())
    }
    /// Create the folder `key`.
    fn create_dir(&self, key: &str) -> StorageFuture<()> {
        let _ = key;
        failed("Creating folders is not supported here".to_string())
    }
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

    /// Whether a provider is registered under `id`.
    pub fn contains(&self, id: &str) -> bool {
        self.providers.contains_key(id)
    }

    /// The places the in-app picker browses, as (id, label): every provider
    /// but the local file system, whose files the OS's own dialogs pick.
    pub fn in_app_sources(&self) -> Vec<(String, String)> {
        self.providers
            .iter()
            .filter(|(id, _)| *id != LOCAL)
            .map(|(id, provider)| (id.clone(), provider.label().to_string()))
            .collect()
    }

    /// List the folder at `location`.
    pub fn list(&self, location: &Location) -> StorageFuture<Vec<Entry>> {
        self.route(location, format!("read folder {location}"), |p, key| p.list(key))
    }

    /// Create the folder at `location`.
    pub fn create_dir(&self, location: &Location) -> StorageFuture<()> {
        self.route(location, format!("create folder {location}"), |p, key| p.create_dir(key))
    }

    /// Read the file at `location`.
    pub fn read(&self, location: &Location) -> StorageFuture<Vec<u8>> {
        self.route(location, format!("read {location}"), |p, key| p.read(key))
    }

    /// Write `bytes` to the file at `location`.
    pub fn write(&self, location: &Location, bytes: Vec<u8>) -> StorageFuture<()> {
        self.route(location, format!("write {location}"), |p, key| p.write(key, bytes))
    }

    /// Write `bytes` to `location` after copying the file they replace to
    /// `keep_at`. If the copy fails, nothing is written.
    pub fn write_keeping_old(
        &self,
        location: &Location,
        keep_at: &Location,
        bytes: Vec<u8>,
    ) -> StorageFuture<()> {
        let (storage, location, keep_at) = (self.clone(), location.clone(), keep_at.clone());
        // One step at a time: providers may start working when called, not
        // when polled, so each write is only requested once its turn comes.
        Box::pin(async move {
            let old = storage.read(&location).await?;
            storage.write(&keep_at, old).await?;
            storage.write(&location, bytes).await
        })
    }

    /// Run `op` on the provider holding `location`; errors say what failed.
    fn route<T: Send + 'static>(
        &self,
        location: &Location,
        what: String,
        op: impl FnOnce(&dyn StorageProvider, &str) -> StorageFuture<T>,
    ) -> StorageFuture<T> {
        let Some(provider) = self.providers.get(&location.provider) else {
            return failed(format!("Failed to {what}: no `{}` storage", location.provider));
        };
        let future = op(provider.as_ref(), &location.key);
        Box::pin(async move { future.await.map_err(|e| format!("Failed to {what}: {e}")) })
    }
}

fn failed<T: Send + 'static>(error: String) -> StorageFuture<T> {
    Box::pin(std::future::ready(Err(error)))
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

    #[test]
    fn children_and_save_names() {
        let root = Location::new("opfs", "");
        assert_eq!(root.child("a"), Location::new("opfs", "a"));
        assert_eq!(root.child("a").child("b.rcad"), Location::new("opfs", "a/b.rcad"));

        let exts = vec!["rcad".to_string()];
        assert_eq!(with_extension(" part ", &exts), "part.rcad");
        assert_eq!(with_extension("part.RCAD", &exts), "part.RCAD");
        assert_eq!(with_extension("part.v2", &exts), "part.v2.rcad");
        assert_eq!(with_extension("notes.txt", &[]), "notes.txt");
    }

    #[test]
    fn a_tag_goes_before_the_extension() {
        let tagged = |key: &str| Location::new("p", key).tagged("v1").key;
        assert_eq!(tagged("a/part.rcad"), "a/part.v1.rcad");
        assert_eq!(tagged(r"C:\work\part.v2.rcad"), r"C:\work\part.v2.v1.rcad");
        assert_eq!(tagged("dir.d/part"), "dir.d/part.v1");
        assert_eq!(tagged(".hidden"), ".hidden.v1");
    }
}
