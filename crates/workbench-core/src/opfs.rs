//! The browser's origin-private file system (OPFS) as a storage provider
//! (web builds): a private folder tree per site, kept by the browser, that
//! the in-app picker browses. Keys are `/`-separated paths from its root.
//!
//! Only the asynchronous API is used: the synchronous access handles exist in
//! workers only, and the app runs on the page's thread.

use js_sys::{Promise, Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    FileSystemDirectoryHandle, FileSystemFileHandle, FileSystemGetDirectoryOptions,
    FileSystemGetFileOptions, FileSystemHandle, FileSystemHandleKind, FileSystemWritableFileStream,
};
use workbench_api as api;

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
            let buffer = JsFuture::from(file.array_buffer()).await.map_err(js_error)?;
            Ok(Uint8Array::new(&buffer).to_vec())
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
                    key: if dir.is_empty() { name.clone() } else { format!("{dir}/{name}") },
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

async fn resolve<T: JsCast>(promise: Promise) -> Result<T, String> {
    Ok(JsFuture::from(promise).await.map_err(js_error)?.unchecked_into())
}

fn has_method(object: &JsValue, name: &str) -> bool {
    Reflect::get(object, &name.into()).is_ok_and(|method| method.is_function())
}

/// A user-readable message for a rejected file-system promise.
fn js_error(error: JsValue) -> String {
    let Some(error) = error.dyn_ref::<web_sys::DomException>() else {
        return format!("{error:?}");
    };
    match error.name().as_str() {
        "NotFoundError" => "not found".to_string(),
        "TypeMismatchError" => "a file and a folder have the same name".to_string(),
        "QuotaExceededError" => "browser storage is full".to_string(),
        "NoModificationAllowedError" => "the file is in use by another tab".to_string(),
        _ => error.message(),
    }
}
