//! Which classes have been read, and which are still owed.
//!
//! One class is one command: the worker opens a session, reads the class's counters and
//! then every bank inside it, and streams a bank at a time back. So the queue holds
//! classes, and the progress each one reports arrives while it is still running.

use std::collections::{HashMap, VecDeque};

use nord_usb::ObjectClass;

/// How far through a class the background read has got.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Progress {
    /// Banks read so far.
    pub done: u32,
    /// Banks expected, once the class's counters have said enough to work it out.
    pub total: Option<u32>,
    /// The walk is still going.
    pub running: bool,
}

/// The classes still to read, and how far each got.
#[derive(Default)]
pub struct Scan {
    queue: VecDeque<ObjectClass>,
    /// Keyed by the raw class number, because [`ObjectClass`] is not `Hash`.
    progress: HashMap<u32, Progress>,
}

impl Scan {
    /// Read `class` from the top. Queued once however often it is asked for.
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

    /// The walk ended — whether it ran out of banks or gave up part-way.
    pub fn finished(&mut self, class: ObjectClass) {
        self.queue.retain(|queued| *queued != class);
        self.progress.entry(class.to_raw()).or_default().running = false;
    }

    pub fn progress(&self, class: ObjectClass) -> Option<Progress> {
        self.progress.get(&class.to_raw()).copied()
    }

    pub fn clear(&mut self) {
        self.queue.clear();
        self.progress.clear();
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

    /// Reading a class again starts its count over rather than carrying on from where
    /// the last walk stopped.
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

    /// One class finishing leaves the others queued.
    #[test]
    fn finishing_one_class_leaves_the_others_queued() {
        let mut scan = Scan::default();
        scan.start(ObjectClass::Program);
        scan.start(ObjectClass::SetList);
        scan.finished(ObjectClass::Program);
        assert_eq!(scan.take(), Some(ObjectClass::SetList));
    }
}
