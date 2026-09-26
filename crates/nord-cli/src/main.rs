//! `nord`, a command-line tool over [`nord_format`] and `nord_usb` for Clavia Nord
//! files and instruments.
//!
//! > This is an unofficial community project: **not affiliated with, endorsed
//! > by, or supported by Clavia DMI AB**. "Nord" and the instrument names are
//! > Clavia's trademarks, used here only to identify which files this crate
//! > reads.
//!
//! The nouns are the protocol's object classes. `nord program`, `nord sample`,
//! `nord piano` and `nord setlist` are [`slot_action`] with the class fixed, plus verbs
//! of their own. `nord live` and `nord settings` keep the verbs their class can answer,
//! plus `edit`. The hidden `nord raw --class N` is [`slot_action`] with the class given
//! as a number.
//!
//! `inspect`, `verify` and `edit` dispatch on the file format, so they sit at the top
//! level. `edit` is how the formats with no noun of their own (the Stage bodies, the
//! Sample Editor project) are edited.
//!
//! ⚠️ `raw` is hidden but supported: it is the only way to reach a class with no noun of
//! its own.

mod device;
mod edit;
mod editors;
mod file;
mod file_edit;
mod piano;
mod sample;
mod slot;
mod summary;
mod ui;
mod wav;

use clap::{Args, Parser, Subcommand};
use nord_usb::ObjectClass;
use std::path::PathBuf;
use std::process::ExitCode;

use ui::{ColorChoice, Ui};

#[derive(Parser)]
#[command(
    name = "nord",
    about = "Inspect and edit Clavia Nord files and instruments",
    version
)]
struct Cli {
    /// When to color output. `auto` colors only when stdout is a terminal and `NO_COLOR`
    /// is not set.
    #[arg(long, global = true, value_name = "WHEN", value_enum, default_value_t)]
    color: ColorChoice,

    /// Record every frame exchanged with the instrument to a replay script at PATH.
    ///
    /// `device status --replay` and the protocol tests read the script back. A
    /// transferred body is written to it in full.
    ///
    /// Only bulk traffic is recorded. `device info` and `device controls` use endpoint 0,
    /// which never reaches the script.
    #[arg(long, global = true, value_name = "PATH")]
    record: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Decode Nord files and print a summary of each.
    Inspect {
        /// Files to read, such as a program (.ne5p), live slot (.ne5l), set list
        /// (.ne5t), settings (.ne5s), piano (.npno), sample (.nsmp), Sample Editor
        /// project (.nsmpproj), or a ZIP backup bundle.
        #[arg(required = true)]
        files: Vec<PathBuf>,

        /// Print the full decoded structure (Rust `Debug` output) instead of the summary.
        #[arg(long)]
        raw: bool,
    },

    /// Re-encode files and check that each matches its input byte for byte.
    ///
    /// A mismatch is reported with the offset of the first differing byte.
    Verify {
        /// Files to round-trip. ZIP backup bundles cannot be re-encoded.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },

    /// Change fields in any editable file, whatever its format.
    ///
    /// This works out the format from the file itself, so it also edits formats that
    /// have no command of their own: Stage programs and presets, and Sample Editor
    /// projects. `--fields` lists what the file offers.
    Edit(file_edit::FileEditArgs),

    /// The connected instrument: what is attached, and what it holds.
    Device {
        #[command(subcommand)]
        action: DeviceAction,
    },

    /// Programs on the instrument (object class 4), or `.ne5p` files.
    ///
    /// Slots are `BANK:SLOT`, as the instrument displays them. The read-only verbs and
    /// `edit` also take a file.
    Program {
        #[command(subcommand)]
        action: ProgramAction,
    },

    /// Set lists on the instrument (object class 5). Same verbs as `nord program`.
    Setlist {
        #[command(subcommand)]
        action: SetlistAction,
    },

    /// Live slots 1:1 to 1:3, which hold the panel as it stands (object class 6).
    Live {
        #[command(subcommand)]
        action: LiveAction,
    },

    /// The global settings singleton (object class 7): the System, MIDI and Sound
    /// menus, plus the panel state the instrument restores at power-up.
    Settings {
        #[command(subcommand)]
        action: SettingsAction,
    },

    /// Sample instruments in the instrument's library (object class 3), or `.nsmp`
    /// files.
    Sample {
        #[command(subcommand)]
        action: SampleAction,
    },

    /// Piano libraries on the instrument (object class 1), or `.npno` files.
    Piano {
        #[command(subcommand)]
        action: PianoAction,
    },

    /// The slot verbs for any object class, given by number.
    ///
    /// The other nouns are this command with the class fixed. Use it for a class with no
    /// noun of its own.
    #[command(hide = true)]
    Raw {
        #[arg(long, global = true, value_name = "N", default_value_t = 4, help = class_help())]
        class: u32,

        #[command(subcommand)]
        action: SlotAction,
    },
}

#[derive(Subcommand)]
enum DeviceAction {
    /// Try each vendor control request on endpoint 0 and print the answers. For reverse
    /// engineering.
    ///
    /// Endpoint 0 is outside the bulk protocol, so these reads cannot open, desynchronize
    /// or wedge a session. An unrecognized request stalls the endpoint and has no other
    /// effect. These requests are reported to carry the model, firmware version, build
    /// and maximum transfer size.
    Controls {
        /// Lowest bRequest to try.
        #[arg(long, default_value_t = 0)]
        from: u8,

        /// Highest bRequest to try, inclusive.
        #[arg(long, default_value_t = 15)]
        to: u8,

        /// Bytes to request from each. A control transfer's wLength is 16 bits.
        #[arg(long, default_value_t = 64)]
        len: u16,

        /// Address the interface instead of the device.
        #[arg(long)]
        interface: bool,

        /// wValue sent with each request.
        #[arg(long, default_value_t = 0)]
        value: u16,

        /// wIndex sent with each request. For --interface this is the interface number.
        #[arg(long, default_value_t = 0)]
        index: u16,
    },

