//! A take's regions as the strokes a library is built from: each region cleaned, its
//! ends and loop shaped, the take's lowest and highest roots pitched out to reach every
//! key, and each stroke's layer value, gain and release balance. [`prepared`] does it
//! all, so every caller builds a take alike.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::{malformed, Result};
use crate::formats::npno::encode::{
    self, parse_stroke_name, Clash, Kind, LayerTag, Recording, Stem, FADE_SUFFIX,
};
use crate::formats::npno::{Bank, Version};
use crate::note;
use crate::take::clean::{self, Noise};
use crate::take::detect::Floor;
use crate::take::layers::{self, TAKE_PEAK_DB};
use crate::take::{plan, resample, sustain};
use crate::wav::{self, Audio, Loop, Marks, Pcm16, Region};

/// One take a build reads.
pub struct Take<'a> {
    /// The name errors give it: its path.
    pub file: &'a str,
    pub audio: &'a Audio,
    pub marks: &'a Marks,
    /// Whether its strokes are cleaned of its hum and hiss, as its editor last said;
    /// raw, they are built as recorded.
    pub clean: bool,
    /// How its noise floor is read before it is cleaned, as prep read it.
    pub floor: Floor,
}

/// A region of a take as a stroke, and what its label says the stroke is.
pub struct Stroke<'a> {
    /// The name of the take it is a region of.
    pub file: &'a str,
    pub take: &'a Audio,
    pub region: &'a Region,
    /// Its frames as the build cleans them, interleaved.
    pub audio: Vec<f32>,
    /// The loop it holds, and the frame its fade starts at, both counted from its start.
    pub sample_loop: Option<Loop>,
    pub fade: Option<u32>,
    /// Semitones it is pitched by, to play a root the take does not hold.
    pub shift: i32,
    pub root: u8,
    pub bank: Bank,
    pub layer: LayerTag,
}

impl Stroke<'_> {
    /// Its region's label, with the shift of a pitched one.
    pub fn name(&self) -> String {
        match self.shift {
            0 => self.region.label.clone(),
            shift => format!("{} {shift:+}", self.region.label),
        }
    }

    /// The stroke as an error names it.
    pub fn whose(&self) -> String {
        format!("{}: region {:?}", self.file, self.region.label)
    }

    /// How loud it is, as [`layers::level`] measures it: none when silent.
    pub fn level(&self) -> Option<f64> {
        layers::level(&self.audio, usize::from(self.take.channels), self.take.rate)
    }

    /// Its loudest sample.
    pub fn peak(&self) -> f32 {
        self.audio
            .iter()
            .fold(0.0, |most, x| f32::max(most, x.abs()))
    }

    /// Its audio shaped, raised by `gain` and dithered to 16 bits, as [`cut`] makes it.
    pub fn pcm(&self, gain: f32) -> Pcm16 {
        cut(
            self.take,
            &self.audio,
            self.bank,
            self.sample_loop,
            self.fade,
            gain,
            dither_seed(self.root, self.bank, self.layer),
        )
    }
}

/// `frames`, a region of `take` holding a stroke of `bank`, shaped, raised by `gain`
/// and dithered to 16-bit PCM with the noise `seed` picks, with `sample_loop` counted
/// from its start.
///
/// An attack fades in over 0.3 ms, and a release over 5 ms, fading out over at most
/// 40 ms. A looped region's loop is evened out and crossfaded so its end meets its
/// start. A region that fades from frame `fade` fades to silence at its end and loops
/// on silence after it.
pub fn cut(
    take: &Audio,
    frames: &[f32],
    bank: Bank,
    sample_loop: Option<Loop>,
    fade: Option<u32>,
    gain: f32,
    seed: u64,
) -> Pcm16 {
    let channels = usize::from(take.channels.max(1));
    let (mut shaped, sample_loop) = sustain::shaped(
        frames,
        channels,
        take.rate,
        bank == Bank::Release,
        sample_loop,
        fade,
    );
    for v in &mut shaped {
        *v *= gain;
    }
    Pcm16 {
        rate: take.rate,
        channels: take.channels,
        samples: wav::dithered(&shaped, seed),
        sample_loop,
    }
}

