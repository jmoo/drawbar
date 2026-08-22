#![cfg(feature = "corpus")]
//! Read coverage, as a snapshot per body type: which body bits anything reads.
//!
//! The write-side sweep in `tests/corpus` starts from a field and asks whether the
//! bytes follow. This asks the other question — **which bits does the instrument
//! write that no decode reads** — and it is the one a new model's first decode is
//! mostly made of.
//!
//! Three measurements per bit, none of them naming a model or a file:
//!
//! - **varies** — OR against AND of every corpus body. A bit that ever differs is
//!   one the instrument writes. All-ones bodies are excluded: an unwritten slot
//!   holds no evidence, the same exemption the sweep makes. So are the committed
//!   fixtures, which this crate's own writers produced over a zeroed body — their
//!   unclaimed bits are evidence of nothing. They still baseline flips.
//! - **claimed** — the registry path that declares the bit, from [`BodyLayout`],
//!   recursively through nested bodies. Intent, not reads: a private field claims
//!   bits it never reports.
//! - **answers** — flip the bit, restamp the container, re-parse, and diff
//!   `field_values()` against the unflipped baseline. The set of paths that moved,
//!   or `refused` where the parser rejected the mutated file — which is a read too.
//!
//! The two lists the file ends with are the point. **vary, unread** is the blind
//! spot: the instrument writes it and nothing here looks. **claimed, unanswered**
//! is the other failure — a declaration no flip can move, so it is dead or aimed at
//! the wrong bits.
//!
//! ⚠️ The flip is per bit. A two-bit encoding where either bit alone is an illegal
//! value reads as `refused` twice rather than as the field it belongs to.
//!
//! A body joins by being a `#[bitbody]` behind `with_registry!`, and a specimen by
//! being in the sweep. Nothing below is per model, so there is no test code to write
//! for the next one — only a snapshot to read.
//!
//! ```sh
//! NORD_CORPUS_ROOT=/path/to/nord-corpus \
//!   cargo test --release -p nord-format --features corpus --test coverage
//! ```
//!
//! Regenerate with `UPDATE_SNAPSHOTS=1`, and **read the diff**.
//!
//! [`BodyLayout`]: nord_format::layout::BodyLayout

#[path = "support/registry.rs"]
mod registry;
#[path = "support/scan.rs"]
mod scan;
#[path = "support/sidecar.rs"]
mod sidecar;
#[path = "support/snapshot.rs"]
mod snapshot;

use nord_format::cbin;
use nord_format::layout::LayoutField;
use nord_format::Entity;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io::Cursor;

/// How the answer set renders before it is summarized by count. Past it a line
/// prints `N [first few, …]`, so a change past the cut still moves the count.
const SHOWN: usize = 4;

/// The parser rejecting a mutated file. Not a path, and deliberately spelled like
/// one: refusing to decode is a way of reading a bit.
const REFUSED: &str = "refused";

/// What the specimens of one body type say about one of its bits.
#[derive(Default, PartialEq, Eq)]
struct Bit {
    /// The registry path declaring it, from the layout walk.
    claimed: Option<String>,
    /// Whether that path reports a value. A private field claims without reporting.
    registered: bool,
    varies: bool,
    /// Registry paths a flip moved, plus [`REFUSED`].
    answers: BTreeSet<String>,
}

impl Bit {
    /// The instrument writes it and nothing reads it — the one that matters.
    fn blind(&self) -> bool {
        self.varies && self.answers.is_empty()
    }

    fn verdict(&self) -> &'static str {
        if self.answers.len() == 1 && self.answers.contains(REFUSED) {
            "refused"
        } else if !self.answers.is_empty() {
            "read"
        } else if self.varies {
            "BLIND"
        } else if self.claimed.is_some() {
            "unanswered"
        } else {
            "spare"
        }
    }
}

/// One body type's specimens and what they measure.
struct Body {
    bits: usize,
    /// Every instrument body OR'd, and AND'd: they differ exactly where a bit varies.
    ones: Vec<u8>,
    zeros: Vec<u8>,
    /// Instrument specimens weighed, and specimens flipped. Not nested sets — a
    /// fixture is flipped without being evidence of what an instrument writes.
    weighed: usize,
    flipped: usize,
    facts: Vec<Bit>,
}

