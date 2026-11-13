//! A take's hits as the regions of a library: every detector in turn.

use crate::formats::npno::encode::{stroke_name, LayerTag, FADE_SUFFIX};
use crate::formats::npno::Bank;
use crate::take::clean::{self, Noise};
use crate::take::cut::{self, CutOptions, Cuts, Take};
use crate::take::detect::{self, Envelope, Hit};
use crate::take::hum::{self, Hum, NoHum};
use crate::take::layers::{self, Choice, Order};
use crate::take::sustain::{self, Sustain, SustainOptions};
use crate::wav::{Audio, Loop, Region};

/// What [`plan`] is told about a take.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanOptions {
    /// The notes played, as MIDI notes, in playing order. Empty, they are found from the
    /// hits' pitches: see [`layers::notes_of`].
    pub notes: Vec<u8>,
    /// The layers each note gets.
    pub layers: usize,
    pub order: Order,
    /// Hits to leave out, by their number in the take.
    pub skip: Vec<usize>,
    /// Hits closer than this in level are one layer, in dB.
    pub min_layer_gap_db: f64,
    /// A hit whose level stands less than this over the hum-free floor is left out, in
    /// dB: leveled with the others, its hiss comes up with it.
    pub min_snr_db: f64,
    pub cut: CutOptions,
    pub sustain: SustainOptions,
    /// Set for a take of one layer: one of a set of takes, each holding every note at
    /// one velocity. Each note's last hit is then that layer, and [`PlanOptions::layers`]
    /// and [`PlanOptions::order`] are not read.
    pub set: Option<SetLayer>,
}

/// A take's place in a set of takes, one per layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetLayer {
    /// Its layer, counting from the loudest.
    pub layer: usize,
    /// The takes in the set.
    pub of: usize,
}

impl PlanOptions {
    pub fn new(notes: Vec<u8>, layers: usize) -> PlanOptions {
        PlanOptions {
            notes,
            layers,
            order: Order::Loudness,
            skip: Vec::new(),
            min_layer_gap_db: 2.0,
            min_snr_db: 54.0,
            cut: CutOptions::default(),
            sustain: SustainOptions::default(),
            set: None,
        }
    }
}

/// What became of a hit.
#[derive(Clone, Debug, PartialEq)]
pub enum Fate {
    /// A layer of its note, with the layer value it is named by, and how its attack
    /// sustains, in frames from the attack's start.
    Kept {
        layer: usize,
        value: u8,
        cuts: Cuts,
        sustain: Sustain,
    },
    Skipped,
    /// Its level stood this far over the floor, too little.
    TooQuiet {
        over_db: f64,
    },
    Chosen(Choice),
}

/// One hit and what became of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Planned {
    pub hit: Hit,
    /// The note it plays.
    pub note: Option<u8>,
    pub warning: Option<String>,
    pub fate: Fate,
}

/// The detectors' findings on a take.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub rate: u32,
    pub hits: Vec<Planned>,
    pub hum: Result<Hum, NoHum>,
    /// The take's floor with its hum taken out, in dB under full scale.
    pub floor_db: f64,
    /// The notes the hits were assigned to, in playing order: the list given, or the one
    /// found from their pitches.
    pub notes: Vec<u8>,
    /// Notes of the list no hit was found for.
    pub missing: Vec<u8>,
}

/// The mean of `audio`'s channels, as the detectors read it.
pub fn mono(audio: &Audio) -> Vec<f64> {
    let channels = usize::from(audio.channels.max(1));
    audio
        .samples
        .chunks_exact(channels)
        .map(|frame| frame.iter().map(|&v| f64::from(v)).sum::<f64>() / channels as f64)
        .collect()
}

