//! A set of takes planned as one library: one take per layer, one per range of keys, or
//! one per note struck.

use crate::take::layers::{self, Placed};
use crate::take::plan::{self, Fate, Plan, PlanOptions, SetLayer};

/// How the takes of a set divide the instrument between them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// Each take plays every note once, at one velocity, the loudest take first.
    PerLayer,
    /// Each take plays its own notes, at every velocity, as a single take does.
    PerRange,
    /// Each take is one note struck: its note comes from its pitch, and its layer from
    /// its level among its note's takes.
    PerNote,
}

/// What became of one take of a set.
#[derive(Clone, Debug, PartialEq)]
pub struct Planned {
    /// The take's plan: its hits named and cut as the set places them.
    pub plan: Plan,
    /// Where a take of one note was placed, or none where its hit was too quiet or it had
    /// none, its plan then the take on its own. None for the other shapes.
    pub placed: Option<Placed>,
}

/// Why a set was not planned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetError {
    /// Two takes of ranges both kept a hit of `note`, by their indices.
    Clash { note: u8, takes: [usize; 2] },
}

impl std::fmt::Display for SetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SetError::Clash { note, takes } => write!(
                f,
                "takes {} and {} both play {}; a range's notes are its own",
                takes[0] + 1,
                takes[1] + 1,
                crate::note::name(*note)
            ),
        }
    }
}

/// Plans `count` takes of `shape` as one library, each read by `read` as its mono audio
/// and rate, and `skips[i]` the hits of take `i` to leave out. A take that does not read
/// is the reason it did not. Each take is read once, or twice for one placed as a note's
/// layer, and never held with the others.
pub fn plan<R>(
    count: usize,
    shape: Shape,
    options: &PlanOptions,
    skips: &[Vec<usize>],
    read: R,
) -> Result<Vec<Result<Planned, String>>, SetError>
where
    R: FnMut(usize) -> Result<(Vec<f64>, u32), String>,
{
    plan_named(count, shape, options, skips, &[], read)
}

/// As [`plan()`], with each take's name in `names`: of one note per take, a take near the
/// level of a louder one of its note is kept as a layer where the two are named as one
/// series ([`one_series`]), not left out as a duplicate. No names, no series.
pub fn plan_named<R>(
    count: usize,
    shape: Shape,
    options: &PlanOptions,
    skips: &[Vec<usize>],
    names: &[&str],
    mut read: R,
) -> Result<Vec<Result<Planned, String>>, SetError>
where
    R: FnMut(usize) -> Result<(Vec<f64>, u32), String>,
{
    let skip = |i: usize| skips.get(i).cloned().unwrap_or_default();
    match shape {
        Shape::PerLayer => Ok((0..count)
            .map(|i| {
                let (mono, rate) = read(i)?;
                let mut options = options.clone();
                options.skip = skip(i);
                options.set = Some(SetLayer {
                    layer: i,
                    of: count,
                });
                Ok(Planned {
                    plan: plan::plan(&mono, rate, &options),
                    placed: None,
                })
            })
            .collect()),
        Shape::PerRange => {
            let planned: Vec<Result<Planned, String>> = (0..count)
                .map(|i| {
                    let (mono, rate) = read(i)?;
                    let mut options = options.clone();
                    options.skip = skip(i);
                    Ok(Planned {
                        plan: plan::plan(&mono, rate, &options),
                        placed: None,
                    })
                })
                .collect();
            let mut owner = std::collections::BTreeMap::new();
            for (i, take) in planned.iter().enumerate() {
                let Ok(take) = take else { continue };
                for note in kept_notes(&take.plan) {
                    if let Some(&first) = owner.get(&note) {
                        if first != i {
                            return Err(SetError::Clash {
                                note,
                                takes: [first, i],
                            });
                        }
                    }
                    owner.insert(note, i);
                }
            }
            Ok(planned)
        }
        Shape::PerNote => Ok(per_note(count, options, names, read)),
    }
}