/// One specimen of the sweep's population, with the sampling verdict the sweep
/// would give it.
struct Pick {
    specimen: &'static scan::Specimen,
    sampled: bool,
    /// Whether an instrument wrote these bytes. ⚠️ The fixtures are this crate's
    /// own writers' output over a zeroed body, so their unclaimed bits say nothing
    /// about what the instrument writes: they baseline flips, never `varies`.
    instrument: bool,
}

/// Every specimen the sweep reads, in its order, each marked with whether the sweep
/// would mutate it: every fixture, every specimen carrying an oracle sidecar, and
/// the first of each container shape among the rest.
fn population() -> Vec<Pick> {
    let mut out: Vec<Pick> = scan::fixtures()
        .iter()
        .map(|specimen| Pick {
            specimen,
            sampled: true,
            instrument: false,
        })
        .collect();

    let mut shapes = BTreeSet::new();
    for specimen in scan::corpus() {
        let sampled = sidecar::sidecar_of(&specimen.path).exists()
            || scan::shape(&specimen.path).is_none_or(|s| shapes.insert(s));
        out.push(Pick {
            specimen,
            sampled,
            instrument: true,
        });
    }
    out
}

/// Every bit's claiming path, from the declared layout. Nested bodies contribute
/// their own layouts at their own offsets, under a dotted path.
fn claims(fields: &'static [LayoutField], base: u32, prefix: &str, out: &mut [Bit]) {
    for field in fields {
        let path = if prefix.is_empty() {
            field.path.to_string()
        } else {
            format!("{prefix}.{}", field.path)
        };
        match field.nested {
            Some(nested) => claims(nested(), base + field.lo, &path, out),
            None => {
                for bit in base + field.lo..=base + field.hi {
                    out[bit as usize].claimed = Some(path.clone());
                }
            }
        }
    }
}

/// Flip every bit of one specimen's body in turn and record which registry paths
/// notice. The container is rewritten each time, so both generations' checksums are
/// restamped and the mutated file is one the reader would accept.
fn answers(bytes: &[u8], entity: &Entity, facts: &mut [Bit]) {
    let baseline: Vec<String> = registry::field_values(entity)
        .expect("a body with a registry")
        .into_iter()
        .map(|f| f.value)
        .collect();
    let mut file = cbin::read_raw(&mut Cursor::new(bytes)).expect("a container the sweep read");
    let mut out = Cursor::new(Vec::with_capacity(bytes.len()));

    for (bit, fact) in facts.iter_mut().enumerate() {
        let mask = 1u8 << (7 - bit % 8);
        file.body.0[bit / 8] ^= mask;
        out.get_mut().clear();
        out.set_position(0);
        file.write_to(&mut out).expect("a raw body re-encodes");

        match nord_format::from_stream(&mut Cursor::new(out.get_ref())) {
            Err(_) => {
                fact.answers.insert(REFUSED.to_string());
            }
            // The registry is a property of the body type, which a body-bit flip
            // cannot change; losing it would be a change worth reporting anyway.
            Ok(flipped) => match registry::field_values(&flipped) {
                None => {
                    fact.answers.insert(REFUSED.to_string());
                }
                Some(values) => {
                    for (was, now) in baseline.iter().zip(values) {
                        if *was != now.value {
                            fact.answers.insert(now.name);
                        }
                    }
                }
            },
        }

        file.body.0[bit / 8] ^= mask;
    }
}