    /// Report what the instrument stores, per object class.
    ///
    /// Read-only: this sends one query per class and reads the counters back.
    Status {
        /// Replay an exchange recorded with `--record` instead of opening a device, to
        /// run the command without hardware.
        #[arg(long, value_name = "SCRIPT")]
        replay: Option<PathBuf>,

        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },

    /// Identify the attached instrument from its USB descriptors. Read-only.
    ///
    /// This opens no session, so it is the first thing to run when nothing else answers.
    Info,

    /// Clear a session that an interrupted run left open on the instrument.
    ///
    /// An abandoned UI session makes every slot read as empty, with no error. An
    /// abandoned class session makes operations fail with status 0x12. One frame clears
    /// each, and this is safe to run on a healthy instrument.
    Recover,

    /// Report the instrument's storage layout: partitions, banks and slot capacity.
    ///
    /// The layout is read from the instrument, so it is correct even for models this
    /// tool has not seen. Partition indices are object class numbers.
    Geometry,

    /// Wedge the instrument by abandoning a session, to test recovery.
    ///
    /// Nothing stored is harmed, but every slot then reads as empty, with no error, until
    /// `nord device recover` clears it.
    #[cfg(feature = "wedge")]
    #[command(hide = true)]
    Wedge {
        /// Object class to open the abandoned session on.
        #[arg(long, value_name = "N", default_value_t = 4)]
        class: u32,

        /// Confirm. Without this nothing is sent.
        #[arg(long)]
        yes: bool,
    },
}

/// `nord program`: the slot verbs, plus `edit`.
#[derive(Subcommand)]
enum ProgramAction {
    #[command(flatten)]
    Slot(SlotAction),

    /// Change fields in a program, in a file or in a slot.
    ///
    /// Fields are named by path, such as `center_panel.transpose` or
    /// `effects_panel.fx1_rate`. `--fields` lists them.
    ///
    /// With no target, the edit starts from a default program: `--fields` needs no file,
    /// and `-o` writes a new `.ne5p`.
    Edit(EditArgs),
}

/// `nord setlist`: the slot verbs, plus `edit` for the four program slots a set list
/// points at.
#[derive(Subcommand)]
enum SetlistAction {
    #[command(flatten)]
    Slot(SlotAction),

    /// Change the programs a set list plays, in a file or in a slot.
    ///
    /// The four slots are `slot1` to `slot4`, each taking a program address as
    /// the instrument shows it: `--set slot1=2:5`. `--fields` lists them. With
    /// no target, the edit starts from a default set list, and `-o` writes a new
    /// `.ne5t`.
    Edit(EditArgs),
}

/// `nord sample`: the slot verbs, plus verbs that edit, decode, encode and build sample
/// instruments.
#[derive(Subcommand)]
enum SampleAction {
    #[command(flatten)]
    Slot(SlotAction),

