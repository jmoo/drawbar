//! A [`Transport`] that replays a recorded exchange instead of touching hardware.
//!
//! The protocol layer can then run anywhere, including under Wine, qemu, and wasm, with
//! no device attached. [`ReplayTransport::sent`] returns what an operation put on the
//! wire, so a test can compare it against a real capture.
//!
//! The script is a flat list of [`Step`]s in the order they occurred. The host's steps
//! are checked against what the code under test sends; the device's are fed back as
//! its responses. Every read and write must meet a step of its own direction, so a
//! read the script does not expect is a mismatch, never a timeout.
//!
//! # Script format
//!
//! One step per line, which may carry a trailing `# label` comment:
//!
//! | Line | Step |
//! |---|---|
//! | `O <hex>` | the host sent a frame |
//! | `O timeout <hex>` | the host offered a frame, and the device did not accept it in time |
//! | `O error <hex>` | the transport failed sending a frame |
//! | `I <hex>` | the device sent a frame |
//! | `I timeout` | nothing arrived within the read's limit |
//! | `I error` | the transport failed reading |
//!
//! Every other `#` line is either prose or a field, `# <key>: <value>` with the key in
//! `[a-z_]+`, so a prose line whose first word is capitalized or hyphenated stays prose.
//! An unknown lowercase key is an error, so the vocabulary cannot drift.
//!
//! `source`, `device`, `trimmed`, `note`, `driven_by`, and `undriven` describe the file
//! and must precede its first step. `intent` and `expect` describe a section. `intent`
//! opens one, which runs to the next `intent` or the end of the file, so a recorded
//! command that opened several transactions is one script of several sections, in
//! order. `expect` names the outcome its section must produce and defaults to `ok`. It
//! may sit anywhere in that section, because a recorder only learns the outcome after
//! the steps are written.

use super::Transport;
use crate::error::{Error, Result};

pub use crate::error::ErrKind;

/// Where a script's bytes came from, which says whether it is an oracle.
///
/// Only [`Source::Nsm`], the vendor application's own traffic, is an oracle for an
/// operation nothing has matched before. [`Source::Nord`] is this project's traffic, a
/// regression baseline; [`Source::Synthetic`] was built by hand for a path no capture
/// covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Nsm,
    Nord,
    Synthetic,
}

/// The file-level fields of a script header. All optional.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Header {
    pub source: Option<Source>,
    /// Free text: the model and firmware the capture was taken from, or the model a
    /// synthetic script imitates.
    pub device: Option<String>,
    /// What was left out of the capture, e.g. `ui-refresh`.
    pub trimmed: Option<String>,
    pub note: Option<String>,
    /// The test files, comma-separated and relative to this crate, that drive a script
    /// declaring no intent.
    pub driven_by: Option<String>,
    /// Why nothing drives a script that declares no intent.
    pub undriven: Option<String>,
}

/// The outcome a section's declared intent must produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Expect {
    #[default]
    Ok,
    Err(ErrKind),
}

/// One declared intent and the steps it accounts for.
#[derive(Debug, Clone, Default)]
pub struct Section {
    /// `<class> <verb> <args…>`, in the CLI's own spellings. `None` means the section
    /// declares nothing, so its steps can be checked but not driven.
    pub intent: Option<String>,
    /// What the section says to expect, if anything. Read through [`Section::expect`],
    /// which supplies the default.
    expect: Option<Expect>,
    pub steps: Vec<Step>,
}

impl Section {
    /// The outcome this section must produce. A section that says nothing expects `ok`.
    pub fn expect(&self) -> Expect {
        self.expect.unwrap_or_default()
    }
}

/// A parsed script: its file-level header, and its sections in wire order.
#[derive(Debug, Clone)]
pub struct Script {
    pub header: Header,
    pub sections: Vec<Section>,
}

/// The header keys a script may carry, listed when one is misspelled.
pub const KEYS: &[&str] = &[
    "intent",
    "expect",
    "source",
    "device",
    "trimmed",
    "note",
    "driven_by",
    "undriven",
];

impl Source {
    fn parse(value: &str) -> std::result::Result<Self, String> {
        match value {
            "nsm" => Ok(Source::Nsm),
            "nord" => Ok(Source::Nord),
            "synthetic" => Ok(Source::Synthetic),
            other => Err(format!(
                "unknown source {other:?}; the vocabulary is nsm, nord, synthetic"
            )),
        }
    }
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Source::Nsm => "nsm",
            Source::Nord => "nord",
            Source::Synthetic => "synthetic",
        })
    }
}

