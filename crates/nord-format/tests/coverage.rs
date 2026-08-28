#![cfg(feature = "corpus")]
//! Verify that every registered bit participates in decoding.
//!
//! Each sampled specimen is mutated one body bit at a time, rechecksummed, and
//! parsed. A declared bit must either change a field value or make the parser
//! reject the mutation. This catches dead and misplaced declarations without
//! treating a rendering of the whole corpus as an oracle.

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

const REFUSED: &str = "refused";

#[derive(Default)]
struct Bit {
    claimed: Option<String>,
    registered: bool,
    answers: BTreeSet<String>,
}

struct Body {
    facts: Vec<Bit>,
    flipped: usize,
}

struct Pick {
    specimen: &'static scan::Specimen,
    sampled: bool,
}

fn population() -> Vec<Pick> {
    let mut out: Vec<Pick> = scan::fixtures()
        .iter()
        .map(|specimen| Pick {
            specimen,
            sampled: true,
        })
        .collect();

    let mut shapes = BTreeSet::new();
    for specimen in scan::corpus() {
        let sampled = sidecar::sidecar_of(&specimen.path).exists()
            || scan::shape(&specimen.path).is_none_or(|shape| shapes.insert(shape));
        out.push(Pick { specimen, sampled });
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
                        fact.claimed.as_deref().unwrap_or_default(),
                    );
                    fact.claimed = Some(path.clone());
                }
            }
        }
    }
}

fn answers(bytes: &[u8], entity: &Entity, facts: &mut [Bit]) {
    let baseline: Vec<String> = registry::field_values(entity)
        .expect("a body with a registry")
        .into_iter()
        .map(|field| field.value)
        .collect();
    let mut file = cbin::read_raw(&mut Cursor::new(bytes)).expect("a parsed container");
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

fn measure() -> BTreeMap<String, Body> {
    let mut bodies = BTreeMap::new();

    for pick in population().into_iter().filter(|pick| pick.sampled) {
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
            let mut facts: Vec<Bit> = (0..body.len() * 8).map(|_| Bit::default()).collect();
            claims(
                registry::layout(entity).expect("a registry body declares a layout"),
                0,
                "",
                &mut facts,
            );
            let reported: BTreeSet<String> = registry::field_values(entity)
                .expect("a body with a registry")
                .into_iter()
                .map(|field| field.name)
                .collect();
            for fact in &mut facts {
                fact.registered = fact
                    .claimed
                    .as_ref()
                    .is_some_and(|path| reported.contains(path));
            }
            Body { facts, flipped: 0 }
        });
        assert_eq!(
            entry.facts.len(),
            body.len() * 8,
            "one body type, two lengths"
        );
        entry.flipped += 1;
        answers(bytes, entity, &mut entry.facts);
    }

    bodies
}

#[test]
fn every_registered_bit_is_read() {
    let bodies = measure();
    assert!(!bodies.is_empty(), "no specimen decodes to a registry body");

    let mut dead = Vec::new();
    for (key, body) in bodies {
        assert!(body.flipped > 0, "{key} had no sampled specimen");
        for (bit, fact) in body.facts.iter().enumerate() {
            if fact.registered && fact.answers.is_empty() {
                dead.push(format!(
                    "{key} bit {bit}: {}",
                    fact.claimed.as_deref().unwrap_or_default(),
                ));
            }
        }
    }

    assert!(
        dead.is_empty(),
        "registered bits that affect no decoded value:\n{}",
        dead.join("\n"),
    );
}