    /// Change fields in a sample instrument, in a file or in a slot.
    ///
    /// The editable fields are those the format can patch in place: the name, each
    /// zone's root key and top note, and its low note in the generations that store
    /// one. `--fields` lists them.
    Edit(sample::EditArgs),

    /// Decode a sample instrument's audio to WAV, one file per zone, from a file or a
    /// slot.
    ///
    /// The WAVs keep the stored sample rate, about 35 kHz. The rate the instrument plays
    /// back at depends on its interpolator, which is not decoded. Audio the decoder
    /// cannot read is reported as unsupported with a reason, and the run ends with a
    /// count of what decoded. A slot is only read, so this never needs `--yes`, and its
    /// WAVs are named after the instrument.
    Decode(sample::DecodeArgs),

    /// Build a one-zone sample instrument from a 44.1 kHz mono or stereo 16-bit WAV.
    ///
    /// The v2 output matches what Nord Sample Editor writes for the same input, byte for
    /// byte, except that floating-point rounding in the resampler leaves an occasional
    /// audio value off by one, which does not change what the instrument plays. Mono,
    /// stereo and looped v2 encodes play on an Electro 5. The v3 and v4 outputs match
    /// the editor's but have never been played, so they need `--unverified`.
    Encode(sample::EncodeArgs),

    /// Build a sample instrument from a Nord Sample Editor project.
    ///
    /// The project supplies the zones, their root keys, top notes and trim points,
    /// and the WAVs they play. Paths in the project are relative to its own directory.
    /// Unsupported layer, detune, velocity and enabled EQ settings are refused by name.
    /// Settings the instrument has no place for are ignored, with a note. The notes on
    /// fidelity and `--unverified` under `encode` apply here too.
    Build(sample::BuildArgs),

    /// Round-trip a sample instrument from a file or a slot, and with `--deep` also
    /// check its encoded audio stream. A slot is only read.
    Verify(sample::VerifyArgs),

    /// Sample Editor projects (`.nsmpproj`), the files the editor builds instruments
    /// from. `nord edit` changes one, and this creates one.
    Project {
        #[command(subcommand)]
        action: SampleProjectAction,
    },
}

/// `nord piano`: the slot verbs, plus verbs that read and reshape a library file.
///
/// A library is tens of megabytes, so these verbs take only a file. Move one to or from
/// the instrument with `get` and `put`.
#[derive(Subcommand)]
enum PianoAction {
    #[command(flatten)]
    Slot(SlotAction),

    /// Report a library's directory: its roots, the layers each holds per bank, the
    /// keys it covers, its channels and its size. Read-only.
    Inspect(piano::InspectArgs),

    /// Decode one stroke to a WAV, at the rate the instrument plays it and with no
    /// gain applied.
    ///
    /// A stroke is one recording: a root note, a bank and a velocity layer. Name it
    /// by index with `--stroke`, or by the key it plays with `--key`, narrowing
    /// with `--bank` and `--layer` when a key selects more than one.
    Decode(piano::DecodeArgs),

    /// Change a library's name, a key's fine tune, or which root a key plays.
    ///
    /// The audio is not touched. A key can only be routed to a root the library's
    /// directory records.
    Edit(piano::EditArgs),

    /// Write a smaller library: without a bank, without the quieter velocity
    /// layers, or covering fewer keys.
    ///
    /// The strokes that remain are copied byte for byte, with no re-encoding. Keys
    /// whose root loses every stroke are left silent, and their count is reported.
    /// Dropping a bank or a layer is confirmed on hardware: the trimmed library loads
    /// and plays. Narrowing the key range is not confirmed on hardware.
    Trim(piano::TrimArgs),

    /// Cut a library in two at a key, writing both halves.
    Split(piano::SplitArgs),

    /// Build a piano library from a directory of WAVs.
    ///
    /// Each WAV's name gives the root, bank and layer it holds. Any sample rate is
    /// resampled to the rate the instrument plays at.
    ///
    /// What the audio does not decide (the length marks, decay coefficients, per-note
    /// tables, playback parameters and stream version) comes from `--template`, taken
    /// from the template's stroke of the same bank and nearest root. Without a template,
    /// the library uses neutral playback: no decay over the recordings, each stroke
    /// trimmed by its own layer value, and the damper limit that `--kind` implies.
    ///
    /// Every key up to one semitone above the highest root sounds, playing the nearest
    /// root at or above it. Keys past that are silent. A key plays the largest layer
    /// value its root holds that is at most (127 − velocity)·31/127, and a root's `l00`,
    /// `l01`, … spread over 0..27 so that each layer covers its own part of the velocity
    /// range.
    ///
    /// Confirmed on hardware: a library built this way loads and plays, mono and
    /// stereo, on every key it covers, and one built without a template sounds the same
    /// as the same audio built with one.
    Build(piano::BuildArgs),

