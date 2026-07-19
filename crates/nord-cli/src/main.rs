//! `nord` — a thin CLI over [`nord_format`], purely to dogfood the library.
//!
//! Not a product; it exists to exercise the parser and surface API friction.
//! For now it does one thing: parse Nord file(s) and print what was decoded.

use clap::{Parser, Subcommand};
use nord_format::common::bank::Item;
use nord_format::electro5::{Instrument, OrganModel};
use nord_format::{Entity, Program, Settings, Song};
use std::path::PathBuf;
use std::process::ExitCode;

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

fn print_summary(entity: &Entity) {
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

            // Organ state (selected preset per model), when the organ is in use.
            if p.lower_part() == Instrument::Organ || p.upper_part() == Instrument::Organ {
                let o = p.organ();
                println!("  organ:     drawbars / vibrato / percussion, selected preset per model");
                for (model, label) in [
                    (OrganModel::B3, "b3  "),
                    (OrganModel::Vox, "vox "),
                    (OrganModel::Farfisa, "farf"),
                    (OrganModel::Pipe, "pipe"),
                ] {
                    let preset = o.preset(model);
                    let bars: String = o.drawbars(model, preset).iter().map(u8::to_string).collect();
                    let vib = match o.vib_type(model) {
                        Some(v) if o.vib_on(model, preset) => format!("  vib {v:?}"),
                        Some(_) => "  vib off".to_string(),
                        None => String::new(),
                    };
                    let perc = if matches!(model, OrganModel::B3) {
                        if o.b3_perc_on(preset) {
                            let third = if o.b3_perc_third() { " +3rd" } else { "" };
                            format!("  perc {:?}{third}", o.b3_perc_speed())
                        } else {
                            "  perc off".to_string()
                        }
                    } else {
                        String::new()
                    };
                    println!("    {label} p{preset}  {bars}{vib}{perc}");
                }
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
