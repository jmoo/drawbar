//! Which note each hit plays, which hits become a note's velocity layers, and how loud
//! a stroke plays.

use crate::formats::npno::encode;
use crate::take::detect::Hit;

/// The peak every region of a take is brought to before it is rounded to 16 bits, in
/// dB under full scale, which leaves room for the loop crossfades.
pub const TAKE_PEAK_DB: f64 = -13.0;

/// How loud a stroke plays, in decibels over a steady sample of one unit, measured as
/// the encoder measures one: the most power any 10 ms of its first half second
/// averages, on its louder channel. `frames` are `channels` interleaved at `rate`, and
/// silence has no level.
pub fn level(frames: &[f32], channels: usize, rate: u32) -> Option<f64> {
    let channels = channels.max(1);
    encode::loudness(channels, frames.len() / channels, rate, |frame, channel| {
        f64::from(frames[frame * channels + channel])
    })
}

/// How far `pitch` is from `note`'s pitch class, in semitones, at most 6.
fn class_distance(pitch: f64, note: u8) -> f64 {
    ((pitch - f64::from(note) + 6.0).rem_euclid(12.0) - 6.0).abs()
}

/// How far a run's offset from its note may move from the run before's, in semitones,
/// before [`notes_by_hit`] takes it for stretch tuning that rounded to the next note.
/// Chosen, not measured: over the few cents stretch tuning moves from note to note, and
/// over the third of a semitone an instrument's own detuning seldom passes.
pub const STRETCH_JUMP: f64 = 0.7;

/// The notes `hits` play, in playing order, found from their pitches: each run of hits
/// on one note is one entry, as [`notes_by_hit`] names them.
pub fn notes_of(hits: &[Hit], skipped: impl Fn(&Hit) -> bool) -> Vec<u8> {
    let mut notes: Vec<u8> = notes_by_hit(hits, skipped).into_iter().flatten().collect();
    notes.dedup();
    notes
}

/// The note each of `hits` plays, found from their pitches, or none for a hit `skipped`
/// leaves out or with no pitch.
///
/// Each hit is named by its pitch, rounded, and a run of hits on one note is named as
/// one. A run whose offset from its note jumps more than [`STRETCH_JUMP`] semitones from
/// the run before's is named the note on the other side: stretch tuning puts a piano's
/// top notes sharp and its bass flat by more than half a semitone at the ends, and
/// changes smoothly from note to note.
///
/// Then, one run at a time, the octave move that most straightens the runs' line is
/// made, while one straightens it by more than half an octave. The line's bend is the
/// sum of how far each run lies off the middle of the runs either side, each counted to
/// two octaves at most. That catches the octave errors pitch estimates make on low
/// notes, and leaves a take that steps evenly, by octaves too, or starts again at the
/// bottom, as played.
pub fn notes_by_hit(hits: &[Hit], skipped: impl Fn(&Hit) -> bool) -> Vec<Option<u8>> {
    named(hits, skipped, true)
}