    /// Re-encode a library's audio from its decoded frames, and report how each
    /// stroke's blocks compare.
    ///
    /// A library this tool wrote comes back byte for byte. Any other library comes back
    /// block for block, except for the attenuation each block declares: the original
    /// encoder measured that value, and decoding never reads it. The re-encoded library
    /// sounds the same as the original. Each stroke keeps its root, bank and layer
    /// value.
    Rebuild(piano::RebuildArgs),

    /// Rebuild each library from its decoded model and check that the bytes match, and
    /// with `--deep` also decode every stroke.
    ///
    /// The rebuild recomputes the per-root counts, every audio offset, the alignment gap
    /// and the container checksum, so a match shows the model accounts for the whole
    /// file. `--deep` adds the codec's own checks: each block repeats the previous
    /// block's last frames exactly, and each stroke decodes to the frame count its
    /// record states.
    Verify(piano::VerifyArgs),
}

/// `nord sample project`: the editor's own save file, which no object class holds.
#[derive(Subcommand)]
enum SampleProjectAction {
    /// Create a project from WAV files, one zone per `--zone WAV=NOTE`.
    ///
    /// Key ranges, zone ids and loop points are set the way the editor sets them for a
    /// new import. Each WAV is stored by the path given, made relative to the project's
    /// directory when it is inside it. A WAV may have any sample rate, but the project
    /// states its frame counts at 44.1 kHz.
    New(sample::ProjectNewArgs),
}

/// `nord live`: the verbs that apply to the live buffer.
///
/// The live buffer is the panel as it stands, so it has nothing to name or delete, and
/// `select` on another class is what loads it. It keeps `get`, `info` and `deps` from
/// [`SlotAction`], plus `edit`.
#[derive(Subcommand)]
enum LiveAction {
    #[command(flatten)]
    Slot(LiveSlotAction),

    /// Change fields in a live slot, in a `.ne5l` file or on the instrument.
    ///
    /// A live slot holds a program body under another tag, so the fields are the same as
    /// `nord program edit`'s. Slots are 1:1 to 1:3, and the instrument overwrites them
    /// in place.
    Edit(EditArgs),
}

/// `nord settings`: read or edit the singleton at 1:1.
#[derive(Subcommand)]
enum SettingsAction {
    #[command(flatten)]
    Slot(SettingsSlotAction),

    /// Change fields in the global settings, in a `.ne5s` file or on the instrument.
    ///
    /// Fields are the menu settings plus the `startup_*` state the instrument restores
    /// at power-up; `--fields` lists them. The singleton is addressed as slot `1:1`, and
    /// the instrument overwrites it in place.
    ///
    /// ⚠️ A settings write reloads the selected program, losing panel state that has
    /// not been stored.
    Edit(EditArgs),
}

/// Read-only actions for the settings singleton.
#[derive(Subcommand)]
enum SettingsSlotAction {
    /// Read the settings from the instrument or a `.ne5s` file. Read-only.
    ///
    /// Prints a summary, or with `--out` writes the file.
    Get {
        /// 1:1 for the instrument's settings, or a file.
        #[arg(value_name = "FILE|BANK:SLOT")]
        at: String,

        /// Write the object to this file instead of printing a summary. With `--sweep`,
        /// the directory every capture lands in.
        #[arg(short, long, value_name = "FILE|DIR")]
        out: Option<PathBuf>,

        /// Save the body as sent over USB, without a CBIN header. Needs `--out`.
        #[arg(long)]
        body: bool,

        /// Read the settings repeatedly, once per prompt, into the `--out` directory.
        ///
        /// Change one menu setting on the instrument, then type what you changed, and the
        /// capture is saved under that name. A blank line stops.
        #[arg(long, requires = "out")]
        sweep: bool,
    },

