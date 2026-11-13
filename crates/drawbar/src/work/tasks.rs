//! Work handed off by name, so the browser can run it in a worker of its own: a worker
//! takes no closure, only a message, so each piece of work it runs is a [`Task`] and its
//! answer something a message holds.
//!
//! Samples and measures travel as their bytes beside the message's text, never as JSON
//! numbers, so an answer from a worker is bit for bit the answer the page would have
//! found itself.

use eframe::egui;
use nord_format::take::clean::Noise;
use nord_format::take::hum::Hum;
use nord_format::wav::Audio;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{Job, Progress};
use crate::store::wire;

/// Every piece of work [`hand`] runs off the page, by name.
#[derive(Serialize, Deserialize)]
pub enum Task {
    Noise(FindNoise),
}

/// A piece of work [`hand`] runs: a [`Task`] and what it answers.
pub trait Work: Into<Task> + Send + 'static {
    type Answer: Serialize + DeserializeOwned + Send + 'static;

    fn run(self, progress: &Progress) -> Self::Answer;
}

/// Start `work`, and request a repaint when it answers: on a thread on the desktop, in a
/// worker of its own in the browser, so the page stays live and the work's memory is not
/// the page's. Dropping the job there ends the worker.
pub fn hand<W: Work>(ctx: &egui::Context, work: W) -> Job<W::Answer> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        super::run(ctx, move |progress| work.run(progress))
    }
    #[cfg(target_arch = "wasm32")]
    {
        super::worker::start(ctx, &work.into())
    }
}

/// `task` run, and its answer as a message, as a worker posts it back.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn answer(task: Task, progress: &Progress) -> Result<wire::Message, String> {
    match task {
        Task::Noise(work) => wire::encode(&work.run(progress)),
    }
}

/// Find the noise of a take, as [`Noise::of`] finds it in its [`mono`] sum.
///
/// [`mono`]: nord_format::take::plan::mono
#[derive(Serialize, Deserialize)]
pub struct FindNoise {
    rate: u32,
    channels: u16,
    /// The take's samples, as their little-endian bytes: the one copy of them made to
    /// hand over, which the message takes.
    samples: wire::Handed,
}

impl FindNoise {
    pub fn of(audio: &Audio) -> FindNoise {
        let bytes = audio.samples.iter().flat_map(|v| v.to_le_bytes()).collect();
        FindNoise {
            rate: audio.rate,
            channels: audio.channels,
            samples: wire::Handed::new(bytes),
        }
    }
}

/// [`nord_format::take::plan::mono`] of samples held as their bytes, summed in the same
/// order, so to the bit what it gives.
fn mono(bytes: &[u8], channels: u16) -> Vec<f64> {
    let channels = usize::from(channels.max(1));
    bytes
        .chunks_exact(4 * channels)
        .map(|frame| {
            let sum: f64 = frame
                .chunks_exact(4)
                .map(|b| f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]])))
                .sum();
            sum / channels as f64
        })
        .collect()
}

impl From<FindNoise> for Task {
    fn from(work: FindNoise) -> Task {
        Task::Noise(work)
    }
}

impl Work for FindNoise {
    type Answer = FoundNoise;

    fn run(self, progress: &Progress) -> FoundNoise {
        progress.say("finding the take's hum and hiss");
        let bytes = self.samples.into_inner().unwrap_or_default();
        let mono = mono(&bytes, self.channels);
        drop(bytes);
        FoundNoise(Noise::of(&mono, self.rate))
    }
}

/// A take's [`Noise`], as a message holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct FoundNoise(pub Noise);

/// [`FoundNoise`] on the wire: each measure as its bytes.
#[derive(Serialize, Deserialize)]
struct NoiseWire {
    rate: u32,
    hum: Option<HumWire>,
    hiss: Option<F64s>,
}

#[derive(Serialize, Deserialize)]
struct HumWire {
    mains: u32,
    /// Each harmonic's number, and beside it in `over` how far it stands out, in dB.
    harmonics: Vec<u32>,
    over: F64s,
    /// The track's seconds and Hz, in turn.
    track: F64s,
}

