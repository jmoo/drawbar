//! The desktop half: the reports an operator sends. The desktop sends no rows.

use super::{id_from, Undelivered, ENDPOINT};

/// Whether a send is worth trying. The desktop cannot know short of trying.
pub fn online() -> bool {
    true
}

/// Send a report, `body` being its JSON.
///
/// ⚠️ Blocks until the collector answers, so run it off the UI thread.
pub async fn submit(body: String) -> Result<(), Undelivered> {
    match crate::net::post(&format!("{ENDPOINT}/report"), body).await {
        Ok(status) if (200..300).contains(&status) => Ok(()),
        Ok(status) => Err(Undelivered::Refused(status)),
        Err(_) => Err(Undelivered::Unreachable),
    }
}

/// A report's id. Its bytes come from the standard library's hasher keys, which are
/// random per process, mixed with the clock: an id only has to be unlikely to collide.
pub fn report_id() -> String {
    use std::hash::{BuildHasher, Hasher};

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();
    let mut random = [0u8; 10];
    for (part, chunk) in random.chunks_mut(8).enumerate() {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_usize(part);
        hasher.write_u128(nanos);
        chunk.copy_from_slice(&hasher.finish().to_le_bytes()[..chunk.len()]);
    }
    id_from(&random)
}