/// Whether two takes are named as one series, as `Grand-C5v00.wav` and `Grand-C5v01.wav`
/// are: the same name once each drops its extension and the last run of digits in it.
pub fn one_series(a: &str, b: &str) -> bool {
    fn stem(name: &str) -> String {
        let leaf = name.rsplit(['/', '\\']).next().unwrap_or(name);
        let stem = leaf.rsplit_once('.').map_or(leaf, |(stem, _)| stem);
        let Some(last) = stem.rfind(|c: char| c.is_ascii_digit()) else {
            return stem.to_string();
        };
        let first = stem[..=last]
            .rfind(|c: char| !c.is_ascii_digit())
            .map_or(0, |at| at + 1);
        format!("{}{}", &stem[..first], &stem[last + 1..])
    }
    a != b && stem(a) == stem(b)
}

/// Each take of one note, planned on its own for its loudest hit, then placed among the
/// others and planned again as its note's layer.
fn per_note<R>(
    count: usize,
    options: &PlanOptions,
    names: &[&str],
    mut read: R,
) -> Vec<Result<Planned, String>>
where
    R: FnMut(usize) -> Result<(Vec<f64>, u32), String>,
{
    let mut alone = options.clone();
    alone.notes = Vec::new();
    alone.one_note = true;
    let found: Vec<Result<Plan, String>> = (0..count)
        .map(|i| read(i).map(|(mono, rate)| plan::plan(&mono, rate, &alone)))
        .collect();
    let struck: Vec<_> = (found.iter())
        .map(|plan| match plan.as_ref().ok().and_then(loudest) {
            Some(planned) if !matches!(planned.fate, Fate::TooQuiet { .. }) => {
                Some(planned.hit.clone())
            }
            _ => None,
        })
        .collect();
    let series = |a: usize, b: usize| match (names.get(a), names.get(b)) {
        (Some(a), Some(b)) => one_series(a, b),
        _ => false,
    };
    let placed = layers::place_series(&struck, options.layers, options.min_layer_gap_db, series);
    (found.into_iter().zip(placed).enumerate())
        .map(|(i, (found, placed))| {
            let found = found?;
            let plan = match placed {
                Some(Placed::Layer { note, layer, of }) => {
                    let (mono, rate) = read(i)?;
                    let mut options = alone.clone();
                    options.notes = vec![note];
                    options.set = Some(SetLayer { layer, of });
                    plan::plan(&mono, rate, &options)
                }
                _ => found,
            };
            Ok(Planned { plan, placed })
        })
        .collect()
}

/// The loudest of `plan`'s hits, which is a one-note take's note, a hit with no pitch
/// included.
pub fn loudest(plan: &Plan) -> Option<&plan::Planned> {
    (plan.hits.iter()).reduce(|a, b| match b.hit.level_db > a.hit.level_db {
        true => b,
        false => a,
    })
}

