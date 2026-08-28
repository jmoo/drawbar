//! Process-wide timer support without an async runtime.
//!
//! Native transfers share one timer thread; creating a thread per chunk would be
//! prohibitive for library transfers containing thousands of chunks.

use std::future::{poll_fn, Future};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::task::{Poll, Waker};
use std::time::{Duration, Instant};

struct Entry {
    at: Instant,
    waker: Mutex<Option<Waker>>,
}

impl Entry {
    fn update(&self, waker: &Waker) {
        let mut current = self.waker.lock().unwrap();
        if current.as_ref().is_some_and(|old| !old.will_wake(waker)) {
            *current = Some(waker.clone());
        }
    }
}

struct Timer {
    pending: Mutex<Vec<Arc<Entry>>>,
    signal: Condvar,
}

struct Registration {
    timer: &'static Timer,
    entry: Arc<Entry>,
}

impl Registration {
    fn new(at: Instant, waker: &Waker) -> Self {
        let timer = shared_timer();
        let entry = Arc::new(Entry {
            at,
            waker: Mutex::new(Some(waker.clone())),
        });
        timer.pending.lock().unwrap().push(Arc::clone(&entry));
        timer.signal.notify_one();
        Self { timer, entry }
    }

    fn update(&self, waker: &Waker) {
        self.entry.update(waker);
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.entry.waker.lock().unwrap().take();
        self.timer.signal.notify_one();
    }
}

fn shared_timer() -> &'static Timer {
    static SHARED: OnceLock<&'static Timer> = OnceLock::new();
    SHARED.get_or_init(|| {
        let timer: &'static Timer = Box::leak(Box::new(Timer {
            pending: Mutex::new(Vec::new()),
            signal: Condvar::new(),
        }));
        std::thread::Builder::new()
            .name("nord-usb-deadline".into())
            .spawn(move || run(timer))
            .expect("spawning the deadline thread");
        timer
    })
}

fn run(timer: &'static Timer) {
    let mut pending = timer.pending.lock().unwrap();
    loop {
        let now = Instant::now();
        let mut due = Vec::new();
        pending.retain(|entry| {
            let mut waker = entry.waker.lock().unwrap();
            match waker.as_ref() {
                None => false,
                Some(_) if entry.at <= now => {
                    due.push(waker.take().unwrap());
                    false
                }
                Some(_) => true,
            }
        });

        if !due.is_empty() {
            drop(pending);
            for waker in due {
                waker.wake();
            }
            pending = timer.pending.lock().unwrap();
            continue;
        }

        pending = match pending.iter().map(|entry| entry.at).min() {
            Some(at) => {
                let wait = at.saturating_duration_since(Instant::now());
                timer.signal.wait_timeout(pending, wait).unwrap().0
            }
            None => timer.signal.wait(pending).unwrap(),
        };
    }
}

/// Run `future` to completion, returning `None` when `limit` passes first.
///
/// Dropping an I/O future does not necessarily cancel work already submitted to
/// the operating system. Transport implementations must cancel that work before
/// issuing another request.
pub async fn with_timeout<F: Future>(future: F, limit: Duration) -> Option<F::Output> {
    let mut future = Box::pin(future);
    let deadline = Instant::now().checked_add(limit);
    let mut registration: Option<Registration> = None;

    poll_fn(move |cx| {
        if let Poll::Ready(value) = future.as_mut().poll(cx) {
            return Poll::Ready(Some(value));
        }
        if deadline.is_some_and(|at| Instant::now() >= at) {
            return Poll::Ready(None);
        }
        if let Some(deadline) = deadline {
            match &registration {
                Some(armed) => armed.update(cx.waker()),
                None => registration = Some(Registration::new(deadline, cx.waker())),
            }
        }
        Poll::Pending
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Wake;

    #[test]
    fn a_ready_future_returns_its_value() {
        let got = pollster::block_on(with_timeout(async { 7 }, Duration::from_secs(60)));
        assert_eq!(got, Some(7));
    }

    #[test]
    fn a_pending_future_times_out() {
        let got = pollster::block_on(with_timeout(
            poll_fn(|_| Poll::<()>::Pending),
            Duration::from_millis(20),
        ));
        assert_eq!(got, None);
    }

    #[test]
    fn a_shorter_deadline_is_not_blocked_by_an_earlier_registration() {
        let slow = std::thread::spawn(|| {
            pollster::block_on(with_timeout(
                poll_fn(|_| Poll::<()>::Pending),
                Duration::from_millis(150),
            ))
        });
        std::thread::sleep(Duration::from_millis(10));
        let quick = pollster::block_on(with_timeout(
            poll_fn(|_| Poll::<()>::Pending),
            Duration::from_millis(20),
        ));
        assert_eq!(quick, None);
        assert_eq!(slow.join().unwrap(), None);
    }

    struct Counter(AtomicUsize);

    impl Wake for Counter {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn a_registration_tracks_a_replacement_waker() {
        let first = Waker::from(Arc::new(Counter(AtomicUsize::new(0))));
        let second = Waker::from(Arc::new(Counter(AtomicUsize::new(0))));
        let entry = Entry {
            at: Instant::now(),
            waker: Mutex::new(Some(first)),
        };

        entry.update(&second);

        assert!(entry
            .waker
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .will_wake(&second));
    }
}
