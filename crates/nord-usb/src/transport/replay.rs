//! A [`Transport`] that replays a recorded exchange instead of touching hardware.
//!
//! The whole protocol layer can then be exercised anywhere — including under Wine,
//! qemu and wasm — with no device attached. It also makes every operation assertable:
//! [`ReplayTransport::sent`] hands back exactly what the operation put on the wire, so
//! a test can compare that against a real capture.
//!
//! The script is a flat list of directed messages, in the order they occurred.
//! `Out` entries are what the *host* sent, and are checked against what the code under
//! test actually sends; `In` entries are fed back as device responses.
//!
//! # Script format
//!
//! A frame is `O <hex>` (host → device) or `I <hex>` (device → host), one per line, and
//! may carry a trailing `# label` that is read as a comment. Every other `#` line is
//! either prose or a machine-readable field, `# <key>: <value>` with the key in
//! `[a-z_]+` — so a prose line whose first word is capitalised or hyphenated stays
//! prose. An unknown lowercase key is an error rather than something to skip, so the
//! vocabulary cannot drift.
//!
//! `source`, `device`, `trimmed` and `note` describe the file and must precede its first
//! frame. `intent` and `expect` describe a **section**: `intent` opens one, which runs to
//! the next `intent` or to the end of the file, so one recorded command that opened
//! several transactions is one script of several sections, in order. `expect` names the
//! outcome its own section must produce and defaults to `ok`; it may sit anywhere
//! inside that section, because a recorder only learns the outcome once the frames are
//! written.

use super::Transport;
use crate::error::{Error, Result};

/// Where a script's bytes came from — which is what says whether it is an oracle.
///
/// Only [`Source::Nsm`] is one for an operation nothing has matched before: it is the
/// vendor application's own traffic. [`Source::Nord`] is this project's, and is a
/// regression baseline rather than a proof; [`Source::Synthetic`] was built by hand for
/// a path no capture covers.
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
    /// Free text: model and firmware the capture was taken against.
    pub device: Option<String>,
    /// What was left out of the capture, e.g. `ui-refresh`.
    pub trimmed: Option<String>,
    pub note: Option<String>,
}

/// The outcome a section's declared intent must produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Expect {
    #[default]
    Ok,
    Err(ErrKind),
}

/// The failures a script may name, spelled in kebab-case after the [`Error`] variant.
///
/// Deliberately a short list: it exists to tell one *expected* refusal from another, not
/// to mirror the error type. A device refusal carries its status code, because the code
/// is the finding — `0x15` (the library classes refusing a rename) and `0x1` (nothing
/// loaded) are different results, not two spellings of one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrKind {
    DeviceStatus(u32),
    UnexpectedResponse,
    UnexpectedLocation,
    Enumeration,
    Transport,
    Replay,
}

/// One declared intent and the frames it accounts for.
#[derive(Debug, Clone, Default)]
pub struct Section {
    /// `<class> <verb> <args…>`, in the CLI's own spellings. `None` means the section
    /// declares nothing, and the frames are checkable but not drivable.
    pub intent: Option<String>,
    /// What the section said to expect, if it said. Read through [`Section::expect`],
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

/// The header keys a script may carry, reported verbatim when one is misspelled.
pub const KEYS: &[&str] = &["intent", "expect", "source", "device", "trimmed", "note"];

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

impl ErrKind {
    /// Whether an error is the one this names.
    pub fn matches(&self, e: &Error) -> bool {
        match (self, e) {
            (ErrKind::DeviceStatus(want), Error::DeviceStatus(got)) => want == got,
            (ErrKind::UnexpectedResponse, Error::UnexpectedResponse { .. }) => true,
            (ErrKind::UnexpectedLocation, Error::UnexpectedLocation { .. }) => true,
            (ErrKind::Enumeration, Error::Enumeration { .. }) => true,
            (ErrKind::Transport, Error::Transport(_)) => true,
            (ErrKind::Replay, Error::Replay(_)) => true,
            _ => false,
        }
    }