/// The dither noise of a stroke's audio, its own: its root, bank and layer pick it.
pub fn dither_seed(root: u8, bank: Bank, layer: LayerTag) -> u64 {
    let layer = match layer {
        LayerTag::Index(index) => index,
        LayerTag::Value(value) => value,
    };
    u64::from(root) << 16 | u64::from(bank.code()) << 8 | u64::from(layer)
}

/// Sorts strokes by root, bank, layer and name.
fn in_stroke_order(strokes: &mut [Stroke]) {
    strokes.sort_by_key(|s| (s.root, s.bank.code(), s.layer, s.name()));
}

impl<'a> Take<'a> {
    /// The take at `file` as its `settings` say it is built: cleaned unless its editor
    /// set it to raw, its noise floor read as prep read it.
    pub fn stated(
        file: &'a str,
        audio: &'a Audio,
        marks: &'a Marks,
        settings: &wav::Settings,
    ) -> Take<'a> {
        Take {
            file,
            audio,
            marks,
            clean: settings.clean != Some(false),
            floor: settings.floor.unwrap_or_default(),
        }
    }
}

/// The kind of instrument `takes` state, each a take's name and settings: the one they
/// agree on, or none where none states one. A kind that is none, or two that disagree,
/// are refused.
pub fn stated_kind<'s>(
    takes: impl IntoIterator<Item = (&'s str, &'s wav::Settings)>,
) -> Result<Option<Kind>> {
    let mut stated: Option<(Kind, &str)> = None;
    for (file, settings) in takes {
        let Some(word) = &settings.kind else {
            continue;
        };
        let Some(kind) = Kind::from_word(word) else {
            return malformed(format!(
                "{file}: states kind {word:?}, which is no kind of instrument"
            ));
        };
        match stated {
            Some((first, at)) if first != kind => {
                return malformed(format!(
                    "{at} states kind {}, but {file} states {}",
                    first.word(),
                    kind.word()
                ));
            }
            _ => stated = Some((kind, file)),
        }
    }
    Ok(stated.map(|(kind, _)| kind))
}

/// A take's stroke ready for the encoder: its layer value in the library, the gain
/// that brings it to the take's peak, and how far under its own attack a release
/// plays.
pub struct Prepared<'a> {
    pub stroke: Stroke<'a>,
    pub value: u8,
    pub gain: f32,
    pub under_attack: Option<f64>,
}

impl Prepared<'_> {
    /// The stroke as the encoder takes it, and how many samples resampling clipped.
    pub fn recording(&self) -> Result<(Recording, usize)> {
        let pcm = self.stroke.pcm(self.gain);
        match recording(
            &pcm,
            self.stroke.root,
            self.stroke.bank,
            self.value,
            self.under_attack,
        ) {
            Ok(made) => Ok(made),
            Err(e) => malformed(format!("{}: {e}", self.stroke.whose())),
        }
    }
}

