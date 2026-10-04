//! `nord convert`: one instrument file into another format, with every loss reported
//! before anything is written.
//!
//! [`nord_format::convert`] decides what converts and how. This module turns its open
//! choices into flags and enforces the command's contract: a loss with no alternative
//! is a failed check that `--force` passes, a loss with alternatives needs its flag
//! even with `--force`, and the input is never overwritten.

use std::path::{Path, PathBuf};

use clap::{Args, ValueEnum};
use nord_format::convert::{
    self, Choice, Choices, GainChoice, Line, LoopMarkChoice, NameChoice, OverlapChoice, Plan,
    Target,
};
use nord_format::formats::nsmp::codec::Layout;
use nord_usb::ObjectClass;

use crate::edit::{same_file, write_file};
use crate::slot;
use crate::ui::Ui;

#[derive(Args)]
pub struct ConvertArgs {
    /// The instrument to convert: a file, or a slot on the instrument (`3:14`), which is
    /// read and never written.
    #[arg(value_name = "FILE|BANK:SLOT")]
    pub input: String,

    /// The format to write, by its extension.
    #[arg(long, value_enum)]
    pub to: To,

    /// Where to write. Defaults to the input's path with the target's extension; a slot
    /// needs it.
    #[arg(short, long, value_name = "FILE")]
    pub out: Option<PathBuf>,

    /// Report what the conversion drops, changes and fills in, make every check a
    /// conversion makes, and write nothing.
    #[arg(long)]
    pub dry_run: bool,

    /// Write even though the target drops values it has no place for. A loss the
    /// target can meet more than one way still needs its own flag.
    #[arg(long)]
    pub force: bool,

    /// Acknowledge that no v3 or v4 file from this tool has been played on an
    /// instrument. Required to write `nsmp3` or `nsmp4`.
    #[arg(long)]
    pub unverified: bool,

    /// Replace the file `-o` names when it exists.
    #[arg(long)]
    pub yes: bool,

    /// A zone gain past what a v2 zone record holds: store the largest it holds
    /// (`clamp`), or store that and scale the audio by the rest (`bake`).
    #[arg(long, value_enum, value_name = "HOW")]
    pub gain: Option<Gain>,

    /// Keep as much of a name as the target's field holds.
    #[arg(long, conflicts_with = "name")]
    pub truncate_name: bool,

    /// Name the converted instrument.
    #[arg(long)]
    pub name: Option<String>,

    /// Zones whose key ranges overlap, going to v2, where zones tile: the lower or
    /// the upper zone keeps the shared keys.
    #[arg(long, value_enum, value_name = "ZONE")]
    pub overlap: Option<Overlap>,