    fn parse(value: &str) -> std::result::Result<Self, String> {
        let (kind, arg) = match value.split_once(char::is_whitespace) {
            Some((kind, arg)) => (kind, arg.trim()),
            None => (value, ""),
        };
        match (kind, arg) {
            ("device-status", "") => Err("device-status needs its code, e.g. \
                                          'err device-status 0x15'"
                .into()),
            ("device-status", code) => parse_u32(code)
                .map(ErrKind::DeviceStatus)
                .ok_or_else(|| format!("bad device status {code:?}")),
            ("unexpected-response", "") => Ok(ErrKind::UnexpectedResponse),
            ("unexpected-location", "") => Ok(ErrKind::UnexpectedLocation),
            ("enumeration", "") => Ok(ErrKind::Enumeration),
            ("transport", "") => Ok(ErrKind::Transport),
            ("replay", "") => Ok(ErrKind::Replay),
            (kind, _) => Err(format!(
                "unknown failure {kind:?}; the vocabulary is device-status <code>, \
                unexpected-response, unexpected-location, enumeration, transport, replay"
            )),
        }
    }
}

impl std::fmt::Display for ErrKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrKind::DeviceStatus(code) => write!(f, "device-status {code:#x}"),
            ErrKind::UnexpectedResponse => f.write_str("unexpected-response"),
            ErrKind::UnexpectedLocation => f.write_str("unexpected-location"),
            ErrKind::Enumeration => f.write_str("enumeration"),
            ErrKind::Transport => f.write_str("transport"),
            ErrKind::Replay => f.write_str("replay"),
        }
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

/// A header field, or `None` for prose. The key must be `[a-z_]+`, which is what keeps
/// an ordinary sentence containing a colon from being read as one.
fn field(comment: &str) -> Option<(&str, &str)> {
    let (key, value) = comment.trim().split_once(':')?;
    let named = !key.is_empty() && key.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
    named.then(|| (key, value.trim()))
}

/// `0x`-prefixed hex or decimal — status codes are quoted both ways.
fn parse_u32(s: &str) -> Option<u32> {
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => s.parse().ok(),
    }
}

impl Script {
    /// Parse a script: header fields, sections, and frames.
    ///
    /// Refuses rather than skips — an unknown key, a file-level field after the first
    /// frame, an `expect` that follows the frames it judges, or a section with no frames
    /// are all errors, so a header that does nothing cannot sit unnoticed in a tree the
    /// sweep walks.
    pub fn parse(text: &str) -> Result<Self> {
        let fail =
            |n: usize, what: std::fmt::Arguments| Error::Replay(format!("line {}: {what}", n + 1));
        let mut header = Header::default();
        let mut sections = vec![Section::default()];
        let mut seen_frame = false;

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
                match key {
                    "intent" => {
                        if section.intent.is_none() && section.steps.is_empty() {
                            section.intent = Some(value.to_string());
                        } else {
                            sections.push(Section {
                                intent: Some(value.to_string()),
                                ..Section::default()
                            });
                        }
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
                    }
                    "source" | "device" | "trimmed" | "note" if seen_frame => {
                        return Err(fail(
                            n,
                            format_args!(
                                "{key} describes the file and must come before its first frame"
                            ),
                        ))
                    }
                    "source" => {
                        let source =
                            Source::parse(value).map_err(|e| fail(n, format_args!("{e}")))?;
                        if header.source.replace(source).is_some() {
                            return Err(fail(n, format_args!("source is given twice")));
                        }
                    }
                    "device" if header.device.replace(value.into()).is_some() => {
                        return Err(fail(n, format_args!("device is given twice")))
                    }
                    "trimmed" if header.trimmed.replace(value.into()).is_some() => {
                        return Err(fail(n, format_args!("trimmed is given twice")))
                    }
                    "note" if header.note.replace(value.into()).is_some() => {
                        return Err(fail(n, format_args!("note is given twice")))
                    }
                    "device" | "trimmed" | "note" => {}
                    other => {
                        return Err(fail(
                            n,
                            format_args!(
                                "unknown header key {other:?}; the vocabulary is {}",
                                KEYS.join(", ")
                            ),
                        ))
                    }
                }
                continue;
            }

