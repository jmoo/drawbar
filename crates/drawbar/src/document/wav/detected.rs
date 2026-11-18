//! Detect on a take: its loud action, the search it runs, and the question it asks
//! before it replaces a take's regions.

use super::*;

/// The take's loud action: Detect, which marks its samples in place of its regions.
pub(in crate::document) fn detect_loud(state: &State) -> crate::document::header::Loud {
    use crate::document::header::{Click, Loud, Tone};
    let ready = Loud {
        label: "Detect".to_string(),
        short: "Detect".to_string(),
        glyph: crate::icon::Glyph::Search,
        tone: Tone::Ready,
        hint: "finds the take's notes, layers, cuts and loops, and marks them in place of \
               its regions"
            .to_string(),
        click: Click::Detect,
    };
    if state.detect.running() {
        return Loud {
            label: "Detecting…".to_string(),
            short: "Detecting…".to_string(),
            tone: Tone::Idle,
            hint: "finding the take's hits".to_string(),
            click: Click::Nothing,
            ..ready
        };
    }
    if state.audio().is_none() {
        return Loud {
            tone: Tone::Idle,
            hint: "the WAV's audio does not read".to_string(),
            click: Click::Nothing,
            ..ready
        };
    }
    ready
}

/// Detect on a take: the too-quiet limit it uses, the search running, and what the last
/// one found.
pub struct Detect {
    /// A hit standing this little over the floor is skipped, in dB.
    pub limit: f64,
    /// The search running, the [`audition::print`] of the audio it searches and the
    /// limit it was given.
    pub(super) running: Option<(crate::work::Job<crate::work::Detected>, u64, f64)>,
    /// What the last search found, with the limit it was given, or why it found nothing.
    pub found: Option<Result<(crate::work::Detected, f64), String>>,
    /// Whether Detect waits for the user to confirm it may replace the take's regions.
    pub(super) asking: bool,
}

/// What Detect asks before it replaces a take's regions.
pub const DETECT_REPLACES: &str = "Detect will replace any hits already defined";

/// How wide the Detect question is at most, in points, as the browser's are.
pub(super) const DETECT_ASK_WIDTH: f32 = 440.0;

impl Default for Detect {
    fn default() -> Detect {
        Detect {
            limit: nord_format::take::plan::MIN_SNR_DB,
            running: None,
            found: None,
            asking: false,
        }
    }
}

impl Detect {
    /// Whether a search is running.
    pub fn running(&self) -> bool {
        self.running.is_some()
    }

    /// Start finding the samples of `audio`, unless a search is running.
    pub fn start(&mut self, ctx: &egui::Context, audio: &Audio) {
        if self.running.is_none() {
            let work = crate::work::FindHits::of(audio, self.limit);
            let job = crate::work::hand(ctx, work);
            self.running = Some((job, audition::print(audio), self.limit));
            self.found = None;
        }
    }

    /// Take what the search found, once it has, as the set that marks it, where the take
    /// still holds the `audio` it searched: a result for audio since changed, as by a
    /// gain or Revert, is let go.
    pub fn follow(&mut self, sets: &mut Sets, audio: Option<&Audio>) {
        let Some((job, searched, limit)) = &self.running else {
            return;
        };
        let (searched, limit) = (*searched, *limit);
        let found = match job.poll() {
            crate::work::Answer::Running => return,
            crate::work::Answer::Answered(found) => found,
            crate::work::Answer::Died => {
                self.found = Some(Err(job.died("Detect")));
                self.running = None;
                return;
            }
        };
        self.running = None;
        if audio.map(audition::print) != Some(searched) {
            self.found = Some(Err(
                "the take's audio changed while Detect ran: run it again".to_string(),
            ));
            return;
        }
        if found.kept > 0 {
            sets.push((DETECTED.to_string(), found.value.clone()));
        }
        self.found = Some(Ok((found, limit)));
    }

    /// What the last search found, in words.
    pub fn said(&self) -> Option<String> {
        if self.running() {
            return Some("finding the take's hits…".to_string());
        }
        let skipped =
            |n: usize, limit: f64| format!("{n} skipped, {limit:.0} dB or less over noise");
        Some(match self.found.as_ref()? {
            Err(why) => why.clone(),
            Ok((found, limit)) if found.kept == 0 => format!(
                "No hits marked · {} · regions unchanged",
                skipped(found.too_quiet, *limit)
            ),
            Ok((found, limit)) => format!(
                "{} of {}{}",
                counted(found.kept, "hit"),
                counted(found.notes, "note"),
                match found.too_quiet {
                    0 => String::new(),
                    n => format!(" · {}", skipped(n, *limit)),
                }
            ),
        })
    }
}

/// `n` of `what`, in the plural unless one.
fn counted(n: usize, what: &str) -> String {
    match n {
        1 => format!("1 {what}"),
        n => format!("{n} {what}s"),
    }
}
