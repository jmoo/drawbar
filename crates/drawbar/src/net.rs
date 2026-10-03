//! HTTP: the browser's `fetch` in a tab, and `ureq` over `rustls` in a window.

/// The most bytes a reply may hold.
pub const LIMIT: usize = 1 << 20;

#[cfg(target_arch = "wasm32")]
pub async fn get(url: &str) -> Result<Vec<u8>, String> {
    use wasm_bindgen::JsCast as _;
    use wasm_bindgen_futures::JsFuture;

    let window = web_sys::window().ok_or("no window to fetch from")?;
    let response: web_sys::Response = JsFuture::from(window.fetch_with_str(url))
        .await
        .map_err(|_| "the request failed".to_string())?
        .dyn_into()
        .map_err(|_| "the reply was not a response".to_string())?;
    if !response.ok() {
        return Err(format!("the server answered {}", response.status()));
    }
    let body = response
        .array_buffer()
        .map_err(|_| "the reply has no body".to_string())?;
    let body = JsFuture::from(body)
        .await
        .map_err(|_| "the body did not arrive".to_string())?;
    bounded(js_sys::Uint8Array::new(&body).to_vec())
}

/// The body `url` answers with, or why there is none: the request failed, the server
/// answered with an error, or the body is over [`LIMIT`] bytes.
///
/// ⚠️ In a window this blocks until the reply is in, so run it off the UI thread.
#[cfg(not(target_arch = "wasm32"))]
pub async fn get(url: &str) -> Result<Vec<u8>, String> {
    let mut response = agent()?.get(url).call().map_err(|e| e.to_string())?;
    read(response.body_mut().as_reader())
}

/// A client for one request: HTTPS only, trusting the system's roots, and given a
/// minute to finish.
#[cfg(not(target_arch = "wasm32"))]
fn agent() -> Result<ureq::Agent, String> {
    use std::time::Duration;
    use ureq::tls::{Certificate, RootCerts, TlsConfig, TlsProvider};

    // ⚠️ ureq is built without a crypto provider, so rustls has none until one is
    // installed. Installing again once one is in place is refused, which is harmless.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let found = rustls_native_certs::load_native_certs();
    if found.certs.is_empty() {
        return Err(match found.errors.first() {
            Some(why) => format!("the system's trusted certificates did not load ({why})"),
            None => "the system trusts no certificates".to_string(),
        });
    }
    let roots: Vec<Certificate<'static>> = found
        .certs
        .iter()
        .map(|cert| Certificate::from_der(cert.as_ref()).to_owned())
        .collect();
    let tls = TlsConfig::builder()
        .provider(TlsProvider::Rustls)
        .root_certs(RootCerts::new_with_certs(&roots))
        .build();
    Ok(ureq::Agent::config_builder()
        .tls_config(tls)
        .https_only(true)
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .into())
}

/// All of `body`, reading no more than one byte past [`LIMIT`].
#[cfg(not(target_arch = "wasm32"))]
fn read(body: impl std::io::Read) -> Result<Vec<u8>, String> {
    use std::io::Read as _;

    let mut bytes = Vec::new();
    body.take(LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    bounded(bytes)
}

fn bounded(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    match bytes.len() > LIMIT {
        true => Err(format!("the reply is over {LIMIT} bytes")),
        false => Ok(bytes),
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::io::Read as _;

    #[test]
    fn a_reply_of_exactly_the_limit_arrives_whole() {
        let bytes = read(std::io::repeat(7).take(LIMIT as u64)).expect("within the limit");
        assert_eq!(bytes.len(), LIMIT);
    }

    #[test]
    fn a_reply_one_byte_over_the_limit_is_refused() {
        let why = read(std::io::repeat(7).take(LIMIT as u64 + 1)).expect_err("over the limit");
        assert_eq!(why, format!("the reply is over {LIMIT} bytes"));
    }

    #[test]
    fn an_endless_reply_is_refused_rather_than_read_to_its_end() {
        assert!(read(std::io::repeat(7)).is_err());
    }
}
