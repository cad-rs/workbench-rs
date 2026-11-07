//! Small JS-interop helpers for the web build's file code (`opfs`,
//! `web_files`).

use js_sys::{Promise, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// Await `promise` and cast what it resolves to.
pub(crate) async fn resolve<T: JsCast>(promise: Promise) -> Result<T, String> {
    Ok(JsFuture::from(promise).await.map_err(js_error)?.unchecked_into())
}

/// The bytes of a blob (a `File` is one).
pub(crate) async fn blob_bytes(blob: &web_sys::Blob) -> Result<Vec<u8>, String> {
    let buffer: JsValue = resolve(blob.array_buffer()).await?;
    Ok(Uint8Array::new(&buffer).to_vec())
}

/// A user-readable message for a rejected promise or a thrown JS error.
pub(crate) fn js_error(error: JsValue) -> String {
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