impl Expect {
    fn parse(value: &str) -> std::result::Result<Self, String> {
        match value.strip_prefix("err") {
            None if value == "ok" => Ok(Expect::Ok),
            None => Err(format!("expected 'ok' or 'err <kind>', got {value:?}")),
            Some(rest) => ErrKind::parse(rest.trim()).map(Expect::Err),
        }
    }

    /// Judge an outcome against what was declared, describing the mismatch.
    pub fn check<T>(&self, outcome: &Result<T>) -> std::result::Result<(), String> {
        match (self, outcome) {
            (Expect::Ok, Ok(_)) => Ok(()),
            (Expect::Ok, Err(e)) => Err(format!("expected ok, got {e}")),
            (Expect::Err(kind), Ok(_)) => Err(format!("expected {kind}, but it succeeded")),
            (Expect::Err(kind), Err(e)) if kind.matches(e) => Ok(()),
            (Expect::Err(kind), Err(e)) => Err(format!("expected {kind}, got {e}")),
        }
    }
}

impl std::fmt::Display for Expect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Expect::Ok => f.write_str("ok"),
            Expect::Err(kind) => write!(f, "err {kind}"),
        }
    }
}

/// A header field, or `None` for prose. The key must be `[a-z_]+`, which keeps an
/// ordinary sentence containing a colon from being read as a field.
fn field(comment: &str) -> Option<(&str, &str)> {
    let (key, value) = comment.trim().split_once(':')?;
    let named = !key.is_empty() && key.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
    named.then(|| (key, value.trim()))
}

