//! A sample heard as the instrument will play it.
//!
//! Its regions are cleaned of the take's hum and hiss as the build cleans them, unless
//! the take is set to build as recorded, shaped from their marks as the build shapes
//! its strokes, and brought
//! to the peak the build brings them to. Then they fade as a library built from rules
//! for the take's kind states: from note-on until the loop, along the loop while the key
//! is held, and away at key-up, where the release starts under the attack at the
//! balance the build plays it at. Those fades are the encoder's rules; how each is known
//! is said where [`encode`] states it.

use std::ops::Range;

use eframe::egui;
use nord_format::formats::npno::encode::{self, Kind, Rules};
use nord_format::formats::npno::{Bank, Version};
use nord_format::formats::nsmp::codec;
use nord_format::take::clean::{self, Noise};
use nord_format::take::layers;
use nord_format::take::sustain;
use nord_format::wav::{Audio, Loop, Play};

use super::hit::Cuts;
use super::wav;
use crate::app;
use crate::audio::{Finger, Player};
use crate::icon::Glyph;
use crate::work;

/// The zones of a WAV document the audition plays on: the held key, the loop's wrap and
/// the release alone. Zone 0 is the document's own Play.
const HELD: usize = 1;
const WRAP: usize = 2;
const RELEASE: usize = 3;

/// The labels of the card's controls.
pub const PLAY_SAMPLE: &str = "Play sample";
pub const LOOP_WRAP: &str = "Loop wrap";
pub const RELEASE_ALONE: &str = "Release";

/// The longest a held key plays, in seconds: a loop fading as slowly as a low grand's
/// would otherwise play on for minutes.
const HOLD_MOST: f64 = 60.0;

/// How far under where it started a fading voice stops, in dB.
const SILENT_DB: f64 = 100.0;

/// How far before the loop's end [`Sounding::wrap`] starts, in seconds.
const WRAP_LEAD: f64 = 1.0;

/// How much of the gap between a sample and its root's loudest layer, in dB, the
/// audition keeps. A rule build plays every layer at its root's loudest's level, and the
/// instrument makes the dynamics from velocity in ways not measured; kept whole, soft
/// layers were too quiet to judge, and brought all to one level, they sounded as loud
/// as the hardest.
///
/// The loudest layer plays at its recorded level, as Play original plays it.
const LAYER_GAP_KEPT: f64 = 0.5;

/// How long [`Sounding::wrap`] cycles the loop for, in seconds.
const WRAP_MOST: f64 = 30.0;

/// How far ahead of where the player has got a key-up starts, in seconds: past what the
/// mixer has already taken, so the key-up joins the held sound at one sample, with
/// nothing played twice. Heard as a click before.
const SPLICE_LEAD: f64 = 0.02;

/// The longest press of Play sample that is a click, in seconds: let go sooner, the
/// key is held on through the whole sample, as [`Sounding::tapped`] says.
const TAP_MOST: f64 = 0.3;

/// A sample's strokes, shaped as the build shapes them, at the level [`Sounding::of`]
/// plays them at, with the fades the instrument plays them under.
#[derive(Clone, Debug)]
pub struct Sounding {
    pub rate: u32,
    pub channels: u16,
    /// The attack as the build writes it, interleaved.
    attack: Vec<f32>,
    /// The loop the attack plays while held, in frames, its end excluded.
    looped: Option<Range<usize>>,
    /// Decibels per second the attack fades by from note-on until its loop, and along
    /// the loop.
    held_fade: f64,
    loop_fade: f64,
    /// The release as the build writes it, at its balance under the attack and faded as
    /// it plays, or none where the sample has no release.
    release: Option<Vec<f32>>,
    /// Whether key-up fades the attack. A key over the kind's damper rings on.
    damped: bool,
}

/// `frames`, `channels` interleaved, each multiplied by `gain` in dB at its frame.
fn faded(frames: &mut [f32], channels: usize, gain: impl Fn(usize) -> f64) {
    for (i, frame) in frames.chunks_exact_mut(channels).enumerate() {
        let g = 10f64.powf(gain(i) / 20.0) as f32;
        for v in frame {
            *v *= g;
        }
    }
}