/// [`notes_by_hit`], with the octave moves made only where `octaves` is set.
fn named(hits: &[Hit], skipped: impl Fn(&Hit) -> bool, octaves: bool) -> Vec<Option<u8>> {
    // Each run's rounded note and the sum and count of its pitches, and each hit's run.
    let mut runs: Vec<(f64, f64, f64)> = Vec::new();
    let run_of: Vec<Option<usize>> = hits
        .iter()
        .map(|hit| {
            let pitch = hit.pitch.filter(|_| !skipped(hit))?;
            let note = pitch.round().clamp(0.0, 127.0);
            match runs.last_mut() {
                Some((last, sum, count)) if *last == note => {
                    *sum += pitch;
                    *count += 1.0;
                }
                _ => runs.push((note, pitch, 1.0)),
            }
            Some(runs.len() - 1)
        })
        .collect();
    // Each run's note after the stretch check, and runs that now name one note merged.
    let mut line: Vec<f64> = Vec::with_capacity(runs.len());
    let mut merged_of: Vec<usize> = Vec::with_capacity(runs.len());
    let mut offset: Option<f64> = None;
    for (note, sum, count) in runs {
        let pitch = sum / count;
        let mut note = note;
        if let Some(before) = offset {
            let jump = (pitch - note) - before;
            if jump < -STRETCH_JUMP && note > 0.0 {
                note -= 1.0;
            } else if jump > STRETCH_JUMP && note < 127.0 {
                note += 1.0;
            }
        }
        offset = Some(pitch - note);
        if line.last() != Some(&note) {
            line.push(note);
        }
        merged_of.push(line.len() - 1);
    }
    let bend = |line: &[f64]| -> f64 {
        line.windows(3)
            .map(|w| (w[0] - 2.0 * w[1] + w[2]).abs().min(24.0))
            .sum()
    };
    let mut moving = octaves;
    while moving {
        let now = bend(&line);
        let mut best: Option<(f64, usize, f64)> = None;
        for i in 0..line.len() {
            for moved in [line[i] - 12.0, line[i] + 12.0] {
                if !(0.0..=127.0).contains(&moved) {
                    continue;
                }
                let mut tried = line.clone();
                tried[i] = moved;
                let gain = now - bend(&tried);
                if gain > 6.0 && best.is_none_or(|(most, _, _)| gain > most) {
                    best = Some((gain, i, moved));
                }
            }
        }
        match best {
            Some((_, i, moved)) => line[i] = moved,
            None => moving = false,
        }
    }
    run_of
        .into_iter()
        .map(|run| run.map(|run| line[merged_of[run]] as u8))
        .collect()
}

/// A warning where `pitch` is more than half a semitone from `note`'s pitch class.
fn off_warning(pitch: f64, note: u8) -> Option<String> {
    (class_distance(pitch, note) > 0.5).then(|| {
        format!("its pitch is {:+.1} semitones off its note's", {
            (pitch - f64::from(note) + 6.0).rem_euclid(12.0) - 6.0
        })
    })
}

/// A hit's note and any warning about its pitch, or none for a hit left out.
pub(crate) type Named = Option<(u8, Option<String>)>;

/// The note and any warning of each of `hits` in a take with no note list: the note
/// [`notes_by_hit`] names, a hit with no pitch taking the note of the named hit before
/// it, and none for a hit `skipped` leaves out. A hit named an octave or more from its
/// pitch, to fit the notes around it, is warned about, before any warning that its pitch
/// is off its note's.
pub(crate) fn self_named(hits: &[Hit], skipped: impl Fn(&Hit) -> bool + Copy) -> Vec<Named> {
    let mut last: Option<u8> = None;
    hits.iter()
        .zip(notes_by_hit(hits, skipped))
        .map(|(hit, note)| {
            if skipped(hit) {
                return None;
            }
            match (note, hit.pitch) {
                (Some(note), Some(pitch)) => {
                    last = Some(note);
                    let octaves = ((f64::from(note) - pitch) / 12.0).round();
                    let moved = (octaves != 0.0).then(|| {
                        format!(
                            "named {}, {octaves:+} octave(s) from its pitch, to fit the notes \
                             around it",
                            crate::note::name(note)
                        )
                    });
                    let warnings: Vec<String> =
                        moved.into_iter().chain(off_warning(pitch, note)).collect();
                    Some((note, (!warnings.is_empty()).then(|| warnings.join("; "))))
                }
                _ => last.map(|note| (note, Some("no pitch found".to_string()))),
            }
        })
        .collect()
}

/// The index into `notes` each of `hits` plays, or none for a hit `skipped` leaves out,
/// with a warning where its pitch disagrees.
///
/// The notes are walked in playing order. A hit moves on to the next note when its
/// pitch class is nearer that note's than the current one's, which tolerates the octave
/// errors pitch estimates make on low notes. A hit more than half a semitone from its
/// note's pitch class is warned about.
pub fn assign(
    hits: &[Hit],
    notes: &[u8],
    skipped: impl Fn(&Hit) -> bool,
) -> Vec<Option<(usize, Option<String>)>> {
    let mut j = 0;
    hits.iter()
        .map(|hit| {
            if skipped(hit) || notes.is_empty() {
                return None;
            }
            let Some(pitch) = hit.pitch else {
                return Some((j, Some("no pitch found".to_string())));
            };
            while j + 1 < notes.len()
                && class_distance(pitch, notes[j + 1]) < class_distance(pitch, notes[j])
            {
                j += 1;
            }
            Some((j, off_warning(pitch, notes[j])))
        })
        .collect()
}

