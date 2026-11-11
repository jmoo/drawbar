//! What the browser library makes of the browser's file system answers, kept apart from
//! the browser so it can be tested anywhere.

use std::io;

/// An error from the browser, by its `DOMException` name, as the kind of I/O error it is.
pub(super) fn failure(name: &str, message: &str) -> io::Error {
    let kind = match name {
        "NotFoundError" => io::ErrorKind::NotFound,
        "QuotaExceededError" => io::ErrorKind::StorageFull,
        "NotSupportedError" => io::ErrorKind::Unsupported,
        "NotAllowedError" | "NoModificationAllowedError" => io::ErrorKind::PermissionDenied,
        _ => io::ErrorKind::Other,
    };
    super::storage::plain(kind, || match name {
        "" | "Error" => message.to_string(),
        _ => format!("{name}: {message}"),
    })
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
        assert_eq!(kind("NotAllowedError"), io::ErrorKind::PermissionDenied);
        assert_eq!(
            kind("NoModificationAllowedError"),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(kind("InvalidStateError"), io::ErrorKind::Other);
        let refused = failure("NotSupportedError", "The implementation did not support");
        assert_eq!(
            refused.to_string(),
            "NotSupportedError: The implementation did not support"
        );
        assert_eq!(failure("Error", "plain").to_string(), "plain");
    }

    #[test]
    fn a_browser_that_will_not_write_says_so_in_toshokans_words() {
        let denied = toshokan::IoError::Denied.to_string();
        for name in ["NotAllowedError", "NoModificationAllowedError"] {
            let said = failure(name, "An attempt was made to write").to_string();
            assert_eq!(said, denied, "{name}");
        }
        let full = failure("QuotaExceededError", "The quota has been exceeded");
        assert_eq!(full.to_string(), toshokan::IoError::NoSpace.to_string());
    }
}
