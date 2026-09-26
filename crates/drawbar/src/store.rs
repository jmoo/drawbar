//! "This computer", kept across restarts.
//!
//! eframe provides one string store (localStorage in a browser, a RON file on the
//! desktop), so each asset is written as a line: its id, its origin, its name, its last
//! saved bytes in base64, and its current bytes too when they differ. It is read back
//! through the same decode-and-verify as any file, because bytes from a store deserve no
//! more trust than bytes from a disk.

use base64::prelude::{Engine as _, BASE64_STANDARD};

use crate::log::Log;
use crate::queue::Queue;
use crate::workspace::{Origin, Saved, Workspace};
use nord_usb::{Location, ObjectClass};

const KEY: &str = "drawbar.this_computer";
const VERSION: &str = "drawbar 2";

/// How many fields a line holds, which is all a version decides.
#[derive(Clone, Copy)]
enum Shape {
    /// `drawbar 1`: id, origin, name, bytes. The bytes are both the asset's current and
    /// its saved bytes, because that version stored no other.
    Four,
    /// `drawbar 2`: those, plus the current bytes when an edit is unsaved.
    Five,
}

impl Shape {
    /// The shape a version line asks for, or `None` for a version this build does not
    /// know.
    ///
    /// ⚠️ An unknown version is not read, and the next [`save`] overwrites it, so running
    /// an older build discards a store a newer one wrote.
    fn of(version: &str) -> Option<Shape> {
        match version {
            "drawbar 1" => Some(Shape::Four),
            VERSION => Some(Shape::Five),
            _ => None,
        }
    }
}

/// The largest asset worth keeping.
///
/// ⚠️ A browser gives an origin about 5 MiB of storage, and base64 adds a third. A sample
/// alone can run to megabytes, so one would fill the store and crowd out every program.
pub(crate) const MAX_ENTITY: usize = 1024 * 1024;

/// The most the whole store may hold.
///
/// ⚠️ The browser refuses a write past the quota without telling the app:
/// `Storage::set_string` cannot report failure. So the budget is enforced here, below the
/// quota, and whatever does not fit is reported instead of silently lost.
const BUDGET: usize = 3 * 1024 * 1024;

/// What a write of the list could not keep.
///
/// ⚠️ Returned, not logged: eframe writes the list every few seconds, and a save that
/// reported its own losses would overwrite the status line and fill the log for as long
/// as the asset remained. The caller calls [`Left::report`] only when what is left out
/// changes.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Left {
    /// Larger than [`MAX_ENTITY`].
    skipped: usize,
    /// Within that limit, but past what [`BUDGET`] had left.
    dropped: usize,
}

impl Left {
    pub fn report(self, log: &mut Log) {
        match (self.skipped, self.dropped) {
            (0, 0) => {}
            (skipped, 0) => log.say(plural(skipped, "too big to keep between sessions")),
            (0, dropped) => log.trouble(plural(
                dropped,
                "not kept between sessions: there is no room left",
            )),
            (skipped, dropped) => log.trouble(plural(
                skipped + dropped,
                "not kept between sessions: too big, or no room left",
            )),
        }
    }
}

/// Write the list. Called by eframe periodically and at exit.
///
/// ⚠️ A view of a slot is written only when it is unsaved or queued. An untouched view
/// shows the instrument's own copy, and persisting it would give the user a local copy
/// they never asked for. An edited view is the only copy of its edit, and quitting with
/// its tab open must not lose it.
///
/// ⚠️ On wasm every call base64-encodes the whole list on the only thread. Callers
/// must rate-limit writes because dragging mutates the list every frame.
///
/// What is written comes back as kept assets (see [`load`]); what is not is returned as
/// [`Left`].
pub fn save(storage: &mut dyn eframe::Storage, workspace: &Workspace, queue: &Queue) -> Left {
    let mut out = format!("{VERSION}\n{}\n", workspace.next_id());
    let mut skipped = 0;
    let mut dropped = 0;
    for entity in workspace.entities() {
        if !entity.kept && !crate::workspace::precious(entity, queue) {
            continue;
        }
        if entity.bytes.len().max(entity.saved.bytes.len()) > MAX_ENTITY {
            skipped += 1;
            continue;
        }
        // The baseline is the saved bytes; only an unsaved asset has a tail, holding its
        // current bytes.
        let unsaved = match entity.is_unsaved() {
            true => format!("\t{}", BASE64_STANDARD.encode(&entity.bytes)),
            false => String::new(),
        };
        let line = format!(
            "{}\t{}\t{}\t{}{unsaved}\n",
            entity.id,
            origin(&entity.origin),
            escape(&entity.name),
            BASE64_STANDARD.encode(&entity.saved.bytes),
        );
        if out.len() + line.len() > BUDGET {
            dropped += 1;
            continue;
        }
        out.push_str(&line);
    }
    storage.set_string(KEY, out);
    Left { skipped, dropped }
}

