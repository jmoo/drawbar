//! `nord` — a thin CLI over [`nord_format`], purely to dogfood the library.
//!
//! Not a product; it exists to exercise the parser and surface API friction.
//! For now it does one thing: parse Nord file(s) and print what was decoded.

mod device;

use clap::{Parser, Subcommand};
use nord_format::common::bank::Item;
use nord_format::electro5::{Instrument, OrganModel};
use nord_format::{Entity, Program, Settings, Song};
use std::path::PathBuf;
use std::process::ExitCode;

/// Object class for programs. `nord program` fixes it so the subcommands need no
/// `--class`; `nord device` keeps the flag for the other classes.
const PROGRAM_CLASS: u32 = 4;

#[derive(Parser)]
#[command(name = "nord", about = "Inspect Clavia / Nord keyboard files", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Parse Nord file(s) and print a summary of the decoded contents.
    Inspect {
        /// Files to read (.ne5p program, .ne5t song, .ne5s settings, .npno
        /// piano, .nsmp sample, or a ZIP backup bundle).
        #[arg(required = true)]
        files: Vec<PathBuf>,

        /// Dump the full `Debug` representation instead of the summary.
        #[arg(long)]
        raw: bool,
    },

    /// Re-encode file(s) and check the result is byte-identical to the input.
    ///
    /// The crate's central invariant: decoded values are read-only views over a
    /// verbatim body, so a parse followed by a re-emit cannot drift. Nothing else in
    /// this CLI exercises the write path, so this is the only place that would catch a
    /// field being reconstructed rather than preserved.
    Verify {
        /// Files to round-trip. Bundles are archives, not re-emittable entities.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },

    /// Work with programs on an attached instrument. Slots are `BANK:SLOT`, as the
    /// instrument displays them.
    ///
    /// The same operations `nord device` offers, scoped to programs so no `--class` is
    /// needed, and with `get` defaulting to a readable summary rather than a file.
    Program {
        #[command(subcommand)]
        action: ProgramAction,
    },

    /// Talk to an attached instrument over USB: inventory, read, write, and organise
    /// slots. Mutating actions require --yes.
    Device {
        #[command(subcommand)]
        action: DeviceAction,
    },
}

#[derive(Subcommand)]
enum ProgramAction {
    /// Read a program off the instrument. Read-only.
    ///
    /// Prints a summary by default; with `--out` writes the `.ne5p` file instead.
    Get {
        /// Slot to read, e.g. 7:4.
        #[arg(value_name = "BANK:SLOT")]
        at: String,

        /// Write the program to this file instead of printing a summary.
        #[arg(short, long, value_name = "FILE")]
        out: Option<PathBuf>,
    },

    /// Write a .ne5p into a slot, OVERWRITING it. Requires --yes.
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

    /// Move a program between slots. OVERWRITES the destination. Requires --yes.
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

