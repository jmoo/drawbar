//! A take's own settings, which no other WAV reader knows: the kind of instrument it is
//! heard and built as, and whether the build cleans it. They sit in a chunk of their
//! own, `npno`, as `key=value` lines, which other readers skip. Saving replaces that
//! chunk whole and copies every other byte of the file as it was.

use super::marks::push_chunk;
use super::{chunks, layout, only};
use crate::error::{malformed, out_of_bounds, Error};

/// The chunk the settings sit in.
const ID: &[u8; 4] = b"npno";

/// What a take states about how it is built, each `None` where it states nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    /// The kind of instrument, as `nord piano build --kind` names it: `wurlitzer`.
    pub kind: Option<String>,
    /// Whether the build cleans the take's regions of its hum and hiss.
    pub clean: Option<bool>,
    /// The `key=value` lines this version does not know, in their order, written back
    /// after the ones it does.
    pub others: Vec<String>,
}

impl Settings {
    fn body(&self) -> Vec<u8> {
        let mut lines = String::new();
        if let Some(kind) = &self.kind {
            lines.push_str(&format!("kind={kind}\n"));
        }
        if let Some(clean) = self.clean {
            lines.push_str(&format!("clean={}\n", if clean { "on" } else { "off" }));
        }
        for line in &self.others {
            lines.push_str(line);
            lines.push('\n');
        }
        lines.into_bytes()
    }

    fn is_empty(&self) -> bool {
        *self == Settings::default()
    }
}

/// The settings `bytes` states, none where it has no `npno` chunk.
///
/// A line this version does not know is kept in [`Settings::others`], so a later one's
/// settings neither stop a take from reading nor are lost when it saves; a known one
/// that does not read is refused.
pub fn read_settings(bytes: &[u8]) -> Result<Settings, Error> {
    let chunks = chunks(bytes)?;
    let Some(chunk) = only(&chunks, ID)? else {
        return Ok(Settings::default());
    };
    let Ok(text) = std::str::from_utf8(&bytes[chunk.body.clone()]) else {
        return malformed("npno chunk is not UTF-8 text");
    };
    let mut settings = Settings::default();
    for line in text.lines().filter(|line| !line.is_empty()) {
        let Some((key, value)) = line.split_once('=') else {
            return malformed(format!("npno chunk line {line:?} is not key=value"));
        };
        match key {
            "kind" => settings.kind = Some(value.to_string()),
            "clean" => {
                settings.clean = Some(match value {
                    "on" => true,
                    "off" => false,
                    _ => return malformed(format!("npno chunk says clean={value}, not on or off")),
                })
            }
            _ => settings.others.push(line.to_string()),
        }
    }
    Ok(settings)
}

