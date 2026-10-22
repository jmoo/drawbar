//! How much space a folder has, what is in it, and what the queue would add.
//!
//! Every figure here comes from the instrument: a [`Status`] entry counted in its
//! partition's unit, and the [`AllocationUnit`] that gives that unit's size in bytes.
//! This app holds no capacity constants.

use eframe::egui;
use nord_usb::wire::{AllocationUnit, Status};
use nord_usb::{Location, ObjectClass};

use crate::app::{accent, canvas, warn};
use crate::device::DeviceState;
use crate::queue::Queue;
use crate::workspace::Workspace;

/// The trough's height and rounding, wherever a meter is drawn, and the gap between what
/// a folder holds and what the queue would add.
pub const TROUGH: f32 = 6.0;
const ROUND: u8 = 3;
const SPLIT: f32 = 2.0;

/// The fill fraction past which a meter shows a warning.
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

    /// The share the queue would add, clamped to what is left of the trough.
    pub fn incoming(&self) -> f32 {
        match self.total {
            0 => 0.0,
            total => (self.queued as f32 / total as f32).clamp(0.0, 1.0 - self.filled()),
        }
    }

    /// Whether the fill has passed the warning point.
    pub fn crowded(&self) -> bool {
        self.filled() > CROWDED
    }
}

/// Whether a class's partition can fill: counted in bytes, or divided into more than one
/// bank.
///
/// ⚠️ Anything else is a single bank of fixed slots. Its heading already shows the count,
/// and a bar would suggest space running out where only slots can.
fn fills(class: ObjectClass, unit: Option<AllocationUnit>, banks: usize) -> bool {
    (class.is_library() && unit.is_some()) || banks > 1
}

/// What a class's partition holds and what is queued for it, or `None` for a class whose
/// counters have not been read or whose partition cannot fill.
///
/// ⚠️ A partition reporting a total of zero has no meter either: nothing can be written
/// there, so an empty trough would misrepresent it.
pub fn meter(
    class: ObjectClass,
    device: &DeviceState,
    queue: &Queue,
    workspace: &Workspace,
) -> Option<Meter> {
    let unit = device.allocation_unit(class);
    if !fills(class, unit, device.banks(class)) {
        return None;
    }
    let status = counted(class, device)?;
    if status.total() == 0 {
        return None;
    }
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

/// What the queue would add to a class, in the unit its meter counts.
///
/// ⚠️ An entry that replaces what is in its slot adds nothing: the write frees what it
/// overwrites. A slot-addressed class counts the slots that would fill; a library counts
/// the blocks its bodies would occupy, which cannot be computed until the partition has
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
                let bytes = usize::try_from(workspace.get(held.id)?.size()).ok()?;
                unit?.blocks_for(bytes).ok().map(u64::from)
            }
        })
        .sum()
}

/// The free space in a class's partition: in bytes when the allocation unit is known,
/// otherwise in bare units.
pub fn free_space(class: ObjectClass, device: &DeviceState) -> Option<String> {
    let status = counted(class, device)?;
    let (free, total) = (status.available(), status.total());
    Some(match device.allocation_unit(class) {
        Some(unit) => format!(
            "{} free of {}",
            measure(free.saturating_mul(u64::from(unit.get()))),
            measure(total.saturating_mul(u64::from(unit.get()))),
        ),
        None => format!("{free} of {total} units free"),
    })
}

/// The free space in a class's partition, in bytes.
///
/// ⚠️ `None` until the partition has reported its allocation unit: free space is a count
/// of units, which cannot be converted to bytes without it.
pub fn free_bytes(class: ObjectClass, device: &DeviceState) -> Option<u64> {
    let unit = device.allocation_unit(class)?;
    let status = counted(class, device)?;
    Some(status.available().saturating_mul(u64::from(unit.get())))
}

/// The counters the instrument last reported for a class's partition.
fn counted(class: ObjectClass, device: &DeviceState) -> Option<&Status> {
    device.inventory.iter().find(|status| status.class == class)
}

/// The bytes a write into a library slot has: the partition's free space and the blocks
/// the slot's occupant frees, since a library write deletes what it replaces first.
///
/// ⚠️ `None` for a slot-addressed class. Its counters count fixed-size records, so a
/// replace needs no room and an empty slot is the room. `None` also until the partition
/// has reported its allocation unit.
pub fn room_for(class: ObjectClass, at: Location, device: &DeviceState) -> Option<u64> {
    if !class.is_library() {
        return None;
    }
    let unit = device.allocation_unit(class)?;
    let freed = match device.slot(class, at).flatten() {
        Some(info) => unit.blocks_for(usize::try_from(info.body_len).ok()?).ok()?,
        None => 0,
    };
    let blocks = counted(class, device)?
        .available()
        .saturating_add(u64::from(freed));
    Some(blocks.saturating_mul(u64::from(unit.get())))
}

