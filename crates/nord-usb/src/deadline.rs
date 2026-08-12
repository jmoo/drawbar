//! A timeout for a single future, without pulling in a runtime.
//!
//! The crate has no timer source: `pollster` parks the calling thread and wakes on the
//! waker, and nothing else in the dependency set can schedule work. One shared thread
//! provides that source for the whole process.
//!
//! Every transfer is bounded ([`crate::session::WRITE_LIMIT`],
//! [`crate::session::READ_LIMIT`]), so this sits under the hot path — a piano is
//! thousands of chunks. A thread per call would mean a thread per chunk, so registrations
//! go to one long-lived thread that sleeps until the nearest deadline.

use std::future::{poll_fn, Future};
use std::sync::{Condvar, Mutex, OnceLock};
use std::task::{Poll, Waker};
use std::time::{Duration, Instant};

/// Deadlines waiting to fire, and the thread that fires them.
struct Timer {
    pending: Mutex<Vec<(Instant, Waker)>>,
    signal: Condvar,
}

fn shared_timer() -> &'static Timer {
    static SHARED: OnceLock<&'static Timer> = OnceLock::new();
    SHARED.get_or_init(|| {
        let shared: &'static Timer = Box::leak(Box::new(Timer {
            pending: Mutex::new(Vec::new()),
            signal: Condvar::new(),
        }));
        std::thread::Builder::new()
            .name("nord-usb-deadline".into())
            .spawn(move || run(shared))
            .expect("spawning the deadline thread");
        shared
    })
}

fn run(t: &'static Timer) {
    let mut pending = t.pending.lock().unwrap();
    loop {
        let now = Instant::now();
        // Wake everything due, and keep the rest.
        let mut i = 0;
        while i < pending.len() {
            if pending[i].0 <= now {
                let (_, waker) = pending.swap_remove(i);
                waker.wake();
            } else {
                i += 1;
            }
        }
        let next = pending.iter().map(|(at, _)| *at).min();
        pending = match next {
            Some(at) => {
                let wait = at.saturating_duration_since(Instant::now());
                t.signal.wait_timeout(pending, wait).unwrap().0
            }
            // Nothing registered: sleep until something is.
            None => t.signal.wait(pending).unwrap(),
        };
    }
}

fn register(at: Instant, waker: Waker) {
    let t = shared_timer();
    t.pending.lock().unwrap().push((at, waker));
    t.signal.notify_one();
}

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
        if Instant::now() >= deadline {
            return Poll::Ready(None);
        }
        // Registered once, not per poll: a transfer that wakes us several times before
        // completing would otherwise pile up duplicate entries on the timer.
        if !armed {
            armed = true;
            register(deadline, cx.waker().clone());
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

    /// The timer is shared, so a slow deadline must not hold up a quicker one behind it.
    #[test]
    fn deadlines_fire_independently_of_registration_order() {
        let slow = std::thread::spawn(|| {
            pollster::block_on(with_timeout(
                poll_fn(|_| Poll::<()>::Pending),
                Duration::from_secs(30),
            ))
        });
        std::thread::sleep(Duration::from_millis(20));
        let quick = pollster::block_on(with_timeout(
            poll_fn(|_| Poll::<()>::Pending),
            Duration::from_millis(50),
        ));
        assert_eq!(quick, None, "a later, shorter deadline did not fire first");
        drop(slow); // left running; the process outlives it
    }
}