/// Every registry body the sweep's specimens reach, measured.
fn measure() -> BTreeMap<String, Body> {
    let mut bodies: BTreeMap<String, Body> = BTreeMap::new();

    for pick in population() {
        let (bytes, entity) = (&pick.specimen.bytes, &pick.specimen.entity);
        let Some(key) = registry::body_type(entity) else {
            continue;
        };
        let file = cbin::read_raw(&mut Cursor::new(bytes)).expect("a CBIN the sweep read");
        let body = file.body.0;
        // An unwritten slot: every field there is out of table and every bit is a
        // difference that means nothing.
        if !body.is_empty() && body.iter().all(|&b| b == 0xff) {
            continue;
        }

        let entry = bodies.entry(key).or_insert_with(|| {
            let bits = body.len() * 8;
            let mut facts: Vec<Bit> = (0..bits).map(|_| Bit::default()).collect();
            claims(
                registry::layout(entity).expect("a registry body declares a layout"),
                0,
                "",
                &mut facts,
            );
            let reported: BTreeSet<String> = registry::field_values(entity)
                .expect("a body with a registry")
                .into_iter()
                .map(|f| f.name)
                .collect();
            for fact in facts.iter_mut() {
                fact.registered = fact.claimed.as_ref().is_some_and(|p| reported.contains(p));
            }
            Body {
                bits,
                ones: vec![0x00; body.len()],
                zeros: vec![0xff; body.len()],
                weighed: 0,
                flipped: 0,
                facts,
            }
        });
        assert_eq!(entry.bits, body.len() * 8, "one body type, two lengths");

        if pick.instrument {
            for (at, byte) in body.iter().enumerate() {
                entry.ones[at] |= byte;
                entry.zeros[at] &= byte;
            }
            entry.weighed += 1;
        }

        if pick.sampled {
            entry.flipped += 1;
            answers(bytes, entity, &mut entry.facts);
        }
    }

    for body in bodies.values_mut() {
        // A body only the fixtures reach has no evidence either way, and an
        // all-ones `zeros` against an all-zero `ones` would read as every bit
        // varying.
        if body.weighed == 0 {
            continue;
        }
        for (bit, fact) in body.facts.iter_mut().enumerate() {
            let mask = 1u8 << (7 - bit % 8);
            fact.varies = (body.ones[bit / 8] ^ body.zeros[bit / 8]) & mask != 0;
        }
    }
    bodies
}

/// A run of bits as a hex dump locates it: a partial leading byte, the whole bytes
/// between as a half-open byte range, then a partial trailing byte.
fn at(lo: usize, hi: usize) -> String {
    let mut parts = Vec::new();
    let mut first = lo;

    if !first.is_multiple_of(8) {
        let byte = first / 8;
        let end = hi.min(byte * 8 + 7);
        parts.push(within(byte, first, end));
        first = end + 1;
        if first > hi {
            return parts.join("+");
        }
    }

    let tail = (!(hi + 1).is_multiple_of(8)).then_some(hi / 8);
    let (start, end) = (first / 8, tail.unwrap_or((hi + 1) / 8));
    if end > start {
        parts.push(if end - start == 1 {
            format!("{start:#04x}")
        } else {
            format!("{start:#04x}..{end:#04x}")
        });
    }
    if let Some(byte) = tail {
        parts.push(within(byte, byte * 8, hi));
    }
    parts.join("+")
}

/// One byte's slice of a run, numbered the way the format notes number bits:
/// LSB-first, high end first — `0x51[3:2]`.
fn within(byte: usize, lo: usize, hi: usize) -> String {
    let (high, low) = (7 - lo % 8, 7 - hi % 8);
    if high == low {
        format!("{byte:#04x}[{high}]")
    } else {
        format!("{byte:#04x}[{high}:{low}]")
    }
}

/// `center_panel.transpose` or `3 [a, b, c, …]` — bare while the set is short,
/// counted once it is not, so a change past the cut still moves the line.
fn summarize(paths: &BTreeSet<String>) -> String {
    if paths.is_empty() {
        return "—".to_string();
    }
    if paths.len() <= SHOWN {
        return paths.iter().cloned().collect::<Vec<_>>().join(", ");
    }
    let head: Vec<_> = paths.iter().take(SHOWN).cloned().collect();
    format!("{} [{}, …]", paths.len(), head.join(", "))
}