/// The queued item with the least room to spare, and whether it fits.
pub fn constraint(queue: &Queue, workspace: &Workspace, device: &DeviceState) -> Option<String> {
    let (name, bytes, room) = queue
        .entries()
        .iter()
        .filter_map(|held| {
            let entity = workspace.get(held.id)?;
            let room = room_for(held.class, held.at, device)?;
            Some((entity.name.clone(), entity.size(), room))
        })
        .max_by_key(|(_, bytes, room)| i128::from(*bytes) - i128::from(*room))?;
    let verdict = match bytes <= room {
        true => "it fits",
        false => "it does not fit",
    };
    Some(format!(
        "{name} is {} and {} is free for it, so {verdict}.",
        measure(bytes),
        measure(room)
    ))
}

/// A size in the largest unit that keeps the figure readable.
pub fn measure(bytes: u64) -> String {
    let (figure, unit) = scaled(bytes, bytes);
    format!("{figure} {unit}")
}

/// A part and the whole it is out of: `121/500 B`, `184.0/192.0 MB`.
///
/// ⚠️ One unit for both, chosen from the whole. A part in its own unit could not be
/// compared with the whole at a glance.
pub fn measure_out_of(part: u64, whole: u64) -> String {
    let (part, unit) = scaled(part, whole);
    let (whole, _) = scaled(whole, whole);
    format!("{part}/{whole} {unit}")
}

/// `bytes` in the unit suited to a size of `scale`, and that unit's name.
fn scaled(bytes: u64, scale: u64) -> (String, &'static str) {
    const K: f64 = 1024.0;
    let held = bytes as f64;
    let scale = scale as f64;
    if scale < K {
        return (bytes.to_string(), "B");
    }
    if scale < K * K {
        return (format!("{:.1}", held / K), "kB");
    }
    if scale < K * K * K {
        return (format!("{:.1}", held / (K * K)), "MB");
    }
    (format!("{:.1}", held / (K * K * K)), "GB")
}

/// The trough, what the folder holds, and what the queue would add.
///
/// ⚠️ The bar takes the status color and the readout beside it keeps the text color: the
/// accent measures 4.1:1 against the panel, enough for a bar but not for 11 px text.
pub fn bar(ui: &mut egui::Ui, meter: Meter) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), TROUGH),
        egui::Sense::hover(),
    );
    let visuals = ui.visuals();
    let tone = match meter.crowded() {
        true => warn(visuals),
        false => accent(visuals),
    };
    let painter = ui.painter();
    painter.rect_filled(rect, ROUND, canvas(visuals));
    for (span, part) in segments(rect.x_range(), meter) {
        let fill = match part {
            Segment::Held => tone,
            Segment::Queued => warn(visuals),
        };
        painter.rect_filled(
            egui::Rect::from_x_y_ranges(span, rect.y_range()),
            ROUND,
            fill,
        );
    }
}

/// A part of a meter's trough.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Segment {
    /// What the folder holds.
    Held,
    /// What the queue would add.
    Queued,
}

