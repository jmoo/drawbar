//! `nord piano prep`: a recorded take's hits as the regions `piano build` reads.

use std::path::PathBuf;

use clap::{Args, ValueEnum};
use nord_format::formats::npno::codec;
use nord_format::note;
use nord_format::take::layers::{Choice, Order};
use nord_format::take::plan::{self, Fate, PlanOptions, SetLayer};
use nord_format::take::sustain::Sustain;
use nord_format::wav;

use crate::edit::write_file;
use crate::ui::Ui;

#[derive(Args)]
pub struct PrepArgs {
    /// The take: one WAV of every note played, 16 to 32-bit PCM or float, mono or
    /// stereo, at any rate. Or several, one per velocity layer, the loudest first, each
    /// holding every note once: a note played again keeps its last hit.
    #[arg(value_name = "TAKE", required = true, num_args = 1..)]
    pub takes: Vec<PathBuf>,

    /// The notes played, in playing order, as names or numbers: `B1,D2,F#2`. In a
    /// single take each note may be hit several times in a row; the hits of one note
    /// become its layers. Without it, each hit is named by its pitch, and the notes
    /// found are listed.
    #[arg(long, value_name = "NOTES", value_delimiter = ',')]
    pub notes: Vec<String>,

    /// The layers each note of a single take gets: 4 unless given. Takes of one layer
    /// each give a note one layer per take.
    #[arg(long, value_name = "N")]
    pub layers: Option<usize>,

    /// How a single take's layers are chosen from each note's hits: by loudness unless
    /// given.
    #[arg(long, value_enum)]
    pub order: Option<OrderName>,

    /// Hits to leave out, by the number the listing gives them: `12,40`, or with several
    /// takes, the take's number too: `2:12`.
    #[arg(long, value_name = "HITS", value_delimiter = ',')]
    pub skip: Vec<String>,

    /// Hits of one note closer than this in level are one layer, in decibels.
    #[arg(long, value_name = "DB", default_value_t = 2.0)]
    pub min_layer_gap: f64,

    /// The longest attack, in seconds.
    #[arg(long, value_name = "SECONDS", default_value_t = 6.0)]
    pub max_seconds: f64,

    /// The longest release, in seconds.
    #[arg(long, value_name = "SECONDS", default_value_t = 1.0)]
    pub release_seconds: f64,

    /// Where to write the take with its regions, or with several takes, the directory
    /// to write each into under its own name. A take at another rate is resampled to
    /// the instrument's first, at its own bit depth.
    #[arg(short, long, value_name = "FILE|DIR")]
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
    let notes = args
        .notes
        .iter()
        .map(|n| note::parse(n))
        .collect::<Result<Vec<u8>, String>>()?;
    let set = args.takes.len() > 1;
    if set && (args.layers.is_some() || args.order.is_some()) {
        return Err(
            "--layers and --order choose among a single take's hits; with several takes, \
             each take is one layer"
                .into(),
        );
    }
    let layers = args.layers.unwrap_or(4);
    if layers == 0 {
        return Err("--layers must be at least 1".into());
    }
    for (flag, value) in [
        ("--max-seconds", args.max_seconds),
        ("--release-seconds", args.release_seconds),
    ] {
        if !(value.is_finite() && value > 0.0) {
            return Err(format!("{flag} {value} is no length; give seconds over 0"));
        }
    }
    if !(args.min_layer_gap.is_finite() && args.min_layer_gap >= 0.0) {
        return Err(format!(
            "--min-layer-gap {} is no gap; give decibels of 0 or more",
            args.min_layer_gap
        ));
    }
    let skips = skips(&args.skip, args.takes.len())?;
    let outs: Vec<PathBuf> = match set {
        false => vec![args.out.clone()],
        true => args
            .takes
            .iter()
            .map(|take| args.out.join(take.file_name().unwrap_or_default()))
            .collect(),
    };
    for (index, (path, out)) in args.takes.iter().zip(&outs).enumerate() {
        if outs[..index].contains(out) {
            return Err(format!(
                "{}: two takes would be written to {}; give them different names",
                path.display(),
                out.display()
            ));
        }
    }
    // Every take is planned before any is written, so one that fails leaves none.
    let mut planned = Vec::new();
    for (index, (path, out)) in args.takes.iter().zip(&outs).enumerate() {
        if set {
            ui.out(format!(
                "take {}: {}, layer {} of {}",
                index + 1,
                path.display(),
                index + 1,
                args.takes.len()
            ));
        }
        let mut options = PlanOptions::new(notes.clone(), layers);
        options.order = match args.order {
            Some(OrderName::Played) => Order::Played,
            Some(OrderName::Loudness) | None => Order::Loudness,
        };
        options.skip = skips[index].clone();
        options.min_layer_gap_db = args.min_layer_gap;
        options.cut.max_seconds = args.max_seconds;
        options.cut.release_seconds = args.release_seconds;
        options.set = set.then_some(SetLayer {
            layer: index,
            of: args.takes.len(),
        });
        let (bytes, hits) = prep_take(ui, path, out, &options)?;
        for &skip in &options.skip {
            if !hits.contains(&skip) {
                let named = match set {
                    true => format!("{}:{skip}", index + 1),
                    false => skip.to_string(),
                };
                return Err(format!(
                    "--skip {named:?}: {} has no hit {skip}",
                    path.display()
                ));
            }
        }
        planned.push((out, bytes));
    }
    if set {
        std::fs::create_dir_all(&args.out).map_err(|e| format!("{}: {e}", args.out.display()))?;
    }
    let mut written = Vec::new();
    for (out, bytes) in planned {
        write_file(ui, out, &bytes)?;
        written.push(out.display().to_string());
    }
    ui.note(format!(
        "build a library from them with `nord piano build {}`",
        written.join(" ")
    ));
    Ok(())
}

