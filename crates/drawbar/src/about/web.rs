//! What the browser reports about itself, for the build lines.

use wasm_bindgen::JsValue;

/// How many characters of an unrecognized user agent are kept. The string can run to a
/// paragraph of version numbers, and the sheet has one line.
const MOST: usize = 60;

/// Whether this browser has WebUSB at all.
///
/// ⚠️ Read from `navigator` directly: `web_sys::Navigator::usb` returns a `Usb` object
/// whatever the browser supports.
pub fn has_usb() -> bool {
    let Some(navigator) = navigator() else {
        return false;
    };
    field(&navigator, "usb").is_some()
}

/// The browser and operating system, as briefly as possible.
pub fn agent() -> String {
    let Some(navigator) = navigator() else {
        return "unknown".to_string();
    };
    field(&navigator, "userAgentData")
        .and_then(|data| branded(&data))
        .or_else(|| {
            field(&navigator, "userAgent")?
                .as_string()
                .map(|raw| super::agent::product(&raw).unwrap_or_else(|| cut(&raw)))
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn navigator() -> Option<JsValue> {
    Some(web_sys::window()?.navigator().into())
}

/// A property of a JavaScript object, unless it is undefined or null.
fn field(object: &JsValue, name: &str) -> Option<JsValue> {
    let held = js_sys::Reflect::get(object, &JsValue::from_str(name)).ok()?;
    (!held.is_undefined() && !held.is_null()).then_some(held)
}

/// `Google Chrome 141 · macOS`, from `navigator.userAgentData`, which only Chromium
/// browsers have. The platform version needs a permission prompt, so the name is all
/// this asks for.
fn branded(data: &JsValue) -> Option<String> {
    let brands: Vec<String> = js_sys::Array::from(&field(data, "brands")?)
        .iter()
        .filter_map(|entry| {
            let brand = field(&entry, "brand")?.as_string()?;
            // Chromium adds a fake brand such as `Not)A;Brand` to catch code that reads
            // the first entry.
            if brand.contains("Not") && brand.contains("Brand") {
                return None;
            }
            Some(format!(
                "{brand} {}",
                field(&entry, "version")?.as_string()?
            ))
        })
        .collect();
    // Every Chromium browser also lists Chromium, in a shuffled order.
    let named = brands
        .iter()
        .find(|brand| !brand.starts_with("Chromium"))
        .or_else(|| brands.first())?;
    match field(data, "platform").and_then(|platform| platform.as_string()) {
        Some(platform) if !platform.is_empty() => Some(format!("{named} · {platform}")),
        _ => Some(named.clone()),
    }
}

/// `text` cut to [`MOST`] characters, with an ellipsis if it was longer.
fn cut(text: &str) -> String {
    match text.char_indices().nth(MOST) {
        None => text.to_string(),
        Some((at, _)) => format!("{}…", &text[..at]),
    }
}