/// The frames of `audio` over `span`, interleaved, cleaned of `noise` as a stroke on
/// `root` where it is given, as the build cuts a region.
fn region(audio: &Audio, span: &Range<u32>, root: u8, noise: Option<&Noise>) -> Vec<f32> {
    let channels = usize::from(audio.channels.max(1));
    match noise {
        Some(noise) => noise.stroke(
            &audio.samples,
            channels,
            span.start as usize,
            span.len(),
            clean::f0(root),
        ),
        None => {
            let at = |frame: u32| (frame as usize * channels).min(audio.samples.len());
            audio.samples[at(span.start)..at(span.end)].to_vec()
        }
    }
}

impl Sounding {
    /// The sample `cuts` marks in `audio`, recorded at `root`, cleaned of `noise` where it
    /// is given, as a library of `rules` plays it, or none where it has no attack.
    ///
    /// Its root's loudest layer, whose recorded peak is `loudest`, plays at its recorded
    /// level, and a softer one [`LAYER_GAP_KEPT`] of its recorded gap under that: easy to
    /// hear, and still softer.
    pub fn of(
        audio: &Audio,
        cuts: &Cuts,
        root: u8,
        rules: &Rules,
        noise: Option<&Noise>,
        loudest: f32,
    ) -> Option<Sounding> {
        let channels = usize::from(audio.channels.max(1));
        let span = cuts.attack.as_ref().filter(|span| !span.is_empty())?;
        let raw = region(audio, span, root, noise);
        let sample_loop = cuts.looped.as_ref().map(|found| Loop {
            start: found.start - span.start,
            end: found.end - 1 - span.start,
            play: Play::Forward,
        });
        let fade = cuts.fade.as_ref().map(|fade| fade.start - span.start);
        let (mut attack, held) =
            sustain::shaped(&raw, channels, audio.rate, false, sample_loop, fade);
        let peak = raw.iter().fold(0.0f32, |most, v| most.max(v.abs()));
        let gain_db = match peak > 0.0 {
            true => {
                let own = 20.0 * f64::from(peak).log10();
                let gap = (20.0 * f64::from(loudest.max(peak)).log10() - own).max(0.0);
                (1.0 - LAYER_GAP_KEPT) * gap
            }
            false => 0.0,
        };
        faded(&mut attack, channels, |_| gain_db);
        let release = cuts
            .release
            .as_ref()
            .filter(|span| !span.is_empty())
            .map(|span| {
                let raw_release = region(audio, span, root, noise);
                let (mut release, _) =
                    sustain::shaped(&raw_release, channels, audio.rate, true, None, None);
                let levels = layers::level(&raw, channels, audio.rate).zip(layers::level(
                    &raw_release,
                    channels,
                    audio.rate,
                ));
                let lift = levels.map_or(0.0, |(attack, release)| {
                    let under = encode::release_under(attack - release);
                    attack - under - release
                });
                let fading = rules.held_fade(Bank::Release, root);
                let rate = f64::from(audio.rate);
                faded(&mut release, channels, |i| {
                    gain_db + lift - fading * i as f64 / rate
                });
                release
            });
        Some(Sounding {
            rate: audio.rate,
            channels: audio.channels.max(1),
            attack,
            looped: held.map(|found| found.start as usize..found.end as usize + 1),
            held_fade: rules.held_fade(Bank::Attack, root),
            loop_fade: rules.loop_fading(root),
            release,
            damped: root <= rules.damper_top,
        })
    }

    /// The take's own rules for a kind of instrument: a format 6 library from rules.
    pub fn rules(kind: Kind) -> Rules {
        Rules::new(Version::V6, kind)
    }

    fn frames(&self) -> usize {
        self.attack.len() / usize::from(self.channels)
    }