/// The strokes of `takes` ready for a `version` library, in stroke order: each take's
/// regions cleaned as it says, given their layer values, and where `leveled`, as a
/// format 6 library from rules is, its lowest and highest roots pitched to reach the
/// keys beyond them, each stroke raised to the take's peak and each release balanced
/// under its attack. A layer value no velocity of `version` selects is refused.
pub fn prepared<'a>(
    takes: &[Take<'a>],
    version: Version,
    leveled: bool,
) -> Result<Vec<Prepared<'a>>> {
    let strokes = strokes(takes, leveled)?;
    let named: Vec<(u8, Bank, LayerTag)> = strokes
        .iter()
        .map(|stroke| (stroke.root, stroke.bank, stroke.layer))
        .collect();
    let values = match encode::layer_values(&named, version) {
        Ok(values) => values,
        Err(clash) => {
            let what = format!("root {} {}", note::name(clash.root), clash.bank.name());
            return malformed(match clash.how {
                Clash::BothForms => format!(
                    "{what} names some of its layers by index (l..) and some by value \
                     (v..); all layers of one root and bank must use the same form"
                ),
                Clash::Twice => format!("{what} names one of its layers twice"),
            });
        }
    };
    let highest = version.highest_played_layer();
    if let Some((stroke, value)) = strokes.iter().zip(&values).find(|(_, &v)| v > highest) {
        return malformed(format!(
            "{}: no velocity selects layer value {value}; {highest} is the largest a key \
             of a format {} library ever sounds",
            stroke.whose(),
            version.number()
        ));
    }
    let refs: Vec<&Stroke> = strokes.iter().collect();
    let gains = gains(&refs, leveled);
    let balances = release_balance(&refs, &values, leveled);
    Ok(strokes
        .into_iter()
        .zip(values)
        .zip(gains)
        .zip(balances)
        .map(|(((stroke, value), gain), under_attack)| Prepared {
            stroke,
            value,
            gain,
            under_attack,
        })
        .collect())
}

/// `pcm`, a stroke of `root` and `bank` at layer value `value`, as the encoder takes
/// it: resampled to the instrument's rate with its loop, and released `under_attack`
/// its attack. Also how many samples resampling clipped.
pub fn recording(
    pcm: &Pcm16,
    root: u8,
    bank: Bank,
    value: u8,
    under_attack: Option<f64>,
) -> Result<(Recording, usize)> {
    let audio = encode::resample(&pcm.samples, usize::from(pcm.channels), pcm.rate)?;
    let sustain = pcm
        .sample_loop
        .map(|found| encode::sustain(found, pcm.rate))
        .transpose()?;
    Ok((
        Recording {
            root,
            bank,
            layer: value,
            channels: audio.channels,
            sustain,
            under_attack,
        },
        audio.clipped,
    ))
}

/// The strokes of `takes`, in stroke order: each take's regions, cleaned unless its
/// editor set it to raw, and, where `leveled`, its lowest and highest roots pitched to
/// reach the keys beyond them, as [`pitched_roots`] says.
fn strokes<'a>(takes: &[Take<'a>], leveled: bool) -> Result<Vec<Stroke<'a>>> {
    let mut out = Vec::new();
    for take in takes {
        let noise = take
            .clean
            .then(|| Noise::of(&plan::mono(take.audio), take.audio.rate, take.floor));
        out.extend(region_strokes(
            take.file,
            take.audio,
            take.marks,
            noise.as_ref(),
        )?);
    }
    if leveled {
        let pitched = pitched_roots(&out)?;
        out.extend(pitched);
    }
    in_stroke_order(&mut out);
    Ok(out)
}