    /// A loop mark nearer the resync point than v2 allows: move the resync point
    /// earlier (`resync`), or move the mark later (`push`). Either plays the same.
    #[arg(long, value_enum, value_name = "HOW")]
    pub loop_mark: Option<LoopMark>,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum To {
    Nsmp,
    Nsmp3,
    Nsmp4,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Gain {
    Clamp,
    Bake,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Overlap {
    Lower,
    Upper,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum LoopMark {
    Resync,
    Push,
}

impl To {
    fn layout(self) -> Layout {
        match self {
            To::Nsmp => Layout::V2,
            To::Nsmp3 => Layout::V3,
            To::Nsmp4 => Layout::V4,
        }
    }
}

/// The flag that answers a choice, and the values it takes.
fn flag(choice: Choice) -> &'static str {
    match choice {
        Choice::Gain => "--gain clamp|bake",
        Choice::Name => "--truncate-name or --name NEW",
        Choice::Overlap => "--overlap lower|upper",
        Choice::LoopMark => "--loop-mark resync|push",
    }
}

fn choices(args: &ConvertArgs) -> Choices {
    Choices {
        gain: args.gain.map(|gain| match gain {
            Gain::Clamp => GainChoice::Clamp,
            Gain::Bake => GainChoice::Bake,
        }),
        name: match (&args.name, args.truncate_name) {
            (Some(name), _) => Some(NameChoice::Rename(name.clone())),
            (None, true) => Some(NameChoice::Truncate),
            (None, false) => None,
        },
        overlap: args.overlap.map(|overlap| match overlap {
            Overlap::Lower => OverlapChoice::Lower,
            Overlap::Upper => OverlapChoice::Upper,
        }),
        loop_mark: args.loop_mark.map(|mark| match mark {
            LoopMark::Resync => LoopMarkChoice::Resync,
            LoopMark::Push => LoopMarkChoice::Push,
        }),
    }
}

/// Plan, report, and write unless a check refuses. A dry run makes every check and
/// writes nothing, so it fails exactly where the conversion would.
pub fn run(ui: &Ui, args: ConvertArgs) -> Result<(), String> {
    let layout = args.to.layout();
    let origin = slot::target(&args.input)?;
    let bytes = match &origin {
        slot::Target::File(path) => {
            std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?
        }
        slot::Target::Slot(at) => crate::device::fetch(*at, ObjectClass::Sample)?,
    };
    let entity = nord_format::from_stream(&mut std::io::Cursor::new(&bytes))
        .map_err(|e| format!("{}: {e}", args.input))?;
    let plan = convert::plan(&entity, Target::Nsmp(layout), &choices(&args))
        .map_err(|e| format!("{}: {e}", args.input))?;
    report(ui, &plan);
    if !plan.open().is_empty() {
        return Err(open(&plan));
    }
    if layout != Layout::V2 && !args.unverified {
        return Err(format!(
            "no {} file from this tool has been played on an instrument. Pass --unverified \
             to write it anyway.",
            layout.generation()
        ));
    }
    if !plan.report().dropped.is_empty() && !args.force {
        return Err(format!(
            "{} value(s) would be dropped; pass --force to convert anyway",
            plan.report().dropped.len()
        ));
    }
    if args.dry_run {
        return Ok(());
    }
    let path = output(&origin, args.out.as_deref(), args.to)?;
    if let slot::Target::File(input) = &origin {
        if same_file(input, &path) {
            return Err(format!(
                "{}: a conversion never overwrites its input; name another file with -o",
                path.display()
            ));
        }
    }
    if path.exists() {
        ui.note(format!(
            "about to {} {}",
            ui.danger("replace"),
            path.display()
        ));
        ui.confirm(args.yes)?;
    }
    let converted = plan.apply().map_err(|e| e.to_string())?;
    let out = nord_format::to_bytes(&nord_format::Entity::Sample(converted))
        .map_err(|e| e.to_string())?;
    write_file(ui, &path, &out)
}

/// The report on stderr, grouped as the user acts on it.
fn report(ui: &Ui, plan: &Plan) {
    let report = plan.report();
    for (heading, lines) in [
        ("dropped", &report.dropped),
        ("changed", &report.changed),
        ("from rules", &report.from_rules),
    ] {
        if lines.is_empty() {
            continue;
        }
        ui.note(ui.bold(heading));
        for line in lines {
            ui.note(format!("  {}", shown(line)));
        }
    }
}

fn shown(line: &Line) -> String {
    format!("{}: {} ({})", line.field, line.value, line.reason)
}

/// The refusal for open choices: each choice once, with its flag and the fields that
/// need it.
fn open(plan: &Plan) -> String {
    let mut choices: Vec<Choice> = Vec::new();
    for open in plan.open() {
        if !choices.contains(&open.choice) {
            choices.push(open.choice);
        }
    }
    let lines: Vec<String> = choices
        .iter()
        .map(|&choice| {
            let fields: Vec<String> = plan
                .open()
                .iter()
                .filter(|open| open.choice == choice)
                .map(|open| format!("{} = {}", open.field, open.value))
                .collect();
            format!("  {}: {choice} ({})", flag(choice), fields.join(", "))
        })
        .collect();
    format!(
        "the conversion needs a choice; --force does not make one:\n{}",
        lines.join("\n")
    )
}

/// Where the converted file goes: `-o`, or the input file with the target's
/// extension. A slot has no path to take one from.
fn output(origin: &slot::Target, out: Option<&Path>, to: To) -> Result<PathBuf, String> {
    match (out, origin) {
        (Some(path), _) => Ok(path.to_path_buf()),
        (None, slot::Target::File(path)) => Ok(path.with_extension(to.layout().extension())),
        (None, slot::Target::Slot(_)) => Err("converting a slot needs -o".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use nord_format::formats::nsmp::encode;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        args: ConvertArgs,
    }

    fn args(line: &[&str]) -> ConvertArgs {
        let mut argv = vec!["convert"];
        argv.extend_from_slice(line);
        Cli::try_parse_from(argv).unwrap().args
    }

    /// What a v4 test instrument holds that v2 cannot.
    #[derive(Clone, Copy, PartialEq)]
    enum Holds {
        Nothing,
        /// A loop decay, which v2 drops.
        Decay,
        /// A name past v2's field, which needs a choice.
        LongName,
    }

    /// A v4 instrument in `dir`.
    fn v4(dir: &Path, holds: Holds) -> PathBuf {
        let source: Vec<i16> = (0..8_000)
            .map(|k| (2_000.0 * (k as f64 * 0.05).sin()) as i16)
            .collect();
        let name = match holds {
            Holds::LongName => "N".repeat(40),
            Holds::Nothing | Holds::Decay => "Tone".into(),
        };
        let zone = encode::NewZone {
            source: &source,
            channels: 1,
            root_key: 60,
            top_note: 84,
            global_id: 1,
            loops: None,
            secondary_start: encode::default_secondary_start(source.len(), None),
            shift: None,
            loop_decay: match holds {
                Holds::Decay => 35.0,
                Holds::Nothing | Holds::LongName => encode::DEFAULT_LOOP_DECAY,
            },
            gain: 1.0,
        };
        let instrument = encode::Instrument {
            name: &name,
            map_gain: 1.0,
            predictor: encode::Predictor::Minimizing,
            layout: Layout::V4,
            preset: encode::Preset::default(),
        };
        let file = encode::multi_zone(instrument, &[zone]).unwrap();
        let path = dir.join("tone.nsmp4");
        std::fs::write(&path, file.to_bytes().unwrap()).unwrap();
        path
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nord-convert-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_lossy_conversion_without_force_writes_nothing() {
        let dir = scratch("lossy");
        let input = v4(&dir, Holds::Decay);
        let out = dir.join("tone.nsmp");
        let path = input.to_string_lossy().into_owned();
        let err = run(&Ui::piped(), args(&[&path, "--to", "nsmp"])).unwrap_err();
        assert!(err.contains("--force"), "{err}");
        assert!(!out.exists());
        run(&Ui::piped(), args(&[&path, "--to", "nsmp", "--force"])).unwrap();
        assert!(out.exists());
    }

    #[test]
    fn an_open_choice_writes_nothing_even_with_force_and_names_its_flag() {
        let dir = scratch("open");
        let input = v4(&dir, Holds::LongName);
        let path = input.to_string_lossy().into_owned();
        let err = run(&Ui::piped(), args(&[&path, "--to", "nsmp", "--force"])).unwrap_err();
        assert!(err.contains("--truncate-name or --name NEW"), "{err}");
        assert!(!dir.join("tone.nsmp").exists());
        let named = args(&[&path, "--to", "nsmp", "--force", "--name", "Short"]);
        run(&Ui::piped(), named).unwrap();
        assert!(dir.join("tone.nsmp").exists());
    }

    #[test]
    fn wide_output_needs_unverified_and_a_dry_run_writes_nothing() {
        let dir = scratch("gate");
        let input = v4(&dir, Holds::Nothing);
        let path = input.to_string_lossy().into_owned();
        let out = dir.join("tone.nsmp3");
        for line in [
            &[&path, "--to", "nsmp3"][..],
            &[&path, "--to", "nsmp3", "--dry-run"],
        ] {
            let err = run(&Ui::piped(), args(line)).unwrap_err();
            assert!(err.contains("--unverified"), "{err}");
        }
        let dry = args(&[&path, "--to", "nsmp3", "--unverified", "--dry-run"]);
        run(&Ui::piped(), dry).unwrap();
        assert!(!out.exists());
        run(
            &Ui::piped(),
            args(&[&path, "--to", "nsmp3", "--unverified"]),
        )
        .unwrap();
        assert!(out.exists());
    }

    #[test]
    fn a_conversion_never_overwrites_its_input_and_asks_before_replacing_a_file() {
        let dir = scratch("overwrite");
        let input = v4(&dir, Holds::Nothing);
        let path = input.to_string_lossy().into_owned();
        let before = std::fs::read(&input).unwrap();
        let onto = args(&[&path, "--to", "nsmp4", "--unverified", "-o", &path, "--yes"]);
        assert!(run(&Ui::piped(), onto)
            .unwrap_err()
            .contains("never overwrites"));
        assert_eq!(std::fs::read(&input).unwrap(), before);

        let other = dir.join("other.nsmp");
        std::fs::write(&other, b"keep").unwrap();
        let target = other.to_string_lossy().into_owned();
        let replace = |yes: bool| {
            let mut line = vec![path.as_str(), "--to", "nsmp", "--force", "-o", &target];
            if yes {
                line.push("--yes");
            }
            run(&Ui::piped(), args(&line))
        };
        assert!(replace(false).unwrap_err().contains("--yes"));
        assert_eq!(std::fs::read(&other).unwrap(), b"keep");
        replace(true).unwrap();
        assert_ne!(std::fs::read(&other).unwrap(), b"keep");
    }
}
