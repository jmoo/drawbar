//! The takes drawbar reads: the browser's size limit, and the rates and lengths the
//! detectors and cues take.

use super::*;

/// A megabyte as the computer's own file sizes count one.
pub(super) const MB: usize = 1 << 20;

/// The largest WAV drawbar-web opens, in bytes. Opening a take costs the page about nine
/// times its file in memory, and finding its noise as much again in its worker (a 132 MB
/// take of 522 s, 24-bit stereo at 44.1 kHz, takes 1.2 GB each), and a browser tab gets
/// at most 4 GB, often far less. A WAV of a few notes is
/// far smaller: past this, a take is for the desktop.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(super) const WEB_MOST_BYTES: usize = 120 * MB;

/// The lowest rate a take is read at, in Hz: the detectors filter, find pitch and find
/// noise up to frequencies a lower rate cannot hold, and the build resamples every take
/// to 35,002 Hz anyway.
pub(super) const LEAST_RATE: u32 = 8_000;

/// `audio`, or why a take at its rate, or of its length, is not read. A cue marks a
/// frame as 32 bits, so the lanes count a take's frames so too.
pub(super) fn heard_rate(audio: Audio) -> Result<Audio, String> {
    if audio.rate < LEAST_RATE {
        return Err(format!(
            "this WAV is at {} Hz; a take needs at least {LEAST_RATE} Hz",
            audio.rate
        ));
    }
    match u32::try_from(audio.frames()) {
        Ok(_) => Ok(audio),
        Err(_) => Err(format!(
            "this WAV holds {} frames, more than a cue can mark",
            audio.frames()
        )),
    }
}

/// Why drawbar-web works on no WAV of `len` bytes, where it is too large for the
/// browser: opened, or built into a library. None on the desktop.
pub(crate) fn web_refusal(len: usize) -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    return too_large(len, WEB_MOST_BYTES);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = len;
        None
    }
}

/// Why a WAV of `len` bytes is not opened, where it is more than `most`.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(super) fn too_large(len: usize, most: usize) -> Option<String> {
    (len > most).then(|| {
        // Whole megabytes cut down, as the computer's own file sizes show them, unless
        // that would show the file no larger than the limit.
        let size = match len / MB > most / MB {
            true => format!("{}", len / MB),
            false => format!("{:.1}", (len as f64 / MB as f64 * 10.0).floor() / 10.0),
        };
        format!(
            "This WAV is {size} MB, more than the browser can work on ({} MB). Use the Desktop version of Drawbar instead.",
            most / MB
        )
    })
}