/// How a note's layers are chosen from its hits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    /// The first hits played, the first the loudest: for players who play loud, medium,
    /// soft, and recordings whose level cannot be trusted.
    Played,
    /// The hits by measured level, near-duplicates dropped, spread evenly in dB.
    Loudness,
}

/// What became of one of a note's hits.
#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    /// The `index`th layer, counting from the loudest.
    Layer(usize),
    /// Within the gap of hit `of`, which was kept.
    Duplicate { of: usize },
    /// More hits than layers.
    Extra,
    /// Played again later in a take of one layer, and hit `by` is kept.
    Replaced { by: usize },
}

/// The most layers a note gets, whatever is asked: [`v6_values`] names each a value
/// from 0 to 100, and two layers of one value would clash in the build.
pub const MOST_LAYERS: usize = 101;

/// What becomes of each of `group`, one note's hits in playing order, as `layers`
/// layers at most chosen by `order`, or with no most as many as it has hits, up to
/// [`MOST_LAYERS`]. By loudness, hits within `min_gap_db` of a louder one kept are
/// duplicates; in playing order, every hit counts.
pub fn pick(group: &[&Hit], layers: Option<usize>, order: Order, min_gap_db: f64) -> Vec<Choice> {
    let mut choices = vec![Choice::Extra; group.len()];
    match order {
        Order::Played => {
            let mut by_number: Vec<usize> = (0..group.len()).collect();
            by_number.sort_by_key(|&i| group[i].number);
            let most = layers.unwrap_or(group.len()).min(MOST_LAYERS);
            for (layer, &i) in by_number.iter().take(most).enumerate() {
                choices[i] = Choice::Layer(layer);
            }
        }
        Order::Loudness => {
            let mut loudest: Vec<usize> = (0..group.len()).collect();
            loudest.sort_by(|&a, &b| group[b].level_db.total_cmp(&group[a].level_db));
            let mut kept: Vec<usize> = Vec::new();
            for &i in &loudest {
                match kept.last() {
                    Some(&last) if group[last].level_db - group[i].level_db < min_gap_db => {
                        choices[i] = Choice::Duplicate {
                            of: group[last].number,
                        };
                    }
                    _ => kept.push(i),
                }
            }
            let layers = layers.unwrap_or(kept.len()).min(MOST_LAYERS);
            if kept.len() > layers && layers > 0 {
                let (top, bottom) = (
                    group[kept[0]].level_db,
                    group[kept[kept.len() - 1]].level_db,
                );
                let mut chosen: Vec<usize> = Vec::new();
                for t in 0..layers {
                    let target = if layers == 1 {
                        top
                    } else {
                        top + (bottom - top) * t as f64 / (layers - 1) as f64
                    };
                    let best = kept
                        .iter()
                        .copied()
                        .filter(|i| !chosen.contains(i))
                        .reduce(|a, b| {
                            let off = |i: usize| (group[i].level_db - target).abs();
                            if off(b) < off(a) {
                                b
                            } else {
                                a
                            }
                        })
                        .expect("more kept than layers");
                    chosen.push(best);
                }
                kept.retain(|i| chosen.contains(i));
            } else if layers == 0 {
                kept.clear();
            }
            for (layer, &i) in kept.iter().enumerate() {
                choices[i] = Choice::Layer(layer);
            }
        }
    }
    choices
}

/// What became of one file of a sample set of one-note files.
#[derive(Clone, Debug, PartialEq)]
pub enum Placed {
    /// The `layer`th of the `of` layers of `note`, counting from the loudest.
    Layer { note: u8, layer: usize, of: usize },
    /// Within the layer gap of file `of`, by its index, which was kept.
    Duplicate { note: u8, of: usize },
    /// More files of its note than layers.
    Extra { note: u8 },
    /// No pitch was found, so it plays no known note.
    NoPitch,
}

