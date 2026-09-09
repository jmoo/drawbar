//! How much room a folder has, what is in it, and what the queue would put there.
//!
//! Every figure here is the instrument's own: a [`Status`] entry counted in its
//! partition's unit, and the [`AllocationUnit`] that says what one of those units is
//! worth in bytes. No capacity constant lives in this app.

use eframe::egui;
use nord_usb::wire::{AllocationUnit, Status};
use nord_usb::ObjectClass;

use crate::app::{accent, warn};
use crate::device::DeviceState;
use crate::queue::Queue;
use crate::workspace::Workspace;

/// The trough's height, wherever a meter is drawn.
pub const TROUGH: f32 = 5.0;

/// The point past which a fill stops reading as room and starts reading as a warning.
const CROWDED: f32 = 0.9;

/// How full one class's partition is, and what the queue would add to it.
///
/// ⚠️ Counted in the partition's own unit, never in bytes: a slot-addressed class counts
/// items and a library counts blocks of [`AllocationUnit`] net bytes each.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Meter {
    pub used: u64,
    pub total: u64,
    /// What the queue would add. An entry replacing what is in its slot adds nothing.
    pub queued: u64,
}

impl Meter {
    /// The share of the partition its contents take.
    pub fn filled(&self) -> f32 {
        match self.total {
            0 => 0.0,
            total => (self.used as f32 / total as f32).clamp(0.0, 1.0),
        }
    }

    /// The share the queue would add, cut to whatever is left of the trough.
    pub fn incoming(&self) -> f32 {
        match self.total {
            0 => 0.0,
            total => (self.queued as f32 / total as f32).clamp(0.0, 1.0 - self.filled()),
        }
    }

    /// Whether the fill has passed the point where it is a warning rather than a figure.
    pub fn crowded(&self) -> bool {
        self.filled() > CROWDED
    }
}

/// What a class's partition holds and what is on its way to it, or nothing for a class
/// whose counters have not been read.
pub fn meter(
    class: ObjectClass,
    inventory: &[Status],
    unit: Option<AllocationUnit>,
    queue: &Queue,
    workspace: &Workspace,
) -> Option<Meter> {
    let status = inventory.iter().find(|status| status.class == class)?;
    let (used, total) = match status.slots() {
        Some(slots) => (u64::from(status.count), u64::from(slots)),
        None => (u64::from(status.used), status.total()),
    };
    Some(Meter {
        used,
        total,
        queued: incoming(class, status.slots().is_some(), unit, queue, workspace),
    })
}

/// What the queue would add to a class, in the unit that class's meter counts in.
///
/// ⚠️ An entry that replaces what is in its slot adds nothing — the write frees what it
/// overwrites. A slot-addressed class counts the slots that would fill; a library counts
/// the blocks its bodies occupy, which nothing can work out until the partition has
/// reported its allocation unit.
fn incoming(
    class: ObjectClass,
    by_slot: bool,
    unit: Option<AllocationUnit>,
    queue: &Queue,
    workspace: &Workspace,
) -> u64 {
    queue
        .entries()
        .iter()
        .filter(|held| held.class == class && held.replaces.occupant().is_none())
        .filter_map(|held| match by_slot {
            true => Some(1),
            false => {
                let bytes = workspace.get(held.id)?.bytes.len();
                unit?.blocks_for(bytes).ok().map(u64::from)
            }
        })
        .sum()
}

/// What is left in a class's partition, in bytes where the allocation unit says what a
/// unit is worth and in bare units where nothing has.
pub fn free_space(
    class: ObjectClass,
    inventory: &[Status],
    unit: Option<AllocationUnit>,
) -> Option<String> {
    let status = inventory.iter().find(|status| status.class == class)?;
    let (free, total) = (status.available(), status.total());
    Some(match unit {
        Some(unit) => format!(
            "{} free of {}",
            measure(free.saturating_mul(u64::from(unit.get()))),
            measure(total.saturating_mul(u64::from(unit.get()))),
        ),
        None => format!("{free} of {total} free, in units this folder counts in"),
    })
}

