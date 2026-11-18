//! The paths a WAV's edits travel as, and the marks each makes of the take's.

use super::*;

/// The value of a [`DETECTED`] set marking `marks`, whose loops are forward.
pub(crate) fn detected_value(marks: &wav::Marks) -> String {
    let regions = marks
        .regions
        .iter()
        .map(|region| format!("{} {} {}", region.start, region.frames, region.label));
    let loops = marks
        .loops
        .iter()
        .map(|found| format!("loop {} {}", found.start, found.end));
    regions.chain(loops).collect::<Vec<_>>().join("\n")
}

/// The marks a [`DETECTED`] set's value holds.
pub(super) fn detected_marks(value: &str) -> Result<wav::Marks, String> {
    let mut marks = wav::Marks::default();
    for line in value.lines().filter(|line| !line.trim().is_empty()) {
        let bad = || format!("{DETECTED}: {line:?} is no region or loop");
        let mut words = line.split_whitespace();
        let first = words.next().ok_or_else(bad)?;
        let number = |word: Option<&str>| word.and_then(|w| w.parse::<u32>().ok()).ok_or_else(bad);
        if first == "loop" {
            let (start, end) = (number(words.next())?, number(words.next())?);
            marks.loops.push(wav::Loop::forward(start, end));
            continue;
        }
        let start = number(Some(first))?;
        let frames = number(words.next())?;
        let label = words.next().ok_or_else(bad)?.to_string();
        marks.regions.push(wav::Region {
            start,
            frames,
            label,
        });
    }
    Ok(marks)
}

/// `marks` with every region of the sample whose attack is `label` renamed to the sample
/// whose attack label is `to`.
pub(super) fn renamed(
    marks: &wav::Marks,
    path: &str,
    label: &str,
    to: &str,
) -> Result<wav::Marks, String> {
    let from = take::mark(label, 0..0)
        .hit
        .ok_or_else(|| format!("{path}: names no sample"))?;
    let to = take::mark(to, 0..0)
        .hit
        .ok_or_else(|| format!("{path}: {to} names no sample"))?;
    let mut out = marks.clone();
    if from == to {
        return Ok(out);
    }
    if out
        .regions
        .iter()
        .any(|region| take::mark(&region.label, 0..0).hit == Some(to))
    {
        return Err(format!(
            "{path}: the take already marks {}",
            take::named(to)
        ));
    }
    for region in &mut out.regions {
        let mark = take::mark(&region.label, 0..0);
        if mark.hit != Some(from) {
            continue;
        }
        region.label = match mark.part {
            take::Part::Attack => adding::attack_label(to),
            take::Part::Resonance => encode::stroke_name(to.0, Bank::Resonance, to.1),
            take::Part::Release => adding::release_label(to),
            // A fade keeps the bank of the stroke it fades, attack or resonance.
            take::Part::Fade => {
                let stroke = region.label.strip_suffix(encode::FADE_SUFFIX);
                let bank = stroke
                    .and_then(|stroke| encode::parse_stroke_name(stroke, encode::Stem::None))
                    .map_or(Bank::Attack, |(_, bank, _)| bank);
                encode::stroke_name(to.0, bank, to.1) + encode::FADE_SUFFIX
            }
            take::Part::Other => continue,
        };
    }
    Ok(out)
}

/// The word a [`DELETED`] path names `piece` by.
pub(super) fn piece_slug(piece: hit::Piece) -> &'static str {
    match piece {
        hit::Piece::Note => "note",
        hit::Piece::Loop => "loop",
        hit::Piece::KeyOff => "key-off",
    }
}

/// `start..end`, as an add's value gives frames.
pub(super) fn span_value(span: &Range<u32>) -> String {
    format!("{}..{}", span.start, span.end)
}

/// The frames `value` gives for the add `path`, of at least one frame.
pub(super) fn frames_in(path: &str, value: &str) -> Result<Range<u32>, String> {
    let bad = || format!("{path}: {value} is not frames as start..end");
    let (start, end) = value.trim().split_once("..").ok_or_else(bad)?;
    let (start, end): (u32, u32) = (
        start.parse().map_err(|_| bad())?,
        end.parse().map_err(|_| bad())?,
    );
    match start < end {
        true => Ok(start..end),
        false => Err(bad()),
    }
}

/// Whether `path` adds, deletes or renames a sample, or marks what Detect found.
pub(super) fn is_piece(path: &str) -> bool {
    path == DETECTED
        || [RENAMED, ADDED, LOOPED, DELETED]
            .iter()
            .any(|prefix| path.starts_with(prefix))
}

/// `marks` with the add or delete `path` with `value` made, or none where `path` is not
/// one.
pub(super) fn pieced(
    marks: &wav::Marks,
    path: &str,
    value: &str,
) -> Option<Result<wav::Marks, String>> {
    if path == DETECTED {
        return Some(detected_marks(value));
    }
    if let Some(label) = path.strip_prefix(RENAMED) {
        return Some(renamed(marks, path, label, value));
    }
    let mut out = marks.clone();
    if let Some(label) = path.strip_prefix(ADDED) {
        return Some(frames_in(path, value).and_then(|span| {
            if out.regions.iter().any(|region| region.label == label) {
                return Err(format!("{path}: the take already marks {label}"));
            }
            out.regions.push(wav::Region {
                start: span.start,
                frames: span.end - span.start,
                label: label.to_string(),
            });
            Ok(out)
        }));
    }
    if let Some(label) = path.strip_prefix(LOOPED) {
        return Some(frames_in(path, value).and_then(|span| {
            let region = out
                .regions
                .iter()
                .find(|region| region.label == label)
                .ok_or_else(|| format!("{path}: the take marks no {label}"))?;
            let end = u64::from(region.start) + u64::from(region.frames);
            if span.start < region.start || u64::from(span.end) > end || span.len() < 2 {
                return Err(format!("{path}: a loop lies inside {label}"));
            }
            // ⚠️ A loop's end is its last frame, not the one after.
            out.loops.push(wav::Loop::forward(span.start, span.end - 1));
            Ok(out)
        }));
    }
    let rest = path.strip_prefix(DELETED)?;
    let mut deleted = || {
        let (label, piece) = rest
            .rsplit_once('/')
            .ok_or_else(|| format!("{path}: names no piece of a sample"))?;
        let hit = take::mark(label, 0..0)
            .hit
            .ok_or_else(|| format!("{path}: names no sample"))?;
        let cuts = hit::Cuts::of(&take::Marked::from_marks(marks), hit);
        // The sample's own loop, as the lanes pair it: never a neighbor's its attack has
        // grown over.
        let inside = |found: &wav::Loop| {
            (cuts.looped.as_ref())
                .is_some_and(|own| found.start == own.start && found.end + 1 == own.end)
        };
        let of = |region: &wav::Region, part: Option<take::Part>| {
            let mark = take::mark(&region.label, 0..0);
            mark.hit == Some(hit) && part.is_none_or(|part| mark.part == part)
        };
        match piece {
            "note" => {
                out.regions.retain(|region| !of(region, None));
                out.loops.retain(|found| !inside(found));
            }
            "loop" => out.loops.retain(|found| !inside(found)),
            "key-off" => out
                .regions
                .retain(|region| !of(region, Some(take::Part::Release))),
            _ => {
                return Err(format!(
                    "{path}: {piece} is not a note, a loop or a key-off"
                ))
            }
        }
        Ok(out.clone())
    };
    Some(deleted())
}