    /// Delete one or more program slots. Requires --yes.
    Delete {
        /// Slots to delete, e.g. 7:50 (repeatable).
        #[arg(value_name = "BANK:SLOT", required = true)]
        slots: Vec<String>,

        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum DeviceAction {
    /// Report what is stored on the instrument, per object class.
    ///
    /// Read-only: this sends one query per class and reads counters back. Nothing
    /// on the instrument is modified.
    Status {
        /// Replay a recorded exchange instead of opening a device. Useful for
        /// demos and for exercising the whole path without hardware.
        #[arg(long, value_name = "SCRIPT")]
        replay: Option<PathBuf>,

        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },

    /// Report everything the instrument knows about one slot. Read-only.
    ///
    /// Shows the fields the CBIN header carries but the wire never transmits — format
    /// tag, version, CRC-32 — plus the slot name, which no file stores at all.
    Info {
        /// Slot as shown on the instrument, e.g. 7-4.
        #[arg(value_name = "BANK-SLOT")]
        at: String,

        /// Object class: 4 programs (default), 5 set lists, 1 pianos, 3 samples.
        #[arg(long, default_value_t = 4)]
        class: u32,
    },

    /// Read a program off the instrument into a .ne5p file. Read-only.
    Read {
        /// Slot as shown on the instrument, e.g. 7-4 for bank 7 slot 4.
        #[arg(value_name = "BANK-SLOT")]
        at: String,

        /// Output file. Defaults to the slot's name.
        #[arg(short, long)]
        out: Option<PathBuf>,

        /// Object class: 4 programs (default), 5 set lists, 1 pianos, 3 samples.
        #[arg(long, default_value_t = 4)]
        class: u32,

        /// Save the wire body verbatim instead of wrapping it in a CBIN header.
        /// Use for formats whose header layout is not yet known.
        #[arg(long)]
        raw: bool,
    },

    /// Write a .ne5p into a slot, OVERWRITING it. Requires --yes.
    Write {
        /// The file to send.
        file: PathBuf,

        /// Destination slot, e.g. 7-4.
        #[arg(value_name = "BANK-SLOT")]
        at: String,

        /// Confirm the overwrite. Without this the command stops after reporting
        /// what currently occupies the slot.
        #[arg(long)]
        yes: bool,
    },

    /// Move an object between slots. OVERWRITES the destination. Requires --yes.
    Move {
        /// Source slot, e.g. 8-13.
        #[arg(value_name = "FROM")]
        from: String,
        /// Destination slot, e.g. 7-16.
        #[arg(value_name = "TO")]
        to: String,
        #[arg(long, default_value_t = 4)]
        class: u32,
        #[arg(long)]
        yes: bool,
    },

    /// Delete one or more slots from the instrument. Requires --yes.
    Delete {
        /// Slots to delete, e.g. 7-50 (repeatable).
        #[arg(value_name = "BANK-SLOT", required = true)]
        slots: Vec<String>,
        #[arg(long, default_value_t = 4)]
        class: u32,
        #[arg(long)]
        yes: bool,
    },

    /// Rename the object in a slot. Requires --yes.
    Rename {
        /// Slot to rename, e.g. 6-13.
        #[arg(value_name = "BANK-SLOT")]
        at: String,
        /// The new name.
        name: String,
        #[arg(long, default_value_t = 4)]
        class: u32,
        #[arg(long)]
        yes: bool,
    },

    /// Duplicate an object into another slot (device-internal deep copy). Requires --yes.
    Duplicate {
        /// Source slot, e.g. 7-2.
        #[arg(value_name = "FROM")]
        from: String,
        /// Destination slot, e.g. 7-3.
        #[arg(value_name = "TO")]
        to: String,
        #[arg(long, default_value_t = 4)]
        class: u32,
        #[arg(long)]
        yes: bool,
    },

    /// Load an object live on the instrument (double-click in NSM). Non-destructive.
    Select {
        /// Slot to load, e.g. 2-12.
        #[arg(value_name = "BANK-SLOT")]
        at: String,
        #[arg(long, default_value_t = 4)]
        class: u32,
    },

    /// List the piano/sample library objects an entity depends on. Read-only.
    Deps {
        /// Slot to inspect, e.g. 7-3.
        #[arg(value_name = "BANK-SLOT")]
        at: String,
        #[arg(long, default_value_t = 4)]
        class: u32,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Command::Inspect { files, raw } => {
            let mut ok = true;
            for (i, path) in files.iter().enumerate() {
                if i > 0 {
                    println!();
                }
                println!("{}", path.display());
                match nord_format::from_path(path) {
                    Ok(entity) if raw => println!("{entity:#?}"),
                    Ok(entity) => print_summary(&entity),
                    Err(e) => {
                        eprintln!("  error: {e}");
                        ok = false;
                    }
                }
            }
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }

        Command::Verify { files } => match verify(&files) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },

        Command::Program { action } => {
            let result = match action {
                ProgramAction::Get { at, out } => {
                    device::parse_location(&at).and_then(|at| device::program_get(at, out))
                }
                ProgramAction::Put { file, at, yes } => {
                    device::parse_location(&at).and_then(|at| device::write(file, at, yes))
                }
                ProgramAction::Move { from, to, yes } => device::parse_location(&from)
                    .and_then(|from| Ok((from, device::parse_location(&to)?)))
                    .and_then(|(from, to)| device::move_object(from, to, PROGRAM_CLASS, yes)),
                ProgramAction::Delete { slots, yes } => slots
                    .iter()
                    .map(|s| device::parse_location(s))
                    .collect::<Result<Vec<_>, _>>()
                    .and_then(|locs| device::delete(&locs, PROGRAM_CLASS, yes)),
            };
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            }
        }

        Command::Device { action } => {
            let result = match action {
                DeviceAction::Status { replay, json } => {
                    let source = match replay {
                        Some(path) => device::Source::Replay(path),
                        None => device::Source::Usb,
                    };
                    device::run(source, json)
                }
                DeviceAction::Read { at, out, class, raw } => {
                    device::parse_location(&at).and_then(|at| device::read(at, out, class, raw))
                }
                DeviceAction::Write { file, at, yes } => {
                    device::parse_location(&at).and_then(|at| device::write(file, at, yes))
                }
                DeviceAction::Move { from, to, class, yes } => device::parse_location(&from)
                    .and_then(|from| Ok((from, device::parse_location(&to)?)))
                    .and_then(|(from, to)| device::move_object(from, to, class, yes)),
                DeviceAction::Delete { slots, class, yes } => slots
                    .iter()
                    .map(|s| device::parse_location(s))
                    .collect::<Result<Vec<_>, _>>()
                    .and_then(|locs| device::delete(&locs, class, yes)),
                DeviceAction::Rename { at, name, class, yes } => {
                    device::parse_location(&at).and_then(|at| device::rename(at, name, class, yes))
                }
                DeviceAction::Duplicate { from, to, class, yes } => device::parse_location(&from)
                    .and_then(|from| Ok((from, device::parse_location(&to)?)))
                    .and_then(|(from, to)| device::duplicate(from, to, class, yes)),
                DeviceAction::Select { at, class } => {
                    device::parse_location(&at).and_then(|at| device::select(at, class))
                }
                DeviceAction::Info { at, class } => {
                    device::parse_location(&at).and_then(|at| device::info(at, class))
                }
                DeviceAction::Deps { at, class } => {
                    device::parse_location(&at).and_then(|at| device::deps(at, class))
                }
            };
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

/// One-indexed `bank N slot M` — matches how the hardware labels locations.
fn location(x: u16, y: u16) -> String {
    format!("bank {} slot {}", x + 1, y + 1)
}

fn yn(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

/// Parse each file and re-emit it, checking the bytes come back identical.
///
/// Reports the offset of the first difference rather than just "differs": for a
/// bit-packed format that offset is usually enough to name the field on its own.
fn verify(files: &[PathBuf]) -> Result<(), String> {
    let mut failed = 0usize;
    for path in files {
        let original = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                println!("error  {} ({e})", path.display());
                failed += 1;
                continue;
            }
        };
        let reencoded = nord_format::from_path(path)
            .and_then(|mut entity| nord_format::to_bytes(&mut entity));
        match reencoded {
            Ok(bytes) if bytes == original => {
                println!("ok     {} ({} bytes)", path.display(), original.len());
            }
            Ok(bytes) => {
                failed += 1;
                let at = bytes
                    .iter()
                    .zip(&original)
                    .position(|(a, b)| a != b)
                    .map(|i| format!("{i:#x}"))
                    .unwrap_or_else(|| "the end (length differs)".to_string());
                println!(
                    "DIFFER {} (in {} bytes, out {}; first difference at {at})",
                    path.display(),
                    original.len(),
                    bytes.len(),
                );
            }
            Err(e) => {
                failed += 1;
                println!("error  {} ({e})", path.display());
            }
        }
    }
    match failed {
        0 => Ok(()),
        n => Err(format!("{n} of {} file(s) did not round-trip", files.len())),
    }
}

/// Effect type names indexed by the value **as stored**, which is rotated relative to
/// the panel's own ordering (stored 0 is trem 1, not pan 1). The rotation is pinned by
/// the named specimens in `nord-corpus/ne5/programs/fx/` and the mapping in
/// nord-format's `tests/ne5.rs` — it is not inferred from the panel layout.
const FX1_TYPES: [&str; 8] =
    ["trem 1", "trem 2", "trem 1&2", "pan 1", "pan 2", "pan 1&2", "wah", "rm"];
const FX2_TYPES: [&str; 6] = ["phaser 1", "phaser 2", "flanger", "chorus 1", "chorus 2", "vibe"];
const FX3_TYPES: [&str; 6] = ["none", "small", "jc", "twin", "rotary", "comp"];
const FX5_TYPES: [&str; 5] = ["room", "stage soft", "stage", "hall soft", "hall"];

/// Render a stored `0..127` value as the `0..10` the panel shows.
///
/// Confirmed linear against hardware: reverb wet reads `43` in the file and the panel
/// shows 3.4, and `43 / 127 * 10 = 3.39`. Only used for the fields the corpus README
/// documents as `0..10` — fx2's rate is in Hz and delay tempo runs backwards in
/// milliseconds, so those stay raw rather than being given false units.
fn knob(v: u8) -> String {
    format!("{} ({:.1})", v, f32::from(v) / 127.0 * 10.0)
}

fn fx_type(table: &[&str], v: u8) -> String {
    table.get(v as usize).map_or_else(|| format!("unknown ({v})"), |s| (*s).to_string())
}

/// Which part an effect is routed to.
///
/// The stored value is **one higher** than the panel position: the corpus specimens are
/// named `fx1_1..` for lower and `fx1_2..` for upper, and nord-format's test asserts
/// `fx1 == digit + 1`, so `1` is off, `2` lower, `3` upper. The comment in
/// `program.rs` reads `0: off, 1: lower, 2: upper`, which describes the *panel*
/// numbering rather than the byte — reading it as the stored value shifts every label
/// by one and turns "upper" into "both".
///
/// `None` means not engaged; the settings are still in the file, they just do nothing.
fn routed(sel: u8) -> Option<&'static str> {
    match sel {
        2 => Some("lower"),
        3 => Some("upper"),
        4 => Some("both"),
        _ => None,
    }
}

/// Format a library dependency id the way it is worth reading: hex, matching what
/// `nord device deps` reports for the same program.
fn dep_id(id: u32) -> String {
    match id {
        0 => "none".to_string(),
        id => format!("{id:#010x}"),
    }
}

pub(crate) fn print_summary(entity: &Entity) {
    match entity {
        Entity::Program(Program::Electro5(p)) => {
            let l = p.location();
            let split = if p.split() {
                format!("yes @ {:?}", p.split_point())
            } else {
                "no".to_string()
            };
            println!("  type:      Electro 5 program (ne5p)");
            println!("  location:  {}", location(l.x(), l.y()));
            println!(
                "  lower:     {:?}  octave {:+}  sustain {}  control {}",
                p.lower_part(),
                p.lower_octave_shift().inner(),
                yn(p.lower_sustain()),
                yn(p.lower_control()),
            );
            println!(
                "  upper:     {:?}  octave {:+}  sustain {}  control {}",
                p.upper_part(),
                p.upper_octave_shift().inner(),
                yn(p.upper_sustain()),
                yn(p.upper_control()),
            );
            println!("  split:     {split}");
            println!(
                "  transpose: {:+}  ({})",
                p.transpose().inner(),
                yn(p.transpose_enabled()),
            );
            println!("  part mix:  {} (lower/upper %)", p.part_mix().as_string());
            println!("  gain:      {}", p.gain());

            let (piano, sample) = (p.piano(), p.sample());
            println!(
                "  piano:     category {}  model {}  clav {}  acoustics {}  touch {}  mono {}",
                piano.category,
                piano.piano_model,
                piano.clav_model,
                piano.acoustics,
                piano.touch,
                yn(piano.mono),
            );
            println!(
                "  sample:    number {}  attack {}  decay/rel {}  dynamics {}  filter {}",
                sample.number, sample.attack, sample.decay_release, sample.dynamics,
                yn(sample.filter),
            );
            // The two library references. `nord device deps` reports these same ids
            // for this program with the piano's and sample's *names* attached — the
            // file itself stores only the id, so that is the only way to resolve them.
            println!(
                "  depends:   piano {}  sample {}",
                dep_id(piano.id),
                dep_id(sample.id),
            );

            // Effects. Values are printed as stored (0..127); the panel shows most of
            // them on a 0..10 scale, and the two do not map linearly for delay tempo,
            // so rescaling here would invent precision the file does not carry.
            let fx = p.fx_panel();
            let extra = p.extra();
            println!("  fx:        stored value, with the panel's 0-10 reading where it applies");
            match routed(fx.fx1) {
                Some(part) => println!(
                    "    fx1   {part:<5}  {:<9}  rate {}  control {}",
                    fx_type(&FX1_TYPES, fx.fx1_type),
                    knob(fx.fx1_rate),
                    yn(extra.fx1_control),
                ),
                None => println!("    fx1   off"),
            }
            match routed(fx.fx2) {
                Some(part) => println!(
                    "    fx2   {part:<5}  {:<9}  rate {}  deep {}",
                    fx_type(&FX2_TYPES, fx.fx2_type),
                    fx.fx2_rate,
                    yn(extra.fx2_deep),
                ),
                None => println!("    fx2   off"),
            }
            match routed(fx.fx3) {
                Some(part) => println!(
                    "    fx3   {part:<5}  {:<9}  compression {}",
                    fx_type(&FX3_TYPES, fx.fx3_type),
                    knob(fx.fx3_compression),
                ),
                None => println!("    fx3   off"),
            }
            match routed(fx.fx4) {
                Some(part) => println!(
                    "    delay {part:<5}  feedback {}  tempo {}  wet {}  ping-pong {}",
                    fx.fx4_feedback,
                    fx.fx4_tempo,
                    knob(fx.fx4_moisture),
                    yn(fx.fx4_ping_pong),
                ),
                None => println!("    delay off"),
            }
            // ⚠️ The reverb enable bit reads `true` in every `fx5_1xx` specimen, and the
            // corpus README's "0: on, 1: off" would make all of them captures of a
            // *disabled* reverb whose type and wet level were then varied — which would
            // be pointless. Rendered as on/off accordingly; worth one confirmation
            // against the panel.
            if fx.fx5 {
                println!(
                    "    reverb {:<9}  wet {}",
                    fx_type(&FX5_TYPES, fx.fx5_type),
                    knob(fx.fx5_moisture),
                );
            } else {
                println!("    reverb off");
            }
            // The EQ enable and its routing live in different words: the enable is a
            // bit at 0x98, the lower/upper/both choice is at 0xa1. `0` for the routing
            // means *lower*, not off, so the enable has to be checked first.
            if fx.equalizer_on {
                // Every observed program uses 0, 1 or 2; name the raw value rather
                // than guessing a label if the instrument ever produces another.
                let part = match extra.equalizer_part {
                    0 => "lower".to_string(),
                    1 => "upper".to_string(),
                    2 => "lower+upper".to_string(),
                    n => format!("unknown ({n})"),
                };
                println!(
                    "    eq    {part:<11}  bass {}  freq {}  gain {}  treble {}",
                    fx.equalizer_bass,
                    fx.equalizer_freq,
                    fx.equalizer_freq_gain,
                    fx.equalizer_treble,
                );
            } else {
                println!("    eq    off");
            }
            println!(
                "    rotary speed {}  stop {}",
                if fx.rotary_speed == 0 { "slow" } else { "fast" },
                if fx.rotary_stop == 0 { "off" } else { "on" },
            );

            // Organ. Both presets are shown for every model: the file keeps the full
            // state of all four, and b3+bass cannot be checked at all without seeing
            // preset 1 and preset 2 side by side.
            if p.lower_part() == Instrument::Organ || p.upper_part() == Instrument::Organ {
                let o = p.organ();
                let selected = p.organ_type();
                let sel_name = match selected {
                    0 => "b3",
                    1 => "b3+bass",
                    2 => "pipe",
                    3 => "vox",
                    4 => "farfisa",
                    _ => "unknown",
                };
                // b3+bass shares the B3's storage, so it marks the B3 rows.
                let sel_model = match selected {
                    0 | 1 => Some(OrganModel::B3),
                    2 => Some(OrganModel::Pipe),
                    3 => Some(OrganModel::Vox),
                    4 => Some(OrganModel::Farfisa),
                    _ => None,
                };
                println!("  organ:     {sel_name} selected (*), drawbar positions 0-8");
                for (model, label) in [
                    (OrganModel::B3, "b3"),
                    (OrganModel::Vox, "vox"),
                    (OrganModel::Farfisa, "farf"),
                    (OrganModel::Pipe, "pipe"),
                ] {
                    for preset in 1..=2u8 {
                        let mark = if Some(model) == sel_model { "*" } else { " " };
                        let live = if o.preset(model) == preset { "<" } else { " " };

                        // In b3+bass, preset 1 is the bass manual: two drawbars, kept
                        // outside the nine-nibble block. The nine nibbles are stale
                        // there, so printing them would be actively misleading.
                        let bars = if selected == 1 && model == OrganModel::B3 && preset == 1 {
                            // Only two of the nine positions exist on the bass manual;
                            // dots for the rest so it lines up with the other rows and
                            // cannot be misread as a nine-drawbar registration.
                            let b = o.b3_bass_drawbars();
                            format!("{}{}.......", b[0], b[1])
                        } else if model == OrganModel::Farfisa {
                            // Farfisa's drawbars are on/off tabs on the panel, but the
                            // file still stores nine positions and the low bits of each
                            // vary independently of the on/off threshold. Show both, or
                            // the display silently discards them.
                            let tabs: String = o
                                .farfisa_tabs(preset)
                                .iter()
                                .map(|on| if *on { '|' } else { '.' })
                                .collect();
                            let pos: String =
                                o.drawbars(model, preset).iter().map(u8::to_string).collect();
                            format!("{tabs} ({pos})")
                        } else {
                            o.drawbars(model, preset).iter().map(u8::to_string).collect()
                        };

                        let vib = match o.vib_type(model) {
                            Some(v) if o.vib_on(model, preset) => format!("  vib {v:?}"),
                            Some(_) => "  vib off".to_string(),
                            None => String::new(),
                        };
                        let perc = if model == OrganModel::B3 {
                            if o.b3_perc_on(preset) {
                                let third = if o.b3_perc_third() { " +3rd" } else { "" };
                                format!("  perc {:?}{third}", o.b3_perc_speed())
                            } else {
                                "  perc off".to_string()
                            }
                        } else {
                            String::new()
                        };
                        println!("   {mark}{label:<5} p{preset}{live} {bars}{vib}{perc}");
                    }
                }
                println!("   (* = selected model, < = its active preset)");
            }
        }
        Entity::Song(Song::Electro5(s)) => {
            let l = s.location();
            println!("  type:      Electro 5 song / set (ne5t)");
            println!("  location:  {}", location(l.x(), l.y()));
            for slot in 0..4u16 {
                let p = s.get(slot);
                println!("    slot {}:  program {}", slot + 1, location(p.x(), p.y()));
            }
        }
        Entity::Settings(Settings::Electro5(s)) => {
            println!("  type:      Electro 5 settings (ne5s)");
            println!("  note:      field decode pending specimens; raw body below");
            let hex: String = s
                .raw()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(" ");
            println!("  body:      {hex}");
        }
        Entity::Piano(_) => {
            println!("  type:      piano (npno) — header/reference only");
        }
        Entity::Sample(_) => {
            println!("  type:      sample (nsmp) — header/reference only");
        }
        Entity::Bundle(nord_format::Bundle::Electro5(b)) => {
            println!("  type:      backup bundle (zip)");
            if let Some(name) = b.name() {
                println!("  name:      {name}");
            }
            println!("  note:      use --raw to list contained programs/songs");
            let _ = (b.programs(), b.songs()); // decoded; shown via --raw
        }
    }
}