    /// Report everything the instrument knows about the settings singleton, or a
    /// `.ne5s` file's header. Read-only.
    Info {
        /// 1:1 for the instrument's settings, or a file.
        #[arg(value_name = "FILE|BANK:SLOT")]
        at: String,
    },
}

/// The [`SlotAction`] verbs the live buffer keeps, with the same names and arguments.
#[derive(Subcommand)]
enum LiveSlotAction {
    /// Read a live slot from the instrument or a `.ne5l` file. Read-only.
    ///
    /// Prints a summary, or with `--out` writes the file.
    Get {
        /// Slot to read (1:1, 1:2 or 1:3), or a file.
        #[arg(value_name = "FILE|BANK:SLOT")]
        at: String,

        /// Write the object to this file instead of printing a summary. With `--sweep`,
        /// the directory every capture lands in.
        #[arg(short, long, value_name = "FILE|DIR")]
        out: Option<PathBuf>,

        /// Save the body as sent over USB, without a CBIN header. Needs `--out`.
        #[arg(long)]
        body: bool,

        /// Read the slot repeatedly, once per prompt, into the `--out` directory.
        ///
        /// The live slot is the panel itself, so each step can capture one change without
        /// storing a program.
        #[arg(long, requires = "out")]
        sweep: bool,
    },

    /// Report everything the instrument knows about a live slot, or a `.ne5l` file's
    /// header. Read-only.
    Info {
        /// Slot to describe (1:1, 1:2 or 1:3), or a file.
        #[arg(value_name = "FILE|BANK:SLOT")]
        at: String,
    },

    /// List the piano and sample library objects the live panel uses. Read-only.
    Deps {
        /// Slot to inspect (1:1, 1:2 or 1:3), or a file.
        #[arg(value_name = "FILE|BANK:SLOT")]
        at: String,
    },
}

/// The verbs shared by every object class.
#[derive(Subcommand)]
enum SlotAction {
    /// Read an object from the instrument or a file. Read-only.
    ///
    /// Prints a summary, or with `--out` writes the file.
    Get {
        /// Slot to read, e.g. 7:4, or a file to read with no instrument attached.
        #[arg(value_name = "FILE|BANK:SLOT")]
        at: String,

        /// Write the object to this file instead of printing a summary. With `--sweep`,
        /// the directory every capture lands in.
        #[arg(short, long, value_name = "FILE|DIR")]
        out: Option<PathBuf>,

        /// Save the body as sent over USB, without a CBIN header. Use this for classes
        /// whose header layout is unknown, where a header would be wrong. On a file,
        /// strips the header. Needs `--out`.
        #[arg(long)]
        body: bool,

        /// Read the slot repeatedly, once per prompt, into the `--out` directory.
        ///
        /// Change one thing on the instrument, then type what you changed, and the capture
        /// is saved under that name. A blank line stops. Comparing the captures shows
        /// which bytes each control changes.
        #[arg(long, requires = "out")]
        sweep: bool,
    },

    /// Write a file into a slot, OVERWRITING it. Requires --yes.
    Put {
        /// The file to send.
        file: PathBuf,

        /// Destination slot, e.g. 7:4.
        #[arg(value_name = "BANK:SLOT")]
        at: String,

        /// Confirm the overwrite. Without this the command stops after reporting what
        /// currently occupies the slot.
        #[arg(long)]
        yes: bool,
    },

    /// Move an object between slots, SWAPPING with any occupant. Requires --yes.
    Move {
        /// Source slot, e.g. 8:13.
        #[arg(value_name = "FROM")]
        from: String,

        /// Destination slot, e.g. 7:16.
        #[arg(value_name = "TO")]
        to: String,

        #[arg(long)]
        yes: bool,
    },

    /// Rename the object in a slot. Requires --yes.
    Rename {
        /// Slot to rename, e.g. 6:13.
        #[arg(value_name = "BANK:SLOT")]
        at: String,

        /// The new name.
        name: String,

        #[arg(long)]
        yes: bool,
    },

    /// Duplicate an object into another slot, copied on the instrument. Requires --yes.
    Duplicate {
        /// Source slot, e.g. 7:2.
        #[arg(value_name = "FROM")]
        from: String,

        /// Destination slot, e.g. 7:3.
        #[arg(value_name = "TO")]
        to: String,

        #[arg(long)]
        yes: bool,
    },

