//! What an asset's accessors read after each way its saved baseline can move, from each
//! way a store can hand an asset back.
//!
//! Each row is one start and one transition. It gives what the accessors read right after
//! the transition and, after `→`, once every check and decode has answered; a row with no
//! `→` reads the same either way. A flag is named only when it holds: `unread`,
//! `reading`, `rests`, `unsaved`, `sendable`, `summary` ([`Summary::of`] gives one) and
//! `edit-held` (an edit is held over a file). Then `might` names the bytes
//! [`LocalEntity::might_hold`] allows, `size` is [`LocalEntity::size`], `badge` the
//! verify badge, `slot` the baseline's slot checksum, `whole` its whole-file CRC, and
//! `held` [`LocalEntity::held_whole`].
//!
//! Values are named for the bytes they belong to: `P` the program the asset was saved
//! as, `E` the edit a store kept, `T` an edit made here, `R` the bytes a rebase takes, `F`
//! the sample instrument a file holds, `G` another file, a piano library, and `B` a piano
//! library whose body fails its checksum.
//!
//! A check in flight has always read its file to the end before a row reads on, so a
//! file's CRC is known unless its check waits behind another's.

use std::sync::Arc;

use nord_usb::{Location, ObjectClass};

use super::*;
use crate::summary::Summary;
use crate::testing::Temp;

const ID: u64 = 1;

struct Fixture {
    p: Vec<u8>,
    e: Vec<u8>,
    t: Vec<u8>,
    r: Vec<u8>,
    /// What a read of `P` found.
    summary: Summary,
    /// Every byte set by name, for the lengths, checksums and `might_hold` the rows name.
    named: Vec<(&'static str, Vec<u8>)>,
}

impl Fixture {
    fn new() -> Fixture {
        let p = Fresh::Program.bytes().unwrap();
        let e = Fresh::Stage3Synth.bytes().unwrap();
        let t = Fresh::Stage4Synth.bytes().unwrap();
        let r = Fresh::Stage4Organ.bytes().unwrap();
        let mut b = crate::testing::piano(4);
        let last = b.len() - 1;
        b[last] ^= 0xff;
        let read = LocalEntity::new(ID, "P.ne5p".into(), Origin::Fresh, p.clone().into(), 0);
        let summary = Summary::of(&read).expect("a decoded program has a summary");
        let named = vec![
            ("P", p.clone()),
            ("E", e.clone()),
            ("T", t.clone()),
            ("R", r.clone()),
            ("F", crate::testing::sample_bytes()),
            ("G", crate::testing::piano(5)),
            ("B", b),
        ];
        let lengths: Vec<usize> = named.iter().map(|(_, bytes)| bytes.len()).collect();
        let mut distinct = lengths.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            lengths.len(),
            "one length per byte set: {lengths:?}"
        );
        Fixture {
            p,
            e,
            t,
            r,
            summary,
            named,
        }
    }

    fn bytes(&self, name: &str) -> &[u8] {
        let named = self.named.iter().find(|(held, _)| *held == name);
        &named.expect("a named byte set").1
    }

    /// The name of the byte set `len` bytes long, or the number.
    fn length(&self, len: u64) -> String {
        let named = self
            .named
            .iter()
            .find(|(_, bytes)| bytes.len() as u64 == len);
        match (len, named) {
            (0, _) => "0".into(),
            (_, Some((name, _))) => (*name).into(),
            (len, None) => len.to_string(),
        }
    }

    /// The names of the byte sets whose lengths add up to `len`, or the number.
    fn sum(&self, len: u64) -> String {
        if len == 0 {
            return "0".into();
        }
        let whole = self
            .named
            .iter()
            .map(|(name, bytes)| (*name, bytes.len() as u64));
        let whole: Vec<(&str, u64)> = whole.collect();
        for (a, x) in &whole {
            if *x == len {
                return (*a).into();
            }
            for (b, y) in &whole {
                if x + y == len && a <= b {
                    return format!("{a}+{b}");
                }
            }
        }
        len.to_string()
    }

    /// The name of the byte set whose checksum by `of` is `crc`, or the number.
    fn crc(&self, crc: Option<u32>, of: impl Fn(&[u8]) -> Option<u32>) -> String {
        let Some(crc) = crc else {
            return "-".into();
        };
        let named = self.named.iter().find(|(_, bytes)| of(bytes) == Some(crc));
        match named {
            Some((name, _)) => (*name).into(),
            None => format!("{crc:#010x}"),
        }
    }

    fn slot(&self, crc: Option<u32>) -> String {
        self.crc(crc, |bytes| {
            Container::read(bytes).map(|held| held.body_crc32)
        })
    }

    fn whole(&self, crc: Option<u32>) -> String {
        self.crc(crc, |bytes| Some(nord_format::crc::crc32(bytes)))
    }
}

fn sample_edit() -> Edit {
    Edit::Sample(vec![("name".to_string(), "Vibes".to_string())])
}

/// What a store hands back: the baseline, and the unsaved edit kept over it.
enum Held {
    Whole(Vec<u8>),
    Unread(u64),
    File(Arc<OnDisk>),
    FileAndUnread(Arc<OnDisk>, u64),
}

fn restored(id: u64, held: Held, unsaved: Option<Vec<u8>>) -> Saved {
    let (saved, file, unread) = match held {
        Held::Whole(bytes) => (bytes, None, None),
        Held::Unread(len) => (Vec::new(), None, Some(len)),
        Held::File(file) => (Vec::new(), Some(file), None),
        Held::FileAndUnread(file, len) => (Vec::new(), Some(file), Some(len)),
    };
    Saved {
        id,
        name: format!("asset {id}"),
        path: None,
        origin: Origin::Fresh,
        saved,
        file,
        unread,
        unsaved,
    }
}

/// One row's workspace, and files of its own, so that no row sees a checksum another
/// row's check took.
struct At {
    workspace: Workspace,
    log: Log,
    dir: Temp,
    f: Arc<OnDisk>,
    g: Arc<OnDisk>,
    b: Arc<OnDisk>,
}

impl At {
    fn new(fixture: &Fixture) -> At {
        let dir = Temp::new();
        let file =
            |name: &str, held: &str| crate::testing::on_disk(&dir, name, fixture.bytes(held));
        let (f, g, b) = (
            file("F.nsmp", "F"),
            file("G.npno", "G"),
            file("B.npno", "B"),
        );
        At {
            workspace: Workspace::new(egui::Context::default()),
            log: Log::default(),
            dir,
            f,
            g,
            b,
        }
    }

    fn restore(&mut self, held: Held, unsaved: Option<Vec<u8>>) {
        self.restore_as(ID, held, unsaved);
    }

    fn restore_as(&mut self, id: u64, held: Held, unsaved: Option<Vec<u8>>) {
        let saved = vec![restored(id, held, unsaved)];
        assert_eq!(self.workspace.restore(saved, None, &mut self.log), 0);
    }