    /// The attack while its key is held: faded from note-on until its loop, then the
    /// loop round and round, fading, until it is silent or [`HOLD_MOST`] has played. An
    /// attack without a loop plays to its end.
    pub fn held(&self) -> Vec<f32> {
        let channels = usize::from(self.channels);
        let rate = f64::from(self.rate);
        let most = (HOLD_MOST * rate) as usize;
        let (length, from) = match &self.looped {
            Some(found) if !found.is_empty() => {
                let start_db = self.held_fade * found.start as f64 / rate;
                let left = (SILENT_DB - start_db).max(0.0);
                let looping = match self.loop_fade > 0.0 {
                    true => (left / self.loop_fade * rate) as usize,
                    false => most,
                };
                (
                    (found.start + looping).min(most).max(found.end),
                    Some(found.clone()),
                )
            }
            _ => (self.frames().min(most), None),
        };
        let mut out = Vec::with_capacity(length * channels);
        for i in 0..length {
            let at = match &from {
                Some(found) if i >= found.end => {
                    found.start + (i - found.start) % (found.end - found.start)
                }
                _ => i,
            };
            out.extend_from_slice(&self.attack[at * channels..(at + 1) * channels]);
        }
        let (held_fade, loop_fade) = (self.held_fade, self.loop_fade);
        let start = from.map_or(usize::MAX, |found| found.start);
        faded(&mut out, channels, |i| {
            let t = i as f64 / rate;
            match i < start {
                true => -held_fade * t,
                false => {
                    let s = start as f64 / rate;
                    -held_fade * s - loop_fade * (t - s)
                }
            }
        });
        out
    }

    /// What plays once the key `held` sounds is let go `frame` frames in: the rest of the
    /// attack fading away at key-up, unless its key rings on, under the release.
    pub fn let_go(&self, held: &[f32], frame: usize) -> Vec<f32> {
        let channels = usize::from(self.channels);
        let rate = f64::from(self.rate);
        let rest = &held[(frame * channels).min(held.len())..];
        let fading = match self.damped {
            true => ((SILENT_DB / encode::KEY_UP_FADE * rate) as usize * channels).min(rest.len()),
            false => rest.len(),
        };
        let mut out = rest[..fading].to_vec();
        if self.damped {
            faded(&mut out, channels, |i| {
                -encode::KEY_UP_FADE * i as f64 / rate
            });
        }
        if let Some(release) = &self.release {
            if out.len() < release.len() {
                out.resize(release.len(), 0.0);
            }
            for (v, r) in out.iter_mut().zip(release) {
                *v += r;
            }
        }
        out
    }

    /// Where a click of Play sample lets the key go, in frames: once the loop has gone
    /// round once after the attack's end, so the whole sample, its loop and its key-off
    /// are heard; at the end of an attack without a loop.
    pub fn tapped(&self) -> usize {
        match &self.looped {
            Some(found) if !found.is_empty() => found.end + (found.end - found.start),
            _ => self.frames(),
        }
    }

    /// The release alone, as key-up plays it, or none where the sample has none.
    pub fn release(&self) -> Option<&[f32]> {
        self.release.as_deref()
    }

    /// The loop's join over and over: from [`WRAP_LEAD`] before the loop's end, then the
    /// loop round for [`WRAP_MOST`], at the level the attack has there, so the wrap is
    /// heard and not the fade. None where the attack does not loop.
    pub fn wrap(&self) -> Option<Vec<f32>> {
        let found = self.looped.as_ref().filter(|found| !found.is_empty())?;
        let channels = usize::from(self.channels);
        let rate = f64::from(self.rate);
        let from = found.end.saturating_sub((WRAP_LEAD * rate) as usize);
        let length = (WRAP_MOST * rate) as usize;
        let mut out = Vec::with_capacity(length * channels);
        for i in 0..length {
            let mut at = from + i;
            if at >= found.end {
                at = found.start + (at - found.start) % (found.end - found.start);
            }
            out.extend_from_slice(&self.attack[at * channels..(at + 1) * channels]);
        }
        let level = -self.held_fade * found.start.min(from) as f64 / rate;
        faded(&mut out, channels, |_| level);
        Some(out)
    }
}