/// The hits `--skip` leaves out of each of `takes` takes: `12` in a single take, and
/// `2:12` in the second of several.
fn skips(given: &[String], takes: usize) -> Result<Vec<Vec<usize>>, String> {
    let mut out = vec![Vec::new(); takes];
    for skip in given {
        let bad = || match takes {
            1 => format!("--skip {skip:?} is no hit number, as in 12"),
            _ => format!("--skip {skip:?} is no take and hit number, as in 2:12"),
        };
        let (take, hit) = match (takes, skip.split_once(':')) {
            (1, None) => (1, skip.as_str()),
            (_, Some((take, hit))) if takes > 1 => {
                (take.trim().parse::<usize>().map_err(|_| bad())?, hit)
            }
            _ => return Err(bad()),
        };
        let hit = hit.trim().parse::<usize>().map_err(|_| bad())?;
        out.get_mut(take.wrapping_sub(1))
            .ok_or_else(|| format!("--skip {skip:?}: there are {takes} takes"))?
            .push(hit);
    }
    Ok(out)
}

/// Finds one take's hits and lists them: the take with its regions, as written to
/// `out`, and the numbers of its hits.
fn prep_take(
    ui: &Ui,
    path: &std::path::Path,
    out: &std::path::Path,
    options: &PlanOptions,
) -> Result<(Vec<u8>, Vec<usize>), String> {
    let named = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let bytes = std::fs::read(path).map_err(|e| named(&e))?;
    let layout = wav::layout(&bytes).map_err(|e| named(&e))?;
    let take = wav::read(&bytes).map_err(|e| named(&e))?;
    let resampled = take.rate != codec::RATE;
    if resampled && same_file(path, out) {
        return Err(named(&format!(
            "a take at {} Hz is resampled to {} Hz, which would replace its audio; \
             write it to another file with -o",
            take.rate,
            codec::RATE
        )));
    }
    let take = match resampled {
        true => nord_format::take::resample::resample(&take, codec::RATE).map_err(|e| named(&e))?,
        false => take,
    };
    let plan = plan::plan(&plan::mono(&take), take.rate, options);

    if options.notes.is_empty() {
        let found: Vec<String> = plan.notes.iter().map(|&n| note::name(n)).collect();
        ui.note(format!(
            "notes found from the hits' pitches: {}",
            found.join(",")
        ));
    }
    list(ui, &plan);
    let regions = plan.regions();
    if regions.is_empty() {
        return Err(named(&"no hit was kept, so there are no regions to write"));
    }
    let marks = wav::Marks {
        regions,
        loops: plan.loops(),
    };
    let audio = match resampled {
        true => wav::with_audio(&bytes, &take, layout.format.encoding).map_err(|e| named(&e))?,
        false => bytes,
    };
    let written = wav::with_marks(&audio, &marks).map_err(|e| named(&e))?;
    if resampled {
        ui.note(format!(
            "resampled to the {} Hz the instrument plays at, so the regions lie where the \
             build reads them",
            codec::RATE
        ));
    }
    ui.note(format!(
        "{} region(s), {} loop(s)",
        marks.regions.len(),
        marks.loops.len()
    ));
    let hits = plan.hits.iter().map(|planned| planned.hit.number).collect();
    Ok((written, hits))
}

fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// A loop whose end matches its start less than this is warned about.
const LOOP_MATCH_WARNING: f64 = 0.5;

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
        "  {:>4} {:>8}  {:>4} {:>6} {:>6}  {:<24} {:>7} {:>7}  {}",
        "hit", "at", "note", "pitch", "level", "", "attack", "release", "sustain"
    )));
    let rate = f64::from(plan.rate);
    for planned in &plan.hits {
        let hit = &planned.hit;
        let sustain = match &planned.fate {
            Fate::Kept {
                sustain: Sustain::Loop { start, len, score },
                ..
            } => format!(
                "loops {:.2} s from {:.2} s, match {score:.2}",
                *len as f64 / rate,
                *start as f64 / rate
            ),
            Fate::Kept {
                sustain: Sustain::Dies { fade, .. },
                ..
            } => format!("fades from {:.2} s", *fade as f64 / rate),
            _ => String::new(),
        };
        let (what, attack, release) = match &planned.fate {
            Fate::Kept {
                layer,
                value,
                cuts,
                sustain,
            } => (
                format!("layer {layer}, value {value}"),
                format!(
                    "{:.2} s",
                    match sustain {
                        Sustain::Dies { end, .. } => *end.min(&cuts.attack.len()),
                        Sustain::Loop { .. } => cuts.attack.len(),
                    } as f64
                        / rate
                ),
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
            Fate::Chosen(Choice::Replaced { by }) => (
                format!("played again as hit {by}"),
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
            "  {:>4} {:>7.2}s  {:>4} {:>6} {:>6.1}  {:<24} {:>7} {:>7}  {}",
            hit.number,
            hit.onset as f64 * nord_format::take::detect::FRAME,
            planned.note.map_or("-".into(), note::name),
            hit.pitch.map_or(
                "-".into(),
                |p| note::name(p.round().clamp(0.0, 127.0) as u8)
            ),
            hit.level_db,
            what,
            attack,
            release,
            sustain,
        ));
        if let Some(warning) = &planned.warning {
            ui.warn(format!("hit {}: {warning}", hit.number));
        }
        if let Fate::Kept {
            sustain: Sustain::Loop { score, .. },
            ..
        } = &planned.fate
        {
            if *score < LOOP_MATCH_WARNING {
                ui.warn(format!(
                    "hit {}: its loop's end matches its start only {score:.2}; listen to \
                     it, and move the loop in an editor if it is heard",
                    hit.number
                ));
            }
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
        let info = b"LIST\x0e\x00\x00\x00INFOISFT\x02\x00\x00\x00me";
        let mut smpl = b"smpl\x28\x00\x00\x00".to_vec();
        // maker, product, period, unity note, pitch, SMPTE x2, no loops, 4 bytes of data
        for word in [0, 0, 22_676, 64, 0, 0, 0, 0, 4] {
            smpl.extend_from_slice(&u32::to_le_bytes(word));
        }
        smpl.extend_from_slice(b"mine");
        let mut bytes = wav::write(&audio, wav::Encoding::Int { bits: 24 }).unwrap();
        bytes.extend_from_slice(info);
        bytes.extend_from_slice(&smpl);
        let riff = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&riff.to_le_bytes());
        std::fs::write(&path, bytes).unwrap();
        let out = dir.join("regions.wav");
        let (take, out_s) = (path.to_str().unwrap(), out.to_str().unwrap());
        let args = prep_args(&[take, "--notes", "C4,E4", "--layers", "2", "-o", out_s]);
        prep(&Ui::piped(), args).unwrap();

        let unnamed = dir.join("unnamed.wav");
        let args = prep_args(&[take, "--layers", "2", "-o", unnamed.to_str().unwrap()]);
        prep(&Ui::piped(), args).unwrap();
        assert!(
            std::fs::read(&unnamed).unwrap() == std::fs::read(&out).unwrap(),
            "with no note list, the notes the hits' pitches name"
        );

        let bytes = std::fs::read(&out).unwrap();
        let layout = wav::layout(&bytes).unwrap();
        assert_eq!(layout.format.rate, codec::RATE);
        assert_eq!(layout.format.encoding, wav::Encoding::Int { bits: 24 });
        let found = |needle: &[u8]| bytes.windows(needle.len()).position(|w| w == needle);
        assert!(found(info).is_some(), "the take's INFO list is kept");
        let at = found(b"smpl").expect("the take's smpl chunk is kept") + 8;
        let word = |i: usize| u32::from_le_bytes(bytes[at + i..at + i + 4].try_into().unwrap());
        assert_eq!(
            (word(8), word(12)),
            (28_570, 64),
            "period at the new rate, unity note kept"
        );
        assert!(found(b"mine").is_some(), "the sampler data is kept");
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

    fn build_args(argv: &[&str]) -> crate::piano::BuildArgs {
        let cli = crate::Cli::try_parse_from(
            [&["nord", "piano", "build", "--name", "Set"], argv].concat(),
        );
        match cli.map(|cli| cli.command) {
            Ok(crate::Command::Piano {
                action: crate::PianoAction::Build(args),
            }) => args,
            _ => panic!("build did not parse"),
        }
    }

    /// Two takes, one per layer, the second recorded with more gain: each note's last
    /// hit in each take is that take's layer, and the two build one library. The same
    /// take given twice names each stroke twice and is refused.
    #[test]
    fn takes_of_one_layer_each_prep_into_one_library() {
        let dir = crate::edit::tests::scratch("piano-prep-set");
        let loud = take(
            44_100,
            9.0,
            &[(1.0, 60.0, -12.0, 3.0), (5.0, 64.0, -12.0, 7.0)],
        );
        let soft = take(
            44_100,
            13.0,
            &[
                (1.0, 60.0, -14.0, 3.0),
                (5.0, 64.0, -30.0, 7.0),
                (9.0, 64.0, -14.0, 11.0),
            ],
        );
        let paths = [dir.join("loud.wav"), dir.join("soft.wav")];
        for (path, audio) in paths.iter().zip([&loud, &soft]) {
            let bytes = wav::write(audio, wav::Encoding::Int { bits: 24 }).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        let out = dir.join("regions");
        let (a, b) = (paths[0].to_str().unwrap(), paths[1].to_str().unwrap());
        let out_s = out.to_str().unwrap();
        let err = prep(
            &Ui::piped(),
            prep_args(&[a, b, "--notes", "C4,E4", "--layers", "2", "-o", out_s]),
        )
        .unwrap_err();
        assert!(err.contains("each take is one layer"), "{err}");
        let err = prep(
            &Ui::piped(),
            prep_args(&[a, b, "--notes", "C4,E4", "--skip", "3", "-o", out_s]),
        )
        .unwrap_err();
        assert!(err.contains("2:12"), "{err}");

        // a take that fails, or a skip that matches no hit, leaves nothing written
        let silent = dir.join("silent.wav");
        let nothing = wav::Audio {
            samples: vec![0.0; 44_100],
            ..loud.clone()
        };
        let bytes = wav::write(&nothing, wav::Encoding::Int { bits: 24 }).unwrap();
        std::fs::write(&silent, bytes).unwrap();
        let silent_s = silent.to_str().unwrap();
        let err = prep(
            &Ui::piped(),
            prep_args(&[a, silent_s, "--notes", "C4,E4", "-o", out_s]),
        )
        .unwrap_err();
        assert!(err.contains("no hit was kept"), "{err}");
        assert!(!out.join("loud.wav").exists(), "nothing written");
        let err = prep(
            &Ui::piped(),
            prep_args(&[a, b, "--notes", "C4,E4", "--skip", "2:9", "-o", out_s]),
        )
        .unwrap_err();
        assert!(err.contains("--skip \"2:9\""), "{err}");
        assert!(!out.join("loud.wav").exists(), "nothing written");

        prep(
            &Ui::piped(),
            prep_args(&[a, b, "--notes", "C4,E4", "-o", out_s]),
        )
        .unwrap();

        let labels = |name: &str| -> Vec<String> {
            let bytes = std::fs::read(out.join(name)).unwrap();
            let marks = wav::read_marks(&bytes).unwrap();
            marks.regions.into_iter().map(|r| r.label).collect()
        };
        assert_eq!(
            labels("loud.wav"),
            ["060-b0-v00", "060-b2-v00", "064-b0-v00", "064-b2-v00"]
        );
        assert_eq!(
            labels("soft.wav"),
            ["060-b0-v40", "060-b2-v40", "064-b0-v40", "064-b2-v40"]
        );

        let (loud_r, soft_r) = (out.join("loud.wav"), out.join("soft.wav"));
        let (loud_r, soft_r) = (loud_r.to_str().unwrap(), soft_r.to_str().unwrap());
        let npno = dir.join("set.npno");
        let npno_s = npno.to_str().unwrap();
        crate::piano::build(&Ui::piped(), build_args(&[loud_r, soft_r, "-o", npno_s])).unwrap();
        let (_, piano) = crate::piano::read(&npno).unwrap();
        // and C4's pitched down to C3 and E4's up to E5, to reach more keys
        assert_eq!(piano.library().unwrap().strokes().len(), 16);
        let twice = crate::piano::build(&Ui::piped(), build_args(&[loud_r, loud_r, "-o", npno_s]));
        assert!(twice.is_err(), "the same strokes twice");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A held middle C loops, and an E6 that dies within 20 dB of the floor long before its key-up fades
    /// instead: its attack ends where the fade reaches silence, the fade is a region of
    /// its own, and the built stroke ends in the silence it loops on.
    #[test]
    fn a_held_note_loops_and_a_dying_one_fades() {
        let dir = crate::edit::tests::scratch("piano-prep-sustain");
        let rate = codec::RATE;
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut samples: Vec<f32> = (0..rate as usize * 12)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                1e-4 * ((seed >> 11) as f32 / (1u64 << 53) as f32 * 2.0 - 1.0)
            })
            .collect();
        for (start, note, db_per_s, up) in [(1.0, 60.0, 3.0, 4.5), (6.0, 88.0, 40.0, 10.0)] {
            let f = 440.0 * 2f64.powf((note - 69.0) / 12.0);
            let from = (start * f64::from(rate)) as usize;
            for (i, v) in samples.iter_mut().enumerate().skip(from) {
                let t = (i - from) as f64 / f64::from(rate);
                let damp = match t < up - start {
                    true => 1.0,
                    false => (-(t - (up - start)) * 90.0).exp(),
                };
                let level = 10f64.powf((-12.0 - db_per_s * t) / 20.0) * damp;
                *v += (level * (2.0 * std::f64::consts::PI * f * t).sin()) as f32;
            }
        }
        let audio = wav::Audio {
            rate,
            channels: 1,
            samples,
        };
        let path = dir.join("take.wav");
        std::fs::write(
            &path,
            wav::write(&audio, wav::Encoding::Int { bits: 24 }).unwrap(),
        )
        .unwrap();
        let take = path.to_str().unwrap();
        prep(
            &Ui::piped(),
            prep_args(&[take, "--notes", "C4,E6", "--layers", "1", "-o", take]),
        )
        .unwrap();

        let marks = wav::read_marks(&std::fs::read(&path).unwrap()).unwrap();
        let labels: Vec<&str> = marks.regions.iter().map(|r| r.label.as_str()).collect();
        assert!(labels.contains(&"088-b0-v00-fade"), "{labels:?}");
        assert_eq!(marks.loops.len(), 1, "only the held note loops");
        let held = &marks.regions[0];
        assert_eq!(held.label, "060-b0-v00");
        assert!(marks.loops[0].end < held.start + held.frames);
        let (attack, fade) = (
            marks
                .regions
                .iter()
                .find(|r| r.label == "088-b0-v00")
                .unwrap(),
            marks
                .regions
                .iter()
                .find(|r| r.label == "088-b0-v00-fade")
                .unwrap(),
        );
        assert_eq!(attack.start + attack.frames, fade.start + fade.frames);
        let fades_at = f64::from(fade.start - attack.start) / f64::from(rate);
        assert!((fades_at - 1.3).abs() < 0.15, "fades from {fades_at:.2} s");

        let npno = dir.join("take.npno");
        let npno_s = npno.to_str().unwrap();
        let cli = crate::Cli::try_parse_from([
            "nord", "piano", "build", take, "--name", "S", "-o", npno_s,
        ]);
        let Ok(crate::Command::Piano {
            action: crate::PianoAction::Build(build),
        }) = cli.map(|cli| cli.command)
        else {
            panic!("build did not parse");
        };
        crate::piano::build(&Ui::piped(), build).unwrap();
        let (_, piano) = crate::piano::read(&npno).unwrap();
        let library = piano.library().unwrap();
        let stroke = library
            .strokes()
            .iter()
            .find(|s| s.root == 88 && s.bank_code() == 0)
            .unwrap();
        let lane = &codec::decode(stroke, library.channels()).unwrap().lanes[0];
        assert!(
            lane[lane.len() - 2_000..].iter().all(|&v| v == 0),
            "ends silent"
        );
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

    #[test]
    fn lengths_and_gaps_that_mean_nothing_are_refused() {
        for (flag, value) in [
            ("--max-seconds", "0"),
            ("--release-seconds", "-1"),
            ("--min-layer-gap", "NaN"),
            ("--min-layer-gap", "-2"),
        ] {
            let given = format!("{flag}={value}");
            let args = prep_args(&["take.wav", "--notes", "C4", &given, "-o", "out.wav"]);
            let err = prep(&Ui::piped(), args).unwrap_err();
            assert!(err.contains(flag), "{flag} {value}: {err}");
        }
    }
}