fn plural(n: usize, tail: &str) -> String {
    match n {
        1 => format!("1 sound is {tail}."),
        n => format!("{n} sounds are {tail}."),
    }
}

/// Read the list back, decoding and verifying every asset.
///
/// Everything restored is on this computer. A view needs a tab and a slot, and at startup
/// there is no tab, so a view that [`save`] kept comes back as a local asset.
pub fn load(storage: &dyn eframe::Storage, workspace: &mut Workspace, log: &mut Log) {
    let Some(text) = storage.get_string(KEY) else {
        return;
    };
    let mut lines = text.lines();
    let Some(shape) = lines.next().and_then(Shape::of) else {
        log.warn("the saved list is in a format this build does not read");
        return;
    };
    let next_id = lines.next().and_then(|line| line.parse().ok());
    let mut restored = Vec::new();
    let mut unreadable = 0;
    for line in lines {
        match entry(line, shape) {
            Some(saved) => restored.push(saved),
            None => unreadable += 1,
        }
    }
    let read = restored.len();
    // A line the workspace refuses (an id that leaves no room for the next, or one
    // already in use) counts as unreadable too.
    let refused = workspace.restore(restored, next_id, log);
    let count = read.saturating_sub(refused);
    let unreadable = unreadable + refused;
    if unreadable > 0 {
        log.warn(format!("{unreadable} saved line(s) did not read"));
    }
    if count > 0 {
        log.say(match count {
            1 => "1 sound is back from last time.".to_string(),
            n => format!("{n} sounds are back from last time."),
        });
    }
}

fn entry(line: &str, shape: Shape) -> Option<Saved> {
    let mut parts = line.splitn(5, '\t');
    let id = parts.next()?.parse().ok()?;
    let origin = unorigin(parts.next()?)?;
    let name = unescape(parts.next()?);
    let saved = BASE64_STANDARD.decode(parts.next()?).ok()?;
    let unsaved = match (shape, parts.next()) {
        (_, None) => None,
        (Shape::Five, Some(text)) => Some(BASE64_STANDARD.decode(text).ok()?),
        (Shape::Four, Some(_)) => return None,
    };
    Some(Saved {
        id,
        name,
        origin,
        saved,
        unsaved,
    })
}

fn origin(origin: &Origin) -> String {
    match origin {
        Origin::File(name) => format!("file:{}", escape(name)),
        Origin::Device { class, at } => {
            format!("device:{}:{}:{}", class.to_raw(), at.bank, at.slot)
        }
        Origin::Fresh => "fresh".into(),
        Origin::Rescued { at } => format!("rescued:{}:{}", at.bank, at.slot),
    }
}

fn unorigin(text: &str) -> Option<Origin> {
    let (head, rest) = text.split_once(':').unwrap_or((text, ""));
    Some(match head {
        "file" => Origin::File(unescape(rest)),
        "fresh" => Origin::Fresh,
        "device" => {
            let mut parts = rest.split(':');
            let class = ObjectClass::from_raw(parts.next()?.parse().ok()?);
            Origin::Device {
                class,
                at: location(parts.next()?, parts.next()?)?,
            }
        }
        "rescued" => {
            let mut parts = rest.split(':');
            Origin::Rescued {
                at: location(parts.next()?, parts.next()?)?,
            }
        }
        _ => return None,
    })
}

fn location(bank: &str, slot: &str) -> Option<Location> {
    Some(Location {
        bank: bank.parse().ok()?,
        slot: slot.parse().ok()?,
    })
}