/// The regions of `take`, named `file`, with what their labels say each stroke is, in
/// stroke order, each cleaned of the take's `noise` by its root's pitch, or as recorded
/// where there is none.
fn region_strokes<'a>(
    file: &'a str,
    take: &'a Audio,
    marks: &'a Marks,
    noise: Option<&Noise>,
) -> Result<Vec<Stroke<'a>>> {
    let named = |e: &dyn std::fmt::Display| format!("{file}: {e}");
    let mut out = Vec::new();
    let mut fades = Vec::new();
    let regions = match marks.regions_with_loops() {
        Ok(regions) => regions,
        Err(e) => return malformed(named(&e)),
    };
    for (region, sample_loop) in regions {
        if let Some(stroke) = region.label.strip_suffix(FADE_SUFFIX) {
            fades.push((stroke, region));
            continue;
        }
        let Some((root, bank, layer)) =
            parse_stroke_name(&region.label, Stem::None).filter(|_| region.frames > 0)
        else {
            return malformed(named(&format!(
                "region {:?} at frame {} is no stroke: a region here must span frames \
                 and be labeled as a stroke's WAV is named, as in 060-b0-l00",
                region.label, region.start
            )));
        };
        let channels = usize::from(take.channels.max(1));
        let total = take.samples.len() / channels;
        if u64::from(region.start) + u64::from(region.frames) > total as u64 {
            return malformed(named(&format!(
                "region {:?} runs to frame {}, past the audio's end at frame {total}",
                region.label,
                u64::from(region.start) + u64::from(region.frames)
            )));
        }
        let audio = match noise {
            Some(noise) => noise.stroke(
                &take.samples,
                channels,
                region.start as usize,
                region.frames as usize,
                clean::f0(root),
            ),
            None => {
                let at = |frame: u32| (frame as usize * channels).min(take.samples.len());
                take.samples[at(region.start)..at(region.start + region.frames)].to_vec()
            }
        };
        out.push(Stroke {
            file,
            take,
            region,
            audio,
            sample_loop,
            fade: None,
            shift: 0,
            root,
            bank,
            layer,
        });
    }
    for (stroke, fade) in fades {
        let refuse = |why: &str| named(&format!("region {:?} {why}", fade.label));
        let Some(owner) = out.iter_mut().find(|s| s.name() == stroke) else {
            return malformed(refuse(&format!(
                "fades no stroke: no region is labeled {stroke:?}"
            )));
        };
        let end = |r: &Region| u64::from(r.start) + u64::from(r.frames);
        if fade.start < owner.region.start || end(fade) != end(owner.region) {
            return malformed(refuse(&format!(
                "must lie within {stroke:?} and end with it, where the fade reaches silence"
            )));
        }
        if owner.bank == Bank::Release || owner.sample_loop.is_some() || owner.fade.is_some() {
            return malformed(refuse(&format!(
                "fades {stroke:?}, which is a release, loops, or fades already; only an \
                 attack or resonance that does not loop fades"
            )));
        }
        owner.fade = Some(fade.start - owner.region.start);
    }
    if out.is_empty() {
        return malformed(named(&"no region to build a library from"));
    }
    in_stroke_order(&mut out);
    Ok(out)
}

/// The gain each stroke's audio is raised or lowered by before it is rounded to 16
/// bits, in the order `strokes` came in.
///
/// A format 6 library built from rules plays each stroke at the level of its root's
/// loudest layer, whatever the stroke's own level, and each root at its loudest layer's
/// audio level. So every region of a take is brought to one peak: a soft layer keeps
/// its detail above the 16-bit steps, and a root recorded quieter plays as loud as the
/// others. Kept at their recorded levels instead, a take's treble roots sounded dead.
/// Confirmed on hardware. In any other library a stroke plays at its own level, so
/// nothing changes.
fn gains(strokes: &[&Stroke], leveled: bool) -> Vec<f32> {
    let target = 10f32.powf(TAKE_PEAK_DB as f32 / 20.0);
    strokes
        .iter()
        .map(|stroke| match (leveled, stroke.peak()) {
            (true, peak) if peak > 0.0 => target / peak,
            _ => 1.0,
        })
        .collect()
}

/// The lowest and highest keys a take's roots are pitched out toward: the instrument's
/// 88.
const KEYS: (u8, u8) = (21, 108);

/// The furthest a root is pitched to play keys its take does not reach, in semitones.
const MOST_PITCHED: i32 = 12;