            // A trailing `# label` says what the frame is; nothing reads it back.
            let frame = match line.split_once('#') {
                Some((frame, _)) => frame.trim(),
                None => line,
            };
            let (tag, hex) = frame
                .split_once(char::is_whitespace)
                .ok_or_else(|| fail(n, format_args!("expected '<O|I> <hex>'")))?;
            let direction = match tag {
                "O" | "o" => Direction::Out,
                "I" | "i" => Direction::In,
                other => {
                    return Err(fail(
                        n,
                        format_args!("unknown direction {other:?}, want O or I"),
                    ))
                }
            };
            let hex = hex.trim();
            if hex.len() % 2 != 0 {
                return Err(fail(n, format_args!("odd-length hex")));
            }
            let bytes = (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
                .collect::<std::result::Result<Vec<u8>, _>>()
                .map_err(|e| fail(n, format_args!("{e}")))?;
            seen_frame = true;
            sections
                .last_mut()
                .expect("one section always exists")
                .steps
                .push(Step { direction, bytes });
        }

        for section in &sections {
            if section.steps.is_empty() {
                let what = match &section.intent {
                    Some(intent) => format!("intent {intent:?} accounts for no frames"),
                    None => "the script holds no frames".into(),
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

    /// Every frame, sections joined, in wire order.
    pub fn steps(&self) -> Vec<Step> {
        self.sections
            .iter()
            .flat_map(|s| s.steps.iter().cloned())
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Host → device.
    Out,
    /// Device → host.
    In,
}

#[derive(Debug, Clone)]
pub struct Step {
    pub direction: Direction,
    pub bytes: Vec<u8>,
}

/// How strictly to police what the code under test transmits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strictness {
    /// Every `Out` must match the script byte-for-byte. Use in tests.
    Exact,
    /// Ignore what is sent and just serve the next `In`. Useful for demos against a
    /// capture whose addressing differs from what is being asked for.
    Lenient,
}

pub struct ReplayTransport {
    script: Vec<Step>,
    pos: usize,
    sent: Vec<Vec<u8>>,
    strictness: Strictness,
}

impl ReplayTransport {
    pub fn new(script: Vec<Step>) -> Self {
        Self {
            script,
            pos: 0,
            sent: Vec::new(),
            strictness: Strictness::Exact,
        }
    }

    pub fn lenient(mut self) -> Self {
        self.strictness = Strictness::Lenient;
        self
    }

    /// Everything the code under test transmitted, in order.
    pub fn sent(&self) -> &[Vec<u8>] {
        &self.sent
    }

    /// Whether the whole script was consumed. A test that leaves steps unread has
    /// usually stopped short of the behavior it meant to check.
    pub fn is_exhausted(&self) -> bool {
        self.pos >= self.script.len()
    }

    /// How many steps have been consumed. With the section boundaries of a [`Script`]
    /// this says which intent stopped short.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Replay every frame of a script, whatever its header declares.
    pub fn from_script(text: &str) -> Result<Self> {
        Ok(Self::new(Script::parse(text)?.steps()))
    }
}

impl Transport for ReplayTransport {
    async fn write(&mut self, buf: &[u8]) -> Result<()> {
        self.sent.push(buf.to_vec());

        let step = self.script.get(self.pos).ok_or_else(|| {
            Error::Replay(format!(
                "script exhausted; host sent an extra {} bytes",
                buf.len()
            ))
        })?;
        if step.direction != Direction::Out {
            return Err(Error::Replay(
                "host wrote, but the script expects the device to speak next".into(),
            ));
        }
        if self.strictness == Strictness::Exact && step.bytes != buf {
            return Err(Error::Replay(format!(
                "sent bytes differ from the script at step {}\n  expected {}\n  got      {}",
                self.pos,
                hex(&step.bytes),
                hex(buf),
            )));
        }
        self.pos += 1;
        Ok(())
    }

    /// A bounded read answers `None` wherever the script has nothing for the device to
    /// say — the end of it, or a frame the host sends next.
    ///
    /// That is what a recording of a timed-out read looks like: the read produced no
    /// frame, so none was written down. Without this, replaying an operation built on
    /// bounded reads — [`crate::op::recover`] drains the stream that way — would fail on
    /// the very silence it was recorded against.
    async fn read_timeout(
        &mut self,
        max: usize,
        _limit: std::time::Duration,
    ) -> Result<Option<Vec<u8>>> {
        match self.script.get(self.pos) {
            Some(step) if step.direction == Direction::In => self.read(max).await.map(Some),
            _ => Ok(None),
        }
    }

    async fn read(&mut self, _max: usize) -> Result<Vec<u8>> {
        let step = self
            .script
            .get(self.pos)
            .ok_or_else(|| Error::Replay("script exhausted; host expected a response".into()))?;
        if step.direction != Direction::In {
            return Err(Error::Replay(
                "host read, but the script expects the host to speak next".into(),
            ));
        }
        self.pos += 1;
        Ok(step.bytes.clone())
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two lines every recording starts with are prose: the key of a field is
    /// `[a-z_]+`, and `Format` is capitalised.
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

    /// One recorded command opens several transactions, so a script is a sequence of
    /// sections and each one accounts for the frames that follow it.
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

    /// A device refusal is only useful if the script can name *which* one, so the code
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
        assert_eq!(script.steps()[0].bytes, vec![0x00, 0x11]);
    }

    #[test]
    fn an_unknown_key_is_refused_rather_than_skipped() {
        let err = Script::parse("# intention: program status\nO 00\n").unwrap_err();
        assert!(err.to_string().contains("unknown header key"), "{err}");
    }

    #[test]
    fn a_file_level_field_after_the_first_frame_is_refused() {
        let err = Script::parse("O 00\n# source: nsm\n").unwrap_err();
        assert!(err.to_string().contains("before its first frame"), "{err}");
    }

    /// A recorder only knows the outcome once the frames are written, so an `expect`
    /// under them belongs to the section it closes, not to the next one.
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

    /// Every kind the recorder writes must read back as the error it names, or a script
    /// would declare a failure the sweep then judges to be a different one.
    #[test]
    fn a_recorded_failure_reads_back_as_the_error_it_names() {
        let named = [
            Error::DeviceStatus(0x15),
            Error::UnexpectedResponse {
                expected: 0x30,
                got: 0x1f,
            },
            Error::Transport("stalled".into()),
            Error::Replay("mismatch".into()),
        ];
        for e in named {
            let line = format!("err {}", e.expect_kind());
            let expect = Expect::parse(&line).unwrap_or_else(|m| panic!("{line}: {m}"));
            assert!(
                expect.check::<()>(&Err(e)).is_ok(),
                "{line} does not read back as itself"
            );
        }
        // A failure the vocabulary does not name is written as the nearest kind rather
        // than left off, where the script would claim the operation succeeded.
        assert_eq!(
            Error::Truncated { got: 2, need: 8 }.expect_kind(),
            "transport"
        );
    }

    #[test]
    fn an_intent_that_accounts_for_no_frames_is_refused() {
        let err =
            Script::parse("# intent: program status\nO 00\n# intent: program focus\n").unwrap_err();
        assert!(err.to_string().contains("no frames"), "{err}");
    }

    #[test]
    fn an_unknown_source_is_refused() {
        let err = Script::parse("# source: pcap\nO 00\n").unwrap_err();
        assert!(err.to_string().contains("unknown source"), "{err}");
    }
}