/// One body type's report: the run table, then the two lists that are the point.
fn render(key: &str, body: &Body) -> String {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for bit in 0..body.bits {
        match runs.last() {
            Some(&(start, _)) if body.facts[bit] == body.facts[start] => {
                runs.last_mut().unwrap().1 = bit;
            }
            _ => runs.push((bit, bit)),
        }
    }

    let mut out = String::new();
    let _ = write!(
        out,
        "# {key} — read coverage over the specimen sweep. Runs of bits sharing a verdict.\n\
         #\n\
         # varies   an instrument writes this bit two ways (OR against AND over every\n\
         #          corpus specimen of this body; all-ones bodies excluded). ⚠️ The\n\
         #          committed fixtures are left out of this column: they are this\n\
         #          crate's own writers' output over a zeroed body, so their unclaimed\n\
         #          bits are evidence of nothing.\n\
         # claimed  the path declaring it, from the layout — intent, not a read. A path\n\
         #          marked (unregistered) is declared but reports no value.\n\
         # answers  registry paths whose value moved when the bit was flipped and the\n\
         #          file re-parsed, over the sampled specimens; `refused` is the parser\n\
         #          rejecting the mutated file, which is a read too.\n\
         #\n\
         # Offsets are body-relative — a hex dump of the file adds the container's body\n\
         # start, 0x2c on a type-1 header and 0x18 on a type-0. Bits inside a byte are\n\
         # numbered LSB-first: `0x51[3:2]`.\n\
         #\n\
         # {} bits; varies over {} corpus specimens, answers over {} flipped.\n\n",
        body.bits, body.weighed, body.flipped,
    );
    let _ = writeln!(
        out,
        "{:<14} {:<34} {:<44} {:<7} {:<10} answers",
        "bits", "at", "claimed by", "varies", "verdict"
    );

    for &(lo, hi) in &runs {
        let fact = &body.facts[lo];
        let claimed = match &fact.claimed {
            Some(path) if fact.registered => path.clone(),
            Some(path) => format!("{path} (unregistered)"),
            None => "—".to_string(),
        };
        let _ = writeln!(
            out,
            "{:<14} {:<34} {:<44} {:<7} {:<10} {}",
            format!("{lo}..={hi}"),
            at(lo, hi),
            claimed,
            if fact.varies { "yes" } else { "no" },
            fact.verdict(),
            summarize(&fact.answers),
        );
    }

    let mut blind = 0;
    let mut lines = String::new();
    for &(lo, hi) in &runs {
        if body.facts[lo].blind() {
            blind += hi - lo + 1;
            let _ = writeln!(lines, "  {:<14} {}", format!("{lo}..={hi}"), at(lo, hi));
        }
    }
    let _ = write!(
        out,
        "\n## vary, unread — {blind} bits\n\
         ## The instrument writes these and nothing reads them.\n"
    );
    out.push_str(if lines.is_empty() { "  none\n" } else { &lines });

    let mut dead = 0;
    let mut lines = String::new();
    for &(lo, hi) in &runs {
        let fact = &body.facts[lo];
        if fact.claimed.is_some() && fact.answers.is_empty() {
            dead += hi - lo + 1;
            let path = fact.claimed.as_deref().unwrap_or_default();
            let mark = if fact.registered {
                ""
            } else {
                " (unregistered)"
            };
            let _ = writeln!(
                lines,
                "  {:<14} {:<34} {path}{mark}",
                format!("{lo}..={hi}"),
                at(lo, hi),
            );
        }
    }
    let _ = write!(
        out,
        "\n## claimed, unanswered — {dead} bits\n\
         ## Declared, but no flip moves a value: dead, or aimed at the wrong bits.\n"
    );
    out.push_str(if lines.is_empty() { "  none\n" } else { &lines });
    out
}

/// One test over every body, so a first run reports every mismatch at once rather
/// than one per invocation.
#[test]
fn coverage() {
    let bodies = measure();
    assert!(!bodies.is_empty(), "no specimen decodes to a registry body");

    let mut failures = Vec::new();
    for (key, body) in &bodies {
        let blind = body.facts.iter().filter(|f| f.blind()).count();
        println!(
            "{key}: {} bits over {} corpus specimens, {} flipped, {blind} vary but read \
             by nothing",
            body.bits, body.weighed, body.flipped,
        );
        if let Err(report) =
            snapshot::check(&format!("coverage/{key}.snapshot"), &render(key, body))
        {
            failures.push(report);
        }
    }

    // A snapshot whose body no specimen reaches any more is a decode that stopped
    // happening, not a leftover file.
    if let Ok(entries) = fs::read_dir(snapshot::path("coverage")) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(key) = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".snapshot"))
            else {
                continue;
            };
            if bodies.contains_key(key) {
                continue;
            }
            if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
                fs::remove_file(&path).unwrap();
                println!("removed {}", path.display());
            } else {
                failures.push(format!(
                    "{}, whose body no specimen decodes any more",
                    path.display()
                ));
            }
        }
    }

    assert!(failures.is_empty(), "\n\n{}", failures.join("\n\n"));
}