/// The one thing in the queue that most nearly does not fit, and whether it does.
///
/// ⚠️ Only where the partition has reported its allocation unit: free space is a count
/// of units, and nothing turns one into bytes without it.
pub fn constraint(queue: &Queue, workspace: &Workspace, device: &DeviceState) -> Option<String> {
    let (name, bytes, class) = queue
        .entries()
        .iter()
        .filter_map(|held| {
            let entity = workspace.get(held.id)?;
            Some((entity.name.clone(), entity.bytes.len() as u64, held.class))
        })
        .max_by_key(|(_, bytes, _)| *bytes)?;
    let unit = device.allocation_unit(class)?;
    let status = device
        .inventory
        .iter()
        .find(|status| status.class == class)?;
    let free = status.available().saturating_mul(u64::from(unit.get()));
    let verdict = match bytes <= free {
        true => "it fits",
        false => "it does not fit",
    };
    Some(format!(
        "{name} is {} and {} is free — {verdict}.",
        measure(bytes),
        measure(free)
    ))
}

/// A size in the widest unit that leaves a figure worth reading.
pub fn measure(bytes: u64) -> String {
    const K: f64 = 1024.0;
    let held = bytes as f64;
    if held < K {
        return format!("{bytes} B");
    }
    if held < K * K {
        return format!("{:.1} kB", held / K);
    }
    format!("{:.1} MB", held / (K * K))
}

