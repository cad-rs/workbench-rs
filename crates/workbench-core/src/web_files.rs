//! Files from and to the user's device on the web: the browser's file
//! chooser (import) and downloads (export). Both move one-off copies; a
//! document stored in the browser lives in OPFS instead (see `opfs`).

use js_sys::{Array, Promise, Uint8Array};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{HtmlAnchorElement, HtmlInputElement};

use crate::js::{blob_bytes, js_error, resolve};

/// Ask the user for a file with one of `extensions` (empty: any) through
/// the browser's chooser; resolves to its name and content. Cancelling
/// resolves to an error.
pub(crate) async fn pick(extensions: &[String]) -> Result<(String, Vec<u8>), String> {
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or("No page document")?;
    let input: HtmlInputElement =
        document.create_element("input").map_err(js_error)?.unchecked_into();
    input.set_type("file");
    let accept: Vec<String> = extensions.iter().map(|ext| format!(".{ext}")).collect();
    input.set_accept(&accept.join(","));
    // Attached (hidden) for browsers that ignore clicks on detached inputs.
    let _ = input.set_attribute("hidden", "");
    if let Some(body) = document.body() {
        let _ = body.append_child(&input);
    }

    // Settles with the chosen File, or null when the chooser is dismissed.
    let chosen = Promise::new(&mut |resolve, _reject| {
        let on_change = {
            let input = input.clone();
            let resolve = resolve.clone();
            Closure::once_into_js(move || {
                let file = input.files().and_then(|files| files.get(0));
                let _ = resolve.call1(&JsValue::NULL, &file.map_or(JsValue::NULL, JsValue::from));
            })
        };
        input.set_onchange(Some(on_change.unchecked_ref()));
        let on_cancel = Closure::once_into_js(move || {
            let _ = resolve.call1(&JsValue::NULL, &JsValue::NULL);
        });
        let _ = input.add_event_listener_with_callback("cancel", on_cancel.unchecked_ref());
    });
    input.click();
    let chosen = resolve::<JsValue>(chosen).await;
    input.remove();
    let file = chosen?;
    if file.is_null() {
        return Err("Cancelled".to_string());
    }
    let file: web_sys::File = file.unchecked_into();
    let bytes = blob_bytes(&file)
        .await
        .map_err(|e| format!("Failed to read {}: {e}", file.name()))?;
    Ok((file.name(), bytes))
}

/// Hand `bytes` to the browser as a download called `name`.
pub(crate) fn download(name: &str, bytes: &[u8]) -> Result<(), String> {
    let window = web_sys::window().ok_or("No page window")?;
    let document = window.document().ok_or("No page document")?;
    let parts = Array::of1(&Uint8Array::from(bytes));
    let blob = web_sys::Blob::new_with_u8_array_sequence(&parts).map_err(js_error)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(js_error)?;
    let anchor: HtmlAnchorElement =
        document.create_element("a").map_err(js_error)?.unchecked_into();
    anchor.set_href(&url);
    anchor.set_download(name);
    anchor.click();
    // Revoked a little later: some browsers start reading the blob only
    // after the click returns.
    let revoke = Closure::once_into_js(move || {
        let _ = web_sys::Url::revoke_object_url(&url);
    });
    let _ = window
        .set_timeout_with_callback_and_timeout_and_arguments_0(revoke.unchecked_ref(), 10_000);
    Ok(())
}
