//! `nord piano prep`: a recorded take's hits as the regions `piano build` reads.

use std::path::PathBuf;

use clap::{Args, ValueEnum};
use nord_format::formats::npno::codec;
use nord_format::note;
use npno_prep::layers::{Choice, Order};
use npno_prep::plan::{self, Fate, PlanOptions};
use npno_prep::wav;

use crate::edit::write_file;
use crate::ui::Ui;

#[derive(Args)]
pub struct PrepArgs {
    /// The take: one WAV of every note played, 16 to 32-bit PCM or float, mono or
    /// stereo, at any rate.
    #[arg(value_name = "TAKE")]
    pub take: PathBuf,

    /// The notes played, in playing order, as names or numbers: `B1,D2,F#2`. Each note
    /// may be hit several times in a row; the hits of one note become its layers.
    #[arg(long, value_name = "NOTES", value_delimiter = ',', required = true)]
    pub notes: Vec<String>,

    /// The layers each note gets.
    #[arg(long, value_name = "N", default_value_t = 4)]
    pub layers: usize,

    /// How a note's layers are chosen from its hits.
    #[arg(long, value_enum, default_value_t = OrderName::Loudness)]
    pub order: OrderName,

    /// Hits to leave out, by the number the listing gives them: `12,40`.
    #[arg(long, value_name = "HITS", value_delimiter = ',')]
    pub skip: Vec<usize>,

    /// Hits of one note closer than this in level are one layer, in decibels.
    #[arg(long, value_name = "DB", default_value_t = 2.0)]
    pub min_layer_gap: f64,

    /// The longest attack, in seconds.
    #[arg(long, value_name = "SECONDS", default_value_t = 6.0)]
    pub max_seconds: f64,

    /// The longest release, in seconds.
    #[arg(long, value_name = "SECONDS", default_value_t = 1.0)]
    pub release_seconds: f64,