    /// Let the check running, if one is, read its file to the end, without folding in its
    /// answer. A check takes its file's CRC as it reads, so without this a row would read
    /// the CRC or not as the check's thread happened to run.
    fn quiet(&mut self) {
        let ctx = self.workspace.ctx.clone();
        let Some(check) = self.workspace.checking.as_mut() else {
            return;
        };
        if let crate::work::Answer::Answered(answer) = check.job.wait() {
            check.job = crate::work::run(&ctx, move |_| answer);
        }
    }

    fn settle(&mut self) {
        self.workspace.settle_files(&mut self.log);
    }

    /// What the accessors read, in the order the module's rows give them.
    fn seen(&self, fixture: &Fixture) -> String {
        let entity = self.workspace.get(ID).expect("the asset is held");
        let mut out = Vec::new();
        let mut flag = |name: &str, on: bool| {
            if on {
                out.push(name.to_string());
            }
        };
        flag("unread", entity.unread());
        flag("reading", entity.reading());
        flag("rests", entity.rests().is_some());
        flag("unsaved", entity.is_unsaved());
        flag("sendable", entity.sendable().is_ok());
        flag("summary", Summary::of(entity).is_some());
        flag("edit-held", self.workspace.edit(ID).is_some());
        let might: Vec<&str> = ["P", "F", "G", "B"]
            .into_iter()
            .filter(|name| entity.might_hold(fixture.bytes(name)))
            .collect();
        if !might.is_empty() {
            out.push(format!("might={}", might.join(",")));
        }
        out.push(format!("size={}", fixture.length(entity.size())));
        out.push(format!("badge={}", entity.verify().badge()));
        out.push(format!("slot={}", fixture.slot(entity.saved.crc32())));
        out.push(format!("whole={}", fixture.whole(entity.saved.whole_crc())));
        out.push(format!("held={}", fixture.sum(entity.held_whole())));
        out.join(" ")
    }
}

type Step = fn(&Fixture, &mut At);

/// Every way a store hands an asset back, and the asset each comes back as.
const STARTS: &[(&str, Step)] = &[
    ("whole", |x, at| at.restore(Held::Whole(x.p.clone()), None)),
    ("whole, decoded", |x, at| {
        at.restore(Held::Whole(x.p.clone()), None);
        at.settle();
    }),
    ("whole, unsaved", |x, at| {
        at.restore(Held::Whole(x.p.clone()), Some(x.e.clone()));
    }),
    ("whole, unsaved as saved", |x, at| {
        at.restore(Held::Whole(x.p.clone()), Some(x.p.clone()));
    }),
    ("unread", |x, at| {
        at.restore(Held::Unread(x.p.len() as u64), None);
    }),
    ("unread, unsaved", |x, at| {
        at.restore(Held::Unread(x.p.len() as u64), Some(x.e.clone()));
    }),
    ("remembered", |x, at| {
        at.restore(Held::Unread(x.p.len() as u64), None);
        at.workspace.remember(ID, x.summary.clone());
    }),
    ("not read", |x, at| {
        at.restore(Held::Unread(x.p.len() as u64), None);
        at.workspace.unreadable(ID, "gone".into(), &mut at.log);
    }),
    ("remembered, not read", |x, at| {
        at.restore(Held::Unread(x.p.len() as u64), None);
        at.workspace.remember(ID, x.summary.clone());
        at.workspace.unreadable(ID, "gone".into(), &mut at.log);
    }),
    ("file", |_, at| at.restore(Held::File(at.f.clone()), None)),
    ("file, check queued", |_, at| {
        let other = crate::testing::on_disk(&at.dir, "other.nsmp", &crate::testing::sample_bytes());
        at.restore_as(ID + 1, Held::File(other), None);
        at.restore(Held::File(at.f.clone()), None);
    }),
    ("file, checked", |_, at| {
        at.restore(Held::File(at.f.clone()), None);
        at.settle();
    }),
    ("file, failed", |_, at| {
        at.restore(Held::File(at.b.clone()), None);
        at.settle();
    }),
    ("file, unsaved", |x, at| {
        at.restore(Held::File(at.f.clone()), Some(x.e.clone()));
    }),
    ("file and unread", |x, at| {
        let held = Held::FileAndUnread(at.f.clone(), x.p.len() as u64);
        at.restore(held, None);
    }),
    ("file and unread, unsaved", |x, at| {
        let held = Held::FileAndUnread(at.f.clone(), x.p.len() as u64);
        at.restore(held, Some(x.e.clone()));
    }),
    ("file, edit held", |_, at| {
        at.restore(Held::File(at.f.clone()), None);
        at.settle();
        at.workspace.hold_edit(ID, Some(sample_edit()));
    }),
    ("file, edit sent", |_, at| {
        at.restore(Held::File(at.f.clone()), None);
        at.settle();
        at.workspace.hold_edit(ID, Some(sample_edit()));
        assert!(at.workspace.save_edit(ID));
        assert!(at.workspace.send_edit(ID).is_some());
    }),
];

/// Every way the saved baseline moves.
const TRANSITIONS: &[(&str, Step)] = &[
    ("as restored", |_, _| {}),
    ("took bytes", |x, at| {
        at.workspace.took(ID, Some(x.p.clone()), None)
    }),
    ("took file", |_, at| {
        at.workspace.took(ID, None, Some(at.g.clone()))
    }),
    ("took nothing", |_, at| at.workspace.took(ID, None, None)),
    ("unreadable", |_, at| {
        at.workspace.unreadable(ID, "gone".into(), &mut at.log);
    }),
    ("retry", |_, at| at.workspace.retry(ID)),
    ("remember", |x, at| {
        at.workspace.remember(ID, x.summary.clone())
    }),
    ("stale", |_, at| at.workspace.stale(ID)),
    ("evict", |_, at| _ = at.workspace.evict(ID)),
    ("rest", |_, at| at.workspace.adopt_file(ID, at.g.clone())),
    ("settle", |_, at| at.settle()),
    ("edit", |x, at| {
        at.workspace.replace_bytes(ID, x.t.clone(), &mut at.log);
    }),
    ("edit back to P", |x, at| {
        at.workspace.replace_bytes(ID, x.p.clone(), &mut at.log);
    }),
    ("edit back to F", |x, at| {
        let f = x.bytes("F").to_vec();
        at.workspace.replace_bytes(ID, f, &mut at.log);
    }),
    ("revert", |_, at| at.workspace.revert(ID, &mut at.log)),
    ("rebase", |x, at| {
        at.workspace.rebase(ID, x.r.clone(), &mut at.log);
    }),
    ("rebase file", |_, at| {
        at.workspace.rebase_file(ID, at.g.clone())
    }),
    ("landed P", |x, at| {
        let slot = Location { bank: 0, slot: 1 };
        at.workspace
            .landed(ID, ObjectClass::Program, slot, x.p.clone());
    }),
    ("landed F", |x, at| {
        let slot = Location { bank: 0, slot: 1 };
        let f = x.bytes("F").to_vec();
        at.workspace.landed(ID, ObjectClass::Sample, slot, f);
    }),
    ("landed file", |x, at| {
        let slot = Location { bank: 0, slot: 1 };
        let crc = Container::read(x.bytes("G")).map(|held| held.body_crc32);
        let crc = crc.expect("a piano library is a container");
        at.workspace
            .landed_file(ID, ObjectClass::Piano, slot, at.g.clone(), crc);
    }),
    ("mark saved", |_, at| at.workspace.mark_saved(ID)),
    ("unsave", |_, at| at.workspace.unsave(ID)),
    ("hold edit", |_, at| {
        at.workspace.hold_edit(ID, Some(sample_edit()));
    }),
    ("edit saved", |_, at| {
        at.workspace.edit_saved(ID, at.g.clone())
    }),
];

