//! Reading the JavaScript values the browser hands back.

use wasm_bindgen::JsValue;

/// A property of a JavaScript object, unless it is undefined or null.
pub fn field(object: &JsValue, name: &str) -> Option<JsValue> {
    let held = js_sys::Reflect::get(object, &JsValue::from_str(name)).ok()?;
    (!held.is_undefined() && !held.is_null()).then_some(held)
}

/// The text of a rejected promise's reason.
///
/// A rejection usually carries a `DOMException`, whose text is read from its properties
/// because it does not downcast to `Error`.
pub fn describe(err: &JsValue) -> String {
    let text = |name| field(err, name)?.as_string();
    match (text("name"), text("message")) {
        (Some(name), Some(message)) => format!("{name}: {message}"),
        (Some(only), None) | (None, Some(only)) => only,
        (None, None) => err.as_string().unwrap_or_else(|| format!("{err:?}")),
    }
}