/// The notes `plan` keeps a hit of.
fn kept_notes(plan: &Plan) -> std::collections::BTreeSet<u8> {
    (plan.hits.iter())
        .filter(|p| matches!(p.fate, Fate::Kept { .. }))
        .filter_map(|p| p.note)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_are_one_series_where_only_their_last_number_differs() {
        assert!(one_series("Grand-C5v00.wav", "Grand-C5v01.wav"));
        assert!(one_series("dir/Organ_158_rr1.wav", "Organ_158_rr2.wav"));
        assert!(!one_series(
            "NT5_Man3Quiet_158_rr1.wav",
            "Rode_Man3Open_49.wav"
        ));
        assert!(!one_series("Grand-C5.wav", "Grand-D5.wav"));
        assert!(
            !one_series("Grand-C5v00.wav", "Grand-C5v00.wav"),
            "not itself"
        );
    }

    const RATE: u32 = 35_002;

    /// `notes` struck in a row, each `gain` loud, held 1.5 s and damped, after a second of
    /// quiet hiss.
    fn take(notes: &[(u8, f64)]) -> Vec<f64> {
        let rate = f64::from(RATE);
        let mut x = vec![0.0; RATE as usize];
        for &(midi, gain) in notes {
            let f0 = 440.0 * 2f64.powf((f64::from(midi) - 69.0) / 12.0);
            for n in 0..(2.5 * rate) as usize {
                let t = n as f64 / rate;
                let held = match t < 1.5 {
                    true => (-t * 1.5).exp(),
                    false => (-1.5 * 1.5f64).exp() * (-(t - 1.5) * 30.0).exp(),
                };
                let tone: f64 = (1..=4)
                    .map(|k| (2.0 * std::f64::consts::PI * f0 * k as f64 * t).sin() / k as f64)
                    .sum();
                x.push(gain * held * tone * 0.5);
            }
        }
        let mut seed = 1u32;
        for v in &mut x {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            *v += (f64::from(seed >> 8) / f64::from(1u32 << 24) - 0.5) * 1e-5;
        }
        x
    }

    fn kept(plan: &Plan) -> Vec<(u8, u8)> {
        (plan.hits.iter())
            .filter_map(|p| match p.fate {
                Fate::Kept { value, .. } => Some((p.note?, value)),
                _ => None,
            })
            .collect()
    }

    fn planned(takes: &[Vec<f64>], shape: Shape) -> Result<Vec<Plan>, SetError> {
        let options = PlanOptions::new(Vec::new(), None);
        let set = plan(takes.len(), shape, &options, &[], |i| {
            Ok((takes[i].clone(), RATE))
        })?;
        Ok(set.into_iter().map(|p| p.expect("read").plan).collect())
    }

    #[test]
    fn a_take_of_each_range_keeps_its_own_notes_and_layers() {
        let low = take(&[(48, 0.8), (48, 0.3), (50, 0.8), (50, 0.3)]);
        let high = take(&[(72, 0.8), (72, 0.4), (72, 0.15)]);
        let plans = planned(&[low, high], Shape::PerRange).expect("no clash");
        let notes = |plan: &Plan| -> std::collections::BTreeMap<u8, usize> {
            kept(plan)
                .into_iter()
                .fold(Default::default(), |mut m, (n, _)| {
                    *m.entry(n).or_default() += 1;
                    m
                })
        };
        assert_eq!(notes(&plans[0]), [(48, 2), (50, 2)].into());
        assert_eq!(notes(&plans[1]), [(72, 3)].into());
    }

    #[test]
    fn two_ranges_playing_one_note_are_refused() {
        let low = take(&[(48, 0.8), (60, 0.8)]);
        let high = take(&[(60, 0.5), (72, 0.8)]);
        assert_eq!(
            planned(&[low, high], Shape::PerRange),
            Err(SetError::Clash {
                note: 60,
                takes: [0, 1]
            })
        );
    }

    #[test]
    fn a_take_of_each_note_is_placed_as_its_note_s_layer() {
        let takes: Vec<Vec<f64>> = [(60, 0.8), (60, 0.2), (64, 0.5)]
            .iter()
            .map(|&note| take(&[note]))
            .collect();
        let options = PlanOptions::new(Vec::new(), None);
        let set = plan(3, Shape::PerNote, &options, &[], |i| {
            Ok((takes[i].clone(), RATE))
        })
        .expect("no clash");
        let placed: Vec<_> = set
            .iter()
            .map(|p| p.as_ref().expect("read").placed.clone())
            .collect();
        assert_eq!(
            placed,
            [
                Some(Placed::Layer {
                    note: 60,
                    layer: 0,
                    of: 2
                }),
                Some(Placed::Layer {
                    note: 60,
                    layer: 1,
                    of: 2
                }),
                Some(Placed::Layer {
                    note: 64,
                    layer: 0,
                    of: 1
                }),
            ]
        );
        let values: Vec<_> = (set.iter())
            .map(|p| kept(&p.as_ref().expect("read").plan))
            .collect();
        assert_ne!(values[0], values[1], "a note's two takes are two layers");
    }

    #[test]
    fn a_take_that_does_not_read_is_its_reason_and_the_rest_go_on() {
        let one = take(&[(60, 0.8)]);
        let options = PlanOptions::new(Vec::new(), None);
        for shape in [Shape::PerLayer, Shape::PerRange, Shape::PerNote] {
            let set = plan(2, shape, &options, &[], |i| match i {
                0 => Err("not a WAV".into()),
                _ => Ok((one.clone(), RATE)),
            })
            .expect("no clash");
            assert_eq!(set[0], Err("not a WAV".into()), "{shape:?}");
            assert!(set[1].is_ok(), "{shape:?}");
        }
    }
}