/// The note and layer of each file of a sample set, from `struck`: each file's hit, or
/// none for a file with no hit loud enough, which is placed nowhere.
///
/// A set comes in no playing order and its file names are not trusted, so the hits are
/// sorted by pitch and named as [`notes_by_hit`] names a take's, whose stretch check then
/// has each note's neighbors to go by. Sorted by pitch they make no playing line, so no
/// octave is moved: each file keeps the octave its pitch was read in. A note's files are
/// its hits, and its layers are chosen from them by level as [`pick`] chooses a take's:
/// `layers` at most, or every one with no most, files within `min_gap_db` of a louder
/// one kept being duplicates, as round-robins of one dynamic often are.
pub fn place(
    struck: &[Option<Hit>],
    layers: Option<usize>,
    min_gap_db: f64,
) -> Vec<Option<Placed>> {
    let mut placed: Vec<Option<Placed>> = vec![None; struck.len()];
    let mut sorted: Vec<Hit> = Vec::new();
    for (i, hit) in struck.iter().enumerate() {
        match hit {
            Some(hit) if hit.pitch.is_some() => sorted.push(Hit {
                number: i + 1,
                ..hit.clone()
            }),
            Some(_) => placed[i] = Some(Placed::NoPitch),
            None => {}
        }
    }
    sorted.sort_by(|a, b| a.pitch.unwrap_or(0.0).total_cmp(&b.pitch.unwrap_or(0.0)));
    let notes = named(&sorted, |_| false, false);
    let mut distinct: Vec<u8> = notes.iter().flatten().copied().collect();
    distinct.sort_unstable();
    distinct.dedup();
    for note in distinct {
        let group: Vec<&Hit> = sorted
            .iter()
            .zip(&notes)
            .filter(|(_, n)| **n == Some(note))
            .map(|(hit, _)| hit)
            .collect();
        if group.is_empty() {
            continue;
        }
        let choices = pick(&group, layers, Order::Loudness, min_gap_db);
        let of = choices
            .iter()
            .filter(|c| matches!(c, Choice::Layer(_)))
            .count();
        for (hit, choice) in group.iter().zip(choices) {
            placed[hit.number - 1] = Some(match choice {
                Choice::Layer(layer) => Placed::Layer { note, layer, of },
                Choice::Duplicate { of } => Placed::Duplicate { note, of: of - 1 },
                Choice::Extra | Choice::Replaced { .. } => Placed::Extra { note },
            });
        }
    }
    placed
}