    /// Delete one or more slots. Requires --yes.
    Delete {
        /// Slots to delete, e.g. 7:50 (repeatable).
        #[arg(value_name = "BANK:SLOT", required = true)]
        slots: Vec<String>,

        #[arg(long)]
        yes: bool,
    },

    /// Load an object on the instrument, as a double-click in Nord Sound Manager does.
    /// Changes nothing stored.
    Select {
        /// Slot to load, e.g. 2:12.
        #[arg(value_name = "BANK:SLOT")]
        at: String,
    },

    /// Report everything the instrument knows about one slot, or a file's header.
    /// Read-only.
    ///
    /// Shows the CBIN header fields that a USB transfer leaves out (format tag, version,
    /// CRC-32), plus the slot name, which files do not store.
    Info {
        /// Slot to describe, e.g. 7:4, or a file.
        #[arg(value_name = "FILE|BANK:SLOT")]
        at: String,
    },

    /// List the piano and sample library objects an object depends on. Read-only.
    ///
    /// A file gives only the stored ids. For a slot, the instrument also supplies the
    /// names.
    Deps {
        /// Slot to inspect, e.g. 7:3, or a file.
        #[arg(value_name = "FILE|BANK:SLOT")]
        at: String,
    },

    /// Report which object of this class the panel has loaded. Read-only.
    ///
    /// The read side of `select`.
    Focus,

    /// List everything the instrument holds in this class. Read-only.
    ///
    /// Walks the instrument's own slot cursor, so it visits only occupied slots. They
    /// are sparse, and their indices run past the class's item count.
    List,

    /// Send a raw command code and print whatever the instrument answers. For reverse
    /// engineering only.
    ///
    /// The reply is not interpreted: the status word and payload are printed as they
    /// arrive, since for an unknown command an error status is itself the result. A
    /// command the instrument ignores is reported as a timeout.
    ///
    /// DANGER: no capture has shown the instrument receiving these bytes. An unknown
    /// command can leave the instrument needing a power cycle, and a command that writes
    /// will destroy whatever object it reaches. Send only codes that read, and back up
    /// first.
    Probe {
        /// Command code, decimal or 0x-prefixed, e.g. 0x20.
        #[arg(value_name = "OP", value_parser = parse_u32)]
        op: u32,

        /// Argument words, appended in order as big-endian u32s: --arg 1 --arg 0.
        #[arg(long = "arg", value_name = "N", value_parser = parse_u32)]
        args: Vec<u32>,

        /// Seconds to wait for a reply before giving up.
        #[arg(long, default_value_t = 5)]
        wait: u64,

        /// Required. A probe is not read-only: the instrument's response to an unknown
        /// code is unknown.
        #[arg(long)]
        yes: bool,

        /// Send with no session: no HELLO, no session open, no close.
        ///
        /// A wedged instrument refuses to open a session, so an ordinary probe fails
        /// before its command is sent. This is the only way to reach a command then.
        #[arg(long)]
        bare: bool,

        /// Service number. 12 is the object/file service, 6 the UI session.
        #[arg(long, default_value_t = 12)]
        service: u32,

        /// Subsystem number. 10 for service 12, 1 for service 6.
        #[arg(long, default_value_t = 10)]
        subsystem: u32,
    },
}

/// Accepts `0x2a` as well as `42`, since command codes are usually written in hex.
fn parse_u32(s: &str) -> Result<u32, String> {
    let s = s.trim();
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => s.parse(),
    }
    .map_err(|e| format!("{s}: {e}"))
}