/// Finds the hits of `mono`, a take at `rate`, assigns them to `options.notes`, or to the
/// notes their pitches name where none are given, chooses each note's layers and cuts
/// their strokes.
///
/// Hits, pitch and key-ups are found on the take as recorded, and levels against the
/// floor on the take with its hum taken out, whose floor is lower. Each attack's loop or
/// fade is found on the attack as the build cleans it.
pub fn plan(mono: &[f64], rate: u32, options: &PlanOptions) -> Plan {
    let x = detect::highpassed(mono, rate);
    let env = Envelope::of(&x, rate);
    let hits = detect::hits(&x, rate, &env);
    let hum = hum::find(&x, rate, &env);
    let cleaned = match &hum {
        Ok(found) => found.removed(&x, rate),
        Err(_) => x.clone(),
    };
    let quiet = Envelope::of(&cleaned, rate);
    let floor = quiet.floor();
    let noise = Noise::measured(rate, hum.as_ref().ok().cloned(), &cleaned, &quiet);

    let skipped = |hit: &Hit| options.skip.contains(&hit.number);
    let notes = match options.notes.is_empty() {
        true => layers::notes_of(&hits, skipped),
        false => options.notes.clone(),
    };
    let assigned = layers::assign(&hits, &notes, skipped);
    let mut planned: Vec<Planned> = hits
        .iter()
        .zip(assigned)
        .map(|(hit, assigned)| {
            let (note, warning) = match assigned {
                Some((j, warning)) => (Some(notes[j]), warning),
                None => (None, None),
            };
            let over_db = hit.level_db - floor;
            let fate = if note.is_none() {
                Fate::Skipped
            } else if over_db < options.min_snr_db {
                Fate::TooQuiet { over_db }
            } else {
                Fate::Chosen(Choice::Extra)
            };
            Planned {
                hit: hit.clone(),
                note,
                warning,
                fate,
            }
        })
        .collect();

    let take = Take {
        rate,
        x: &x,
        env: &env,
        quiet: &quiet,
    };
    let mut missing = Vec::new();
    let mut seen = Vec::new();
    for &note in &notes {
        if seen.contains(&note) {
            continue;
        }
        seen.push(note);
        let members: Vec<usize> = (0..planned.len())
            .filter(|&i| {
                planned[i].note == Some(note) && planned[i].fate == Fate::Chosen(Choice::Extra)
            })
            .collect();
        if members.is_empty() {
            missing.push(note);
            continue;
        }
        let group: Vec<&Hit> = members.iter().map(|&i| &planned[i].hit).collect();
        let (choices, values) = match options.set {
            Some(set) => {
                let last = group[group.len() - 1].number;
                let choices = (0..group.len())
                    .map(|i| match i + 1 == group.len() {
                        true => Choice::Layer(set.layer),
                        false => Choice::Replaced { by: last },
                    })
                    .collect();
                (choices, layers::v6_values(set.of))
            }
            None => {
                let choices = layers::pick(
                    &group,
                    options.layers,
                    options.order,
                    options.min_layer_gap_db,
                );
                let count = choices
                    .iter()
                    .filter(|c| matches!(c, Choice::Layer(_)))
                    .count();
                (choices, layers::v6_values(count))
            }
        };
        for (&i, choice) in members.iter().zip(choices) {
            planned[i].fate = match choice {
                Choice::Layer(layer) => {
                    let cuts = cut::cut(&take, &planned[i].hit, &options.cut);
                    let attack = noise.stroke_mono(
                        mono,
                        cuts.attack.start,
                        cuts.attack.len(),
                        clean::f0(note),
                    );
                    let sustain = sustain::find(&attack, rate, note, floor, &options.sustain);
                    Fate::Kept {
                        layer,
                        value: values[layer],
                        cuts,
                        sustain,
                    }
                }
                other => Fate::Chosen(other),
            };
        }
    }
    Plan {
        rate,
        hits: planned,
        hum,
        floor_db: floor,
        notes,
        missing,
    }
}

impl Plan {
    /// The regions of every kept hit's strokes, in the take's order: an attack labeled
    /// `060-b0-v00`, and a release `060-b2-v00` where it has one. A dying attack ends
    /// where its fade reaches silence, and its fade is a region of its own within it,
    /// labeled `060-b0-v00-fade`, ending with it.
    pub fn regions(&self) -> Vec<Region> {
        let mut regions = Vec::new();
        for planned in &self.hits {
            let (
                Some(note),
                Fate::Kept {
                    value,
                    cuts,
                    sustain,
                    ..
                },
            ) = (planned.note, &planned.fate)
            else {
                continue;
            };
            let mut attack = cuts.attack.clone();
            let mut fade = None;
            if let Sustain::Dies { fade: from, end } = *sustain {
                attack.end = attack.start + end.min(attack.len());
                fade = Some(attack.start + from..attack.end);
            }
            let strokes = [
                (Bank::Attack, Some(attack), ""),
                (Bank::Attack, fade, FADE_SUFFIX),
                (Bank::Release, cuts.release.clone(), ""),
            ];
            for (bank, span, suffix) in strokes {
                let Some(span) = span.filter(|span| !span.is_empty()) else {
                    continue;
                };
                regions.push(Region {
                    start: span.start as u32,
                    frames: span.len() as u32,
                    label: stroke_name(note, bank, LayerTag::Value(*value)) + suffix,
                });
            }
        }
        regions
    }