fn frame(hex: &str) -> std::result::Result<Vec<u8>, String> {
    // ⚠️ The pairs below are byte slices: a multi-byte character would be split across
    // a char boundary and panic before `from_str_radix` saw it.
    if !hex.is_ascii() {
        return Err("non-hex byte".into());
    }
    if !hex.len().is_multiple_of(2) {
        return Err("odd-length hex".into());
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
        .collect::<std::result::Result<Vec<u8>, _>>()
        .map_err(|e| e.to_string())
}

impl Step {
    /// One step line, its trailing label already removed.
    fn parse(line: &str) -> std::result::Result<Self, String> {
        let words: Vec<&str> = line.split_whitespace().collect();
        Ok(match words.as_slice() {
            ["O", "timeout", hex] => Step::OutTimeout(frame(hex)?),
            ["O", "error", hex] => Step::OutError(frame(hex)?),
            ["O", hex] if !matches!(*hex, "timeout" | "error") => Step::Out(frame(hex)?),
            ["I", "timeout"] => Step::InTimeout,
            ["I", "error"] => Step::InError,
            ["I", hex] => Step::In(frame(hex)?),
            [tag @ ("O" | "I"), ..] => {
                return Err(format!(
                    "expected '{tag} <hex>', '{tag} timeout', or '{tag} error'"
                ))
            }
            [other, ..] => return Err(format!("unknown direction {other:?}, want O or I")),
            [] => unreachable!("blank lines are skipped"),
        })
    }

    /// The frame the step carries, whether or not it crossed the wire.
    pub fn frame(&self) -> Option<&[u8]> {
        match self {
            Step::Out(b) | Step::OutTimeout(b) | Step::OutError(b) | Step::In(b) => Some(b),
            Step::InTimeout | Step::InError => None,
        }
    }
}

impl Script {
    /// Parse a script: header fields, sections, and steps.
    ///
    /// An unknown key, a file-level field after the first step, a second `expect` in one
    /// section, an `expect` without an intent, and a section with no steps are all
    /// errors, so a header that does nothing cannot sit unnoticed in a tree the sweep
    /// walks.
    pub fn parse(text: &str) -> Result<Self> {
        let fail =
            |n: usize, what: std::fmt::Arguments| Error::Replay(format!("line {}: {what}", n + 1));
        let mut header = Header::default();
        let mut sections = vec![Section::default()];
        let mut seen_step = false;

        for (n, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(comment) = line.strip_prefix('#') {
                let Some((key, value)) = field(comment) else {
                    continue;
                };
                let section = sections.last_mut().expect("one section always exists");
                let text_field = match key {
                    "intent" => {
                        if section.intent.is_none() && section.steps.is_empty() {
                            section.intent = Some(value.to_string());
                        } else {
                            sections.push(Section {
                                intent: Some(value.to_string()),
                                ..Section::default()
                            });
                        }
                        continue;
                    }
                    "expect" => {
                        if section.expect.is_some() {
                            return Err(fail(
                                n,
                                format_args!("this section already says what to expect"),
                            ));
                        }
                        section.expect =
                            Some(Expect::parse(value).map_err(|e| fail(n, format_args!("{e}")))?);
                        continue;
                    }
                    _ if !KEYS.contains(&key) => {
                        return Err(fail(
                            n,
                            format_args!(
                                "unknown header key {key:?}; the vocabulary is {}",
                                KEYS.join(", ")
                            ),
                        ))
                    }
                    _ if seen_step => {
                        return Err(fail(
                            n,
                            format_args!(
                                "{key} describes the file and must come before its first step"
                            ),
                        ))
                    }
                    "source" => {
                        let source =
                            Source::parse(value).map_err(|e| fail(n, format_args!("{e}")))?;
                        if header.source.replace(source).is_some() {
                            return Err(fail(n, format_args!("source is given twice")));
                        }
                        continue;
                    }
                    "device" => &mut header.device,
                    "trimmed" => &mut header.trimmed,
                    "note" => &mut header.note,
                    "driven_by" => &mut header.driven_by,
                    "undriven" => &mut header.undriven,
                    _ => unreachable!("every key in KEYS is matched"),
                };
                if text_field.replace(value.into()).is_some() {
                    return Err(fail(n, format_args!("{key} is given twice")));
                }
                continue;
            }

            // A trailing `# label` says what the step is; nothing reads it back.
            let step = match line.split_once('#') {
                Some((step, _)) => step,
                None => line,
            };
            let step = Step::parse(step).map_err(|e| fail(n, format_args!("{e}")))?;
            seen_step = true;
            sections
                .last_mut()
                .expect("one section always exists")
                .steps
                .push(step);
        }

        for section in &sections {
            if section.steps.is_empty() {
                let what = match &section.intent {
                    Some(intent) => format!("intent {intent:?} accounts for no steps"),
                    None => "the script holds no steps".into(),
                };
                return Err(Error::Replay(what));
            }
            if section.intent.is_none() && section.expect.is_some() {
                return Err(Error::Replay(
                    "expect without an intent: nothing would be driven, so nothing could \
                     produce it"
                        .into(),
                ));
            }
        }
        Ok(Self { header, sections })
    }

    /// Every step, sections joined, in wire order.
    pub fn steps(&self) -> Vec<Step> {
        self.sections
            .iter()
            .flat_map(|s| s.steps.iter().cloned())
            .collect()
    }
}

/// One line of a script: a frame that crossed the wire, or a transfer that did not.
///
/// A timeout answers only a transfer that carried a limit: [`Transport::read`] and
/// [`Transport::write`] would wait forever for what the step says never came, so a
/// replay refuses them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// `O <hex>`: the host sent a frame.
    Out(Vec<u8>),
    /// `O timeout <hex>`: the device did not accept the frame within the write's limit.
    OutTimeout(Vec<u8>),
    /// `O error <hex>`: the transport failed sending the frame.
    OutError(Vec<u8>),
    /// `I <hex>`: the device sent a frame.
    In(Vec<u8>),
    /// `I timeout`: nothing arrived within the read's limit.
    InTimeout,
    /// `I error`: the transport failed reading.
    InError,
}

/// How strictly to police what the code under test transmits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strictness {
    /// Every host step must match the script byte for byte. Use in tests.
    Exact,
    /// Ignore what is sent and serve the device's next step. Useful for demos against a
    /// capture whose addressing differs from what is being asked for.
    Lenient,
}

pub struct ReplayTransport {
    script: Vec<Step>,
    pos: usize,
    sent: Vec<Vec<u8>>,
    strictness: Strictness,
    mismatch: Option<String>,
}

impl ReplayTransport {
    pub fn new(script: Vec<Step>) -> Self {
        Self {
            script,
            pos: 0,
            sent: Vec::new(),
            strictness: Strictness::Exact,
            mismatch: None,
        }
    }

    pub fn lenient(mut self) -> Self {
        self.strictness = Strictness::Lenient;
        self
    }