impl Serialize for FoundNoise {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let Noise { rate, hum, hiss } = &self.0;
        let hum = hum.as_ref().map(|hum| HumWire {
            mains: hum.mains,
            harmonics: hum.harmonics.iter().map(|&(n, _)| n).collect(),
            over: F64s(hum.harmonics.iter().map(|&(_, db)| db).collect()),
            track: F64s(hum.track.iter().flat_map(|&(at, hz)| [at, hz]).collect()),
        });
        let wire = NoiseWire {
            rate: *rate,
            hum,
            hiss: hiss.clone().map(F64s),
        };
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for FoundNoise {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<FoundNoise, D::Error> {
        use serde::de::Error as _;
        let wire = NoiseWire::deserialize(deserializer)?;
        let hum = match wire.hum {
            None => None,
            Some(hum) => {
                if hum.harmonics.len() != hum.over.0.len() || hum.track.0.len() % 2 != 0 {
                    return Err(D::Error::custom("a hum whose measures do not pair up"));
                }
                Some(Hum {
                    mains: hum.mains,
                    harmonics: hum.harmonics.into_iter().zip(hum.over.0).collect(),
                    track: hum.track.0.chunks_exact(2).map(|p| (p[0], p[1])).collect(),
                })
            }
        };
        Ok(FoundNoise(Noise {
            rate: wire.rate,
            hum,
            hiss: wire.hiss.map(|hiss| hiss.0),
        }))
    }
}

/// Measures handed over as their little-endian bytes beside a message's text.
struct F64s(Vec<f64>);

impl Serialize for F64s {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let bytes: Vec<u8> = self.0.iter().flat_map(|v| v.to_le_bytes()).collect();
        wire::bytes::serialize(&bytes, serializer)
    }
}

impl<'de> Deserialize<'de> for F64s {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<F64s, D::Error> {
        let bytes = wire::bytes::deserialize(deserializer)?;
        let whole = bytes.chunks_exact(8);
        if !whole.remainder().is_empty() {
            return Err(serde::de::Error::custom("measures cut short"));
        }
        Ok(F64s(
            whole
                .map(|b| f64::from_le_bytes(b.try_into().expect("8 bytes")))
                .collect(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A take with a hum and a hiss to find, as nord-format's own hum tests make one:
    /// 12 s of hiss near -90 dB, the 3rd and 5th harmonics of 50 Hz mains at -70 and
    /// -76 dB, and a strike at 3 s that decays, in stereo.
    fn take() -> Audio {
        let rate = 44_100;
        let frames = rate as usize * 12;
        let db = |db: f64| 10f64.powf(db / 20.0);
        let mut seed = 0x2545_f491_u32;
        let mut samples = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let t = i as f64 / f64::from(rate);
            let phase = std::f64::consts::TAU * 50.0 * t;
            let hum = db(-70.0) * (3.0 * phase).sin() + db(-76.0) * (5.0 * phase).sin();
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            // Uniform over a width of sqrt(12) x the level is that level's power.
            let hiss = (f64::from(seed) / f64::from(u32::MAX) - 0.5) * 12f64.sqrt() * db(-90.0);
            let strike = if t >= 3.0 {
                db(-10.0) * (-(t - 3.0) * 1.5).exp() * (std::f64::consts::TAU * 220.0 * t).sin()
            } else {
                0.0
            };
            let v = (hum + hiss + strike) as f32;
            samples.extend([v, v * 0.9]);
        }
        Audio {
            rate,
            channels: 2,
            samples,
        }
    }

    #[test]
    fn a_take_s_noise_found_through_a_message_is_the_noise_found_in_place() {
        let audio = take();
        let here = Noise::of(&nord_format::take::plan::mono(&audio), audio.rate);
        assert!(
            here.hum.is_some() && here.hiss.is_some(),
            "the take has both"
        );

        let sent = wire::encode(&Task::from(FindNoise::of(&audio))).unwrap();
        let task: Task = wire::decode(sent).unwrap();
        let answered = answer(task, &Progress::default()).unwrap();
        let FoundNoise(there) = wire::decode(answered).unwrap();

        assert_eq!(there, here);
        let bits = |noise: &Noise| -> Vec<u64> {
            let hum = noise.hum.as_ref().unwrap();
            let hiss = noise.hiss.as_ref().unwrap();
            let over = hum.harmonics.iter().map(|&(_, db)| db);
            let track = hum.track.iter().flat_map(|&(at, hz)| [at, hz]);
            over.chain(track)
                .chain(hiss.iter().copied())
                .map(f64::to_bits)
                .collect()
        };
        assert_eq!(bits(&there), bits(&here), "every measure, to the bit");
    }

    #[test]
    fn a_take_s_mono_from_its_bytes_is_nord_format_s_mono_to_the_bit() {
        for channels in [1u16, 2, 3] {
            let samples: Vec<f32> = (0..3_000)
                .map(|i| ((i * 37) % 101) as f32 / 97.0 - 0.5)
                .collect();
            let audio = Audio {
                rate: 48_000,
                channels,
                samples,
            };
            let bytes: Vec<u8> = audio.samples.iter().flat_map(|v| v.to_le_bytes()).collect();
            let bits = |m: Vec<f64>| m.into_iter().map(f64::to_bits).collect::<Vec<_>>();
            assert_eq!(
                bits(mono(&bytes, channels)),
                bits(nord_format::take::plan::mono(&audio)),
                "{channels} channels"
            );
        }
    }

    #[test]
    fn a_take_with_no_hum_or_hiss_found_through_a_message_has_none() {
        let none = FoundNoise(Noise {
            rate: 48_000,
            hum: None,
            hiss: None,
        });
        let back: FoundNoise = wire::decode(wire::encode(&none).unwrap()).unwrap();
        assert_eq!(back, none);
    }
}
