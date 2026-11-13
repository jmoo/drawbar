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

/// The notes `hits` play, in playing order, found from their pitches: the list [`assign`]
/// walks where the take comes with none. Hits `skipped` leaves out, and hits with no
/// pitch, take no part.
///
/// Each hit is named by its pitch, rounded, and a run of hits on one note is one entry.
/// Then, one run at a time, the octave move that most straightens the notes' line is
/// made, while one straightens it by more than half an octave: the line's bend is the
/// sum of how far each run lies off the middle of the runs either side, each counted to
/// two octaves at most, so a take that starts again at the bottom keeps its jump. That
/// catches the octave errors pitch estimates make on low notes, and leaves a take that
/// steps evenly, by octaves too, as played. A take of two notes has no line to go by.
pub fn notes_of(hits: &[Hit], skipped: impl Fn(&Hit) -> bool) -> Vec<u8> {
    let mut runs: Vec<f64> = Vec::new();
    for hit in hits.iter().filter(|hit| !skipped(hit)) {
        let Some(pitch) = hit.pitch else {
            continue;
        };
        let note = pitch.round().clamp(0.0, 127.0);
        if runs.last() != Some(&note) {
            runs.push(note);
        }
    }
    let bend = |runs: &[f64]| -> f64 {
        runs.windows(3)
            .map(|w| (w[0] - 2.0 * w[1] + w[2]).abs().min(24.0))
            .sum()
    };
    loop {
        let now = bend(&runs);
        let mut best: Option<(f64, usize, f64)> = None;
        for i in 0..runs.len() {
            for moved in [runs[i] - 12.0, runs[i] + 12.0] {
                if !(0.0..=127.0).contains(&moved) {
                    continue;
                }
                let mut tried = runs.clone();
                tried[i] = moved;
                let gain = now - bend(&tried);
                if gain > 6.0 && best.is_none_or(|(most, _, _)| gain > most) {
                    best = Some((gain, i, moved));
                }
            }
        }
        match best {
            Some((_, i, moved)) => runs[i] = moved,
            None => break,
        }
    }
    runs.dedup();
    runs.into_iter().map(|note| note as u8).collect()
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
            let warning = (class_distance(pitch, notes[j]) > 0.5).then(|| {
                format!("its pitch is {:+.1} semitones off its note's", {
                    (pitch - f64::from(notes[j]) + 6.0).rem_euclid(12.0) - 6.0
                })
            });
            Some((j, warning))
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

/// What becomes of each of `group`, one note's hits in playing order, as `layers`
/// layers chosen by `order`. Hits within `min_gap_db` of a louder one kept are
/// duplicates.
pub fn pick(group: &[&Hit], layers: usize, order: Order, min_gap_db: f64) -> Vec<Choice> {
    let mut choices = vec![Choice::Extra; group.len()];
    match order {
        Order::Played => {
            let mut by_number: Vec<usize> = (0..group.len()).collect();
            by_number.sort_by_key(|&i| group[i].number);
            for (layer, &i) in by_number.iter().take(layers).enumerate() {
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
    fn a_take_that_steps_by_octaves_keeps_its_octaves() {
        let hits = played(&[36.0, 48.0, 60.0, 72.0], 2);
        assert_eq!(notes_of(&hits, |_| false), [36, 48, 60, 72]);
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
        let choices = pick(&group, 3, Order::Loudness, 2.0);
        assert_eq!(choices[0], Choice::Layer(0));
        assert_eq!(choices[1], Choice::Duplicate { of: 1 });
        assert_eq!(choices[3], Choice::Layer(1));
        assert_eq!(choices[5], Choice::Layer(2));
        assert_eq!(choices[2], Choice::Extra);
        assert_eq!(choices[4], Choice::Extra);
    }

    #[test]
    fn played_order_keeps_the_first_hits_loudest_first() {
        let hits = [
            hit(3, -30.0, 60.0),
            hit(1, -40.0, 60.0),
            hit(2, -10.0, 60.0),
        ];
        let group: Vec<&Hit> = hits.iter().collect();
        let choices = pick(&group, 2, Order::Played, 2.0);
        assert_eq!(choices, [Choice::Extra, Choice::Layer(0), Choice::Layer(1)]);
    }

    #[test]
    fn layer_values_spread_from_the_loudest() {
        assert_eq!(v6_values(4), [0, 22, 50, 85]);
        assert_eq!(v6_values(9), [0, 12, 25, 38, 50, 62, 75, 88, 100]);
    }
}