/// Tabs separate fields and newlines separate lines, so an unescaped name holding either
/// would corrupt the rest of the store.
pub(crate) fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}

pub(crate) fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (c, chars.clone().next()) {
            ('\\', Some('t')) => {
                chars.next();
                out.push('\t');
            }
            ('\\', Some('n')) => {
                chars.next();
                out.push('\n');
            }
            ('\\', Some('\\')) => {
                chars.next();
                out.push('\\');
            }
            _ => out.push(c),
        }
    }
    out
}

/// A store in a map, for the tests that need one to write into and read back.
#[cfg(test)]
#[derive(Default)]
pub struct Fake(std::collections::HashMap<String, String>);

#[cfg(test)]
impl eframe::Storage for Fake {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }

    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.to_string(), value);
    }

    fn flush(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A restored asset still knows where it came from, so it can be sent back there.
    #[test]
    fn an_origin_round_trips() {
        let at = Location { bank: 6, slot: 3 };
        for before in [
            Origin::Fresh,
            Origin::File("Africa Split.ne5p".into()),
            Origin::Device {
                class: ObjectClass::Program,
                at,
            },
            Origin::Rescued { at },
        ] {
            let text = origin(&before);
            let after = unorigin(&text).expect(&text);
            assert_eq!(origin(&after), text);
        }
    }

    #[test]
    fn a_name_holding_a_separator_survives() {
        for name in ["plain", "with\ttab", "with\nnewline", "back\\slash", "\\t"] {
            assert_eq!(unescape(&escape(name)), name);
        }
    }

    /// The bytes of a program a store line can carry, made the way the app makes one.
    fn a_program() -> Vec<u8> {
        let (mut workspace, mut log) = workspace();
        let id = workspace
            .create(crate::workspace::Fresh::Program, &mut log)
            .unwrap();
        workspace.get(id).unwrap().bytes.clone()
    }

    fn workspace() -> (Workspace, Log) {
        (
            Workspace::new(eframe::egui::Context::default()),
            Log::default(),
        )
    }

    #[test]
    fn the_list_comes_back_as_it_was() {
        use crate::workspace::Fresh;

        let (mut before, mut log) = workspace();
        let id = before.create(Fresh::Program, &mut log).unwrap();
        before.rename(id, "Africa-Split.ne5p".into());
        before.create(Fresh::Settings, &mut log).unwrap();

        let mut store = Fake::default();
        save(&mut store, &before, &Queue::default());

        let (mut after, mut log) = workspace();
        load(&store, &mut after, &mut log);

        assert_eq!(after.entities().len(), 2);
        let restored = after.get(id).expect("kept its id");
        assert_eq!(restored.name, "Africa-Split.ne5p");
        assert_eq!(restored.bytes, before.get(id).unwrap().bytes);
        assert!(matches!(restored.verify, crate::workspace::VerifyState::Ok));
        assert_eq!(after.export_name(id).as_deref(), Some("Africa-Split.ne5p"));
    }

    #[test]
    fn a_restored_note_is_still_a_note() {
        use crate::workspace::Fresh;

        let (mut before, mut log) = workspace();
        let id = before.create(Fresh::Text, &mut log).unwrap();
        before.replace_bytes(id, b"Set 1\n".to_vec(), &mut log);

        let mut store = Fake::default();
        save(&mut store, &before, &Queue::default());

        let (mut after, mut log) = workspace();
        load(&store, &mut after, &mut log);

        let restored = after.get(id).expect("kept its id");
        assert_eq!(restored.name, "untitled.txt");
        assert_eq!(restored.bytes, b"Set 1\n");
        assert_eq!(
            crate::browser::Kind::of(restored),
            crate::browser::Kind::Text
        );
        assert!(
            restored.is_unsaved(),
            "it was not saved before it was stored"
        );
    }

    #[test]
    fn an_edited_view_survives_a_session_and_an_untouched_one_does_not() {
        use crate::workspace::Fresh;

        let (mut before, mut log) = workspace();
        let bytes = {
            let id = before.create(Fresh::Program, &mut log).unwrap();
            let bytes = before.get(id).unwrap().bytes.clone();
            before.remove(id, &mut log);
            bytes
        };
        let at = |slot| Location { bank: 6, slot };
        let view = |workspace: &mut Workspace, slot, log: &mut Log| {
            workspace.view(
                format!("view-{slot}.ne5p"),
                Origin::Device {
                    class: ObjectClass::Program,
                    at: at(slot),
                },
                bytes.clone(),
                log,
            )
        };
        let edited = view(&mut before, 0, &mut log);
        let owed = view(&mut before, 1, &mut log);
        let untouched = view(&mut before, 2, &mut log);
        let held = before.get(edited).unwrap().bytes.clone();
        before.replace_bytes(edited, [held, vec![0]].concat(), &mut log);
        let mut queue = Queue::default();
        crate::queue::enqueue(
            &before,
            &mut crate::device::Device::new(before.ctx().clone()),
            &mut queue,
            &mut log,
            owed,
            ObjectClass::Program,
            at(1),
        );

        let mut store = Fake::default();
        save(&mut store, &before, &queue);
        let (mut after, mut log) = workspace();
        load(&store, &mut after, &mut log);

        assert!(after.get(untouched).is_none(), "the slot still has it");
        let restored: Vec<u64> = after.listed().map(|e| e.id).collect();
        assert_eq!(restored, vec![edited, owed]);
        assert_eq!(after.get(edited).unwrap().name, "view-0.ne5p");
        // Restored assets are kept, not views: there is no tab at startup.
        assert!(!after.is_view(edited) && !after.is_view(owed));
        // Its source slot is still recorded, so it can be sent back.
        assert_eq!(
            after.get(owed).unwrap().origin.slot(),
            Some((ObjectClass::Program, at(1)))
        );
    }

    /// Revert works the same in the next session.
    #[test]
    fn an_unsaved_edit_and_the_baseline_under_it_both_survive() {
        use crate::workspace::Fresh;

        let (mut before, mut log) = workspace();
        let id = before.create(Fresh::Program, &mut log).unwrap();
        let saved = before.get(id).unwrap().bytes.clone();
        let (_, edited) =
            crate::fields::apply(&saved, &[("center_panel.gain".into(), "96".into())]).unwrap();
        before.replace_bytes(id, edited.clone(), &mut log);

        let mut store = Fake::default();
        save(&mut store, &before, &Queue::default());
        let (mut after, mut log) = workspace();
        load(&store, &mut after, &mut log);

        let restored = after.get(id).expect("kept its id");
        assert_eq!(restored.bytes, edited, "it holds the edit");
        assert_eq!(restored.saved.bytes, saved, "it keeps its saved baseline");
        assert!(restored.is_unsaved());
        after.revert(id, &mut log);
        assert_eq!(after.get(id).unwrap().bytes, saved);
    }

    #[test]
    fn a_saved_asset_has_no_unsaved_tail() {
        use crate::workspace::Fresh;

        let (mut before, mut log) = workspace();
        before.create(Fresh::Program, &mut log).unwrap();
        let mut store = Fake::default();
        save(&mut store, &before, &Queue::default());
        let text = eframe::Storage::get_string(&store, KEY).expect("something was written");
        let line = text.lines().nth(2).expect("the one asset's line");
        assert_eq!(line.split('\t').count(), 4);
    }

    #[test]
    fn an_oversized_asset_is_left_out_and_reported() {
        use crate::workspace::Origin;

        let (mut before, mut log) = workspace();
        before.ingest(
            "huge.nsmp".into(),
            Origin::Fresh,
            vec![0; MAX_ENTITY + 1],
            &mut log,
        );
        let mut store = Fake::default();
        let left = save(&mut store, &before, &Queue::default());
        assert_eq!((left.skipped, left.dropped), (1, 0));
        left.report(&mut log);
        assert!(log.status().1.contains("too big"), "{}", log.status().1);

        let (mut after, mut log) = workspace();
        load(&store, &mut after, &mut log);
        assert!(after.entities().is_empty());
    }

    #[test]
    fn a_store_from_another_build_is_not_guessed_at() {
        let mut store = Fake::default();
        eframe::Storage::set_string(&mut store, KEY, "drawbar 99\n1\n".to_string());
        let (mut after, mut log) = workspace();
        load(&store, &mut after, &mut log);
        assert!(after.entities().is_empty());
    }

    #[test]
    fn a_damaged_line_is_refused() {
        let read = |line| entry(line, Shape::Five);
        assert!(read("").is_none());
        assert!(read("7\tfresh\tname").is_none(), "no bytes");
        assert!(
            read("7\tfresh\tname\tZm9v\t!!!").is_none(),
            "the tail is not base64"
        );
        assert!(
            read("7\tfresh\tname\tZm9v\tYmFy").is_some(),
            "saved, and a tail"
        );
        assert!(
            read("7\tfresh\tname\tZm9v\tYmFy\tYmFy").is_none(),
            "a sixth field is not part of the tail"
        );
        assert!(read("seven\tfresh\tname\tZm9v").is_none(), "no id");
        assert!(read("7\tnonesuch\tname\tZm9v").is_none(), "no such origin");
        assert!(read("7\tfresh\tname\t!!!").is_none(), "not base64");
        assert!(read("7\tfresh\tname\tZm9v").is_some());
        // A version-1 line has four fields; a fifth makes it invalid.
        assert!(entry("7\tfresh\tname\tZm9v", Shape::Four).is_some());
        assert!(entry("7\tfresh\tname\tZm9v\tYmFy", Shape::Four).is_none());
    }

    /// The largest id leaves no next id, so the line is refused.
    #[test]
    fn a_line_whose_id_leaves_no_room_for_the_next_is_refused() {
        let mut store = Fake::default();
        eframe::Storage::set_string(
            &mut store,
            KEY,
            format!(
                "{VERSION}\n1\n{}\tfresh\tlast.ne5p\t{}\n",
                u64::MAX,
                BASE64_STANDARD.encode(a_program()),
            ),
        );
        let (mut after, mut log) = workspace();
        load(&store, &mut after, &mut log);

        assert!(after.entities().is_empty());
        assert!(after.get(u64::MAX).is_none());
        assert!(log.iter().any(|entry| entry.text.contains("did not read")));
    }

    /// Two lines with one id would be two assets nothing could tell apart (a tab, a send,
    /// or a removal would reach whichever came first), so the second is refused.
    #[test]
    fn a_second_line_under_an_id_already_restored_is_refused() {
        let line = format!(
            "7\tfresh\tone.ne5p\t{}\n",
            BASE64_STANDARD.encode(a_program()),
        );
        let mut store = Fake::default();
        eframe::Storage::set_string(&mut store, KEY, format!("{VERSION}\n8\n{line}{line}"));
        let (mut after, mut log) = workspace();
        load(&store, &mut after, &mut log);

        assert_eq!(after.entities().len(), 1);
        assert_eq!(after.get(7).expect("the first line").name, "one.ne5p");
        assert!(log.iter().any(|entry| entry.text.contains("did not read")));
        assert!(
            log.status().1.contains("1 sound is back"),
            "{}",
            log.status().1
        );
    }

    /// A version-1 line is an asset holding what it was last saved as.
    #[test]
    fn a_version_1_store_comes_back_kept_and_saved() {
        use crate::workspace::Fresh;

        let (mut before, mut log) = workspace();
        let id = before.create(Fresh::Program, &mut log).unwrap();
        let bytes = before.get(id).unwrap().bytes.clone();

        let mut store = Fake::default();
        eframe::Storage::set_string(
            &mut store,
            KEY,
            format!(
                "drawbar 1\n8\n7\tfile:Africa Split.ne5p\tAfrica Split.ne5p\t{}\n",
                BASE64_STANDARD.encode(&bytes)
            ),
        );

        let (mut after, mut log) = workspace();
        load(&store, &mut after, &mut log);

        let restored = after.get(7).expect("the line's own id");
        assert_eq!(restored.name, "Africa Split.ne5p");
        assert_eq!(restored.bytes, bytes);
        assert_eq!(restored.saved.bytes, bytes, "there was no other copy");
        assert!(!restored.is_unsaved());
        assert!(restored.kept);
        assert!(matches!(restored.verify, crate::workspace::VerifyState::Ok));
    }
}