    /// The loop of every kept attack that loops, in the take's order.
    pub fn loops(&self) -> Vec<Loop> {
        self.hits
            .iter()
            .filter_map(|planned| match &planned.fate {
                Fate::Kept { cuts, sustain, .. } => sustain.sample_loop(cuts.attack.start),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::take::detect::tests::take;

    #[test]
    fn a_take_becomes_a_region_per_stroke_named_by_its_note_and_layer() {
        let rate = 35_002;
        let notes = [
            (1.0, 48.0, -12.0, 3.0),
            (4.0, 48.0, -24.0, 6.0),
            (7.0, 48.0, -24.5, 9.0),
            (10.0, 55.0, -14.0, 12.0),
            (13.0, 55.0, -75.0, 15.0),
        ];
        let x = take(rate, 16.5, &notes, -110.0);
        let plan = plan(&x, rate, &PlanOptions::new(vec![48, 55], 2));
        let fates: Vec<&Fate> = plan.hits.iter().map(|p| &p.fate).collect();
        assert_eq!(fates.len(), 5, "{fates:?}");
        assert!(
            matches!(
                fates[1],
                Fate::Kept {
                    layer: 1,
                    value: 40,
                    ..
                }
            ),
            "{fates:?}"
        );
        assert_eq!(fates[2], &Fate::Chosen(Choice::Duplicate { of: 2 }));
        assert!(matches!(fates[4], Fate::TooQuiet { .. }), "{fates:?}");
        let labels: Vec<String> = plan.regions().into_iter().map(|r| r.label).collect();
        assert_eq!(
            labels,
            [
                "048-b0-v00",
                "048-b2-v00",
                "048-b0-v40",
                "048-b2-v40",
                "055-b0-v00",
                "055-b2-v00"
            ]
        );
        assert!(plan.missing.is_empty());
    }

    #[test]
    fn a_take_with_no_note_list_is_planned_as_with_the_notes_its_pitches_name() {
        let rate = 35_002;
        let notes = [
            (1.0, 48.0, -12.0, 3.0),
            (4.0, 48.0, -24.0, 6.0),
            (7.0, 48.0, -24.5, 9.0),
            (10.0, 55.0, -14.0, 12.0),
            (13.0, 55.0, -75.0, 15.0),
        ];
        let x = take(rate, 16.5, &notes, -110.0);
        let given = plan(&x, rate, &PlanOptions::new(vec![48, 55], 2));
        let found = plan(&x, rate, &PlanOptions::new(Vec::new(), 2));
        assert_eq!(found.notes, [48, 55]);
        assert_eq!(given.notes, [48, 55]);
        let fates = |plan: &Plan| plan.hits.iter().map(|p| p.fate.clone()).collect::<Vec<_>>();
        assert_eq!(fates(&found), fates(&given));
        assert_eq!(found.regions(), given.regions());
        assert_eq!(found.loops(), given.loops());
    }

    /// The second of a set of three takes names every note's last hit with the second
    /// of three layer values, however loud it was played.
    #[test]
    fn a_take_of_one_layer_keeps_each_notes_last_hit_as_that_layer() {
        let rate = 35_002;
        let notes = [
            (1.0, 48.0, -30.0, 3.0),
            (4.0, 48.0, -12.0, 6.0),
            (7.0, 55.0, -14.0, 9.0),
        ];
        let x = take(rate, 10.5, &notes, -110.0);
        let mut options = PlanOptions::new(vec![48, 55], 4);
        options.set = Some(SetLayer { layer: 1, of: 3 });
        let plan = plan(&x, rate, &options);
        let fates: Vec<&Fate> = plan.hits.iter().map(|p| &p.fate).collect();
        assert_eq!(fates[0], &Fate::Chosen(Choice::Replaced { by: 2 }));
        let labels: Vec<String> = plan.regions().into_iter().map(|r| r.label).collect();
        assert_eq!(
            labels,
            ["048-b0-v20", "048-b2-v20", "055-b0-v20", "055-b2-v20"]
        );
    }

    #[test]
    fn a_take_shorter_than_one_envelope_frame_has_no_hits_and_no_noise() {
        let rate = 35_002;
        for len in [0, 1, 349] {
            let x = vec![0.1; len];
            let plan = plan(&x, rate, &PlanOptions::new(vec![48], 4));
            assert!(plan.hits.is_empty(), "{len} samples");
            assert_eq!(plan.missing, [48]);
            assert_eq!(plan.hum, Err(NoHum::NoQuiet));
            let noise = Noise::of(&x, rate);
            assert_eq!((noise.hum, noise.hiss), (None, None));
        }
    }
}