/// Where a meter's segments lie across `trough`, with [`SPLIT`] between them when both
/// show. A segment with no width is left out.
///
/// ⚠️ The split comes out of the queued segment, so the held one always measures the
/// share it stands for, and the queued one never runs past the end of the trough.
fn segments(trough: egui::Rangef, meter: Meter) -> Vec<(egui::Rangef, Segment)> {
    let held = trough.min + trough.span() * meter.filled();
    let queued = trough.span() * meter.incoming();
    let mut drawn = Vec::new();
    if held > trough.min {
        drawn.push((egui::Rangef::new(trough.min, held), Segment::Held));
    }
    let start = match held > trough.min {
        true => held + SPLIT,
        false => held,
    };
    let end = held + queued;
    if queued > 0.0 && end > start {
        drawn.push((egui::Rangef::new(start, end), Segment::Queued));
    }
    drawn
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::Device;
    use crate::queue::enqueue;
    use crate::testing::Bench;
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

    #[test]
    fn a_slot_folder_meters_items_and_counts_only_what_would_fill_a_slot() {
        let Bench {
            mut workspace,
            mut device,
            mut log,
            ..
        } = Bench::new();
        let class = ObjectClass::Program;
        let bytes = Fresh::Program.bytes().unwrap();
        // 100 of 400 slots, each program costing 121 bytes of the partition's count.
        device
            .state
            .inventory
            .push(status(class, 100, 300 * 121, 100 * 121));
        device.pretend_geometry(class, &[("1", 100), ("2", 100), ("3", 100), ("4", 100)]);
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

        let held = meter(class, &device.state, &queue, &workspace).expect("the class was read");
        assert_eq!(held.used, 100);
        assert_eq!(held.total, 400);
        assert_eq!(held.queued, 2, "the third replaces what is there");
        assert_eq!(held.filled(), 0.25);
        assert_eq!(held.incoming(), 2.0 / 400.0);
        assert!(!held.crowded());
    }

    #[test]
    fn a_library_meters_blocks_and_has_no_meter_at_all_without_its_unit() {
        let Bench {
            mut workspace,
            mut device,
            mut log,
            ..
        } = Bench::new();
        let class = ObjectClass::Sample;
        device.state.inventory.push(status(class, 84, 64, 1472));
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

        assert_eq!(meter(class, &device.state, &queue, &workspace), None);

        device.pretend_partitions(&[(class, "Samp Lib", 131_064)]);
        let known = meter(class, &device.state, &queue, &workspace).unwrap();
        assert_eq!(known.used, 1472);
        assert_eq!(known.total, 1536);
        // 300 000 / 131 064 = 2.29, and a partial block still costs a whole one.
        assert_eq!(known.queued, 3);
        assert!(known.crowded(), "1472 of 1536 is past nine tenths");
    }

    #[test]
    fn a_partition_that_counts_nothing_at_all_has_no_meter() {
        let Bench {
            workspace,
            mut device,
            queue,
            ..
        } = Bench::new();
        let class = ObjectClass::Piano;
        device.pretend_partitions(&[(class, "Piano", 261_632)]);
        device.pretend_geometry(ObjectClass::Program, &[("1", 5), ("2", 5)]);
        device.state.inventory = vec![
            status(class, 0, 0, 0),
            status(ObjectClass::Program, 1, 9, 1),
        ];

        let drawn = |class| meter(class, &device.state, &queue, &workspace);
        assert_eq!(drawn(class), None);
        assert!(drawn(ObjectClass::Program).is_some());
    }

    #[test]
    fn only_a_partition_that_can_fill_gets_a_meter() {
        let Bench {
            workspace,
            mut device,
            queue,
            ..
        } = Bench::new();
        let (one, many, library) = (ObjectClass::Live, ObjectClass::Program, ObjectClass::Sample);
        device.state.inventory = vec![
            status(one, 1, 4, 1),
            status(many, 100, 300, 100),
            status(library, 84, 64, 1472),
        ];
        device.pretend_geometry(one, &[("Live", 5)]);
        device.pretend_geometry(many, &[("1", 50), ("2", 50), ("3", 50), ("4", 50)]);
        device.pretend_geometry(library, &[("Samp Lib", 1)]);
        let drawn =
            |device: &Device, class| meter(class, &device.state, &queue, &workspace).is_some();

        assert!(drawn(&device, many), "a bank division needs no unit");
        assert!(
            !drawn(&device, library),
            "nothing counts bytes without a unit"
        );

        device.pretend_partitions(&[
            (one, "Live", 1),
            (many, "Program", 1),
            (library, "Samp Lib", 1),
        ]);
        assert!(!drawn(&device, one), "one bank of fixed slots");
        assert!(drawn(&device, many), "more than one bank");
        assert!(drawn(&device, library), "one bank, and counted in bytes");
    }

    #[test]
    fn the_queued_segment_stops_at_the_end_of_the_trough() {
        let full = Meter {
            used: 380,
            total: 400,
            queued: 500,
        };
        assert_eq!(full.filled(), 0.95);
        assert!((full.filled() + full.incoming() - 1.0).abs() < f32::EPSILON);
        // A total of zero gives zero fractions.
        let unread = Meter {
            used: 0,
            total: 0,
            queued: 4,
        };
        assert_eq!((unread.filled(), unread.incoming()), (0.0, 0.0));
    }

    #[test]
    fn the_queued_segment_stands_apart_from_what_is_held_and_inside_the_trough() {
        let trough = egui::Rangef::new(0.0, 100.0);
        let meter = |used, queued| Meter {
            used,
            total: 100,
            queued,
        };
        assert_eq!(
            segments(trough, meter(50, 10)),
            [
                (egui::Rangef::new(0.0, 50.0), Segment::Held),
                (egui::Rangef::new(52.0, 60.0), Segment::Queued),
            ]
        );
        assert_eq!(
            segments(trough, meter(0, 10)),
            [(egui::Rangef::new(0.0, 10.0), Segment::Queued)],
            "with nothing held there is nothing to stand apart from"
        );
        assert_eq!(
            segments(trough, meter(95, 50)),
            [
                (egui::Rangef::new(0.0, 95.0), Segment::Held),
                (egui::Rangef::new(97.0, 100.0), Segment::Queued),
            ]
        );
        assert_eq!(
            segments(trough, meter(99, 1)),
            [(egui::Rangef::new(0.0, 99.0), Segment::Held)],
            "a queued sliver narrower than the gap is not drawn"
        );
        assert!(segments(trough, meter(0, 0)).is_empty());
    }

    #[test]
    fn the_binding_constraint_is_the_largest_thing_waiting_against_the_room_left() {
        let Bench {
            mut workspace,
            mut device,
            mut log,
            ..
        } = Bench::new();
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
        // Without the block size, whether anything fits is unknown.
        assert_eq!(constraint(&queue, &workspace, &device.state), None);

        // 64 blocks of 131 064 bytes is 8.0 MB, and 5 347 738 bytes is 5.1 MB.
        device.pretend_partitions(&crate::device::ELECTRO5);
        assert_eq!(
            constraint(&queue, &workspace, &device.state).as_deref(),
            Some("Grand is 5.1 MB and 8.0 MB is free for it, so it fits.")
        );

        device.state.inventory.clear();
        device.state.inventory.push(status(class, 84, 8, 1528));
        assert!(constraint(&queue, &workspace, &device.state)
            .is_some_and(|said| said.ends_with("it does not fit.")));
    }

    #[test]
    fn a_queued_library_replace_has_the_blocks_its_occupant_frees() {
        let Bench {
            mut workspace,
            mut device,
            mut log,
            ..
        } = Bench::new();
        let class = ObjectClass::Sample;
        let mut queue = Queue::default();
        device.pretend_scanned(class, 1, &["Old"]);
        device.pretend_partitions(&crate::device::ELECTRO5);
        device.state.inventory.push(status(class, 1, 0, 1));
        let id = workspace.ingest("New".into(), Origin::Fresh, vec![0; 4096], &mut log);
        enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            id,
            class,
            at(0),
        );
        assert_eq!(
            constraint(&queue, &workspace, &device.state).as_deref(),
            Some("New is 4.0 kB and 128.0 kB is free for it, so it fits.")
        );
    }

    #[test]
    fn a_slot_class_has_no_room_to_run_out_of_in_bytes() {
        let Bench {
            mut workspace,
            mut device,
            mut log,
            ..
        } = Bench::new();
        let class = ObjectClass::SetList;
        let mut queue = Queue::default();
        device.pretend_scanned(class, 1, &["Old"]);
        device.pretend_partitions(&crate::device::ELECTRO5);
        device.state.inventory.push(status(class, 1, 0, 121));
        let id = workspace.ingest("New".into(), Origin::Fresh, vec![0; 4096], &mut log);
        enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            id,
            class,
            at(0),
        );
        assert_eq!(room_for(class, at(0), &device.state), None);
        assert_eq!(constraint(&queue, &workspace, &device.state), None);
    }

    #[test]
    fn free_space_reads_in_bytes_only_once_the_allocation_unit_has_arrived() {
        let mut device = Device::new(egui::Context::default());
        let class = ObjectClass::Sample;
        device.state.inventory.push(status(class, 84, 64, 1472));
        assert_eq!(
            free_space(class, &device.state).as_deref(),
            Some("64 of 1536 units free")
        );
        device.pretend_partitions(&[(class, "Samp Lib", 131_064)]);
        assert_eq!(
            free_space(class, &device.state).as_deref(),
            Some("8.0 MB free of 192.0 MB")
        );
        assert_eq!(free_space(ObjectClass::Program, &device.state), None);
    }

    #[test]
    fn a_size_reads_in_the_largest_unit_it_fills() {
        assert_eq!(measure(1023), "1023 B");
        assert_eq!(measure(1536), "1.5 kB");
        assert_eq!(measure(5 << 20), "5.0 MB");
        assert_eq!(measure(3 << 30), "3.0 GB");
        assert_eq!(measure_out_of(512 << 20, 2 << 30), "0.5/2.0 GB");
    }
}