#[derive(Args)]
pub struct EditArgs {
    /// A file (`.ne5p` for `nord program`, `.ne5t` for `nord setlist`, `.ne5l` for
    /// `nord live`, `.ne5s` for `nord settings`) or a slot on the instrument (`7:4`).
    /// Editing a slot reads it, changes it and writes it back over USB, so it needs
    /// `--yes` or a confirmation. Omit it to start from a default, which then needs `-o`.
    #[arg(
        value_name = "FILE|BANK:SLOT",
        required_unless_present_any = ["fields", "out"],
    )]
    pub target: Option<String>,

    #[command(flatten)]
    pub common: edit::SetArgs,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let ui = Ui::new(cli.color);
    device::set_recording(cli.record);

    let result = match cli.command {
        Command::Inspect { files, raw } => inspect(&ui, &files, raw),
        Command::Verify { files } => verify(&ui, &files),
        Command::Edit(args) => file_edit::run(&ui, args),
        Command::Device { action } => match action {
            DeviceAction::Status { replay, json } => {
                let source = match replay {
                    Some(path) => device::Source::Replay(path),
                    None => device::Source::Usb,
                };
                device::status(&ui, source, json)
            }
            DeviceAction::Info => device::info(&ui),
            DeviceAction::Recover => device::recover(&ui),
            DeviceAction::Geometry => device::geometry(&ui),
            #[cfg(feature = "wedge")]
            DeviceAction::Wedge { class, yes } => {
                device::wedge(&ui, ObjectClass::from_raw(class), yes)
            }
            DeviceAction::Controls {
                from,
                to,
                len,
                interface,
                value,
                index,
            } => device::controls(&ui, from, to, len, interface, value, index),
        },
        Command::Program { action } => match action {
            ProgramAction::Slot(action) => slot_action(&ui, action, ObjectClass::Program),
            ProgramAction::Edit(args) => edit::run(&ui, args, ObjectClass::Program),
        },
        Command::Sample { action } => match action {
            SampleAction::Slot(action) => slot_action(&ui, action, ObjectClass::Sample),
            SampleAction::Edit(args) => sample::run(&ui, args),
            SampleAction::Decode(args) => sample::decode(&ui, args),
            SampleAction::Encode(args) => sample::encode(&ui, args),
            SampleAction::Build(args) => sample::build(&ui, args),
            SampleAction::Verify(args) => sample::verify(&ui, args),
            SampleAction::Project { action } => match action {
                SampleProjectAction::New(args) => sample::project_new(&ui, args),
            },
        },
        Command::Piano { action } => match action {
            PianoAction::Slot(action) => slot_action(&ui, action, ObjectClass::Piano),
            PianoAction::Inspect(args) => piano::inspect(&ui, args),
            PianoAction::Decode(args) => piano::decode(&ui, args),
            PianoAction::Edit(args) => piano::edit(&ui, args),
            PianoAction::Trim(args) => piano::trim(&ui, args),
            PianoAction::Split(args) => piano::split(&ui, args),
            PianoAction::Build(args) => piano::build(&ui, args),
            PianoAction::Rebuild(args) => piano::rebuild(&ui, args),
            PianoAction::Verify(args) => piano::verify(&ui, args),
        },
        Command::Setlist { action } => match action {
            SetlistAction::Slot(action) => slot_action(&ui, action, ObjectClass::SetList),
            SetlistAction::Edit(args) => edit::run(&ui, args, ObjectClass::SetList),
        },
        Command::Live { action } => match action {
            LiveAction::Slot(action) => slot_action(&ui, action.into(), ObjectClass::Live),
            LiveAction::Edit(args) => edit::run(&ui, args, ObjectClass::Live),
        },
        Command::Settings { action } => match action {
            SettingsAction::Slot(action) => slot_action(&ui, action.into(), ObjectClass::Settings),
            SettingsAction::Edit(args) => edit::run(&ui, args, ObjectClass::Settings),
        },
        Command::Raw { class, action } => slot_action(&ui, action, ObjectClass::from_raw(class)),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            ui.note(format!("{}: {e}", ui.danger("error")));
            ExitCode::FAILURE
        }
    }
}

impl From<SettingsSlotAction> for SlotAction {
    fn from(action: SettingsSlotAction) -> SlotAction {
        match action {
            SettingsSlotAction::Get {
                at,
                out,
                body,
                sweep,
            } => SlotAction::Get {
                at,
                out,
                body,
                sweep,
            },
            SettingsSlotAction::Info { at } => SlotAction::Info { at },
        }
    }
}

impl From<LiveSlotAction> for SlotAction {
    fn from(action: LiveSlotAction) -> SlotAction {
        match action {
            LiveSlotAction::Get {
                at,
                out,
                body,
                sweep,
            } => SlotAction::Get {
                at,
                out,
                body,
                sweep,
            },
            LiveSlotAction::Info { at } => SlotAction::Info { at },
            LiveSlotAction::Deps { at } => SlotAction::Deps { at },
        }
    }
}

/// `nord raw --class` help, naming every class [`ObjectClass::from_raw`] recognizes.
fn class_help() -> String {
    // Every class `from_raw` names has a one-byte code.
    let named: Vec<String> = (0..=u8::MAX.into())
        .map(ObjectClass::from_raw)
        .filter(|class| !matches!(class, ObjectClass::Unknown(_)))
        .map(|class| format!("{} {}", class.to_raw(), class.label()))
        .collect();
    format!("Object class: {}", named.join(", "))
}

