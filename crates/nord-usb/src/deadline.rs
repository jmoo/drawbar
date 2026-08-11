//! A timeout for a single future, without pulling in a runtime.
//!
//! The crate has no timer source: `pollster` parks the calling thread and wakes on the
//! waker, and nothing else in the dependency set can schedule work. This arms one
//! sleeping thread per call to wake that parked thread when the deadline passes.
//!
//! Thread-per-call is fine for its one caller — [`Transport::read_timeout`] on the
//! probe path, which runs a handful of times in a session. It is not suitable for a
//! hot loop.
//!
//! [`Transport::read_timeout`]: crate::transport::Transport::read_timeout

use std::future::{poll_fn, Future};
use std::task::Poll;
use std::time::{Duration, Instant};

/// Run `fut` to completion, giving up after `limit`.
///
/// `None` means the deadline passed first. The future is dropped at that point, which
/// cancels it as far as Rust is concerned — but **a dropped future does not cancel work
/// already handed to the OS**. A caller that submitted an I/O request must still cancel
/// it at the device layer, or the next read will collect the abandoned reply and every
/// request after it will be paired with the wrong response.
pub async fn with_timeout<F: Future>(fut: F, limit: Duration) -> Option<F::Output> {
    let mut fut = Box::pin(fut);
    let deadline = Instant::now() + limit;
    let mut armed = false;

    poll_fn(move |cx| {
        if let Poll::Ready(v) = fut.as_mut().poll(cx) {
            return Poll::Ready(Some(v));
        }
        let now = Instant::now();
        if now >= deadline {
            return Poll::Ready(None);
        }
        if !armed {
            armed = true;
            let waker = cx.waker().clone();
            let remaining = deadline - now;
            std::thread::spawn(move || {
                std::thread::sleep(remaining);
                waker.wake();
            });
        }
        Poll::Pending
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ready_future_returns_its_value() {
        let got = pollster::block_on(with_timeout(async { 7 }, Duration::from_secs(60)));
        assert_eq!(got, Some(7));
    }

    #[test]
    fn a_future_that_never_completes_times_out() {
        let got = pollster::block_on(with_timeout(
            poll_fn(|_| Poll::<()>::Pending),
            Duration::from_millis(50),
        ));
        assert_eq!(got, None);
    }
}