/// The rows, one block per start in [`STARTS`] order, one row per transition in
/// [`TRANSITIONS`] order.
const EXPECTED: &[(&str, &[(&str, &str)])] = &[
    (
        "whole",
        &[
            (
                "as restored",
                "reading sendable size=P badge=reading… slot=- whole=- held=P → sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "took bytes",
                "reading sendable size=P badge=reading… slot=- whole=- held=P → sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "took file",
                "reading sendable size=P badge=reading… slot=- whole=- held=P → sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "took nothing",
                "reading sendable size=P badge=reading… slot=- whole=- held=P → sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "unreadable",
                "reading sendable size=P badge=reading… slot=- whole=- held=P → sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "retry",
                "reading sendable size=P badge=reading… slot=- whole=- held=P → sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "remember",
                "reading sendable size=P badge=reading… slot=- whole=- held=P → sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "stale",
                "reading sendable size=P badge=reading… slot=- whole=- held=P → sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "evict",
                "unread reading might=P size=P badge=reading… slot=- whole=- held=0",
            ),
            (
                "rest",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "settle",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "edit",
                "unsaved sendable summary size=T badge=ok slot=P whole=P held=P+T",
            ),
            (
                "edit back to P",
                "reading sendable size=P badge=reading… slot=- whole=- held=P → sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "edit back to F",
                "unsaved sendable summary size=F badge=ok slot=P whole=P held=F+P",
            ),
            (
                "revert",
                "reading sendable size=P badge=reading… slot=- whole=- held=P → sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "rebase",
                "reading unsaved sendable size=P badge=reading… slot=R whole=R held=P+R → unsaved sendable summary size=P badge=ok slot=P whole=P held=P+R",
            ),
            (
                "rebase file",
                "reading unsaved sendable size=P badge=reading… slot=- whole=G held=P → unsaved sendable summary size=P badge=ok slot=G whole=G held=P",
            ),
            (
                "landed P",
                "reading sendable size=P badge=reading… slot=P whole=P held=P → sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "landed F",
                "reading unsaved sendable size=P badge=reading… slot=F whole=F held=F+P → unsaved sendable summary size=P badge=ok slot=P whole=P held=F+P",
            ),
            (
                "landed file",
                "reading unsaved sendable size=P badge=reading… slot=G whole=- held=P → unsaved sendable summary size=P badge=ok slot=P whole=- held=P",
            ),
            (
                "mark saved",
                "reading sendable size=P badge=reading… slot=- whole=- held=P → sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "unsave",
                "reading unsaved sendable size=P badge=reading… slot=- whole=- held=P → unsaved sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "hold edit",
                "reading sendable size=P badge=reading… slot=- whole=- held=P → sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "edit saved",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
        ],
    ),
    (
        "whole, decoded",
        &[
            (
                "as restored",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "took bytes",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "took file",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "took nothing",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "unreadable",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "retry",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "remember",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "stale",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "evict",
                "unread summary might=P size=P badge=ok slot=P whole=- held=0",
            ),
            (
                "rest",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "settle",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "edit",
                "unsaved sendable summary size=T badge=ok slot=P whole=P held=P+T",
            ),
            (
                "edit back to P",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "edit back to F",
                "unsaved sendable summary size=F badge=ok slot=P whole=P held=F+P",
            ),
            (
                "revert",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "rebase",
                "unsaved sendable summary size=P badge=ok slot=R whole=R held=P+R",
            ),
            (
                "rebase file",
                "unsaved sendable summary size=P badge=ok slot=- whole=G held=P → unsaved sendable summary size=P badge=ok slot=G whole=G held=P",
            ),
            (
                "landed P",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "landed F",
                "unsaved sendable summary size=P badge=ok slot=F whole=F held=F+P",
            ),
            (
                "landed file",
                "unsaved sendable summary size=P badge=ok slot=G whole=- held=P",
            ),
            (
                "mark saved",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "unsave",
                "unsaved sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "hold edit",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "edit saved",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
        ],
    ),
    (
        "whole, unsaved",
        &[
            (
                "as restored",
                "unsaved sendable summary size=E badge=ok slot=P whole=P held=E+P",
            ),
            (
                "took bytes",
                "unsaved sendable summary size=E badge=ok slot=P whole=P held=E+P",
            ),
            (
                "took file",
                "unsaved sendable summary size=E badge=ok slot=P whole=P held=E+P",
            ),
            (
                "took nothing",
                "unsaved sendable summary size=E badge=ok slot=P whole=P held=E+P",
            ),
            (
                "unreadable",
                "unsaved sendable summary size=E badge=ok slot=P whole=P held=E+P",
            ),
            (
                "retry",
                "unsaved sendable summary size=E badge=ok slot=P whole=P held=E+P",
            ),
            (
                "remember",
                "unsaved sendable summary size=E badge=ok slot=P whole=P held=E+P",
            ),
            (
                "stale",
                "unsaved sendable summary size=E badge=ok slot=P whole=P held=E+P",
            ),
            (
                "evict",
                "unsaved sendable summary size=E badge=ok slot=P whole=P held=E+P",
            ),
            (
                "rest",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "settle",
                "unsaved sendable summary size=E badge=ok slot=P whole=P held=E+P",
            ),
            (
                "edit",
                "unsaved sendable summary size=T badge=ok slot=P whole=P held=P+T",
            ),
            (
                "edit back to P",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "edit back to F",
                "unsaved sendable summary size=F badge=ok slot=P whole=P held=F+P",
            ),
            (
                "revert",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "rebase",
                "unsaved sendable summary size=E badge=ok slot=R whole=R held=E+R",
            ),
            (
                "rebase file",
                "unsaved sendable summary size=E badge=ok slot=- whole=G held=E → unsaved sendable summary size=E badge=ok slot=G whole=G held=E",
            ),
            (
                "landed P",
                "unsaved sendable summary size=E badge=ok slot=P whole=P held=E+P",
            ),
            (
                "landed F",
                "unsaved sendable summary size=E badge=ok slot=F whole=F held=E+F",
            ),
            (
                "landed file",
                "unsaved sendable summary size=E badge=ok slot=G whole=- held=E",
            ),
            (
                "mark saved",
                "sendable summary size=E badge=ok slot=E whole=- held=E",
            ),
            (
                "unsave",
                "unsaved sendable summary size=E badge=ok slot=P whole=P held=E+P",
            ),
            (
                "hold edit",
                "unsaved sendable summary size=E badge=ok slot=P whole=P held=E+P",
            ),
            (
                "edit saved",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
        ],
    ),
    (
        "whole, unsaved as saved",
        &[
            (
                "as restored",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "took bytes",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "took file",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "took nothing",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "unreadable",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "retry",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "remember",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "stale",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "evict",
                "unread summary might=P size=P badge=ok slot=P whole=- held=0",
            ),
            (
                "rest",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "settle",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "edit",
                "unsaved sendable summary size=T badge=ok slot=P whole=P held=P+T",
            ),
            (
                "edit back to P",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "edit back to F",
                "unsaved sendable summary size=F badge=ok slot=P whole=P held=F+P",
            ),
            (
                "revert",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "rebase",
                "unsaved sendable summary size=P badge=ok slot=R whole=R held=P+R",
            ),
            (
                "rebase file",
                "unsaved sendable summary size=P badge=ok slot=- whole=G held=P → unsaved sendable summary size=P badge=ok slot=G whole=G held=P",
            ),
            (
                "landed P",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "landed F",
                "unsaved sendable summary size=P badge=ok slot=F whole=F held=F+P",
            ),
            (
                "landed file",
                "unsaved sendable summary size=P badge=ok slot=G whole=- held=P",
            ),
            (
                "mark saved",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "unsave",
                "unsaved sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "hold edit",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "edit saved",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
        ],
    ),
    (
        "unread",
        &[
            (
                "as restored",
                "unread reading might=P size=P badge=reading… slot=- whole=- held=0",
            ),
            (
                "took bytes",
                "reading sendable size=P badge=reading… slot=- whole=- held=P → sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "took file",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "took nothing",
                "unread reading might=P size=P badge=reading… slot=- whole=- held=0",
            ),
            (
                "unreadable",
                "unread size=P badge=not read slot=- whole=- held=0",
            ),
            (
                "retry",
                "unread reading might=P size=P badge=reading… slot=- whole=- held=0",
            ),
            (
                "remember",
                "unread summary might=P size=P badge=ok slot=P whole=- held=0",
            ),
            (
                "stale",
                "unread reading might=P size=P badge=reading… slot=- whole=- held=0",
            ),
            (
                "evict",
                "unread reading might=P size=P badge=reading… slot=- whole=- held=0",
            ),
            (
                "rest",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "settle",
                "unread reading might=P size=P badge=reading… slot=- whole=- held=0",
            ),
            (
                "edit",
                "unsaved sendable summary size=T badge=ok slot=- whole=- held=T",
            ),
            (
                "edit back to P",
                "unsaved sendable summary size=P badge=ok slot=- whole=- held=P",
            ),
            (
                "edit back to F",
                "unsaved sendable summary size=F badge=ok slot=- whole=- held=F",
            ),
            (
                "revert",
                "unread reading might=P size=P badge=reading… slot=- whole=- held=0",
            ),
            (
                "rebase",
                "reading unsaved size=0 badge=reading… slot=R whole=R held=R",
            ),
            (
                "rebase file",
                "reading unsaved size=0 badge=reading… slot=- whole=G held=0 → reading unsaved size=0 badge=reading… slot=G whole=G held=0",
            ),
            (
                "landed P",
                "reading unsaved size=0 badge=reading… slot=P whole=P held=P",
            ),
            (
                "landed F",
                "reading unsaved size=0 badge=reading… slot=F whole=F held=F",
            ),
            (
                "landed file",
                "reading unsaved size=0 badge=reading… slot=G whole=- held=0",
            ),
            (
                "mark saved",
                "unread reading might=P size=P badge=reading… slot=- whole=- held=0",
            ),
            (
                "unsave",
                "reading unsaved size=0 badge=reading… slot=- whole=- held=0",
            ),
            (
                "hold edit",
                "unread reading might=P size=P badge=reading… slot=- whole=- held=0",
            ),
            (
                "edit saved",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
        ],
    ),
    (
        "unread, unsaved",
        &[
            (
                "as restored",
                "unsaved sendable summary size=E badge=ok slot=- whole=- held=E",
            ),
            (
                "took bytes",
                "unsaved sendable summary size=E badge=ok slot=- whole=- held=E",
            ),
            (
                "took file",
                "unsaved sendable summary size=E badge=ok slot=- whole=- held=E",
            ),
            (
                "took nothing",
                "unsaved sendable summary size=E badge=ok slot=- whole=- held=E",
            ),
            (
                "unreadable",
                "unsaved sendable summary size=E badge=ok slot=- whole=- held=E",
            ),
            (
                "retry",
                "unsaved sendable summary size=E badge=ok slot=- whole=- held=E",
            ),
            (
                "remember",
                "unsaved sendable summary size=E badge=ok slot=- whole=- held=E",
            ),
            (
                "stale",
                "unsaved sendable summary size=E badge=ok slot=- whole=- held=E",
            ),
            (
                "evict",
                "unsaved sendable summary size=E badge=ok slot=- whole=- held=E",
            ),
            (
                "rest",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "settle",
                "unsaved sendable summary size=E badge=ok slot=- whole=- held=E",
            ),
            (
                "edit",
                "unsaved sendable summary size=T badge=ok slot=- whole=- held=T",
            ),
            (
                "edit back to P",
                "unsaved sendable summary size=P badge=ok slot=- whole=- held=P",
            ),
            (
                "edit back to F",
                "unsaved sendable summary size=F badge=ok slot=- whole=- held=F",
            ),
            (
                "revert",
                "unsaved sendable summary size=E badge=ok slot=- whole=- held=E",
            ),
            (
                "rebase",
                "unsaved sendable summary size=E badge=ok slot=R whole=R held=E+R",
            ),
            (
                "rebase file",
                "unsaved sendable summary size=E badge=ok slot=- whole=G held=E → unsaved sendable summary size=E badge=ok slot=G whole=G held=E",
            ),
            (
                "landed P",
                "unsaved sendable summary size=E badge=ok slot=P whole=P held=E+P",
            ),
            (
                "landed F",
                "unsaved sendable summary size=E badge=ok slot=F whole=F held=E+F",
            ),
            (
                "landed file",
                "unsaved sendable summary size=E badge=ok slot=G whole=- held=E",
            ),
            (
                "mark saved",
                "sendable summary size=E badge=ok slot=E whole=- held=E",
            ),
            (
                "unsave",
                "unsaved sendable summary size=E badge=ok slot=- whole=- held=E",
            ),
            (
                "hold edit",
                "unsaved sendable summary size=E badge=ok slot=- whole=- held=E",
            ),
            (
                "edit saved",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
        ],
    ),
    (
        "remembered",
        &[
            (
                "as restored",
                "unread summary might=P size=P badge=ok slot=P whole=- held=0",
            ),
            (
                "took bytes",
                "reading sendable size=P badge=reading… slot=- whole=- held=P → sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "took file",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "took nothing",
                "unread summary might=P size=P badge=ok slot=P whole=- held=0",
            ),
            (
                "unreadable",
                "unread size=P badge=not read slot=P whole=- held=0",
            ),
            (
                "retry",
                "unread summary might=P size=P badge=ok slot=P whole=- held=0",
            ),
            (
                "remember",
                "unread summary might=P size=P badge=ok slot=P whole=- held=0",
            ),
            (
                "stale",
                "unread reading might=P size=P badge=reading… slot=- whole=- held=0",
            ),
            (
                "evict",
                "unread summary might=P size=P badge=ok slot=P whole=- held=0",
            ),
            (
                "rest",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "settle",
                "unread summary might=P size=P badge=ok slot=P whole=- held=0",
            ),
            (
                "edit",
                "unsaved sendable summary size=T badge=ok slot=P whole=- held=T",
            ),
            (
                "edit back to P",
                "unsaved sendable summary size=P badge=ok slot=P whole=- held=P",
            ),
            (
                "edit back to F",
                "unsaved sendable summary size=F badge=ok slot=P whole=- held=F",
            ),
            (
                "revert",
                "unread summary might=P size=P badge=ok slot=P whole=- held=0",
            ),
            (
                "rebase",
                "unsaved summary size=0 badge=ok slot=R whole=R held=R",
            ),
            (
                "rebase file",
                "unsaved summary size=0 badge=ok slot=- whole=G held=0 → unsaved summary size=0 badge=ok slot=G whole=G held=0",
            ),
            (
                "landed P",
                "unsaved summary size=0 badge=ok slot=P whole=P held=P",
            ),
            (
                "landed F",
                "unsaved summary size=0 badge=ok slot=F whole=F held=F",
            ),
            (
                "landed file",
                "unsaved summary size=0 badge=ok slot=G whole=- held=0",
            ),
            (
                "mark saved",
                "unread summary might=P size=P badge=ok slot=P whole=- held=0",
            ),
            (
                "unsave",
                "unsaved summary size=0 badge=ok slot=P whole=- held=0",
            ),
            (
                "hold edit",
                "unread summary might=P size=P badge=ok slot=P whole=- held=0",
            ),
            (
                "edit saved",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
        ],
    ),
    (
        "not read",
        &[
            (
                "as restored",
                "unread size=P badge=not read slot=- whole=- held=0",
            ),
            (
                "took bytes",
                "reading sendable size=P badge=reading… slot=- whole=- held=P → sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "took file",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "took nothing",
                "unread size=P badge=not read slot=- whole=- held=0",
            ),
            (
                "unreadable",
                "unread size=P badge=not read slot=- whole=- held=0",
            ),
            (
                "retry",
                "unread reading might=P size=P badge=reading… slot=- whole=- held=0",
            ),
            (
                "remember",
                "unread size=P badge=not read slot=- whole=- held=0",
            ),
            (
                "stale",
                "unread size=P badge=not read slot=- whole=- held=0",
            ),
            (
                "evict",
                "unread size=P badge=not read slot=- whole=- held=0",
            ),
            (
                "rest",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "settle",
                "unread size=P badge=not read slot=- whole=- held=0",
            ),
            (
                "edit",
                "unsaved sendable summary size=T badge=ok slot=- whole=- held=T",
            ),
            (
                "edit back to P",
                "unsaved sendable summary size=P badge=ok slot=- whole=- held=P",
            ),
            (
                "edit back to F",
                "unsaved sendable summary size=F badge=ok slot=- whole=- held=F",
            ),
            (
                "revert",
                "unread size=P badge=not read slot=- whole=- held=0",
            ),
            (
                "rebase",
                "unsaved size=0 badge=not read slot=R whole=R held=R",
            ),
            (
                "rebase file",
                "unsaved size=0 badge=not read slot=- whole=G held=0 → unsaved size=0 badge=not read slot=G whole=G held=0",
            ),
            (
                "landed P",
                "unsaved size=0 badge=not read slot=P whole=P held=P",
            ),
            (
                "landed F",
                "unsaved size=0 badge=not read slot=F whole=F held=F",
            ),
            (
                "landed file",
                "unsaved size=0 badge=not read slot=G whole=- held=0",
            ),
            (
                "mark saved",
                "unread size=P badge=not read slot=- whole=- held=0",
            ),
            (
                "unsave",
                "unsaved size=0 badge=not read slot=- whole=- held=0",
            ),
            (
                "hold edit",
                "unread size=P badge=not read slot=- whole=- held=0",
            ),
            (
                "edit saved",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
        ],
    ),
    (
        "remembered, not read",
        &[
            (
                "as restored",
                "unread size=P badge=not read slot=P whole=- held=0",
            ),
            (
                "took bytes",
                "reading sendable size=P badge=reading… slot=- whole=- held=P → sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "took file",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "took nothing",
                "unread size=P badge=not read slot=P whole=- held=0",
            ),
            (
                "unreadable",
                "unread size=P badge=not read slot=P whole=- held=0",
            ),
            (
                "retry",
                "unread reading might=P size=P badge=reading… slot=P whole=- held=0",
            ),
            (
                "remember",
                "unread size=P badge=not read slot=P whole=- held=0",
            ),
            (
                "stale",
                "unread size=P badge=not read slot=- whole=- held=0",
            ),
            (
                "evict",
                "unread size=P badge=not read slot=P whole=- held=0",
            ),
            (
                "rest",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "settle",
                "unread size=P badge=not read slot=P whole=- held=0",
            ),
            (
                "edit",
                "unsaved sendable summary size=T badge=ok slot=P whole=- held=T",
            ),
            (
                "edit back to P",
                "unsaved sendable summary size=P badge=ok slot=P whole=- held=P",
            ),
            (
                "edit back to F",
                "unsaved sendable summary size=F badge=ok slot=P whole=- held=F",
            ),
            (
                "revert",
                "unread size=P badge=not read slot=P whole=- held=0",
            ),
            (
                "rebase",
                "unsaved size=0 badge=not read slot=R whole=R held=R",
            ),
            (
                "rebase file",
                "unsaved size=0 badge=not read slot=- whole=G held=0 → unsaved size=0 badge=not read slot=G whole=G held=0",
            ),
            (
                "landed P",
                "unsaved size=0 badge=not read slot=P whole=P held=P",
            ),
            (
                "landed F",
                "unsaved size=0 badge=not read slot=F whole=F held=F",
            ),
            (
                "landed file",
                "unsaved size=0 badge=not read slot=G whole=- held=0",
            ),
            (
                "mark saved",
                "unread size=P badge=not read slot=P whole=- held=0",
            ),
            (
                "unsave",
                "unsaved size=0 badge=not read slot=P whole=- held=0",
            ),
            (
                "hold edit",
                "unread size=P badge=not read slot=P whole=- held=0",
            ),
            (
                "edit saved",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
        ],
    ),
    (
        "file",
        &[
            (
                "as restored",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "took bytes",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "took file",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "took nothing",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "unreadable",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "retry",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "remember",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "stale",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "evict",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "rest",
                "rests might=G size=G badge=checking… slot=- whole=- held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "settle",
                "rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "edit",
                "unsaved sendable summary size=T badge=ok slot=- whole=F held=T → unsaved sendable summary size=T badge=ok slot=F whole=F held=T",
            ),
            (
                "edit back to P",
                "unsaved sendable summary size=P badge=ok slot=- whole=F held=P → unsaved sendable summary size=P badge=ok slot=F whole=F held=P",
            ),
            (
                "edit back to F",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "revert",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "rebase",
                "sendable summary size=R badge=ok slot=R whole=- held=R",
            ),
            (
                "rebase file",
                "rests might=G size=G badge=checking… slot=- whole=- held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "landed P",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "landed F",
                "size=0 badge=checking… slot=- whole=0x00000000 held=0",
            ),
            (
                "landed file",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "mark saved",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "unsave",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "hold edit",
                "rests unsaved edit-held size=F badge=checking… slot=- whole=F held=0 → rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "edit saved",
                "rests might=G size=G badge=checking… slot=- whole=- held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
        ],
    ),
    (
        "file, check queued",
        &[
            (
                "as restored",
                "rests might=F size=F badge=checking… slot=- whole=- held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "took bytes",
                "rests might=F size=F badge=checking… slot=- whole=- held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "took file",
                "rests might=F size=F badge=checking… slot=- whole=- held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "took nothing",
                "rests might=F size=F badge=checking… slot=- whole=- held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "unreadable",
                "rests might=F size=F badge=checking… slot=- whole=- held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "retry",
                "rests might=F size=F badge=checking… slot=- whole=- held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "remember",
                "rests might=F size=F badge=checking… slot=- whole=- held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "stale",
                "rests might=F size=F badge=checking… slot=- whole=- held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "evict",
                "rests might=F size=F badge=checking… slot=- whole=- held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "rest",
                "rests might=G size=G badge=checking… slot=- whole=- held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "settle",
                "rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "edit",
                "unsaved sendable summary size=T badge=ok slot=- whole=- held=T → unsaved sendable summary size=T badge=ok slot=F whole=F held=T",
            ),
            (
                "edit back to P",
                "unsaved sendable summary size=P badge=ok slot=- whole=- held=P → unsaved sendable summary size=P badge=ok slot=F whole=F held=P",
            ),
            (
                "edit back to F",
                "unsaved sendable summary size=F badge=ok slot=- whole=- held=F → unsaved sendable summary size=F badge=ok slot=F whole=F held=F",
            ),
            (
                "revert",
                "rests might=F size=F badge=checking… slot=- whole=- held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "rebase",
                "sendable summary size=R badge=ok slot=R whole=- held=R",
            ),
            (
                "rebase file",
                "rests might=G size=G badge=checking… slot=- whole=- held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "landed P",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "landed F",
                "sendable summary size=F badge=ok slot=F whole=F held=F",
            ),
            (
                "landed file",
                "rests might=F size=F badge=checking… slot=- whole=- held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "mark saved",
                "rests might=F size=F badge=checking… slot=- whole=- held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "unsave",
                "rests might=F size=F badge=checking… slot=- whole=- held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "hold edit",
                "rests unsaved edit-held might=F size=F badge=checking… slot=- whole=- held=0 → rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "edit saved",
                "rests might=G size=G badge=checking… slot=- whole=- held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
        ],
    ),
    (
        "file, checked",
        &[
            (
                "as restored",
                "rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "took bytes",
                "rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "took file",
                "rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "took nothing",
                "rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "unreadable",
                "rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "retry",
                "rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "remember",
                "rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "stale",
                "rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "evict",
                "rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "rest",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "settle",
                "rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "edit",
                "unsaved sendable summary size=T badge=ok slot=F whole=F held=T",
            ),
            (
                "edit back to P",
                "unsaved sendable summary size=P badge=ok slot=F whole=F held=P",
            ),
            (
                "edit back to F",
                "rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "revert",
                "rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "rebase",
                "sendable summary size=R badge=ok slot=R whole=- held=R",
            ),
            (
                "rebase file",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "landed P",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "landed F",
                "rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "landed file",
                "rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "mark saved",
                "rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "unsave",
                "rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "hold edit",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "edit saved",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
        ],
    ),
    (
        "file, failed",
        &[
            (
                "as restored",
                "rests summary size=B badge=failed slot=- whole=B held=0",
            ),
            (
                "took bytes",
                "rests summary size=B badge=failed slot=- whole=B held=0",
            ),
            (
                "took file",
                "rests summary size=B badge=failed slot=- whole=B held=0",
            ),
            (
                "took nothing",
                "rests summary size=B badge=failed slot=- whole=B held=0",
            ),
            (
                "unreadable",
                "rests summary size=B badge=failed slot=- whole=B held=0",
            ),
            (
                "retry",
                "rests summary size=B badge=failed slot=- whole=B held=0",
            ),
            (
                "remember",
                "rests summary size=B badge=failed slot=- whole=B held=0",
            ),
            (
                "stale",
                "rests summary size=B badge=failed slot=- whole=B held=0",
            ),
            (
                "evict",
                "rests summary size=B badge=failed slot=- whole=B held=0",
            ),
            (
                "rest",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "settle",
                "rests summary size=B badge=failed slot=- whole=B held=0",
            ),
            (
                "edit",
                "unsaved sendable summary size=T badge=ok slot=- whole=B held=T",
            ),
            (
                "edit back to P",
                "unsaved sendable summary size=P badge=ok slot=- whole=B held=P",
            ),
            (
                "edit back to F",
                "unsaved sendable summary size=F badge=ok slot=- whole=B held=F",
            ),
            (
                "revert",
                "rests summary size=B badge=failed slot=- whole=B held=0",
            ),
            (
                "rebase",
                "sendable summary size=R badge=ok slot=R whole=- held=R",
            ),
            (
                "rebase file",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "landed P",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "landed F",
                "sendable summary size=F badge=ok slot=F whole=F held=F",
            ),
            (
                "landed file",
                "rests summary size=B badge=failed slot=- whole=B held=0",
            ),
            (
                "mark saved",
                "rests summary size=B badge=failed slot=- whole=B held=0",
            ),
            (
                "unsave",
                "rests summary size=B badge=failed slot=- whole=B held=0",
            ),
            (
                "hold edit",
                "rests unsaved summary edit-held size=B badge=failed slot=- whole=B held=0",
            ),
            (
                "edit saved",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
        ],
    ),
    (
        "file, unsaved",
        &[
            (
                "as restored",
                "unsaved sendable summary size=E badge=ok slot=- whole=F held=E → unsaved sendable summary size=E badge=ok slot=F whole=F held=E",
            ),
            (
                "took bytes",
                "unsaved sendable summary size=E badge=ok slot=- whole=F held=E → unsaved sendable summary size=E badge=ok slot=F whole=F held=E",
            ),
            (
                "took file",
                "unsaved sendable summary size=E badge=ok slot=- whole=F held=E → unsaved sendable summary size=E badge=ok slot=F whole=F held=E",
            ),
            (
                "took nothing",
                "unsaved sendable summary size=E badge=ok slot=- whole=F held=E → unsaved sendable summary size=E badge=ok slot=F whole=F held=E",
            ),
            (
                "unreadable",
                "unsaved sendable summary size=E badge=ok slot=- whole=F held=E → unsaved sendable summary size=E badge=ok slot=F whole=F held=E",
            ),
            (
                "retry",
                "unsaved sendable summary size=E badge=ok slot=- whole=F held=E → unsaved sendable summary size=E badge=ok slot=F whole=F held=E",
            ),
            (
                "remember",
                "unsaved sendable summary size=E badge=ok slot=- whole=F held=E → unsaved sendable summary size=E badge=ok slot=F whole=F held=E",
            ),
            (
                "stale",
                "unsaved sendable summary size=E badge=ok slot=- whole=F held=E → unsaved sendable summary size=E badge=ok slot=F whole=F held=E",
            ),
            (
                "evict",
                "unsaved sendable summary size=E badge=ok slot=- whole=F held=E → unsaved sendable summary size=E badge=ok slot=F whole=F held=E",
            ),
            (
                "rest",
                "rests might=G size=G badge=checking… slot=- whole=- held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "settle",
                "unsaved sendable summary size=E badge=ok slot=F whole=F held=E",
            ),
            (
                "edit",
                "unsaved sendable summary size=T badge=ok slot=- whole=F held=T → unsaved sendable summary size=T badge=ok slot=F whole=F held=T",
            ),
            (
                "edit back to P",
                "unsaved sendable summary size=P badge=ok slot=- whole=F held=P → unsaved sendable summary size=P badge=ok slot=F whole=F held=P",
            ),
            (
                "edit back to F",
                "rests summary size=F badge=n/a slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "revert",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "rebase",
                "unsaved sendable summary size=E badge=ok slot=R whole=R held=E+R",
            ),
            (
                "rebase file",
                "unsaved sendable summary size=E badge=ok slot=- whole=- held=E → unsaved sendable summary size=E badge=ok slot=G whole=G held=E",
            ),
            (
                "landed P",
                "unsaved sendable summary size=E badge=ok slot=P whole=P held=E+P",
            ),
            (
                "landed F",
                "unsaved sendable summary size=E badge=ok slot=F whole=F held=E+F",
            ),
            (
                "landed file",
                "unsaved sendable summary size=E badge=ok slot=G whole=- held=E",
            ),
            (
                "mark saved",
                "sendable summary size=E badge=ok slot=E whole=- held=E",
            ),
            (
                "unsave",
                "unsaved sendable summary size=E badge=ok slot=- whole=F held=E → unsaved sendable summary size=E badge=ok slot=F whole=F held=E",
            ),
            (
                "hold edit",
                "unsaved sendable summary size=E badge=ok slot=- whole=F held=E → unsaved sendable summary size=E badge=ok slot=F whole=F held=E",
            ),
            (
                "edit saved",
                "rests might=G size=G badge=checking… slot=- whole=- held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
        ],
    ),
    (
        "file and unread",
        &[
            (
                "as restored",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "took bytes",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "took file",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "took nothing",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "unreadable",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "retry",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "remember",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "stale",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "evict",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "rest",
                "rests might=G size=G badge=checking… slot=- whole=- held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "settle",
                "rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "edit",
                "unsaved sendable summary size=T badge=ok slot=- whole=F held=T → unsaved sendable summary size=T badge=ok slot=F whole=F held=T",
            ),
            (
                "edit back to P",
                "unsaved sendable summary size=P badge=ok slot=- whole=F held=P → unsaved sendable summary size=P badge=ok slot=F whole=F held=P",
            ),
            (
                "edit back to F",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "revert",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "rebase",
                "sendable summary size=R badge=ok slot=R whole=- held=R",
            ),
            (
                "rebase file",
                "rests might=G size=G badge=checking… slot=- whole=- held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "landed P",
                "sendable summary size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "landed F",
                "size=0 badge=checking… slot=- whole=0x00000000 held=0",
            ),
            (
                "landed file",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "mark saved",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "unsave",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "hold edit",
                "rests unsaved edit-held size=F badge=checking… slot=- whole=F held=0 → rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "edit saved",
                "rests might=G size=G badge=checking… slot=- whole=- held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
        ],
    ),
    (
        "file and unread, unsaved",
        &[
            (
                "as restored",
                "unsaved sendable summary size=E badge=ok slot=- whole=F held=E → unsaved sendable summary size=E badge=ok slot=F whole=F held=E",
            ),
            (
                "took bytes",
                "unsaved sendable summary size=E badge=ok slot=- whole=F held=E → unsaved sendable summary size=E badge=ok slot=F whole=F held=E",
            ),
            (
                "took file",
                "unsaved sendable summary size=E badge=ok slot=- whole=F held=E → unsaved sendable summary size=E badge=ok slot=F whole=F held=E",
            ),
            (
                "took nothing",
                "unsaved sendable summary size=E badge=ok slot=- whole=F held=E → unsaved sendable summary size=E badge=ok slot=F whole=F held=E",
            ),
            (
                "unreadable",
                "unsaved sendable summary size=E badge=ok slot=- whole=F held=E → unsaved sendable summary size=E badge=ok slot=F whole=F held=E",
            ),
            (
                "retry",
                "unsaved sendable summary size=E badge=ok slot=- whole=F held=E → unsaved sendable summary size=E badge=ok slot=F whole=F held=E",
            ),
            (
                "remember",
                "unsaved sendable summary size=E badge=ok slot=- whole=F held=E → unsaved sendable summary size=E badge=ok slot=F whole=F held=E",
            ),
            (
                "stale",
                "unsaved sendable summary size=E badge=ok slot=- whole=F held=E → unsaved sendable summary size=E badge=ok slot=F whole=F held=E",
            ),
            (
                "evict",
                "unsaved sendable summary size=E badge=ok slot=- whole=F held=E → unsaved sendable summary size=E badge=ok slot=F whole=F held=E",
            ),
            (
                "rest",
                "rests might=G size=G badge=checking… slot=- whole=- held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "settle",
                "unsaved sendable summary size=E badge=ok slot=F whole=F held=E",
            ),
            (
                "edit",
                "unsaved sendable summary size=T badge=ok slot=- whole=F held=T → unsaved sendable summary size=T badge=ok slot=F whole=F held=T",
            ),
            (
                "edit back to P",
                "unsaved sendable summary size=P badge=ok slot=- whole=F held=P → unsaved sendable summary size=P badge=ok slot=F whole=F held=P",
            ),
            (
                "edit back to F",
                "rests summary size=F badge=n/a slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "revert",
                "rests size=F badge=checking… slot=- whole=F held=0 → rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "rebase",
                "unsaved sendable summary size=E badge=ok slot=R whole=R held=E+R",
            ),
            (
                "rebase file",
                "unsaved sendable summary size=E badge=ok slot=- whole=- held=E → unsaved sendable summary size=E badge=ok slot=G whole=G held=E",
            ),
            (
                "landed P",
                "unsaved sendable summary size=E badge=ok slot=P whole=P held=E+P",
            ),
            (
                "landed F",
                "unsaved sendable summary size=E badge=ok slot=F whole=F held=E+F",
            ),
            (
                "landed file",
                "unsaved sendable summary size=E badge=ok slot=G whole=- held=E",
            ),
            (
                "mark saved",
                "sendable summary size=E badge=ok slot=E whole=- held=E",
            ),
            (
                "unsave",
                "unsaved sendable summary size=E badge=ok slot=- whole=F held=E → unsaved sendable summary size=E badge=ok slot=F whole=F held=E",
            ),
            (
                "hold edit",
                "unsaved sendable summary size=E badge=ok slot=- whole=F held=E → unsaved sendable summary size=E badge=ok slot=F whole=F held=E",
            ),
            (
                "edit saved",
                "rests might=G size=G badge=checking… slot=- whole=- held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
        ],
    ),
    (
        "file, edit held",
        &[
            (
                "as restored",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "took bytes",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "took file",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "took nothing",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "unreadable",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "retry",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "remember",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "stale",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "evict",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "rest",
                "rests unsaved edit-held size=G badge=checking… slot=- whole=G held=0 → rests unsaved sendable summary edit-held size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "settle",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "edit",
                "unsaved sendable summary edit-held size=T badge=ok slot=F whole=F held=T",
            ),
            (
                "edit back to P",
                "unsaved sendable summary edit-held size=P badge=ok slot=F whole=F held=P",
            ),
            (
                "edit back to F",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "revert",
                "rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "rebase",
                "unsaved sendable summary edit-held size=R badge=ok slot=R whole=- held=R",
            ),
            (
                "rebase file",
                "rests unsaved edit-held size=G badge=checking… slot=- whole=G held=0 → rests unsaved sendable summary edit-held size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "landed P",
                "unsaved sendable summary edit-held size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "landed F",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "landed file",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "mark saved",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "unsave",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "hold edit",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "edit saved",
                "rests unsaved edit-held size=G badge=checking… slot=- whole=G held=0 → rests unsaved sendable summary edit-held size=G badge=ok slot=G whole=G held=0",
            ),
        ],
    ),
    (
        "file, edit sent",
        &[
            (
                "as restored",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "took bytes",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "took file",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "took nothing",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "unreadable",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "retry",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "remember",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "stale",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "evict",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "rest",
                "rests unsaved edit-held size=G badge=checking… slot=- whole=G held=0 → rests unsaved sendable summary edit-held size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "settle",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "edit",
                "unsaved sendable summary edit-held size=T badge=ok slot=F whole=F held=T",
            ),
            (
                "edit back to P",
                "unsaved sendable summary edit-held size=P badge=ok slot=F whole=F held=P",
            ),
            (
                "edit back to F",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "revert",
                "rests sendable summary size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "rebase",
                "unsaved sendable summary edit-held size=R badge=ok slot=R whole=- held=R",
            ),
            (
                "rebase file",
                "rests unsaved edit-held size=G badge=checking… slot=- whole=G held=0 → rests unsaved sendable summary edit-held size=G badge=ok slot=G whole=G held=0",
            ),
            (
                "landed P",
                "unsaved sendable summary edit-held size=P badge=ok slot=P whole=P held=P",
            ),
            (
                "landed F",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "landed file",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "mark saved",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "unsave",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "hold edit",
                "rests unsaved sendable summary edit-held size=F badge=ok slot=F whole=F held=0",
            ),
            (
                "edit saved",
                "rests size=G badge=checking… slot=- whole=G held=0 → rests sendable summary size=G badge=ok slot=G whole=G held=0",
            ),
        ],
    ),
];

/// What the accessors read after `step` from `begin`, and once every check and decode
/// has answered.
fn row(fixture: &Fixture, begin: Step, step: Step) -> String {
    let mut at = At::new(fixture);
    begin(fixture, &mut at);
    at.quiet();
    step(fixture, &mut at);
    at.quiet();
    let now = at.seen(fixture);
    at.settle();
    let settled = at.seen(fixture);
    match now == settled {
        true => now,
        false => format!("{now} → {settled}"),
    }
}

#[test]
fn every_transition_reads_as_the_table_says_from_every_restored_asset() {
    let fixture = Fixture::new();
    let starts = STARTS.iter().map(|(start, _)| *start);
    let blocks = EXPECTED.iter().map(|(start, _)| *start);
    assert!(starts.eq(blocks), "one block per start, in order");
    let mut wrong = Vec::new();
    for ((start, begin), (_, rows)) in STARTS.iter().zip(EXPECTED) {
        let transitions = TRANSITIONS.iter().map(|(transition, _)| *transition);
        let named = rows.iter().map(|(transition, _)| *transition);
        assert!(
            transitions.eq(named),
            "{start}: one row per transition, in order"
        );
        for ((transition, step), (_, expected)) in TRANSITIONS.iter().zip(*rows) {
            let seen = row(&fixture, *begin, *step);
            if seen != *expected {
                wrong.push(format!(
                    "{start}, {transition}:\n  expected {expected}\n  seen     {seen}"
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A baseline taken whole from a store under an unsaved edit has its slot checksum at
/// once, read from its header. One whose bytes are the asset's own, still to be decoded,
/// has none until the decode answers.
#[test]
fn a_baseline_under_an_edit_has_its_slot_checksum_before_a_decode_and_a_held_one_after() {
    let fixture = Fixture::new();
    let slot = |at: &At| at.workspace.get(ID).unwrap().saved.crc32;
    let p = Container::read(&fixture.p).map(|held| held.body_crc32);
    assert!(p.is_some());

    let mut edited = At::new(&fixture);
    edited.restore(Held::Whole(fixture.p.clone()), Some(fixture.e.clone()));
    assert_eq!(slot(&edited), p, "taken from the header");

    let mut held = At::new(&fixture);
    held.restore(Held::Whole(fixture.p.clone()), None);
    assert_eq!(slot(&held), None, "not decoded yet");
    held.settle();
    assert_eq!(slot(&held), p, "taken from the decode");
}
