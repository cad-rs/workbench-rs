//! The browser's origin-private file system (OPFS) as a storage provider
//! (web builds): a private folder tree per site, kept by the browser, that
//! the in-app picker browses. Keys are `/`-separated paths from its root.
//!
//! Only the asynchronous API is used: the synchronous access handles exist in
//! workers only, and the app runs on the page's thread.

use js_sys::Reflect;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    FileSystemDirectoryHandle, FileSystemFileHandle, FileSystemGetDirectoryOptions,
    FileSystemGetFileOptions, FileSystemHandle, FileSystemHandleKind, FileSystemWritableFileStream,
};
use workbench_api as api;

use crate::js::{blob_bytes, js_error, resolve};

/// The provider id the web build registers OPFS under.
pub(crate) const ID: &str = "opfs";

pub(crate) struct Opfs;

impl api::StorageProvider for Opfs {
    fn label(&self) -> &str {
        "Browser storage"
    }

    fn read(&self, key: &str) -> api::StorageFuture<Vec<u8>> {
        let key = key.to_string();
        Box::pin(async move {
            let file: web_sys::File = resolve(file_handle(&key, false).await?.get_file()).await?;
            blob_bytes(&file).await
        })
    }

    fn write(&self, key: &str, bytes: Vec<u8>) -> api::StorageFuture<()> {
        let key = key.to_string();
        Box::pin(async move {
            let handle = file_handle(&key, true).await?;
            // Safari only gained writable streams on the page's thread in 2025.
            if !has_method(&handle, "createWritable") {
                return Err("This browser can't write to browser storage".to_string());
            }
            // The stream writes to a swap file that `close` moves into place,
            // so an interrupted write leaves the old content.
            let stream: FileSystemWritableFileStream = resolve(handle.create_writable()).await?;
            JsFuture::from(stream.write_with_u8_array(&bytes).map_err(js_error)?)
                .await
                .map_err(js_error)?;
            JsFuture::from(stream.close()).await.map_err(js_error)?;
            Ok(())
        })
    }

    fn list(&self, dir: &str) -> api::StorageFuture<Vec<api::Entry>> {
        let dir = dir.to_string();
        Box::pin(async move {
            let handle = dir_handle(&dir, false).await?;
            let entries = handle.values();
            let mut listing = Vec::new();
            loop {
                let next: js_sys::IteratorNext =
                    resolve(entries.next().map_err(js_error)?).await?;
                if next.done() {
                    return Ok(listing);
                }
                let item: FileSystemHandle = next.value().unchecked_into();
                let name = item.name();
                listing.push(api::Entry {
                    key: api::storage::join_key(&dir, &name),
                    is_dir: item.kind() == FileSystemHandleKind::Directory,
                    name,
                });
            }
        })
    }

    fn create_dir(&self, key: &str) -> api::StorageFuture<()> {
        let key = key.to_string();
        Box::pin(async move { dir_handle(&key, true).await.map(|_| ()) })
    }
}

/// The folder at `path`, its missing folders created when `create`.
async fn dir_handle(path: &str, create: bool) -> Result<FileSystemDirectoryHandle, String> {
    let storage = web_sys::window()
        .map(|window| window.navigator().storage())
        .filter(|storage| has_method(storage, "getDirectory"))
        .ok_or_else(|| "This browser has no browser storage (OPFS)".to_string())?;
    let mut dir: FileSystemDirectoryHandle = resolve(storage.get_directory()).await?;
    let options = FileSystemGetDirectoryOptions::new();
    options.set_create(create);
    for part in path.split('/').filter(|part| !part.is_empty()) {
        dir = resolve(dir.get_directory_handle_with_options(part, &options)).await?;
    }
    Ok(dir)
}

/// The file at `key`, created (with its folders) when `create`.
async fn file_handle(key: &str, create: bool) -> Result<FileSystemFileHandle, String> {
    let (dir, name) = key.rsplit_once('/').unwrap_or(("", key));
    let options = FileSystemGetFileOptions::new();
    options.set_create(create);
    resolve(dir_handle(dir, create).await?.get_file_handle_with_options(name, &options)).await
}

fn has_method(object: &JsValue, name: &str) -> bool {
    Reflect::get(object, &name.into()).is_ok_and(|method| method.is_function())
}
