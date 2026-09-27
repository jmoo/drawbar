#![cfg(feature = "corpus")]
//! Bit ownership of every registry body the specimens hold. No bit is claimed by two
//! fields, every claimed bit belongs to a field the registry reports, and flipping a
//! claimed bit changes the field that claims it and no other, or makes the file one
//! the reader refuses. The specimens are the fixtures and one of each container shape
//! in the corpus.

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
    answers: BTreeSet<String>,
    rejected: bool,
}

fn population() -> Vec<&'static scan::Specimen> {
    let mut shapes = BTreeSet::new();
    scan::fixtures()
        .iter()
        .chain(
            scan::corpus()
                .iter()
                .filter(|specimen| scan::sampled(&specimen.path, &mut shapes)),
        )
        .collect()
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

/// Flips each claimed body bit in turn and records which fields change. A flip the
/// reader refuses is recorded as a rejection, which is not an answer.
fn answers(bytes: &[u8], entity: &Entity, facts: &mut [Bit]) {
    let baseline = registry::field_values(entity)
        .expect("a body with a registry")
        .into_iter()
        .map(|field| field.value)
        .collect::<Vec<_>>();
    let mut file = cbin::read_raw(&mut Cursor::new(bytes)).expect("a parsed container");
    let mut out = Cursor::new(Vec::with_capacity(bytes.len()));
    for (bit, fact) in facts
        .iter_mut()
        .enumerate()
        .filter(|(_, fact)| fact.claimed.is_some())
    {
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

/// Each registry body type among the specimens, with what its bits answered.
fn measure() -> BTreeMap<String, Vec<Bit>> {
    let mut bodies = BTreeMap::new();
    for specimen in population() {
        let (bytes, entity) = (&specimen.bytes, &specimen.entity);
        let Some(key) = registry::body_type(entity) else {
            continue;
        };
        let body = cbin::read_raw(&mut Cursor::new(bytes))
            .expect("a parsed CBIN")
            .body
            .0;
        if scan::unwritten(&body) {
            continue;
        }
        let facts: &mut Vec<Bit> = bodies.entry(key).or_insert_with(|| {
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
            facts
        });
        assert_eq!(
            facts.len(),
            body.len() * 8,
            "{}: specimens of one body type differ in length",
            specimen.path.display()
        );
        answers(bytes, entity, facts);
    }
    bodies
}

#[test]
fn each_claimed_bit_answers_to_its_own_field_alone() {
    let mut failures = Vec::new();
    for (key, facts) in &measure() {
        let unregistered = facts
            .iter()
            .enumerate()
            .filter(|(_, fact)| fact.claimed.is_some() && !fact.registered)
            .map(|(bit, fact)| format!("{bit} ({})", fact.claimed.as_deref().unwrap_or_default()))
            .collect::<Vec<_>>();
        if !unregistered.is_empty() {
            failures.push(format!(
                "{key}: claimed bits whose field is not in the registry: {}",
                unregistered.join(", ")
            ));
        }
        let strays = facts
            .iter()
            .enumerate()
            .filter(|(_, fact)| {
                let Some(owner) = fact.claimed.as_ref().filter(|_| fact.registered) else {
                    return false;
                };
                let foreign = fact.answers.iter().any(|field| field != owner);
                let silent = fact.answers.is_empty() && !fact.rejected;
                foreign || silent
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
        if !strays.is_empty() {
            failures.push(format!(
                "{key}: claimed bits whose flip does not change their own field alone: {}",
                strays.join(", ")
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
