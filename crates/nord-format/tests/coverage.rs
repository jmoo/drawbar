#![cfg(feature = "corpus")]
//! Check that instrument-written bits have answers, and keep known blind debt small.

#[path = "support/registry.rs"]
mod registry;
#[path = "support/scan.rs"]
mod scan;
#[path = "support/sidecar.rs"]
mod sidecar;

use nord_format::cbin;
use nord_format::layout::LayoutField;
use nord_format::Entity;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

#[derive(Default)]
struct Bit {
    claimed: Option<String>,
    registered: bool,
    varies: bool,
    answers: BTreeSet<String>,
    rejected: bool,
}
impl Bit {
    fn blind(&self) -> bool {
        self.varies && self.answers.is_empty() && !self.rejected
    }

    fn refusal_only(&self) -> bool {
        self.rejected && self.answers.is_empty()
    }

    fn owned_by(&self, owner: &str) -> bool {
        self.answers.len() == 1 && self.answers.contains(owner)
    }
}

struct Body {
    facts: Vec<Bit>,
    ones: Vec<u8>,
    zeros: Vec<u8>,
    weighed: usize,
    flipped: usize,
}

struct Pick {
    specimen: &'static scan::Specimen,
    sampled: bool,
    instrument: bool,
}

fn population() -> Vec<Pick> {
    let mut out = scan::fixtures()
        .iter()
        .map(|specimen| Pick {
            specimen,
            sampled: true,
            instrument: false,
        })
        .collect::<Vec<_>>();
    let mut shapes = BTreeSet::new();
    for specimen in scan::corpus() {
        let sampled = sidecar::sidecar_of(&specimen.path).exists()
            || scan::shape(&specimen.path).is_none_or(|shape| shapes.insert(shape));
        out.push(Pick {
            specimen,
            sampled,
            instrument: true,
        });
    }
    out
}

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
                    let fact = &mut out[bit as usize];
                    assert!(
                        fact.claimed.is_none(),
                        "bit {bit} is claimed by both {} and {path}",
                        fact.claimed.as_deref().unwrap_or_default()
                    );
                    fact.claimed = Some(path.clone());
                }
            }
        }
    }
}

/// Rejection is tracked separately: it is not an answer from the claimed field.
fn answers(bytes: &[u8], entity: &Entity, facts: &mut [Bit]) {
    let baseline = registry::field_values(entity)
        .expect("a body with a registry")
        .into_iter()
        .map(|field| field.value)
        .collect::<Vec<_>>();
    let mut file = cbin::read_raw(&mut Cursor::new(bytes)).expect("a parsed container");
    let mut out = Cursor::new(Vec::with_capacity(bytes.len()));
    for (bit, fact) in facts.iter_mut().enumerate() {
        file.body.0[bit / 8] ^= 1 << (7 - bit % 8);
        out.get_mut().clear();
        out.set_position(0);
        file.write_to(&mut out).expect("a raw body re-encodes");
        match nord_format::from_stream(&mut Cursor::new(out.get_ref())) {
            Err(_) => fact.rejected = true,
            Ok(flipped) => {
                let values = registry::field_values(&flipped)
                    .expect("a body-bit mutation retains its registry");
                for (was, now) in baseline.iter().zip(values) {
                    if *was != now.value {
                        fact.answers.insert(now.name);
                    }
                }
            }
        }
        file.body.0[bit / 8] ^= 1 << (7 - bit % 8);
    }
}