/// The take's audition: the key held on it, and the noise it is cleaned of. Kept across
/// new bytes, as the lanes' views are.
#[derive(Default)]
pub struct Audition {
    /// The sample sounding while its key is down, and what it plays.
    held: Option<(Sounding, Vec<f32>)>,
    /// Whether Play sample is held down.
    pressed: bool,
    /// Whether a control was pressed while the take's noise is still being found, so
    /// the wait is said.
    waiting: bool,
    /// The noise of the audio whose [`print`] this is, found or being found.
    noise: Option<(u64, Search)>,
    /// The picked sample made ready to play, and what it was made from: the bytes'
    /// stamp, the sample, the kind and whether it is cleaned.
    pub(super) ready: Option<(Made, Option<Sounding>)>,
}

/// What a ready [`Sounding`] was made from.
pub(super) type Made = (u64, super::take::Hit, Kind, bool);

/// The take's noise, as the search for it goes.
enum Search {
    Running(work::Job<Noise>),
    Done(Noise),
    /// The finding died; the take is heard as recorded.
    Lost,
}

/// Where cleaning a take has got to.
pub enum Cleaning<'a> {
    /// The take is set to build as recorded.
    Off,
    /// Its noise is being found.
    Finding,
    /// Found, or lost, where the take is heard as recorded.
    Ready(Option<&'a Noise>),
}

/// A print of `audio` that changes when a gain does: its length and a spread of its
/// samples, so its noise is found again only for other audio.
fn print(audio: &Audio) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    audio.samples.len().hash(&mut hasher);
    for v in audio.samples.iter().step_by(4_099) {
        v.to_bits().hash(&mut hasher);
    }
    hasher.finish()
}

impl Audition {
    /// Start finding the noise of `audio`, where `clean` asks for it and it is not
    /// already found or being found, and take it once found.
    pub fn follow(&mut self, audio: &Audio, clean: bool, ctx: &egui::Context) {
        if !clean {
            return;
        }
        let at = print(audio);
        match &mut self.noise {
            Some((held, Search::Running(job))) if *held == at => match job.poll() {
                work::Answer::Running => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(250))
                }
                work::Answer::Answered(noise) => self.noise = Some((at, Search::Done(noise))),
                work::Answer::Died => self.noise = Some((at, Search::Lost)),
            },
            Some((held, _)) if *held == at => {}
            _ => {
                let mono = nord_format::take::plan::mono(audio);
                let rate = audio.rate;
                let job = work::run(ctx, move |_| Noise::of(&mono, rate));
                self.noise = Some((at, Search::Running(job)));
            }
        }
    }

    /// How far cleaning `audio` has got, where `clean` asks for it.
    pub fn cleaning(&self, audio: &Audio, clean: bool) -> Cleaning<'_> {
        if !clean {
            return Cleaning::Off;
        }
        match &self.noise {
            Some((at, found)) if *at == print(audio) => match found {
                Search::Running(_) => Cleaning::Finding,
                Search::Done(noise) => Cleaning::Ready(Some(noise)),
                Search::Lost => Cleaning::Ready(None),
            },
            _ => Cleaning::Finding,
        }
    }
}

/// What the card asks the player to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    /// Play sample went down: the key is held.
    Hold,
    /// Play sample came up: the key is let go, or, after a click, held on through the
    /// whole sample.
    LetGo,
    /// Start or stop the loop's wrap.
    Wrap,
    /// Start or stop the release alone.
    Release,
    /// Clean the take of its hum and hiss, or build it as recorded.
    Clean(bool),
}

/// The label of the clean toggle.
pub const CLEAN_AUDIO: &str = "Clean audio";

/// The label of the category picker.
pub const CATEGORY: &str = "Category";

/// The category picker: the category the take is heard and built as, `kind`, whose
/// defaults the build applies. The one the operator picks, where it differs.
pub fn category(ui: &mut egui::Ui, kind: Kind) -> Option<Kind> {
    ui.label(CATEGORY);
    let mut chosen = kind;
    egui::ComboBox::from_id_salt("take-category")
        .selected_text(super::piano::kind_name(kind.code()))
        .show_ui(ui, |ui| {
            for each in Kind::ALL {
                ui.selectable_value(&mut chosen, each, super::piano::kind_name(each.code()));
            }
        });
    (chosen != kind).then_some(chosen)
}