/// Extra roots that reach the keys a take's lowest and highest roots do not, each the
/// strokes of that root pitched, for a format 6 library built from rules.
///
/// A root plays from 12 semitones under it to one over, and a key past them is silent.
/// The lowest root's strokes are pitched down to the root that reaches [`KEYS`]' lowest,
/// and the highest's up to the one that reaches its highest, by at most
/// [`MOST_PITCHED`] semitones.
fn pitched_roots<'a>(strokes: &[Stroke<'a>]) -> Result<Vec<Stroke<'a>>> {
    let roots: BTreeSet<u8> = strokes.iter().map(|stroke| stroke.root).collect();
    let (Some(&lowest), Some(&highest)) = (roots.first(), roots.last()) else {
        return Ok(Vec::new());
    };
    let mut wanted = Vec::new();
    if lowest > KEYS.0 + 12 {
        let to = (KEYS.0 + 12).max(lowest.saturating_sub(MOST_PITCHED as u8));
        wanted.push((lowest, to));
    }
    if highest < KEYS.1 - 1 {
        let to = (KEYS.1 - 1).min(highest + MOST_PITCHED as u8);
        wanted.push((highest, to));
    }
    let mut out = Vec::new();
    for (from, to) in wanted {
        let shift = i32::from(to) - i32::from(from);
        for stroke in strokes.iter().filter(|stroke| stroke.root == from) {
            let take = stroke.take;
            let pitched = match resample::pitched(
                &Audio {
                    rate: take.rate,
                    channels: take.channels,
                    samples: stroke.audio.clone(),
                },
                shift,
            ) {
                Ok(pitched) => pitched,
                Err(e) => return malformed(format!("{}: {e}", stroke.whose())),
            };
            let frames = pitched.frames() as u32;
            let before = stroke.audio.len() / usize::from(take.channels.max(1));
            let scale = |at: u32| {
                ((f64::from(at) * f64::from(frames) / before as f64).round() as u32).min(frames)
            };
            out.push(Stroke {
                file: stroke.file,
                take,
                region: stroke.region,
                audio: pitched.samples,
                // A loop's end is its last frame, so it stays on one.
                sample_loop: stroke.sample_loop.map(|found| Loop {
                    start: scale(found.start).min(frames.saturating_sub(1)),
                    end: scale(found.end).min(frames.saturating_sub(1)),
                    ..found
                }),
                fade: stroke.fade.map(scale),
                shift,
                root: to,
                bank: stroke.bank,
                layer: stroke.layer,
            });
        }
    }
    Ok(out)
}

