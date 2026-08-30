//! Process-wide timer support without an async runtime.
//!
//! Native transfers share one timer thread; creating a thread per chunk would be
//! prohibitive for library transfers containing thousands of chunks.

use std::future::{poll_fn, Future};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::task::{Poll, Waker};
use std::time::{Duration, Instant};

struct Entry {
    at: Instant,
    waker: Mutex<Option<Waker>>,
}

impl Entry {
    fn update(&self, waker: &Waker) {
        let replacement = waker.clone();
        let previous = {
            let mut current = self.waker.lock().unwrap();
            if current.as_ref().is_none_or(|old| !old.will_wake(waker)) {
                current.replace(replacement)
            } else {
                None
            }
        };
        drop(previous);
    }

    fn cancel(&self) {
        let current = self.waker.lock().unwrap().take();
        drop(current);
    }
}

struct Timer {
    pending: Mutex<Vec<Arc<Entry>>>,
    signal: Condvar,
}

impl Timer {
    fn new() -> Self {
        Self {
            pending: Mutex::new(Vec::new()),
            signal: Condvar::new(),
        }
    }

    /// Remove cancelled and due entries while holding the queue lock, but wake only
    /// after releasing it so a waker may immediately register another deadline.
    fn take_due(&self, now: Instant) -> Vec<Waker> {
        let mut pending = self.pending.lock().unwrap();
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
        due
    }
}

struct Registration {
    timer: &'static Timer,
    entry: Arc<Entry>,
}

impl Registration {
    fn new(at: Instant, waker: &Waker) -> Self {
        Self::new_on(shared_timer(), at, waker)
    }

    fn new_on(timer: &'static Timer, at: Instant, waker: &Waker) -> Self {
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
        self.entry.cancel();
        self.timer.signal.notify_one();
    }
}

fn shared_timer() -> &'static Timer {
    static SHARED: OnceLock<&'static Timer> = OnceLock::new();
    SHARED.get_or_init(|| {
        let timer: &'static Timer = Box::leak(Box::new(Timer::new()));
        std::thread::Builder::new()
            .name("nord-usb-deadline".into())
            .spawn(move || run(timer))
            .expect("spawning the deadline thread");
        timer
    })
}

fn run(timer: &'static Timer) {
    loop {
        let due = timer.take_due(Instant::now());

        if !due.is_empty() {
            for waker in due {
                let _ = catch_unwind(AssertUnwindSafe(|| waker.wake()));
            }
            continue;
        }

        let pending = timer.pending.lock().unwrap();
        match pending.iter().map(|entry| entry.at).min() {
            Some(at) => {
                let wait = at.saturating_duration_since(Instant::now());
                drop(timer.signal.wait_timeout(pending, wait).unwrap().0);
            }
            None => drop(timer.signal.wait(pending).unwrap()),
        }
    }
}