/// The picked sample's controls, under it beside Play original: the sample played as
/// the instrument will play it, and whether the take is cleaned. Nothing where no
/// attack is picked. `played` says how far the pointer's voice on one of the document's
/// zones has played, in seconds, where it sounds.
pub fn controls(
    ui: &mut egui::Ui,
    state: &mut wav::State,
    played: impl Fn(usize) -> Option<f64>,
) -> Option<Ask> {
    let cuts = state.cuts().filter(|cuts| cuts.attack.is_some())?;
    let mut asked = None;
    let clean = state.clean();
    state.prepare();
    let finding = state
        .audio()
        .is_some_and(|audio| matches!(state.audition.cleaning(audio, clean), Cleaning::Finding));
    let audition = &mut state.audition;
    audition.waiting &= finding;
    let accent = app::accent(ui.visuals());
    let play = super::sample::action_response(ui, PLAY_SAMPLE, Glyph::Piano, accent);
    // ⚠️ The press and let-go are the pointer's own, not the button's: egui stops
    // counting a press held past its click time as on the button, and that let the key
    // go after a second and struck it again.
    let (struck, down) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down()));
    if struck && play.contains_pointer() {
        match finding {
            true => audition.waiting = true,
            false => asked = Some(Ask::Hold),
        }
    } else if audition.pressed && !down {
        asked = Some(Ask::LetGo);
    }
    if audition.pressed {
        // Let go is seen on the next frame: without one, the key-up waits for the
        // pointer to move.
        ui.ctx().request_repaint();
    }
    let rect = play.rect;
    play.on_hover_text(PLAY_SAMPLE_HINT);
    let mut toggle = |label: &str, glyph: Glyph, zone: usize, there: bool| {
        let (label, glyph) = match played(zone).is_some() {
            true => ("Stop", Glyph::X),
            false => (label, glyph),
        };
        ui.add_enabled_ui(there, |ui| super::sample::action(ui, label, glyph, accent))
            .inner
    };
    for (label, glyph, zone, there, ask) in [
        (
            LOOP_WRAP,
            Glyph::RefreshCw,
            WRAP,
            cuts.looped.is_some(),
            Ask::Wrap,
        ),
        (
            RELEASE_ALONE,
            Glyph::Waves,
            RELEASE,
            cuts.release.is_some(),
            Ask::Release,
        ),
    ] {
        if toggle(label, glyph, zone, there) {
            match finding {
                true => state.audition.waiting = true,
                false => asked = Some(ask),
            }
        }
    }
    if state.audition.waiting {
        egui::Area::new(egui::Id::new("take-noise-wait"))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.left_bottom() + egui::vec2(0.0, 4.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| ui.label(NOISE_WAIT));
            });
    }
    ui.add_space(8.0);
    let mut cleaned = clean;
    if ui.checkbox(&mut cleaned, CLEAN_AUDIO).changed() {
        asked = Some(Ask::Clean(cleaned));
    }
    if finding {
        ui.label(
            egui::RichText::new("finding the take's hum and hiss…")
                .small()
                .color(app::caption(ui.visuals())),
        );
    }
    asked
}

/// What a control pressed while the take's noise is being found says.
pub const NOISE_WAIT: &str = "Generating the noise profile, please wait…";

/// What Play sample does, on hover.
pub const PLAY_SAMPLE_HINT: &str =
    "Click to play the whole sample, its loop and its key-off; hold to keep the key down, and let go for the key-off";

/// The clock `sounding` plays at on the player, against the codec's rate.
fn clock(sounding: &Sounding) -> f32 {
    sounding.rate as f32 / codec::FIELD_RATE as f32
}