fn measure() -> BTreeMap<String, Body> {
    let mut bodies = BTreeMap::new();
    for pick in population() {
        let (bytes, entity) = (&pick.specimen.bytes, &pick.specimen.entity);
        let Some(key) = registry::body_type(entity) else {
            continue;
        };
        let body = cbin::read_raw(&mut Cursor::new(bytes))
            .expect("a parsed CBIN")
            .body
            .0;
        if !body.is_empty() && body.iter().all(|&byte| byte == 0xff) {
            continue;
        }
        let entry = bodies.entry(key).or_insert_with(|| {
            let mut facts = (0..body.len() * 8)
                .map(|_| Bit::default())
                .collect::<Vec<_>>();
            claims(
                registry::layout(entity).expect("a registry body declares a layout"),
                0,
                "",
                &mut facts,
            );
            let reported = registry::field_values(entity)
                .expect("a body with a registry")
                .into_iter()
                .map(|field| field.name)
                .collect::<BTreeSet<_>>();
            for fact in &mut facts {
                fact.registered = fact
                    .claimed
                    .as_ref()
                    .is_some_and(|path| reported.contains(path));
            }
            Body {
                facts,
                ones: vec![0; body.len()],
                zeros: vec![0xff; body.len()],
                weighed: 0,
                flipped: 0,
            }
        });
        assert_eq!(
            entry.facts.len(),
            body.len() * 8,
            "one body type, two lengths"
        );
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
    for body in bodies.values_mut().filter(|body| body.weighed > 0) {
        for (bit, fact) in body.facts.iter_mut().enumerate() {
            fact.varies = (body.ones[bit / 8] ^ body.zeros[bit / 8]) & (1 << (7 - bit % 8)) != 0;
        }
    }
    bodies
}

// Ranges are body-relative and come from the last full-corpus measurement.
const KNOWN_BLIND: &[(&str, &[(usize, usize)])] = &[
    (
        "ne5-Program",
        &[(422, 422), (426, 426), (764, 764), (925, 938)],
    ),
    ("ne5-Settings", &[]),
    ("ne5-Song", &[]),
    (
        "ns2-Program",
        &[(14, 15), (170, 183), (1295, 1297), (3287, 3289)],
    ),
    (
        "ns3-Program",
        &[
            (82, 95),
            (273, 273),
            (356, 547),
            (549, 579),
            (581, 643),
            (2460, 2651),
            (2653, 2683),
            (2685, 2747),
        ],
    ),
    ("ns3-SynthPreset", &[]),
    (
        "ns4-OrganPreset",
        &[
            (43, 43),
            (109, 114),
            (124, 130),
            (154, 155),
            (162, 168),
            (900, 902),
            (1082, 1084),
        ],
    ),
    (
        "ns4-PianoPreset",
        &[
            (40, 40),
            (43, 43),
            (114, 114),
            (556, 558),
            (737, 740),
            (1177, 1177),
            (1179, 1179),
        ],
    ),
    (
        "ns4-Program",
        &[
            (67, 78),
            (86, 123),
            (127, 201),
            (203, 315),
            (324, 324),
            (333, 333),
            (338, 338),
            (341, 341),
            (344, 346),
            (400, 400),
            (403, 403),
            (469, 471),
            (473, 474),
            (491, 492),
            (494, 494),
            (496, 496),
            (498, 499),
            (501, 502),
            (504, 504),
            (511, 511),
            (1259, 1262),
            (1281, 1287),
            (1441, 1444),
            (1480, 1480),
            (1483, 1483),
            (1553, 1554),
            (1995, 1998),
            (2017, 2023),
            (2048, 2053),
            (2177, 2180),
            (2435, 2438),
            (2457, 2463),
            (2488, 2493),
            (2617, 2620),
            (2656, 2657),
            (2661, 2662),
            (2765, 2765),
            (2796, 2796),
            (2827, 2827),
            (2852, 2852),
            (2854, 2854),
            (2859, 2861),
            (2971, 2972),
            (3028, 3028),
            (3379, 3380),
            (3436, 3436),
            (3787, 3788),
            (3844, 3844),
            (4189, 4190),
            (4573, 4574),
            (4957, 4958),
            (5499, 5502),
            (5521, 5527),
            (5681, 5684),
            (5939, 5942),
            (5961, 5967),
            (6121, 6124),
            (6379, 6382),
            (6401, 6407),
            (6561, 6564),
        ],
    ),
    (
        "ns4-SynthPreset",
        &[
            (40, 41),
            (45, 46),
            (149, 149),
            (180, 180),
            (211, 211),
            (239, 239),
            (244, 245),
            (355, 356),
            (412, 412),
            (763, 764),
            (820, 820),
            (1171, 1172),
            (1228, 1228),
            (1573, 1574),
            (1957, 1958),
            (2341, 2342),
            (2883, 2886),
            (2905, 2911),
            (3065, 3068),
            (3323, 3325),
            (3345, 3351),
            (3505, 3507),
            (3763, 3765),
            (3785, 3791),
            (3945, 3947),
        ],
    ),
];

// Sparse values can make a one-bit trial invalid; these are explicit debt, not answers.
const KNOWN_REJECTIONS: &[(&str, &[usize])] = &[("ne5-Program", &[16, 19, 22, 26])];

fn known_blind(key: &str) -> Option<&'static [(usize, usize)]> {
    KNOWN_BLIND
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, ranges)| *ranges)
}
fn known_rejections(key: &str) -> &'static [usize] {
    KNOWN_REJECTIONS
        .iter()
        .find(|(name, _)| *name == key)
        .map_or(&[], |(_, bits)| *bits)
}
fn runs(bits: impl Iterator<Item = usize>) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for bit in bits {
        match out.last_mut() {
            Some((_, hi)) if *hi + 1 == bit => *hi = bit,
            _ => out.push((bit, bit)),
        }
    }
    out
}
fn show(ranges: &[(usize, usize)]) -> String {
    ranges
        .iter()
        .map(|&(lo, hi)| {
            if lo == hi {
                lo.to_string()
            } else {
                format!("{lo}..={hi}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[test]
fn blind_and_claimed_bits_match_the_reviewed_contracts() {
    let bodies = measure();
    assert!(!bodies.is_empty(), "no specimen decodes to a registry body");
    let mut failures = Vec::new();
    for (key, body) in &bodies {
        if body.weighed == 0 {
            failures.push(format!(
                "{key}: no instrument specimen supplied variation evidence"
            ));
        }
        if body.flipped == 0 {
            failures.push(format!(
                "{key}: no sampled specimen supplied mutation answers"
            ));
        }
        let blind = runs(
            body.facts
                .iter()
                .enumerate()
                .filter(|(_, fact)| fact.blind())
                .map(|(bit, _)| bit),
        );
        match known_blind(key) {
            Some(expected) if blind != expected => failures.push(format!(
                "{key}: blind bits changed: expected [{}], got [{}]",
                show(expected),
                show(&blind)
            )),
            None => failures.push(format!("{key}: no reviewed blind-bit contract")),
            _ => {}
        }
        let refusal_only = body
            .facts
            .iter()
            .enumerate()
            .filter(|(_, fact)| fact.refusal_only())
            .map(|(bit, _)| bit)
            .collect::<Vec<_>>();
        let expected_refusals = known_rejections(key);
        if refusal_only != expected_refusals {
            failures.push(format!(
                "{key}: refusal-only bits changed: expected {expected_refusals:?}, got {refusal_only:?}"
            ));
        }
        let missing = body
            .facts
            .iter()
            .enumerate()
            .filter(|(_, fact)| fact.claimed.is_some() && !fact.registered)
            .map(|(bit, fact)| format!("{bit} ({})", fact.claimed.as_deref().unwrap_or_default()))
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            failures.push(format!(
                "{key}: claimed bits without readers: {}",
                missing.join(", ")
            ));
        }
        let unanswered = body
            .facts
            .iter()
            .enumerate()
            .filter(|(bit, fact)| {
                let Some(owner) = fact.claimed.as_ref() else {
                    return false;
                };
                fact.registered
                    && !(fact.refusal_only() && known_rejections(key).contains(bit))
                    && !fact.owned_by(owner)
            })
            .map(|(bit, fact)| {
                format!(
                    "{bit} (owner={}, rejected={}, answers={:?})",
                    fact.claimed.as_deref().unwrap_or_default(),
                    fact.rejected,
                    fact.answers,
                )
            })
            .collect::<Vec<_>>();
        if !unanswered.is_empty() {
            failures.push(format!(
                "{key}: claimed bits without their owner's answer: {}",
                unanswered.join(", ")
            ));
        }
    }
    for (key, _) in KNOWN_BLIND {
        if !bodies.contains_key(*key) {
            failures.push(format!("{key}: blind-bit contract has no measured body"));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