/// Run `future` to completion, returning `None` when `limit` passes first.
///
/// Dropping an I/O future does not necessarily cancel work already submitted to
/// the operating system. Transport implementations must cancel that work before
/// issuing another request.
///
/// # Panics
///
/// Panics when `limit` cannot be represented as a deadline from the current instant.
pub async fn with_timeout<F: Future>(future: F, limit: Duration) -> Option<F::Output> {
    let mut future = Box::pin(future);
    let deadline = Instant::now()
        .checked_add(limit)
        .expect("deadline overflow: limit cannot be represented from the current instant");
    let mut registration: Option<Registration> = None;

    poll_fn(move |cx| {
        if let Poll::Ready(value) = future.as_mut().poll(cx) {
            return Poll::Ready(Some(value));
        }
        if Instant::now() >= deadline {
            return Poll::Ready(None);
        }
        match &registration {
            Some(armed) => armed.update(cx.waker()),
            None => registration = Some(Registration::new(deadline, cx.waker())),
        }
        Poll::Pending
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::task::{Context, Wake};

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
    fn an_unrepresentable_deadline_panics() {
        let result = std::panic::catch_unwind(|| {
            pollster::block_on(with_timeout(async {}, Duration::MAX));
        });
        assert!(result.is_err());
    }

    #[test]
    fn a_new_earlier_deadline_interrupts_the_timers_wait() {
        let waker = Waker::from(Arc::new(Counter(AtomicUsize::new(0))));
        let _later = Registration::new(Instant::now() + Duration::from_secs(5), &waker);
        std::thread::sleep(Duration::from_millis(20));

        let started = Instant::now();
        let result = pollster::block_on(with_timeout(
            poll_fn(|_| Poll::<()>::Pending),
            Duration::from_millis(40),
        ));

        assert_eq!(result, None);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    struct Counter(AtomicUsize);

    impl Wake for Counter {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn cancelling_a_registration_purges_it_when_the_timer_scans() {
        let timer: &'static Timer = Box::leak(Box::new(Timer::new()));
        let waker = Waker::from(Arc::new(Counter(AtomicUsize::new(0))));
        let registration = Registration::new_on(timer, Instant::now(), &waker);
        assert_eq!(timer.pending.lock().unwrap().len(), 1);

        drop(registration);

        assert!(timer.take_due(Instant::now()).is_empty());
        assert!(timer.pending.lock().unwrap().is_empty());
    }

    struct Signal {
        woke: AtomicBool,
        count: AtomicUsize,
        lock: Mutex<()>,
        changed: Condvar,
    }

    impl Signal {
        fn new() -> Self {
            Self {
                woke: AtomicBool::new(false),
                count: AtomicUsize::new(0),
                lock: Mutex::new(()),
                changed: Condvar::new(),
            }
        }

        fn wait(&self) {
            let guard = self.lock.lock().unwrap();
            let (_guard, result) = self
                .changed
                .wait_timeout_while(guard, Duration::from_secs(1), |_| {
                    !self.woke.load(Ordering::Acquire)
                })
                .unwrap();
            assert!(!result.timed_out(), "timer did not wake the future");
        }
    }

    impl Wake for Signal {
        fn wake(self: Arc<Self>) {
            self.count.fetch_add(1, Ordering::Relaxed);
            self.woke.store(true, Ordering::Release);
            self.changed.notify_all();
        }
    }

    #[test]
    fn a_repolled_future_is_woken_by_its_new_waker() {
        let first = Arc::new(Signal::new());
        let second = Arc::new(Signal::new());
        let first_waker = Waker::from(Arc::clone(&first));
        let second_waker = Waker::from(Arc::clone(&second));
        let mut future = Box::pin(with_timeout(
            poll_fn(|_| Poll::<()>::Pending),
            Duration::from_millis(40),
        ));

        assert!(matches!(
            future.as_mut().poll(&mut Context::from_waker(&first_waker)),
            Poll::Pending
        ));
        assert!(matches!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(&second_waker)),
            Poll::Pending
        ));

        second.wait();
        assert_eq!(second.count.load(Ordering::Acquire), 1);
        assert_eq!(first.count.load(Ordering::Acquire), 0);
        assert!(matches!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(&second_waker)),
            Poll::Ready(None)
        ));
    }

    struct PanicWake;

    impl Wake for PanicWake {
        fn wake(self: Arc<Self>) {
            panic!("test waker panic");
        }
    }

    #[test]
    fn a_panicking_waker_does_not_kill_the_timer_thread() {
        let bad = Waker::from(Arc::new(PanicWake));
        let good = Arc::new(Signal::new());
        let good_waker = Waker::from(Arc::clone(&good));
        let at = Instant::now() + Duration::from_millis(20);
        let _bad = Registration::new(at, &bad);
        let _good = Registration::new(at + Duration::from_millis(20), &good_waker);

        good.wait();
        assert_eq!(good.count.load(Ordering::Acquire), 1);
    }
}