/// The layer values of a format 6 root with `count` layers, loudest first: those of the
/// instrument's own Wurlitzer libraries, and from 6 layers on spread evenly over 0 to
/// 100.
///
/// Three layers at 0, 20 and 70 were chosen by ear as the best spread, and four at 0,
/// 22, 50 and 85 after 0, 15, 40 and 75 played mostly the two softest. Confirmed on
/// hardware. The others are inferred from specimens; not confirmed on hardware.
pub fn v6_values(count: usize) -> Vec<u8> {
    match count {
        1 => vec![0],
        2 => vec![0, 40],
        3 => vec![0, 20, 70],
        4 => vec![0, 22, 50, 85],
        5 => vec![0, 12, 30, 55, 85],
        _ => (0..count)
            .map(|i| (i as f64 * 100.0 / (count - 1) as f64).round_ties_even() as u8)
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(number: usize, level_db: f64, pitch: f64) -> Hit {
        Hit {
            number,
            onset: number * 100,
            next: number * 100 + 100,
            level_db,
            pitch: Some(pitch),
        }
    }

    /// Hits of `notes` played in turn, each `per` times, measured at those pitches.
    fn played(pitches: &[f64], per: usize) -> Vec<Hit> {
        pitches
            .iter()
            .flat_map(|&p| std::iter::repeat_n(p, per))
            .enumerate()
            .map(|(i, p)| hit(i + 1, -20.0, p))
            .collect()
    }

    #[test]
    fn a_take_s_notes_are_named_by_its_hits_pitches_in_playing_order() {
        let hits = played(&[47.1, 49.8, 53.9, 57.2], 3);
        assert_eq!(notes_of(&hits, |_| false), [47, 50, 54, 57]);
    }

    #[test]
    fn a_note_read_an_octave_off_its_neighbors_is_moved_back() {
        // D2 read as D3 between B1 and F#2, and the lowest note, B1, read as B2.
        let hits = played(&[47.0, 38.0, 42.0, 45.0], 2);
        assert_eq!(notes_of(&hits, |_| false), [35, 38, 42, 45], "the bottom");
        let hits = played(&[35.0, 50.0, 42.0, 45.0], 2);
        assert_eq!(notes_of(&hits, |_| false), [35, 38, 42, 45], "between");
        let hits = played(&[35.0, 38.0, 42.0, 33.0], 2);
        assert_eq!(notes_of(&hits, |_| false), [35, 38, 42, 45], "the top");
    }

    #[test]
    fn a_stretched_top_and_bass_are_named_by_the_notes_they_stretch_from() {
        // 0.45 sharp at A6, 0.66 at C7: rounding would name C#7.
        let hits = played(&[90.3, 93.45, 96.66], 2);
        assert_eq!(notes_of(&hits, |_| false), [90, 93, 96], "the top");
        // 0.35 flat at C1, then 0.6 flat at A0: rounding would name G#0.
        let hits = played(&[23.65, 20.4, 17.0], 2);
        assert_eq!(notes_of(&hits, |_| false), [24, 21, 17], "the bass");
    }

    #[test]
    fn a_detuned_instrument_keeps_its_nearest_notes() {
        let hits = played(&[48.3, 51.0, 53.75, 57.1], 2);
        assert_eq!(notes_of(&hits, |_| false), [48, 51, 54, 57]);
    }

    #[test]
    fn a_take_that_steps_by_octaves_keeps_its_octaves() {
        let hits = played(&[36.0, 48.0, 60.0, 72.0], 2);
        assert_eq!(notes_of(&hits, |_| false), [36, 48, 60, 72]);
    }

    /// Each hit keeps its own run's note: C2 and C3 are the same pitch class, and a walk
    /// by pitch class would name them as one.
    #[test]
    fn a_take_that_steps_by_octaves_names_each_hit_its_octave() {
        let hits = played(&[36.0, 48.0, 60.0, 72.0], 2);
        let named: Vec<u8> = self_named(&hits, |_| false)
            .into_iter()
            .map(|n| n.unwrap().0)
            .collect();
        assert_eq!(named, [36, 36, 48, 48, 60, 60, 72, 72]);
    }

    #[test]
    fn a_hit_with_no_pitch_takes_the_note_before_it_with_a_warning() {
        let mut hits = played(&[48.0, 50.0], 1);
        hits[1].pitch = None;
        let named = self_named(&hits, |_| false);
        assert_eq!(named[0], Some((48, None)));
        assert_eq!(named[1], Some((48, Some("no pitch found".to_string()))));
    }

    /// A run moved an octave to fit the line is named so: played C1, C2, D2, E2, the C1
    /// hits are taken for C2 read an octave low, and each says it was moved.
    #[test]
    fn a_hit_moved_an_octave_says_so() {
        let hits = played(&[24.0, 36.0, 38.0, 40.0], 1);
        let named = self_named(&hits, |_| false);
        let (note, warning) = named[0].clone().unwrap();
        assert_eq!(note, 36);
        let warning = warning.expect("a warning");
        assert_eq!(
            warning,
            "named C2, +1 octave(s) from its pitch, to fit the notes around it"
        );
        assert_eq!(
            named[1],
            Some((36, None)),
            "a hit at its pitch says nothing"
        );
    }

    /// A hit moved an octave and off its pitch class says both, the move first: played
    /// C2, D2, E2 a little flat, G2 read an octave low and sharp, the G2 is moved up by
    /// the stretch check a semitone and by the line an octave.
    #[test]
    fn a_hit_moved_an_octave_and_off_its_note_says_both() {
        let hits = played(&[36.0, 38.0, 39.7, 30.48], 1);
        let (note, warning) = self_named(&hits, |_| false)[3].clone().unwrap();
        assert_eq!(note, 43);
        assert_eq!(
            warning.as_deref(),
            Some(
                "named G2, +1 octave(s) from its pitch, to fit the notes around it; \
                 its pitch is -0.5 semitones off its note's"
            )
        );
    }

    /// Sorted by pitch, a set has no playing line, so its octaves are kept as read: a
    /// file a semitone apart or an octave apart is its own note.
    #[test]
    fn a_set_s_files_keep_their_octaves() {
        let notes = |pitches: &[f64]| -> Vec<u8> {
            let struck: Vec<Option<Hit>> =
                pitches.iter().map(|&p| Some(hit(0, -10.0, p))).collect();
            place(&struck, Some(4), 1.0)
                .into_iter()
                .map(|p| match p {
                    Some(Placed::Layer { note, .. }) => note,
                    other => panic!("{other:?}"),
                })
                .collect()
        };
        assert_eq!(notes(&[36.0, 48.0, 60.0, 72.0]), [36, 48, 60, 72]);
        assert_eq!(
            notes(&[24.0, 36.0, 38.0, 40.0]),
            [24, 36, 38, 40],
            "a sparse bass"
        );
        assert_eq!(
            notes(&[93.45, 94.5, 95.6, 96.66, 97.7]),
            [93, 94, 95, 96, 97],
            "stretched top notes"
        );
    }

    #[test]
    fn a_take_that_starts_again_at_the_bottom_keeps_its_jump() {
        let hits = played(&[84.0, 87.0, 90.0, 36.0, 39.0, 42.0], 2);
        assert_eq!(notes_of(&hits, |_| false), [84, 87, 90, 36, 39, 42]);
    }

    #[test]
    fn skipped_hits_and_hits_with_no_pitch_name_no_note() {
        let mut hits = played(&[48.0, 60.0, 52.0], 1);
        hits[2].pitch = None;
        assert_eq!(notes_of(&hits, |hit| hit.number == 2), [48]);
        assert!(notes_of(&[], |_| false).is_empty());
    }

    #[test]
    fn hits_walk_the_note_list_and_tolerate_an_octave_off() {
        let hits = [
            hit(1, -10.0, 40.0),
            hit(2, -20.0, 52.1),
            hit(3, -10.0, 43.0),
            hit(4, -12.0, 43.8),
            hit(5, -9.0, 47.0),
        ];
        let found = assign(&hits, &[40, 43, 47], |h| h.number == 4);
        let notes: Vec<Option<usize>> = found.iter().map(|a| a.as_ref().map(|a| a.0)).collect();
        assert_eq!(notes, [Some(0), Some(0), Some(1), None, Some(2)]);
        assert!(found.iter().flatten().all(|(_, warning)| warning.is_none()));
        let off = assign(&[hit(1, -10.0, 40.8)], &[40], |_| false);
        assert!(off[0].as_ref().unwrap().1.is_some());
    }

    #[test]
    fn loudness_drops_near_duplicates_and_spreads_the_rest_evenly() {
        let levels = [-10.0, -10.5, -14.0, -20.0, -26.0, -30.0];
        let hits: Vec<Hit> = levels
            .iter()
            .enumerate()
            .map(|(i, &l)| hit(i + 1, l, 60.0))
            .collect();
        let group: Vec<&Hit> = hits.iter().collect();
        let choices = pick(&group, Some(3), Order::Loudness, 2.0);
        assert_eq!(choices[0], Choice::Layer(0));
        assert_eq!(choices[1], Choice::Duplicate { of: 1 });
        assert_eq!(choices[3], Choice::Layer(1));
        assert_eq!(choices[5], Choice::Layer(2));
        assert_eq!(choices[2], Choice::Extra);
        assert_eq!(choices[4], Choice::Extra);
    }

    #[test]
    fn with_no_most_every_hit_is_a_layer() {
        let hits: Vec<Hit> = (0..6)
            .map(|i| hit(i + 1, -10.0 - 4.0 * i as f64, 60.0))
            .collect();
        let group: Vec<&Hit> = hits.iter().collect();
        let layers = |choices: Vec<Choice>| {
            choices
                .iter()
                .filter(|c| matches!(c, Choice::Layer(_)))
                .count()
        };
        assert_eq!(layers(pick(&group, None, Order::Loudness, 1.0)), 6);
        assert_eq!(layers(pick(&group, None, Order::Played, 1.0)), 6);
        assert_eq!(layers(pick(&group, Some(4), Order::Loudness, 1.0)), 4);
        let near: Vec<Hit> = [-10.0, -10.5, -20.0]
            .iter()
            .enumerate()
            .map(|(i, &l)| hit(i + 1, l, 60.0))
            .collect();
        let group: Vec<&Hit> = near.iter().collect();
        assert_eq!(
            layers(pick(&group, None, Order::Loudness, 1.0)),
            2,
            "a hit within the gap of a louder one is still a duplicate"
        );
        let many: Vec<Hit> = (0..120)
            .map(|i| hit(i + 1, -0.1 * i as f64, 60.0))
            .collect();
        let group: Vec<&Hit> = many.iter().collect();
        assert_eq!(
            layers(pick(&group, None, Order::Loudness, 0.0)),
            MOST_LAYERS,
            "no more layers than values to name them"
        );
    }

    #[test]
    fn played_order_keeps_the_first_hits_loudest_first() {
        let hits = [
            hit(3, -30.0, 60.0),
            hit(1, -40.0, 60.0),
            hit(2, -10.0, 60.0),
        ];
        let group: Vec<&Hit> = hits.iter().collect();
        let choices = pick(&group, Some(2), Order::Played, 2.0);
        assert_eq!(choices, [Choice::Extra, Choice::Layer(0), Choice::Layer(1)]);
    }

    #[test]
    fn layer_values_spread_from_the_loudest() {
        assert_eq!(v6_values(4), [0, 22, 50, 85]);
        assert_eq!(v6_values(9), [0, 12, 25, 38, 50, 62, 75, 88, 100]);
    }

    /// A set's files, in no order: each note's files become its layers by level, a
    /// round-robin as loud as one kept is a duplicate, and a file with no hit or no
    /// pitch is placed nowhere.
    #[test]
    fn a_sample_set_s_files_are_placed_by_pitch_and_level() {
        let mut no_pitch = hit(0, -10.0, 0.0);
        no_pitch.pitch = None;
        let struck = [
            Some(hit(0, -12.0, 64.2)),
            Some(hit(0, -20.0, 59.9)),
            None,
            Some(hit(0, -10.0, 60.1)),
            Some(hit(0, -20.5, 60.0)),
            Some(no_pitch),
        ];
        let placed = place(&struck, Some(4), 2.0);
        assert_eq!(
            placed,
            [
                Some(Placed::Layer {
                    note: 64,
                    layer: 0,
                    of: 1
                }),
                Some(Placed::Layer {
                    note: 60,
                    layer: 1,
                    of: 2
                }),
                None,
                Some(Placed::Layer {
                    note: 60,
                    layer: 0,
                    of: 2
                }),
                Some(Placed::Duplicate { note: 60, of: 1 }),
                Some(Placed::NoPitch),
            ]
        );
    }

    /// Stretch tuning puts a set's top notes sharp by more than half a semitone: sorted
    /// by pitch, each has its neighbors, and is named by the line they make.
    #[test]
    fn a_set_s_stretched_top_notes_keep_their_names() {
        let pitches = [96.62, 84.3, 88.4, 92.5, 100.75, 80.15];
        let struck: Vec<Option<Hit>> = pitches.iter().map(|&p| Some(hit(0, -10.0, p))).collect();
        let notes: Vec<u8> = place(&struck, Some(4), 2.0)
            .into_iter()
            .map(|p| match p {
                Some(Placed::Layer { note, .. }) => note,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(notes, [96, 84, 88, 92, 100, 80]);
    }
}