    /// Everything the code under test transmitted or offered, in order.
    pub fn sent(&self) -> &[Vec<u8>] {
        &self.sent
    }

    /// Whether the whole script was consumed with no mismatch on the way. A test that
    /// leaves steps unread has usually stopped short of the behavior it meant to check.
    pub fn is_exhausted(&self) -> bool {
        self.pos >= self.script.len() && self.mismatch.is_none()
    }

    /// The first time the code under test and the script disagreed, even if the code
    /// swallowed the error, as a best-effort release does.
    pub fn mismatch(&self) -> Option<&str> {
        self.mismatch.as_deref()
    }

    fn noting<T>(&mut self, result: Result<T>) -> Result<T> {
        if let Err(Error::Replay(what)) = &result {
            self.mismatch.get_or_insert_with(|| what.clone());
        }
        result
    }

    /// How many steps have been consumed. With the section boundaries of a [`Script`]
    /// this says which intent stopped short.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Replay every step of a script, whatever its header declares.
    pub fn from_script(text: &str) -> Result<Self> {
        Ok(Self::new(Script::parse(text)?.steps()))
    }

    /// Meet a write with the host's next step. `Ok(false)` is a frame the device did not
    /// accept, which only a write with a limit can report.
    fn take_out(&mut self, buf: &[u8], limited: bool) -> Result<bool> {
        self.sent.push(buf.to_vec());
        let at = self.pos;
        let (expected, accepted) = match self.script.get(at) {
            Some(Step::Out(b)) => (b, Ok(true)),
            Some(Step::OutTimeout(b)) if limited => (b, Ok(false)),
            Some(Step::OutError(b)) => (b, Err(replayed_failure(at))),
            Some(Step::OutTimeout(_)) => {
                return Err(Error::Replay(format!(
                    "step {at}: the device never accepts this frame, and a write without a \
                     limit would wait forever"
                )))
            }
            Some(Step::In(_) | Step::InTimeout | Step::InError) => {
                return Err(Error::Replay(format!(
                    "step {at}: host wrote, but the script expects the host to read next"
                )))
            }
            None => {
                return Err(Error::Replay(format!(
                    "script exhausted; host sent an extra {} bytes",
                    buf.len()
                )))
            }
        };
        if self.strictness == Strictness::Exact && expected != buf {
            return Err(Error::Replay(format!(
                "sent bytes differ from the script at step {at}\n  expected {}\n  got      {}",
                hex(expected),
                hex(buf),
            )));
        }
        self.pos += 1;
        accepted
    }

    /// Meet a read with the device's next step. `Ok(None)` is silence, which only a read
    /// with a limit can report.
    fn take_in(&mut self, max: usize, limited: bool) -> Result<Option<Vec<u8>>> {
        let at = self.pos;
        let read = match self.script.get(at) {
            Some(Step::In(b)) if b.len() > max => {
                return Err(Error::Replay(format!(
                    "step {at}: device sent {} bytes, but the read buffer holds at most {max}",
                    b.len()
                )))
            }
            Some(Step::In(b)) => Ok(Some(b.clone())),
            Some(Step::InTimeout) if limited => Ok(None),
            Some(Step::InError) => Err(replayed_failure(at)),
            Some(Step::InTimeout) => {
                return Err(Error::Replay(format!(
                    "step {at}: the device says nothing, and a read without a limit would \
                     wait forever"
                )))
            }
            Some(Step::Out(_) | Step::OutTimeout(_) | Step::OutError(_)) => {
                return Err(Error::Replay(format!(
                    "step {at}: host read, but the script expects the host to write next"
                )))
            }
            None => {
                return Err(Error::Replay(
                    "script exhausted; host expected a response".into(),
                ))
            }
        };
        self.pos += 1;
        read
    }
}

fn replayed_failure(at: usize) -> Error {
    Error::Transport(format!("the transport failed at step {at}, as recorded"))
}

impl Transport for ReplayTransport {
    async fn write(&mut self, buf: &[u8]) -> Result<()> {
        let written = self.take_out(buf, false);
        self.noting(written).map(|_| ())
    }

    async fn write_timeout(&mut self, buf: &[u8], _limit: std::time::Duration) -> Result<bool> {
        let written = self.take_out(buf, true);
        self.noting(written)
    }