/// The trough, what the folder holds, and what the queue would add to it.
///
/// ⚠️ The bar takes the tone and the readout beside it keeps its own ink: accent on the
/// panel measures 4.1:1, which carries as a bar and fails as 11 px text.
pub fn bar(ui: &mut egui::Ui, meter: Meter) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), TROUGH),
        egui::Sense::hover(),
    );
    let visuals = ui.visuals().clone();
    let painter = ui.painter().clone();
    painter.rect_filled(rect, 1.0, visuals.extreme_bg_color);

    let filled = rect.width() * meter.filled();
    let tone = match meter.crowded() {
        true => warn(&visuals),
        false => accent(&visuals),
    };
    if filled > 0.0 {
        painter.rect_filled(
            egui::Rect::from_min_size(rect.min, egui::vec2(filled, rect.height())),
            1.0,
            tone,
        );
    }
    let incoming = rect.width() * meter.incoming();
    if incoming > 0.0 {
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(rect.left() + filled, rect.top()),
                egui::vec2(incoming, rect.height()),
            ),
            1.0,
            warn(&visuals),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{pretend_allocation_unit, Device};
    use crate::log::Log;
    use crate::queue::enqueue;
    use crate::workspace::{Fresh, Origin};
    use nord_usb::Location;

    fn status(class: ObjectClass, count: u32, free: u32, used: u32) -> Status {
        Status {
            class,
            count,
            free,
            used,
            dirty: 0,
            spare: 0,
        }
    }

    fn at(slot: u32) -> Location {
        Location { bank: 0, slot }
    }

    /// A slot-addressed folder counts items, and what is queued for a slot nothing holds
    /// is a slot that would fill.
    #[test]
    fn a_slot_folder_meters_items_and_counts_only_what_would_fill_a_slot() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = Log::default();
        let class = ObjectClass::Program;
        let bytes = {
            let id = workspace.create(Fresh::Program, &mut log).unwrap();
            let held = workspace.get(id).unwrap().bytes.clone();
            workspace.remove(id, &mut log);
            held
        };
        // 100 of 400 slots, each program costing 121 bytes of the partition's count.
        let inventory = [status(class, 100, 300 * 121, 100 * 121)];
        // Two vacant destinations and one that is taken.
        device.pretend_scanned(class, 1, &["", "", "Africa Split"]);

        let mut queue = Queue::default();
        for slot in 0..3 {
            let id = workspace.ingest(
                format!("sound {slot}"),
                Origin::Fresh,
                bytes.clone(),
                &mut log,
            );
            enqueue(
                &workspace,
                &mut device,
                &mut queue,
                &mut log,
                id,
                class,
                at(slot),
            );
        }

        let held = meter(class, &inventory, None, &queue, &workspace).expect("the class was read");
        assert_eq!(held.used, 100);
        assert_eq!(held.total, 400);
        assert_eq!(held.queued, 2, "the third replaces what is there");
        assert_eq!(held.filled(), 0.25);
        assert_eq!(held.incoming(), 2.0 / 400.0);
        assert!(!held.crowded());
    }

    /// A library counts blocks of its partition's allocation unit, and what a body would
    /// occupy cannot be worked out until that unit has arrived.
    #[test]
    fn a_library_meters_blocks_and_says_nothing_about_them_without_its_unit() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = Log::default();
        let class = ObjectClass::Sample;
        let inventory = [status(class, 84, 64, 1472)];
        device.pretend_scanned(class, 1, &[""]);

        let id = workspace.ingest("a sample".into(), Origin::Fresh, vec![0; 300_000], &mut log);
        let mut queue = Queue::default();
        enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            id,
            class,
            at(0),
        );

        let blind = meter(class, &inventory, None, &queue, &workspace).unwrap();
        assert_eq!(blind.used, 1472);
        assert_eq!(blind.total, 1536);
        assert_eq!(blind.queued, 0, "nothing sizes a body without the unit");

        let unit = pretend_allocation_unit(class, 131_064);
        let known = meter(class, &inventory, Some(unit), &queue, &workspace).unwrap();
        // 300 000 / 131 064 = 2.29, and a partial block still costs a whole one.
        assert_eq!(known.queued, 3);
        assert!(known.crowded(), "1472 of 1536 is past nine tenths");
    }

    /// The queued segment never runs past the end of the trough, whatever is waiting.
    #[test]
    fn the_queued_segment_stops_at_the_end_of_the_trough() {
        let full = Meter {
            used: 380,
            total: 400,
            queued: 500,
        };
        assert_eq!(full.filled(), 0.95);
        assert!((full.filled() + full.incoming() - 1.0).abs() < f32::EPSILON);
        // A class whose counters say nothing divides into nothing.
        let unread = Meter {
            used: 0,
            total: 0,
            queued: 4,
        };
        assert_eq!((unread.filled(), unread.incoming()), (0.0, 0.0));
    }

    /// The sentence names the largest thing waiting, its size, the room left, and
    /// whether one goes into the other — and says nothing at all with an empty queue.
    #[test]
    fn the_binding_constraint_is_the_largest_thing_waiting_against_the_room_left() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = Log::default();
        let class = ObjectClass::Sample;
        let mut queue = Queue::default();
        assert_eq!(constraint(&queue, &workspace, &device.state), None);

        device.pretend_scanned(class, 1, &["", ""]);
        device.state.inventory.push(status(class, 84, 64, 1472));
        for (name, size, slot) in [("small", 4096, 0), ("Grand", 5_347_738, 1)] {
            let id = workspace.ingest(name.into(), Origin::Fresh, vec![0; size], &mut log);
            enqueue(
                &workspace,
                &mut device,
                &mut queue,
                &mut log,
                id,
                class,
                at(slot),
            );
        }
        // Nothing says what a block is worth, so nothing says whether anything fits.
        assert_eq!(constraint(&queue, &workspace, &device.state), None);

        // 64 blocks of 131 064 bytes is 8.0 MB, and 5 347 738 bytes is 5.1 MB.
        device.pretend_partitions(&crate::device::ELECTRO5);
        assert_eq!(
            constraint(&queue, &workspace, &device.state).as_deref(),
            Some("Grand is 5.1 MB and 8.0 MB is free — it fits.")
        );

        device.state.inventory.clear();
        device.state.inventory.push(status(class, 84, 8, 1528));
        assert!(constraint(&queue, &workspace, &device.state)
            .is_some_and(|said| said.ends_with("it does not fit.")));
    }

    /// What is left reads in bytes once the partition has said what a unit is worth, and
    /// in the partition's own units before that.
    #[test]
    fn free_space_reads_in_bytes_only_once_the_allocation_unit_has_arrived() {
        let class = ObjectClass::Sample;
        let inventory = [status(class, 84, 64, 1472)];
        assert_eq!(
            free_space(class, &inventory, None).as_deref(),
            Some("64 of 1536 free, in units this folder counts in")
        );
        assert_eq!(
            free_space(
                class,
                &inventory,
                Some(pretend_allocation_unit(class, 131_064))
            )
            .as_deref(),
            Some("8.0 MB free of 192.0 MB")
        );
        assert_eq!(free_space(ObjectClass::Program, &inventory, None), None);
    }
}
