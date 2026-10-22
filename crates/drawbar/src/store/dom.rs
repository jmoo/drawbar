//! What the browser library makes of the browser's file system answers, kept apart from
//! the browser so it can be tested anywhere.

use std::io;

/// An error from the browser, by its `DOMException` name, as the kind of I/O error it is.
pub(super) fn failure(name: &str, message: &str) -> io::Error {
    let kind = match name {
        "NotFoundError" => io::ErrorKind::NotFound,
        "QuotaExceededError" => io::ErrorKind::StorageFull,
        "NotSupportedError" => io::ErrorKind::Unsupported,
        _ => io::ErrorKind::Other,
    };
    match name {
        "" | "Error" => io::Error::new(kind, message.to_string()),
        _ => io::Error::new(kind, format!("{name}: {message}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_browsers_errors_keep_their_kind_and_name() {
        let kind = |name| failure(name, "why").kind();
        assert_eq!(kind("NotFoundError"), io::ErrorKind::NotFound);
        assert_eq!(kind("QuotaExceededError"), io::ErrorKind::StorageFull);
        assert_eq!(kind("NotSupportedError"), io::ErrorKind::Unsupported);
        assert_eq!(kind("InvalidStateError"), io::ErrorKind::Other);
        let refused = failure("NotSupportedError", "The implementation did not support");
        assert_eq!(
            refused.to_string(),
            "NotSupportedError: The implementation did not support"
        );
        assert_eq!(failure("Error", "plain").to_string(), "plain");
    }
}