/// `bytes` with its `npno` chunk replaced by one stating `settings`, written after every
/// other chunk, which is copied as it was. Settings that state nothing write no chunk.
pub fn with_settings(bytes: &[u8], settings: &Settings) -> Result<Vec<u8>, Error> {
    layout(bytes)?;
    if let Some(kind) = &settings.kind {
        if kind.is_empty() || kind.contains(['\n', '\r', '=']) {
            return out_of_bounds(format!("kind {kind:?}"), "a name on one line, with no =");
        }
    }
    if let Some(line) = settings
        .others
        .iter()
        .find(|line| !line.contains('=') || line.contains(['\n', '\r']))
    {
        return out_of_bounds(format!("npno line {line:?}"), "key=value on one line");
    }
    let chunks = chunks(bytes)?;
    only(&chunks, ID)?;
    let mut out = bytes[..12].to_vec();
    for chunk in chunks.iter().filter(|chunk| &chunk.id != ID) {
        out.extend_from_slice(&bytes[chunk.whole()]);
    }
    if !settings.is_empty() {
        push_chunk(&mut out, ID, &settings.body());
    }
    let Ok(riff) = u32::try_from(out.len() - 8) else {
        return out_of_bounds(
            format!("{} bytes", out.len()),
            "a WAV whose RIFF length fits u32",
        );
    };
    out[4..8].copy_from_slice(&riff.to_le_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wav::{read, read_marks, with_marks, write, Audio, Encoding, Marks, Region};

    fn take() -> Vec<u8> {
        let audio = Audio {
            rate: 35_002,
            channels: 2,
            samples: (0..2000).map(|i| (i as f32 / 2000.0) - 0.5).collect(),
        };
        let bytes = write(&audio, Encoding::Int { bits: 24 }).unwrap();
        let marks = Marks {
            regions: vec![Region {
                start: 10,
                frames: 100,
                label: "060-b0-v22".into(),
            }],
            loops: Vec::new(),
        };
        with_marks(&bytes, &marks).unwrap()
    }

    #[test]
    fn settings_are_written_beside_the_marks_and_read_back_with_the_audio_untouched() {
        let bytes = take();
        assert_eq!(read_settings(&bytes).unwrap(), Settings::default());
        let settings = Settings {
            kind: Some("wurlitzer".into()),
            clean: Some(false),
            ..Settings::default()
        };
        let saved = with_settings(&bytes, &settings).unwrap();
        assert_eq!(read_settings(&saved).unwrap(), settings);
        assert_eq!(read(&saved).unwrap(), read(&bytes).unwrap());
        assert_eq!(read_marks(&saved).unwrap(), read_marks(&bytes).unwrap());

        let again = with_settings(
            &saved,
            &Settings {
                clean: Some(true),
                ..settings
            },
        )
        .unwrap();
        assert_eq!(read_settings(&again).unwrap().clean, Some(true));
        assert_eq!(
            again.windows(4).filter(|id| id == ID).count(),
            1,
            "replaced, not added"
        );
        assert_eq!(with_settings(&again, &Settings::default()).unwrap(), bytes);
    }

    #[test]
    fn a_line_a_later_version_wrote_does_not_stop_a_read_and_a_bad_one_is_refused() {
        let bytes = take();
        let mut odd = bytes[..].to_vec();
        push_chunk(&mut odd, ID, b"kind=grand\ntempo=fast\n");
        let riff = (odd.len() - 8) as u32;
        odd[4..8].copy_from_slice(&riff.to_le_bytes());
        assert_eq!(read_settings(&odd).unwrap().kind.as_deref(), Some("grand"));

        let mut bad = bytes[..].to_vec();
        push_chunk(&mut bad, ID, b"clean=maybe\n");
        let riff = (bad.len() - 8) as u32;
        bad[4..8].copy_from_slice(&riff.to_le_bytes());
        assert!(read_settings(&bad).is_err());
        let refused = Settings {
            kind: Some("a=b".into()),
            ..Settings::default()
        };
        assert!(with_settings(&bytes, &refused).is_err());
        for line in ["tempo", "tempo=fast\nmood=calm"] {
            let refused = Settings {
                others: vec![line.into()],
                ..Settings::default()
            };
            assert!(with_settings(&bytes, &refused).is_err(), "{line:?}");
        }
    }

    #[test]
    fn lines_a_later_version_wrote_are_saved_again_in_their_order() {
        let mut odd = take();
        push_chunk(&mut odd, ID, b"tempo=fast\nkind=grand\nmood=calm\n");
        let riff = (odd.len() - 8) as u32;
        odd[4..8].copy_from_slice(&riff.to_le_bytes());

        let mut read = read_settings(&odd).unwrap();
        read.clean = Some(true);
        let saved = with_settings(&odd, &read).unwrap();
        let chunks = chunks(&saved).unwrap();
        let body = only(&chunks, ID).unwrap().unwrap().body.clone();
        assert_eq!(
            &saved[body],
            b"kind=grand\nclean=on\ntempo=fast\nmood=calm\n"
        );
    }
}
