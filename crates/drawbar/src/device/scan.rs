//! Which classes have been read, and which are still to read.
//!
//! Each class is one command: the worker opens a session, reads the class's counters and
//! then every bank in it, and streams the banks back one at a time. The queue holds
//! classes, and a class reports progress while it runs.

use std::collections::{HashMap, VecDeque};

use nord_usb::ObjectClass;

/// How far the background read has progressed through a class.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Progress {
    /// Banks read so far.
    pub done: u32,
    /// Banks expected, once the class's counters give enough to work it out.
    pub total: Option<u32>,
    /// The walk is still going.
    pub running: bool,
}

/// The classes still to read, and how far each got.
///
/// ⚠️ A class asked for while its own walk is running is queued again behind it: the
/// walk in flight finishes on the instrument whatever happens here, and what it has
/// read may already be stale. [`Scan::finished`] therefore never touches the queue.
#[derive(Default)]
pub struct Scan {
    queue: VecDeque<ObjectClass>,
    /// Keyed by the raw class number, because [`ObjectClass`] is not `Hash`.
    progress: HashMap<u32, Progress>,
    /// When each class last answered, on egui's clock.
    read: HashMap<u32, f64>,
}

impl Scan {
    /// Read `class` from the start. Queued once, however often it is asked for.
    pub fn start(&mut self, class: ObjectClass) {
        if !self.queue.contains(&class) {
            self.queue.push_back(class);
        }
        self.progress.insert(
            class.to_raw(),
            Progress {
                done: 0,
                total: None,
                running: true,
            },
        );
    }

    /// Take the next class to read off the queue.
    pub fn take(&mut self) -> Option<ObjectClass> {
        self.queue.pop_front()
    }

    /// The class's counters arrived, and with them how many banks to expect.
    pub fn expect(&mut self, class: ObjectClass, total: Option<u32>) {
        self.progress.entry(class.to_raw()).or_default().total = total;
    }

    /// One bank landed.
    pub fn bank(&mut self, class: ObjectClass, bank: u32) {
        let progress = self.progress.entry(class.to_raw()).or_default();
        progress.done = progress.done.max(bank);
    }

    /// The walk ended, whether it ran out of banks or gave up partway. The class stays
    /// running if it was asked for again meanwhile.
    pub fn finished(&mut self, class: ObjectClass) {
        let again = self.queue.contains(&class);
        self.progress.entry(class.to_raw()).or_default().running = again;
    }

    pub fn progress(&self, class: ObjectClass) -> Option<Progress> {
        self.progress.get(&class.to_raw()).copied()
    }

    /// Note that the class has just answered, at `now` on egui's clock.
    ///
    /// ⚠️ Called for every bank a walk delivers and at the end of the walk, because what
    /// matters is how stale the names on screen are, not when the session opened.
    pub fn heard(&mut self, class: ObjectClass, now: f64) {
        self.read.insert(class.to_raw(), now);
    }

    /// When the class last answered, if it ever has.
    pub fn read_at(&self, class: ObjectClass) -> Option<f64> {
        self.read.get(&class.to_raw()).copied()
    }

    pub fn clear(&mut self) {
        self.queue.clear();
        self.progress.clear();
        self.read.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A class is queued once and reports itself as it goes.
    #[test]
    fn a_class_reports_its_banks_as_they_land() {
        let mut scan = Scan::default();
        scan.start(ObjectClass::Program);
        scan.start(ObjectClass::Program);
        assert_eq!(scan.take(), Some(ObjectClass::Program));
        assert_eq!(scan.take(), None, "one class, one walk");

        scan.expect(ObjectClass::Program, Some(8));
        for bank in 1..=3 {
            scan.bank(ObjectClass::Program, bank);
        }
        let progress = scan.progress(ObjectClass::Program).unwrap();
        assert_eq!((progress.done, progress.total), (3, Some(8)));
        assert!(progress.running);
    }

    /// A walk that ends stops reporting itself as running, however it ended.
    #[test]
    fn a_finished_walk_stops_running() {
        let mut scan = Scan::default();
        scan.start(ObjectClass::Sample);
        scan.take();
        scan.bank(ObjectClass::Sample, 1);
        scan.finished(ObjectClass::Sample);
        let progress = scan.progress(ObjectClass::Sample).unwrap();
        assert!(!progress.running);
        assert_eq!(progress.done, 1);
    }

    #[test]
    fn reading_a_class_again_starts_its_count_over() {
        let mut scan = Scan::default();
        scan.start(ObjectClass::Program);
        scan.take();
        scan.bank(ObjectClass::Program, 4);
        scan.finished(ObjectClass::Program);

        scan.start(ObjectClass::Program);
        assert_eq!(scan.progress(ObjectClass::Program).unwrap().done, 0);
        assert_eq!(scan.take(), Some(ObjectClass::Program));
    }

    /// Clearing the scan, as releasing the instrument does, forgets when each class
    /// answered.
    #[test]
    fn a_class_remembers_when_it_last_answered() {
        let mut scan = Scan::default();
        assert_eq!(scan.read_at(ObjectClass::Program), None);

        scan.heard(ObjectClass::Program, 12.5);
        scan.heard(ObjectClass::Program, 31.0);
        assert_eq!(scan.read_at(ObjectClass::Program), Some(31.0));
        assert_eq!(scan.read_at(ObjectClass::Sample), None, "another folder");

        scan.clear();
        assert_eq!(scan.read_at(ObjectClass::Program), None);
    }

    #[test]
    fn finishing_one_class_leaves_the_others_queued() {
        let mut scan = Scan::default();
        scan.start(ObjectClass::Program);
        scan.start(ObjectClass::SetList);
        assert_eq!(scan.take(), Some(ObjectClass::Program));
        scan.finished(ObjectClass::Program);
        assert_eq!(scan.take(), Some(ObjectClass::SetList));
    }

    /// Asking to read a folder while it is being read queues a second read.
    #[test]
    fn a_class_asked_for_during_its_own_walk_is_read_again() {
        let mut scan = Scan::default();
        scan.start(ObjectClass::Program);
        assert_eq!(scan.take(), Some(ObjectClass::Program));
        scan.bank(ObjectClass::Program, 2);

        scan.start(ObjectClass::Program);
        scan.finished(ObjectClass::Program);
        assert!(
            scan.progress(ObjectClass::Program).unwrap().running,
            "the second walk is still queued"
        );
        assert_eq!(scan.take(), Some(ObjectClass::Program));
        scan.finished(ObjectClass::Program);
        assert!(!scan.progress(ObjectClass::Program).unwrap().running);
        assert_eq!(scan.take(), None);
    }
}