/// Dispatch one verb against a fixed object class, whichever noun asked for it.
///
/// The read-only verbs take a file as well as a slot ([`slot::Target`]); the rest name
/// device storage, which no file stands in for.
fn slot_action(ui: &Ui, action: SlotAction, class: ObjectClass) -> Result<(), String> {
    match action {
        SlotAction::Get {
            at,
            out,
            body,
            sweep,
        } => match slot::target(&at)? {
            slot::Target::File(path) if sweep => Err(format!(
                "--sweep re-reads the instrument as the panel changes; {} has only one state",
                path.display()
            )),
            slot::Target::File(path) => file::get(ui, &path, out, class, body),
            slot::Target::Slot(at) => match (sweep, out) {
                (true, Some(dir)) => device::sweep(ui, at, dir, class, body),
                (true, None) => Err("--sweep fills a directory; give -o a path".into()),
                (false, out) => device::get(ui, at, out, class, body),
            },
        },
        SlotAction::Put { file, at, yes } => device::put(ui, file, slot::parse(&at)?, class, yes),
        SlotAction::Move { from, to, yes } => {
            device::move_object(ui, slot::parse(&from)?, slot::parse(&to)?, class, yes)
        }
        SlotAction::Rename { at, name, yes } => {
            device::rename(ui, slot::parse(&at)?, name, class, yes)
        }
        SlotAction::Duplicate { from, to, yes } => {
            device::duplicate(ui, slot::parse(&from)?, slot::parse(&to)?, class, yes)
        }
        SlotAction::Delete { slots, yes } => {
            device::delete(ui, &slot::parse_all(&slots)?, class, yes)
        }
        SlotAction::Select { at } => device::select(ui, slot::parse(&at)?, class),
        SlotAction::Info { at } => match slot::target(&at)? {
            slot::Target::File(path) => file::info(ui, &path, class),
            slot::Target::Slot(at) => device::slot_info(ui, at, class),
        },
        SlotAction::Deps { at } => match slot::target(&at)? {
            slot::Target::File(path) => file::deps(ui, &path, class),
            slot::Target::Slot(at) => device::deps(ui, at, class),
        },
        SlotAction::Focus => device::focus(ui, class),
        SlotAction::List => device::list(ui, class),
        SlotAction::Probe {
            op,
            args,
            wait,
            yes,
            bare,
            service,
            subsystem,
        } => device::probe(ui, class, op, &args, wait, yes, bare, service, subsystem),
    }
}

fn inspect(ui: &Ui, files: &[PathBuf], raw: bool) -> Result<(), String> {
    let mut failed = 0usize;
    for (i, path) in files.iter().enumerate() {
        if i > 0 {
            ui.out("");
        }
        ui.out(path.display());
        match nord_format::from_path(path) {
            Ok(entity) if raw => ui.out(format!("{entity:#?}")),
            Ok(entity) => summary::print(ui, &entity),
            Err(e) => {
                ui.note(format!("  error: {e}"));
                failed += 1;
            }
        }
    }
    match failed {
        0 => Ok(()),
        n => Err(format!("{n} of {} file(s) did not parse", files.len())),
    }
}

/// Decode each file and re-encode it, checking that the bytes match.
///
/// Reports the offset of the first difference, which in a bit-packed format usually
/// identifies the field.
fn verify(ui: &Ui, files: &[PathBuf]) -> Result<(), String> {
    file::check_each(ui, files, "file(s) did not round-trip", |path| {
        let named = |e: &dyn std::fmt::Display| format!("error  {} ({e})", path.display());
        let original = std::fs::read(path).map_err(|e| named(&e))?;
        let reencoded = nord_format::from_path(path)
            .and_then(|entity| nord_format::to_bytes(&entity))
            .map_err(|e| named(&e))?;
        if reencoded == original {
            return Ok(format!(
                "ok     {} ({} bytes)",
                path.display(),
                original.len()
            ));
        }
        Err(format!(
            "DIFFER {} (in {} bytes, out {}; first difference at {})",
            path.display(),
            original.len(),
            reencoded.len(),
            file::first_difference(&reencoded, &original),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_class_help_names_the_settings_singleton() {
        let help = class_help();
        assert!(help.contains("7 settings"), "{help}");
    }
}