    async fn read(&mut self, max: usize) -> Result<Vec<u8>> {
        let read = self.take_in(max, false);
        self.noting(read)
            .map(|read| read.expect("an unlimited read never replays silence"))
    }

    async fn read_timeout(
        &mut self,
        max: usize,
        _limit: std::time::Duration,
    ) -> Result<Option<Vec<u8>>> {
        let read = self.take_in(max, true);
        self.noting(read)
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two lines every recording starts with are prose: the key of a field is
    /// `[a-z_]+`, and `Format` is capitalized.
    #[test]
    fn a_bare_recording_parses_as_one_section_with_no_intent() {
        let script = Script::parse(
            "# nord-usb replay script, recorded from hardware.\n\
             # Format: '<O|I> <hex>' -- O = host->device, I = device->host.\n\
             O 00\n\
             I 0102\n",
        )
        .unwrap();
        assert_eq!(script.header, Header::default());
        assert_eq!(script.sections.len(), 1);
        assert!(script.sections[0].intent.is_none());
        assert_eq!(script.sections[0].expect(), Expect::Ok);
        assert_eq!(script.steps().len(), 2);
    }

    #[test]
    fn the_file_level_fields_are_read() {
        let script = Script::parse(
            "# source: nsm\n\
             # device: Nord Electro 5, firmware v2.04\n\
             # trimmed: ui-refresh\n\
             # note: the dependency read from the duplicate capture\n\
             # intent: program deps 7:3\n\
             O 00\n",
        )
        .unwrap();
        assert_eq!(script.header.source, Some(Source::Nsm));
        assert_eq!(script.header.trimmed.as_deref(), Some("ui-refresh"));
        assert_eq!(
            script.sections[0].intent.as_deref(),
            Some("program deps 7:3")
        );
    }

    #[test]
    fn each_intent_opens_a_section_over_the_frames_that_follow() {
        let script = Script::parse(
            "# source: nord\n\
             # intent: program info 7:11\n\
             O 00\n\
             I 01\n\
             # intent: program info 7:12\n\
             # expect: err device-status 0x1\n\
             O 02\n\
             # intent: program move 7:11 7:12\n\
             O 03\n\
             I 04\n",
        )
        .unwrap();
        let intents: Vec<&str> = script
            .sections
            .iter()
            .map(|s| s.intent.as_deref().unwrap())
            .collect();
        assert_eq!(
            intents,
            [
                "program info 7:11",
                "program info 7:12",
                "program move 7:11 7:12"
            ]
        );
        assert_eq!(
            script
                .sections
                .iter()
                .map(|s| s.steps.len())
                .collect::<Vec<_>>(),
            [2, 1, 2]
        );
        assert_eq!(
            script.sections[1].expect(),
            Expect::Err(ErrKind::DeviceStatus(1))
        );
        assert_eq!(script.sections[2].expect(), Expect::Ok);
    }

    /// A device refusal is only useful if the script can name which one, so the code
    /// survives the round trip through the header.
    #[test]
    fn a_device_status_expectation_round_trips_its_code() {
        for (text, code) in [("err device-status 0x15", 0x15), ("err device-status 1", 1)] {
            let expect = Expect::parse(text).unwrap();
            assert_eq!(expect, Expect::Err(ErrKind::DeviceStatus(code)));
            assert!(expect.check::<()>(&Err(Error::DeviceStatus(code))).is_ok());
            assert!(expect
                .check::<()>(&Err(Error::DeviceStatus(code + 1)))
                .is_err());
        }
        assert_eq!(
            Expect::Err(ErrKind::DeviceStatus(0x15)).to_string(),
            "err device-status 0x15"
        );
    }

    #[test]
    fn an_expectation_is_judged_against_the_outcome() {
        let unexpected = Expect::parse("err unexpected-response").unwrap();
        assert!(unexpected
            .check::<()>(&Err(Error::UnexpectedResponse {
                expected: 0x30,
                got: 0x1f
            }))
            .is_ok());
        assert!(unexpected.check(&Ok(())).is_err());
        assert!(Expect::Ok.check(&Ok(())).is_ok());
        assert!(Expect::Ok
            .check::<()>(&Err(Error::DeviceStatus(5)))
            .is_err());
    }

    #[test]
    fn a_frame_may_carry_a_trailing_label() {
        let script = Script::parse("O 0011 # SESSION_OPEN\nI 22\n").unwrap();
        assert_eq!(script.steps()[0], Step::Out(vec![0x00, 0x11]));
    }

    #[test]
    fn a_read_rejects_a_frame_larger_than_its_buffer() {
        let mut transport = ReplayTransport::new(vec![Step::In(vec![0; 2])]);
        let err = pollster::block_on(transport.read(1)).expect_err("the frame is too large");
        assert!(matches!(err, Error::Replay(_)));
        assert_eq!(
            transport.position(),
            0,
            "an oversized frame was not consumed"
        );
    }

    #[test]
    fn an_unknown_key_is_refused_rather_than_skipped() {
        let err = Script::parse("# intention: program status\nO 00\n").unwrap_err();
        assert!(err.to_string().contains("unknown header key"), "{err}");
    }

    #[test]
    fn a_file_level_field_after_the_first_step_is_refused() {
        let err = Script::parse("O 00\n# source: nsm\n").unwrap_err();
        assert!(err.to_string().contains("before its first step"), "{err}");
    }

    #[test]
    fn an_expect_below_its_frames_judges_the_section_it_closes() {
        let script = Script::parse(
            "# intent: program info 7:10\n\
             O 00\n\
             # expect: err device-status 0x1\n\
             # intent: program focus\n\
             O 01\n",
        )
        .unwrap();
        assert_eq!(
            script.sections[0].expect(),
            Expect::Err(ErrKind::DeviceStatus(1))
        );
        assert_eq!(script.sections[1].expect(), Expect::Ok);
    }

    #[test]
    fn a_section_may_only_say_what_to_expect_once() {
        let err = Script::parse("# intent: program status\n# expect: ok\nO 00\n# expect: ok\n")
            .unwrap_err();
        assert!(err.to_string().contains("already says"), "{err}");
    }

    /// Every kind the recorder writes must read back as the error it names, or the sweep
    /// would judge a declared failure to be a different one. A failure the vocabulary
    /// does not name is written as the nearest kind, so every variant round-trips.
    #[test]
    fn a_recorded_failure_reads_back_as_the_error_it_names() {
        let at = crate::wire::Location { bank: 1, slot: 2 };
        let every = [
            Error::Truncated { got: 2, need: 8 },
            Error::LengthMismatch {
                declared: 34,
                actual: 30,
            },
            Error::BadCrc {
                expected: 0x4a55,
                actual: 0x7197,
            },
            Error::DeviceStatus(0x15),
            Error::ClassRefused {
                class: crate::wire::ObjectClass::Piano,
                status: 5,
            },
            Error::UnexpectedResponse {
                expected: 0x30,
                got: 0x1f,
            },
            Error::UnexpectedLocation {
                requested: at,
                reported: crate::wire::Location { bank: 1, slot: 3 },
            },
            Error::UnexpectedPartition {
                requested: 4,
                reported: 5,
            },
            Error::Enumeration {
                bank: 1,
                answered: at,
                slots: 50,
            },
            Error::ScanLimit {
                bank: 1,
                limit: 4096,
            },
            Error::Transport("stalled".into()),
            Error::Envelope("bad magic".into()),
            Error::Replay("mismatch".into()),
            Error::InvalidArgument("no such bank".into()),
            Error::Io(std::io::Error::from(std::io::ErrorKind::BrokenPipe)),
        ];
        for e in every {
            let line = format!("err {}", e.expect_kind());
            let expect = Expect::parse(&line).unwrap_or_else(|m| panic!("{line}: {m}"));
            assert!(
                expect.check::<()>(&Err(e)).is_ok(),
                "{line} does not read back as itself"
            );
        }
    }

    #[test]
    fn an_intent_that_accounts_for_no_steps_is_refused() {
        let err =
            Script::parse("# intent: program status\nO 00\n# intent: program focus\n").unwrap_err();
        assert!(err.to_string().contains("no steps"), "{err}");
    }

    /// A frame line is read two hex digits at a time, so a multi-byte character must be
    /// refused before it is sliced through.
    #[test]
    fn a_frame_carrying_a_non_ascii_character_is_refused() {
        let err = Script::parse("O aéa\n").unwrap_err();
        assert!(err.to_string().contains("line 1: non-hex byte"), "{err}");
    }

    #[test]
    fn an_unknown_source_is_refused() {
        let err = Script::parse("# source: pcap\nO 00\n").unwrap_err();
        assert!(err.to_string().contains("unknown source"), "{err}");
    }

    #[test]
    fn a_transfer_that_did_not_happen_is_a_step_of_its_own() {
        let script = Script::parse(
            "I timeout # the drain found nothing\n\
             O timeout 00\n\
             O error 01\n\
             I error # bulk read: stall\n",
        )
        .unwrap();
        assert_eq!(
            script.steps(),
            [
                Step::InTimeout,
                Step::OutTimeout(vec![0]),
                Step::OutError(vec![1]),
                Step::InError
            ]
        );
    }

    #[test]
    fn a_host_fault_without_its_frame_is_refused() {
        let err = Script::parse("O timeout\n").unwrap_err();
        assert!(err.to_string().contains("'O timeout'"), "{err}");
    }

    fn limit() -> std::time::Duration {
        std::time::Duration::from_secs(1)
    }

    /// Silence is a recorded step, so a read where the script has the host speaking, or
    /// nothing at all, is the script and the code disagreeing.
    #[test]
    fn a_bounded_read_the_script_does_not_expect_is_a_mismatch() {
        let mut transport = ReplayTransport::new(vec![Step::Out(vec![0])]);
        let err = pollster::block_on(transport.read_timeout(8, limit())).unwrap_err();
        assert!(matches!(err, Error::Replay(_)), "{err}");

        let mut transport = ReplayTransport::new(Vec::new());
        let err = pollster::block_on(transport.read_timeout(8, limit())).unwrap_err();
        assert!(matches!(err, Error::Replay(_)), "{err}");
    }

    #[test]
    fn recorded_silence_answers_only_a_bounded_read() {
        let mut transport = ReplayTransport::new(vec![Step::InTimeout]);
        let err = pollster::block_on(transport.read(8)).unwrap_err();
        assert!(matches!(err, Error::Replay(_)), "{err}");
        assert_eq!(transport.position(), 0);

        let read = pollster::block_on(transport.read_timeout(8, limit())).unwrap();
        assert_eq!(read, None);
        assert_eq!(transport.position(), 1);
    }

    #[test]
    fn a_refused_write_answers_only_a_bounded_write_of_the_same_frame() {
        let mut transport = ReplayTransport::new(vec![Step::OutTimeout(vec![7])]);
        let err = pollster::block_on(transport.write(&[7])).unwrap_err();
        assert!(matches!(err, Error::Replay(_)), "{err}");
        let err = pollster::block_on(transport.write_timeout(&[8], limit())).unwrap_err();
        assert!(matches!(err, Error::Replay(_)), "{err}");
        assert_eq!(transport.position(), 0);

        let accepted = pollster::block_on(transport.write_timeout(&[7], limit())).unwrap();
        assert!(!accepted);
        assert_eq!(transport.position(), 1);
    }

    #[test]
    fn a_recorded_transport_failure_replays_as_one() {
        let mut transport = ReplayTransport::new(vec![Step::OutError(vec![7]), Step::InError]);
        let err = pollster::block_on(transport.write(&[7])).unwrap_err();
        assert!(matches!(err, Error::Transport(_)), "{err}");
        let err = pollster::block_on(transport.read_timeout(8, limit())).unwrap_err();
        assert!(matches!(err, Error::Transport(_)), "{err}");
        assert!(transport.is_exhausted());
    }

    #[test]
    fn what_drives_an_intentless_script_is_a_file_level_field() {
        let script =
            Script::parse("# driven_by: tests/ops.rs\n# undriven: evidence\nO 00\n").unwrap();
        assert_eq!(script.header.driven_by.as_deref(), Some("tests/ops.rs"));
        assert_eq!(script.header.undriven.as_deref(), Some("evidence"));

        let err = Script::parse("O 00\n# driven_by: tests/ops.rs\n").unwrap_err();
        assert!(err.to_string().contains("before its first step"), "{err}");
    }

    #[test]
    fn a_mismatch_the_caller_swallowed_still_leaves_the_replay_unfinished() {
        let mut transport = ReplayTransport::new(vec![Step::Out(vec![0])]);
        pollster::block_on(transport.write(&[0])).unwrap();
        let _ = pollster::block_on(transport.read_timeout(8, limit()));
        assert!(!transport.is_exhausted());
        assert!(transport.mismatch().is_some());
    }
}