    /// Where to write the take with its regions. A take at another rate is resampled
    /// to the instrument's first, at its own bit depth.
    #[arg(short, long, value_name = "FILE")]
    pub out: PathBuf,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum OrderName {
    /// The hits by measured level, near-duplicates dropped, spread evenly in dB.
    Loudness,
    /// The first hits of each note, the first the loudest: for recordings whose level
    /// cannot be trusted, such as a phone's.
    Played,
}

pub fn prep(ui: &Ui, args: PrepArgs) -> Result<(), String> {
    let named = |e: &dyn std::fmt::Display| format!("{}: {e}", args.take.display());
    let notes = args
        .notes
        .iter()
        .map(|n| note::parse(n))
        .collect::<Result<Vec<u8>, String>>()?;
    if args.layers == 0 {
        return Err("--layers must be at least 1".into());
    }
    let bytes = std::fs::read(&args.take).map_err(|e| named(&e))?;
    let layout = wav::layout(&bytes).map_err(|e| named(&e))?;
    let take = wav::read(&bytes).map_err(|e| named(&e))?;
    let resampled = take.rate != codec::RATE;
    if resampled && same_file(&args.take, &args.out) {
        return Err(named(&format!(
            "a take at {} Hz is resampled to {} Hz, which would replace its audio; \
             write it to another file with -o",
            take.rate,
            codec::RATE
        )));
    }
    let take = match resampled {
        true => npno_prep::resample::resample(&take, codec::RATE).map_err(|e| named(&e))?,
        false => take,
    };

    let mut options = PlanOptions::new(notes, args.layers);
    options.order = match args.order {
        OrderName::Loudness => Order::Loudness,
        OrderName::Played => Order::Played,
    };
    options.skip = args.skip;
    options.min_layer_gap_db = args.min_layer_gap;
    options.cut.max_seconds = args.max_seconds;
    options.cut.release_seconds = args.release_seconds;
    let plan = plan::plan(&plan::mono(&take), take.rate, &options);

    list(ui, &plan);
    let regions = plan.regions();
    if regions.is_empty() {
        return Err(named(&"no hit was kept, so there are no regions to write"));
    }
    let marks = wav::Marks {
        regions,
        loops: Vec::new(),
    };
    let audio = match resampled {
        true => wav::write(&take, layout.format.encoding).map_err(|e| named(&e))?,
        false => bytes,
    };
    let out = wav::with_marks(&audio, &marks).map_err(|e| named(&e))?;
    write_file(ui, &args.out, &out)?;
    if resampled {
        ui.note(format!(
            "resampled to the {} Hz the instrument plays at, so the regions lie where the \
             build reads them",
            codec::RATE
        ));
    }
    ui.note(format!(
        "{} region(s); build a library from them with `nord piano build {}`",
        marks.regions.len(),
        args.out.display()
    ));
    Ok(())
}

fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Every hit, what became of it and where its strokes lie.
fn list(ui: &Ui, plan: &plan::Plan) {
    match &plan.hum {
        Ok(hum) => {
            let (lo, hi) = hum
                .track
                .iter()
                .fold((f64::INFINITY, 0.0f64), |(lo, hi), &(_, f)| {
                    (lo.min(f), hi.max(f))
                });
            ui.note(format!(
                "hum at {} Hz mains ({lo:.2}-{hi:.2} Hz), {} harmonic(s); levels are measured \
                 against the floor without it, {:.1} dBFS",
                hum.mains,
                hum.harmonics.len(),
                plan.floor_db
            ));
        }
        Err(why) => ui.note(format!("{why}; the floor is {:.1} dBFS", plan.floor_db)),
    }
    ui.out(ui.dim(format!(
        "  {:>4} {:>8}  {:>4} {:>6} {:>6}  {:<24} {:>7} {:>7}",
        "hit", "at", "note", "pitch", "level", "", "attack", "release"
    )));
    let rate = f64::from(plan.rate);
    for planned in &plan.hits {
        let hit = &planned.hit;
        let (what, attack, release) = match &planned.fate {
            Fate::Kept { layer, value, cuts } => (
                format!("layer {layer}, value {value}"),
                format!("{:.2} s", cuts.attack.len() as f64 / rate),
                match (&cuts.release, &cuts.no_release) {
                    (Some(span), _) => format!("{:.2} s", span.len() as f64 / rate),
                    (None, Some(_)) => "none".to_string(),
                    (None, None) => "-".to_string(),
                },
            ),
            Fate::Skipped => ("skipped".into(), String::new(), String::new()),
            Fate::TooQuiet { over_db } => (
                format!("too quiet, {over_db:.0} dB over the floor"),
                String::new(),
                String::new(),
            ),
            Fate::Chosen(Choice::Duplicate { of }) => (
                format!("within the layer gap of hit {of}"),
                String::new(),
                String::new(),
            ),
            Fate::Chosen(_) => ("more hits than layers".into(), String::new(), String::new()),
        };
        ui.out(format!(
            "  {:>4} {:>7.2}s  {:>4} {:>6} {:>6.1}  {:<24} {:>7} {:>7}",
            hit.number,
            hit.onset as f64 * npno_prep::detect::FRAME,
            planned.note.map_or("-".into(), note::name),
            hit.pitch.map_or(
                "-".into(),
                |p| note::name(p.round().clamp(0.0, 127.0) as u8)
            ),
            hit.level_db,
            what,
            attack,
            release,
        ));
        if let Some(warning) = &planned.warning {
            ui.warn(format!("hit {}: {warning}", hit.number));
        }
        if let Fate::Kept { cuts, .. } = &planned.fate {
            match (&cuts.release, &cuts.no_release) {
                (None, Some(why)) => ui.note(format!("hit {}: no release: {why}", hit.number)),
                (None, None) => ui.warn(format!(
                    "hit {}: no key-up found before the next hit, so its attack runs on to \
                     it and it has no release",
                    hit.number
                )),
                (Some(_), _) => {}
            }
        }
    }
    for &missing in &plan.missing {
        ui.warn(format!("no hit was kept for {}", note::name(missing)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A take of decaying tones, one per (start, MIDI note, peak dB, key-up) in seconds.
    fn take(rate: u32, seconds: f64, notes: &[(f64, f64, f64, f64)]) -> wav::Audio {
        let n = (seconds * f64::from(rate)) as usize;
        let mut samples = vec![0.0f32; n];
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        for v in &mut samples {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            *v = 3e-5 * ((seed >> 11) as f32 / (1u64 << 53) as f32 * 2.0 - 1.0);
        }
        for &(start, note, peak_db, up) in notes {
            let f = 440.0 * 2f64.powf((note - 69.0) / 12.0);
            let from = (start * f64::from(rate)) as usize;
            for (i, v) in samples.iter_mut().enumerate().skip(from) {
                let t = (i - from) as f64 / f64::from(rate);
                let damp = if t < up - start {
                    1.0
                } else {
                    (-(t - (up - start)) * 90.0).exp()
                };
                let level = 10f64.powf(peak_db / 20.0) * (-t * 0.7).exp() * damp;
                *v += (level * (2.0 * std::f64::consts::PI * f * t).sin()) as f32;
            }
        }
        wav::Audio {
            rate,
            channels: 1,
            samples,
        }
    }

    fn prep_args(argv: &[&str]) -> PrepArgs {
        match crate::Cli::try_parse_from([&["nord", "piano", "prep"], argv].concat()) {
            Ok(crate::Cli {
                command:
                    crate::Command::Piano {
                        action: crate::PianoAction::Prep(args),
                    },
                ..
            }) => args,
            _ => panic!("prep did not parse"),
        }
    }

    use clap::Parser;

    /// A take at another rate is resampled, its hits become regions named as stroke
    /// WAVs, and `piano build` makes a library from them.
    #[test]
    fn a_take_preps_into_regions_that_build() {
        let dir = crate::edit::tests::scratch("piano-prep");
        let audio = take(
            44_100,
            13.0,
            &[
                (1.0, 60.0, -12.0, 3.0),
                (5.0, 60.0, -24.0, 7.0),
                (9.0, 64.0, -12.0, 11.0),
            ],
        );
        let path = dir.join("take.wav");
        std::fs::write(
            &path,
            wav::write(&audio, wav::Encoding::Int { bits: 24 }).unwrap(),
        )
        .unwrap();
        let out = dir.join("regions.wav");
        let (take, out_s) = (path.to_str().unwrap(), out.to_str().unwrap());
        let args = prep_args(&[take, "--notes", "C4,E4", "--layers", "2", "-o", out_s]);
        prep(&Ui::piped(), args).unwrap();

        let bytes = std::fs::read(&out).unwrap();
        let layout = wav::layout(&bytes).unwrap();
        assert_eq!(layout.format.rate, codec::RATE);
        assert_eq!(layout.format.encoding, wav::Encoding::Int { bits: 24 });
        let labels: Vec<String> = wav::read_marks(&bytes)
            .unwrap()
            .regions
            .into_iter()
            .map(|r| r.label)
            .collect();
        assert_eq!(
            labels,
            [
                "060-b0-v00",
                "060-b2-v00",
                "060-b0-v40",
                "060-b2-v40",
                "064-b0-v00",
                "064-b2-v00"
            ]
        );

        let npno = dir.join("take.npno");
        let Ok(crate::PianoAction::Build(build)) = crate::Cli::try_parse_from([
            "nord",
            "piano",
            "build",
            out_s,
            "--name",
            "Prep",
            "-o",
            npno.to_str().unwrap(),
        ])
        .map(|cli| match cli.command {
            crate::Command::Piano { action } => action,
            _ => panic!("not a piano verb"),
        }) else {
            panic!("build did not parse");
        };
        crate::piano::build(&Ui::piped(), build).unwrap();
        assert!(npno.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_take_to_resample_is_not_written_over() {
        let dir = crate::edit::tests::scratch("piano-prep-over");
        let audio = take(44_100, 4.0, &[(1.0, 60.0, -12.0, 3.0)]);
        let path = dir.join("take.wav");
        std::fs::write(
            &path,
            wav::write(&audio, wav::Encoding::Int { bits: 16 }).unwrap(),
        )
        .unwrap();
        let take = path.to_str().unwrap();
        let err = prep(
            &Ui::piped(),
            prep_args(&[take, "--notes", "C4", "-o", take]),
        )
        .unwrap_err();
        assert!(err.contains("resampled"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