/// How far under its own attack each release region was recorded, in the order
/// `strokes` came in, with `values` their layer values: under the attack region of its
/// root at its layer value. None for every other stroke, for a release with no such
/// attack, and unless `leveled`, as [`gains`] says.
///
/// One recording holds both strokes, so their balance is the instrument's: a format 6
/// library from rules plays the release at it, lifted as quiet key-offs need.
fn release_balance(strokes: &[&Stroke], values: &[u8], leveled: bool) -> Vec<Option<f64>> {
    let attacks: BTreeMap<(u8, u8), f64> = strokes
        .iter()
        .zip(values)
        .filter(|(stroke, _)| stroke.bank == Bank::Attack)
        .filter_map(|(stroke, &value)| Some(((stroke.root, value), stroke.level()?)))
        .collect();
    strokes
        .iter()
        .zip(values)
        .map(|(stroke, &value)| match (leveled, stroke.bank) {
            (true, Bank::Release) => Some(attacks.get(&(stroke.root, value))? - stroke.level()?),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A root's attack region, `frames` frames of a tone, with a loop ending on its last
    /// frame and a fade from its middle.
    fn region_stroke<'a>(take: &'a Audio, region: &'a Region, root: u8) -> Stroke<'a> {
        let frames = region.frames;
        Stroke {
            file: "take.wav",
            take,
            region,
            audio: take.samples.clone(),
            sample_loop: Some(Loop::forward(frames / 2, frames - 1)),
            fade: Some(frames / 2),
            shift: 0,
            root,
            bank: Bank::Attack,
            layer: LayerTag::Index(0),
        }
    }

    #[test]
    fn a_pitched_root_keeps_its_loop_inside_its_frames_and_reaches_the_keys() {
        for frames in [99u32, 100, 101, 1001] {
            let take = Audio {
                rate: 35_002,
                channels: 1,
                samples: (0..frames).map(|i| (i as f32 / 20.0).sin() / 2.0).collect(),
            };
            let region = Region {
                start: 0,
                frames,
                label: "060-b0-l00".into(),
            };
            let pitched = pitched_roots(&[region_stroke(&take, &region, 60)]).unwrap();
            let roots: Vec<(u8, i32)> = pitched.iter().map(|s| (s.root, s.shift)).collect();
            assert_eq!(roots, [(48, -12), (72, 12)], "{frames} frames");
            for stroke in &pitched {
                let (Some(found), Some(fade)) = (stroke.sample_loop, stroke.fade) else {
                    panic!("the loop and fade are kept");
                };
                let len = stroke.audio.len() as u32;
                assert!(
                    found.start <= found.end && found.end < len && fade <= len,
                    "{frames} frames pitched to {}: loop {found:?}, fade {fade}, {len} frames",
                    stroke.root
                );
            }
        }
    }

    #[test]
    fn no_root_is_pitched_where_the_take_reaches_the_keys_or_holds_none() {
        assert!(pitched_roots(&[]).unwrap().is_empty());
        let take = Audio {
            rate: 35_002,
            channels: 1,
            samples: vec![0.0; 100],
        };
        let low = Region {
            start: 0,
            frames: 100,
            label: "033-b0-l00".into(),
        };
        let high = Region {
            label: "107-b0-l00".into(),
            ..low.clone()
        };
        let strokes = [
            region_stroke(&take, &low, 33),
            region_stroke(&take, &high, 107),
        ];
        assert!(pitched_roots(&strokes).unwrap().is_empty());
    }

    #[test]
    fn takes_state_the_kind_they_agree_on() {
        let says = |kind: Option<&str>| wav::Settings {
            kind: kind.map(str::to_string),
            ..wav::Settings::default()
        };
        let (grand, wurli, none) = (says(Some("grand")), says(Some("wurlitzer")), says(None));
        let kind = |takes: &[(&str, &wav::Settings)]| stated_kind(takes.iter().copied());
        assert_eq!(kind(&[("a", &none)]).unwrap(), None);
        assert_eq!(
            kind(&[("a", &grand), ("b", &none), ("c", &grand)]).unwrap(),
            Some(Kind::Grand)
        );
        let err = kind(&[("a", &grand), ("b", &wurli)]).unwrap_err();
        assert!(
            err.to_string().contains("a states kind grand, but b"),
            "{err}"
        );
        let err = kind(&[("a", &says(Some("kazoo")))]).unwrap_err();
        assert!(err.to_string().contains("no kind of instrument"), "{err}");
    }

    /// A take whose regions name one layer twice is refused before anything is built.
    #[test]
    fn a_layer_named_twice_is_refused() {
        let take = Audio {
            rate: 35_002,
            channels: 1,
            samples: vec![0.1; 1_000],
        };
        let marks = Marks {
            regions: vec![
                Region {
                    start: 0,
                    frames: 400,
                    label: "060-b0-v00".into(),
                },
                Region {
                    start: 500,
                    frames: 400,
                    label: "060-b0-v00".into(),
                },
            ],
            loops: Vec::new(),
        };
        let takes = [Take::stated(
            "take.wav",
            &take,
            &marks,
            &wav::Settings {
                clean: Some(false),
                ..wav::Settings::default()
            },
        )];
        let err = prepared(&takes, Version::V6, true).err().expect("refused");
        assert!(err.to_string().contains("twice"), "{err}");
    }

    /// Any WAV's cue chunk can reach the build: a region past the audio is refused, not
    /// read out of bounds, cleaned or not.
    #[test]
    fn a_region_past_the_audio_s_end_is_refused() {
        let take = Audio {
            rate: 35_002,
            channels: 2,
            samples: vec![0.0; 200],
        };
        let noise = Noise::of(&plan::mono(&take), take.rate, Floor::Take);
        for (start, frames) in [(50, 51), (u32::MAX, 2)] {
            let marks = Marks {
                regions: vec![Region {
                    start,
                    frames,
                    label: "060-b0-l00".into(),
                }],
                loops: Vec::new(),
            };
            for cleaned in [None, Some(&noise)] {
                let err = region_strokes("take.wav", &take, &marks, cleaned)
                    .err()
                    .expect("refused");
                assert!(err.to_string().contains("past the audio's end"), "{err}");
            }
        }
    }
}