/// Do what the card asked, on document `id`'s zones of `player`.
pub fn hear(ask: Ask, state: &mut wav::State, player: &mut Player, id: u64) -> Result<(), String> {
    let sounding = |state: &wav::State| state.audition.ready.as_ref()?.1.clone();
    match ask {
        Ask::Clean(_) => {}
        Ask::Hold => {
            let Some(sounding) = sounding(state) else {
                return Ok(());
            };
            state.audition.pressed = true;
            let held = sounding.held();
            player.strike(
                Finger::Pointer,
                (id, HELD),
                &held,
                sounding.channels,
                clock(&sounding),
            )?;
            state.audition.held = Some((sounding, held));
        }
        Ask::LetGo => {
            state.audition.pressed = false;
            let Some((sounding, held)) = state.audition.held.take() else {
                return Ok(());
            };
            let rate = f64::from(sounding.rate);
            let Some(seconds) = player.played((id, HELD)) else {
                // A held attack that does not loop has played out: its release is left.
                if let Some(release) = sounding.release() {
                    player.strike(
                        Finger::Pointer,
                        (id, HELD),
                        release,
                        sounding.channels,
                        clock(&sounding),
                    )?;
                }
                return Ok(());
            };
            let now = (seconds * rate) as usize;
            let up = match seconds < TAP_MOST {
                // A click: the key stays down through the whole sample, then goes up.
                true => sounding.tapped().max(now),
                false => now + (SPLICE_LEAD * rate) as usize,
            };
            let after = sounding.let_go(&held, up);
            if after.is_empty() {
                return Ok(());
            }
            let spliced = player.splice(
                (id, HELD),
                up as f64 / rate,
                &after,
                sounding.channels,
                clock(&sounding),
            )?;
            if !spliced {
                player.strike(
                    Finger::Pointer,
                    (id, HELD),
                    &after,
                    sounding.channels,
                    clock(&sounding),
                )?;
            }
        }
        Ask::Wrap | Ask::Release => {
            // The pointer's voice is the held key's too: this one takes its place.
            (state.audition.held, state.audition.pressed) = (None, false);
            let zone = match ask {
                Ask::Wrap => WRAP,
                _ => RELEASE,
            };
            if player.sounds((id, zone)) {
                player.silence((id, zone));
                return Ok(());
            }
            let Some(sounding) = sounding(state) else {
                return Ok(());
            };
            let frames = match ask {
                Ask::Wrap => sounding.wrap(),
                _ => sounding.release().map(<[f32]>::to_vec),
            };
            if let Some(frames) = frames {
                player.strike(
                    Finger::Pointer,
                    (id, zone),
                    &frames,
                    sounding.channels,
                    clock(&sounding),
                )?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 35_002;

    /// A mono tone at `hz`, `seconds` long, at `level` of full scale.
    fn tone(hz: f64, seconds: f64, level: f32) -> Vec<f32> {
        (0..(seconds * f64::from(RATE)) as usize)
            .map(|i| level * (std::f64::consts::TAU * hz * i as f64 / f64::from(RATE)).sin() as f32)
            .collect()
    }

    /// A take holding a 2 s attack of a 100 Hz tone looping over its last half second,
    /// and a quieter release after it.
    fn take() -> (Audio, Cuts) {
        let mut samples = tone(100.0, 2.0, 0.5);
        samples.extend(tone(100.0, 0.5, 0.01));
        let second = RATE;
        let cuts = Cuts {
            attack: Some(0..2 * second),
            looped: Some(3 * second / 2..2 * second),
            release: Some(2 * second..5 * second / 2),
            ..Cuts::default()
        };
        let audio = Audio {
            rate: RATE,
            channels: 1,
            samples,
        };
        (audio, cuts)
    }

    fn peak(frames: &[f32]) -> f32 {
        frames.iter().fold(0.0, |most, v| most.max(v.abs()))
    }

    fn db(v: f32) -> f64 {
        20.0 * f64::from(v).log10()
    }

    #[test]
    fn a_note_s_loudest_layer_plays_as_recorded_and_fades_along_its_loop() {
        let (audio, cuts) = take();
        let rules = Sounding::rules(encode::Kind::Wurlitzer);
        let sounding = Sounding::of(&audio, &cuts, 60, &rules, None, 0.0).unwrap();
        let held = sounding.held();
        let second = RATE as usize;
        assert!((db(peak(&held[..second / 10])) - db(0.5)).abs() < 0.1);
        assert!(held.len() > 4 * second, "the loop plays on past the attack");
        let at = |s: usize| db(peak(&held[s * second..s * second + second / 10]));
        let fell = at(3) - at(4);
        let expected = rules.loop_fading(60);
        assert!((fell - expected).abs() < 0.2, "{fell} vs {expected}");
    }

    #[test]
    fn key_up_fades_the_attack_away_and_plays_the_release_at_its_balance() {
        let (audio, cuts) = take();
        let rules = Sounding::rules(encode::Kind::Wurlitzer);
        let sounding = Sounding::of(&audio, &cuts, 60, &rules, None, 0.0).unwrap();
        let held = sounding.held();
        let after = sounding.let_go(&held, RATE as usize);
        let release = sounding.release().unwrap();
        let tail = RATE as usize / 5;
        assert_eq!(
            after[tail..release.len()],
            release[tail..],
            "the attack is gone"
        );
        let (attack_db, release_db) = (
            layers::level(&held, 1, RATE).unwrap(),
            layers::level(release, 1, RATE).unwrap(),
        );
        let recorded = 20.0 * (0.5f64 / 0.01).log10();
        let under = encode::release_under(recorded);
        assert!(
            (attack_db - release_db - under).abs() < 0.5,
            "{} under, the build plays it {under} under",
            attack_db - release_db
        );
    }

    #[test]
    fn a_key_over_the_damper_rings_on_at_key_up() {
        let (audio, cuts) = take();
        let rules = Sounding::rules(encode::Kind::Wurlitzer);
        let sounding =
            Sounding::of(&audio, &cuts, rules.damper_top + 1, &rules, None, 0.0).unwrap();
        let held = sounding.held();
        let after = sounding.let_go(&held, RATE as usize);
        assert_eq!(after.len(), held.len() - RATE as usize);
    }

    #[test]
    fn the_wrap_cycles_the_loop_at_one_level() {
        let (audio, cuts) = take();
        let rules = Sounding::rules(encode::Kind::Wurlitzer);
        let wrap = Sounding::of(&audio, &cuts, 60, &rules, None, 0.0)
            .unwrap()
            .wrap()
            .unwrap();
        let second = RATE as usize;
        let at = |s: usize| peak(&wrap[s * second..s * second + second / 10]);
        assert!((db(at(1)) - db(at(20))).abs() < 0.1);
        let none = Cuts {
            looped: None,
            ..take().1
        };
        assert!(Sounding::of(&audio, &none, 60, &rules, None, 0.0)
            .unwrap()
            .wrap()
            .is_none());
    }

    /// A layer recorded 12 dB under its root's loudest plays 6 dB under it.
    #[test]
    fn a_softer_layer_keeps_half_its_gap_to_the_loudest() {
        let (audio, cuts) = take();
        let rules = Sounding::rules(encode::Kind::Wurlitzer);
        let louder = 0.5 * 10f32.powf(12.0 / 20.0);
        let sounding = Sounding::of(&audio, &cuts, 60, &rules, None, louder).unwrap();
        let held = sounding.held();
        let start = db(peak(&held[..RATE as usize / 10]));
        assert!((start - (db(louder) - 6.0)).abs() < 0.1, "{start}");
    }

    /// A click holds the key through the attack and once round the loop; without a
    /// loop, to the attack's end.
    #[test]
    fn a_click_lets_the_key_go_once_the_loop_has_gone_round() {
        let (audio, cuts) = take();
        let rules = Sounding::rules(encode::Kind::Wurlitzer);
        let sounding = Sounding::of(&audio, &cuts, 60, &rules, None, 0.0).unwrap();
        let found = sounding.looped.clone().unwrap();
        assert_eq!(sounding.tapped(), 2 * found.end - found.start);
        assert!(
            sounding.tapped() <= sounding.held().len(),
            "within what is held"
        );
        let none = Cuts {
            looped: None,
            ..take().1
        };
        let sounding = Sounding::of(&audio, &none, 60, &rules, None, 0.0).unwrap();
        assert_eq!(sounding.tapped(), sounding.frames());
    }
}
