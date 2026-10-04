//! Generation changes: the narrow chain, early or not, and the two wide generations,
//! one into another.

use super::hub::{self, Instrument, Origin};
use super::report::{Field, Line, Reason, Report, ZoneField};
use super::{
    Choice, Choices, ConvertError, GainChoice, LoopMarkChoice, NameChoice, Open, OverlapChoice,
    Plan,
};
use crate::formats::nsmp::cat::NarrowCat;
use crate::formats::nsmp::codec::{Lattice, Layout, Peak};
use crate::formats::nsmp::encode::{self, DEFAULT_LOOP_DECAY};
use crate::formats::nsmp::keymap::{KeyTable, GAIN_UNITY};
use crate::formats::nsmp::zone::{VelocityWindow, KEY_FLOOR};
use crate::formats::nsmp::{self, Chain};
use crate::Sample;
use std::ops::Range;

pub(super) fn plan(sample: &Sample, to: Layout, choices: &Choices) -> Result<Plan, ConvertError> {
    let (source, origin) = hub::read(sample).map_err(ConvertError::Read)?;
    let mut edge = Edge {
        from: origin,
        to,
        choices,
        report: Report::default(),
        open: Vec::new(),
        order: (0..source.zones.len()).collect(),
    };
    edge.unmodelled(sample, &source)?;
    let mut target = source.clone();
    edge.instrument(&mut target);
    edge.zones(&mut target);
    if !edge.open.is_empty() {
        return Ok(Plan {
            report: edge.report,
            open: edge.open,
            output: None,
        });
    }
    let output = hub::write(&target, to).map_err(ConvertError::Write)?;
    edge.streams(&target, &output)?;
    edge.recoded(sample, &source);
    Ok(Plan {
        report: edge.report,
        open: edge.open,
        output: Some(output),
    })
}

/// One conversion's direction, its answers, and what it has found so far.
struct Edge<'a> {
    from: Origin,
    to: Layout,
    choices: &'a Choices,
    report: Report,
    open: Vec<Open>,
    /// Each target zone's index in the source, which lines name zones by.
    order: Vec<usize>,
}

/// The gain, in record units, where a narrow zone record's 24 bits wrap.
const RECORD_GAIN_LIMIT: f64 = (1u32 << 24) as f64;

impl Edge<'_> {
    fn widening(&self) -> bool {
        self.to != Layout::V2 && self.from.layout == Layout::V2
    }

    fn dropped(&mut self, field: Field, value: impl ToString, reason: Reason) {
        self.report.dropped.push(line(field, value, reason));
    }

    fn changed(&mut self, field: Field, value: impl ToString, reason: Reason) {
        self.report.changed.push(line(field, value, reason));
    }

    fn by_rule(&mut self, field: Field, value: impl ToString, reason: Reason) {
        self.report.from_rules.push(line(field, value, reason));
    }

    fn ask(&mut self, choice: Choice, field: Field, value: impl ToString) {
        self.open.push(Open {
            choice,
            field,
            value: value.to_string(),
        });
    }

    /// Stored bytes and header words the model does not carry: whatever differs
    /// between the source and the source's own generation written from the model.
    fn unmodelled(&mut self, sample: &Sample, source: &Instrument) -> Result<(), ConvertError> {
        let header = header_of(sample);
        let frame_layout = match self.from.chain {
            Chain::Early => Layout::V2,
            Chain::Library2 | Chain::Wide => self.from.layout,
        };
        let frame = hub::frame(source, frame_layout).map_err(ConvertError::Read)?;
        let written = header_of(&frame);
        if header.version != written.version {
            self.changed(
                Field::ContentVersion,
                header.version,
                Reason::Written {
                    value: encode::version(self.to).to_string(),
                },
            );
        }
        if header.location != written.location {
            self.changed(
                Field::Location,
                format!("{:#010x}", header.location),
                Reason::Written {
                    value: format!("{:#010x}", written.location),
                },
            );
        }
        let ours = sections(&frame);
        for (tag, version, payload) in sections(sample) {
            if tag.ends_with("stk") || tag.ends_with("meta") || payload.is_empty() {
                continue;
            }
            let Some((_, our_version, our_payload)) = ours.iter().find(|(t, ..)| *t == tag) else {
                self.dropped(
                    bytes(&tag, 0..payload.len(), None),
                    hex(&payload),
                    Reason::Unmodelled,
                );
                continue;
            };
            if self.from.chain == Chain::Early && tag == "hdr" {
                self.dropped(
                    bytes(&tag, 0..payload.len(), None),
                    hex(&payload),
                    Reason::Unmodelled,
                );
                continue;
            }
            if version != *our_version && !(self.from.chain == Chain::Early && tag == "map") {
                self.dropped(
                    bytes(&tag, 0..payload.len(), None),
                    format!("version {version}"),
                    Reason::Schema { version },
                );
                continue;
            }
            let regions = known(&tag, self.from.layout, &payload);
            for range in differing(&payload, our_payload, self.compared(&tag, &payload)) {
                let known_as = regions
                    .iter()
                    .find(|(r, _)| r.start <= range.start && range.end <= r.end);
                let (range, known_as) = match known_as {
                    Some((region, name)) => (region.clone(), Some(*name)),
                    None => (range, None),
                };
                if self
                    .report
                    .dropped
                    .iter()
                    .any(|l| l.field == bytes(&tag, range.clone(), known_as))
                {
                    continue;
                }
                let value = hex(&payload[range.clone()]);
                self.dropped(bytes(&tag, range, known_as), value, Reason::Unmodelled);
            }
        }
        Ok(())
    }

    /// Where each byte of a source section sits in a frame written from the model.
    /// The early chain's zone records are three bytes short of the later ones', and
    /// every field they share sits at the same offset.
    fn compared(&self, tag: &str, payload: &[u8]) -> Vec<(usize, usize)> {
        let early = Chain::Early.zone_record_len();
        let later = Chain::Library2.zone_record_len();
        (0..payload.len())
            .map(|at| match at.checked_sub(nsmp::zone::RECORDS_AT) {
                Some(rel) if self.from.chain == Chain::Early && tag == "map" => (
                    at,
                    nsmp::zone::RECORDS_AT + rel / early * later + rel % early,
                ),
                _ => (at, at),
            })
            .collect()
    }

    /// The instrument-level fields, in model order.
    fn instrument(&mut self, target: &mut Instrument) {
        self.name(target);
        if self.to == Layout::V2 && !target.sub_name.is_empty() {
            self.dropped(
                Field::SubName,
                format!("{:?}", target.sub_name),
                Reason::NoField(self.to),
            );
            target.sub_name.clear();
        }
        self.categories(target);
        if self.to != Layout::V2 {
            let adjusted: Vec<String> = target.keys.adjusted().map(|n| n.to_string()).collect();
            if !adjusted.is_empty() {
                self.dropped(
                    Field::KeyTable,
                    format!("notes {}", adjusted.join(", ")),
                    Reason::NoField(self.to),
                );
                let instrument = target.keys.instrument;
                target.keys = KeyTable::NEUTRAL;
                target.keys.instrument = instrument;
            }
        }
        self.velocity_depths(target);
    }

    fn name(&mut self, target: &mut Instrument) {
        let capacity = match self.to {
            Layout::V2 => nsmp::MAX_NAME_LEN,
            Layout::V3 | Layout::V4 => nsmp::MAX_NAME_V3_LEN,
        };
        let stated = format!("{:?}", target.name);
        match &self.choices.name {
            Some(NameChoice::Rename(name)) => {
                self.changed(Field::Name, stated, Reason::Renamed);
                target.name = name.clone();
                return;
            }
            Some(NameChoice::Truncate) if target.name.len() > capacity => {
                let mut end = capacity;
                while !target.name.is_char_boundary(end) {
                    end -= 1;
                }
                target.name.truncate(end);
                self.changed(Field::Name, stated, Reason::Truncated { capacity });
                return;
            }
            Some(NameChoice::Truncate) => {}
            None if target.name.len() > capacity => {
                self.ask(Choice::Name, Field::Name, stated);
                return;
            }
            None => {}
        }
        if self.from.chain == Chain::Early {
            self.by_rule(Field::Name, "\"\"", Reason::EarlyChain);
        }
    }

    fn categories(&mut self, target: &mut Instrument) {
        let defaults = NarrowCat::editor_default();
        if self.from.chain == Chain::Early {
            for (field, value) in [
                (Field::Category, defaults.category.to_string()),
                (Field::SubCategory, defaults.sub_category.to_string()),
            ] {
                self.by_rule(field, value, Reason::EarlyChain);
            }
        }
        let narrow_only = [
            (Field::Timbre, target.timbre, defaults.timbre),
            (Field::Envelope, target.envelope, defaults.envelope),
            (Field::Motion, target.motion, defaults.motion),
        ];
        let labels = [
            (
                Field::Production,
                target.production.clone(),
                defaults.production,
            ),
            (Field::Origin, target.origin.clone(), defaults.origin),
        ];
        if self.to != Layout::V2 {
            for (field, value, default) in narrow_only {
                if let Some(value) = value.filter(|&v| v != default) {
                    self.dropped(field, value, Reason::NoField(self.to));
                }
            }
            for (field, value, default) in labels {
                if let Some(value) = value.filter(|v| *v != default) {
                    self.dropped(field, format!("{value:?}"), Reason::NoField(self.to));
                }
            }
            (target.timbre, target.envelope, target.motion) = (None, None, None);
            (target.production, target.origin) = (None, None);
            return;
        }
        let reason = match self.from.chain {
            Chain::Early => Reason::EarlyChain,
            Chain::Library2 | Chain::Wide => Reason::EditorDefault,
        };
        for (field, value, default) in narrow_only {
            if value.is_none() {
                self.by_rule(field, default, reason.clone());
            }
        }
        for (field, value, default) in labels {
            if value.is_none() {
                self.by_rule(field, format!("{default:?}"), reason.clone());
            }
        }
    }

    fn velocity_depths(&mut self, target: &mut Instrument) {
        let defaults = encode::Preset::default();
        let depths = [
            (
                Field::VelocityToAmplitude,
                &mut target.velocity_to_amplitude,
                defaults.velocity_to_amplitude,
            ),
            (
                Field::VelocityToTimbre,
                &mut target.velocity_to_timbre,
                defaults.velocity_to_timbre,
            ),
        ];
        for (field, depth, default) in depths {
            match (self.to, *depth) {
                (Layout::V2, None) => {
                    self.report
                        .from_rules
                        .push(line(field, default, Reason::EditorDefault));
                }
                (Layout::V2, Some(_)) => {}
                (_, Some(value)) => {
                    if value != default {
                        self.report
                            .dropped
                            .push(line(field, value, Reason::NoField(self.to)));
                    }
                    *depth = None;
                }
                (_, None) => {}
            }
        }
    }

    /// The zone-level fields, and the order a narrow target stores zones in. Lines
    /// name zones by their index in the source.
    fn zones(&mut self, target: &mut Instrument) {
        for index in 0..target.zones.len() {
            let at = |field| Field::Zone { index, field };
            if self.to == Layout::V2 {
                let zone = &mut target.zones[index];
                let window = zone.velocity.take().filter(|w| *w != VelocityWindow::FULL);
                let decay = zone.loop_decay.take().filter(|&d| d != DEFAULT_LOOP_DECAY);
                if let Some(window) = window {
                    self.dropped(
                        at(ZoneField::Velocity),
                        format!("{}..={}", window.low, window.high),
                        Reason::NoField(self.to),
                    );
                }
                if let Some(decay) = decay {
                    self.dropped(at(ZoneField::LoopDecay), decay, Reason::NoField(self.to));
                }
                self.gain(index, target);
            } else if self.widening() {
                self.by_rule(
                    at(ZoneField::LoopDecay),
                    DEFAULT_LOOP_DECAY,
                    Reason::EditorDefault,
                );
            }
            self.loop_mark(index, &mut target.zones[index].audio);
        }
        if self.to == Layout::V2 {
            let mut order = std::mem::take(&mut self.order);
            order.sort_by_key(|&i| std::cmp::Reverse(target.zones[i].top_note));
            let mut zones: Vec<_> = order.iter().map(|&i| target.zones[i].clone()).collect();
            self.key_ranges(&mut zones, &order);
            self.stroke_ids(&mut zones, &order);
            for zone in &mut zones {
                zone.low_note = None;
            }
            target.zones = zones;
            self.order = order;
        }
    }

    /// A narrow zone record names its stroke by the id's low byte, so ids that share
    /// one are numbered again, from the top zone down as the editor numbers them.
    /// `zones` are high to low, and `order` holds each one's index in the source.
    fn stroke_ids(&mut self, zones: &mut [hub::Zone], order: &[usize]) {
        let low = |zone: &hub::Zone| zone.global_id & 0xff;
        let shared = zones
            .iter()
            .enumerate()
            .any(|(i, z)| zones[..i].iter().any(|y| low(y) == low(z)));
        if !shared {
            return;
        }
        let count = zones.len() as u32;
        for (at, zone) in zones.iter_mut().enumerate() {
            let id = count - at as u32;
            if zone.global_id != id {
                let field = Field::Zone {
                    index: order[at],
                    field: ZoneField::GlobalId,
                };
                self.report
                    .changed
                    .push(line(field, zone.global_id, Reason::Renumbered));
                zone.global_id = id;
            }
        }
    }

    /// Zone gain into a narrow record, whose 24 bits wrap past a gain of 16.
    fn gain(&mut self, index: usize, target: &mut Instrument) {
        let zone = &mut target.zones[index];
        let units = (zone.gain * f64::from(GAIN_UNITY)).round();
        if units.is_nan() || units < RECORD_GAIN_LIMIT {
            return;
        }
        let field = Field::Zone {
            index,
            field: ZoneField::Gain,
        };
        let stated = format!("{}", zone.gain);
        let ceiling = (RECORD_GAIN_LIMIT - 1.0) / f64::from(GAIN_UNITY);
        match self.choices.gain {
            None => self.ask(Choice::Gain, field, stated),
            Some(GainChoice::Clamp) => {
                zone.gain = ceiling;
                self.changed(
                    field,
                    stated,
                    Reason::Clamped {
                        to: format!("{ceiling}"),
                    },
                );
            }
            Some(GainChoice::Bake) => {
                let factor = zone.gain / ceiling;
                zone.gain = ceiling;
                let clipped = bake(&mut zone.audio, factor);
                self.changed(field, stated, Reason::Baked { factor, clipped });
            }
        }
    }

    /// Where the narrow chain's tiling disagrees with stated low notes: a gap below a
    /// zone plays that zone, and an overlap waits for a choice. `zones` are high to
    /// low, and `order` holds each one's index in the source.
    fn key_ranges(&mut self, zones: &mut [hub::Zone], order: &[usize]) {
        for at in 0..zones.len() {
            let Some(low) = zones[at].low_note else {
                continue;
            };
            let field = |at: usize, field| Field::Zone {
                index: order[at],
                field,
            };
            let reaches = match zones.get(at + 1) {
                Some(below) => below.top_note.saturating_add(1),
                None if low <= KEY_FLOOR => continue,
                None => 0,
            };
            if low > reaches {
                self.dropped(
                    field(at, ZoneField::LowNote),
                    low,
                    Reason::Tiled { reaches },
                );
                continue;
            }
            if low == reaches {
                continue;
            }
            match self.choices.overlap {
                None => self.ask(Choice::Overlap, field(at, ZoneField::LowNote), low),
                Some(OverlapChoice::Lower) => {
                    self.changed(field(at, ZoneField::LowNote), low, Reason::LowerKeeps)
                }
                Some(OverlapChoice::Upper) => {
                    let below = &mut zones[at + 1];
                    let top = below.top_note;
                    below.top_note = low.saturating_sub(1);
                    self.changed(field(at + 1, ZoneField::TopNote), top, Reason::UpperKeeps);
                }
            }
        }
    }

    /// A loop mark against the target's minimum gap from the resync point.
    fn loop_mark(&mut self, index: usize, audio: &mut Lattice) {
        let Some(at) = audio.mark else {
            return;
        };
        let channels = usize::from(audio.channels);
        let field = |field| Field::Zone { index, field };
        let floor = audio.resync_at + encode::min_resync_gap(self.to) * channels;
        if at < floor {
            match self.choices.loop_mark {
                None => self.ask(Choice::LoopMark, field(ZoneField::LoopMark), at),
                Some(LoopMarkChoice::Resync) => {
                    let to = at - encode::min_resync_gap(self.to) * channels;
                    self.changed(
                        field(ZoneField::Resync),
                        audio.resync_at,
                        Reason::ResyncMoved { to },
                    );
                    audio.resync_at = to;
                }
                Some(LoopMarkChoice::Push) => {
                    push(audio, floor);
                    self.changed(
                        field(ZoneField::LoopMark),
                        at,
                        Reason::MarkPushed { to: floor },
                    );
                }
            }
            return;
        }
        if self.widening() && at == audio.resync_at + encode::min_resync_gap(Layout::V2) * channels
        {
            if let Some(to) = pull(audio, floor) {
                self.by_rule(field(ZoneField::LoopMark), at, Reason::MarkPulled { to });
            }
        }
    }

    /// Lines that depend on the converted streams: a shift the target's rule
    /// coarsens, a loop laid out over more periods, and a statistic B whose sign the
    /// narrow chain did not store.
    fn streams(&mut self, target: &Instrument, output: &Sample) -> Result<(), ConvertError> {
        let (written, _) = hub::read(output).map_err(ConvertError::Write)?;
        let order = self.order.clone();
        for ((zone, out), index) in target.zones.iter().zip(&written.zones).zip(order) {
            let field = |field| Field::Zone { index, field };
            if out.audio.shift > zone.audio.shift {
                self.by_rule(
                    field(ZoneField::Shift),
                    zone.audio.shift,
                    Reason::ShiftRule {
                        layout: self.to,
                        to: out.audio.shift,
                    },
                );
            }
            let (laid, given) = (out.audio.fields.len(), zone.audio.fields.len());
            if let Some(mark) = zone.audio.mark.filter(|_| laid > given) {
                let periods = (laid - mark) / (given - mark);
                self.changed(
                    field(ZoneField::Stream),
                    given,
                    Reason::LoopRepeated { periods },
                );
            }
            if let (Peak::Magnitude(m), Peak::Signed(_)) = (zone.audio.peak, out.audio.peak) {
                self.by_rule(field(ZoneField::Peak), m, Reason::SignFromContent);
            }
        }
        Ok(())
    }

    /// Streams whose bytes the editor's record coding does not reproduce in their own
    /// generation, and stroke header bytes no field names.
    fn recoded(&mut self, sample: &Sample, source: &Instrument) {
        if self.from.chain == Chain::Early {
            return;
        }
        let rebuilt = hub::write(source, self.from.layout).ok();
        let rebuilt = rebuilt.as_ref().map(strokes).unwrap_or_default();
        for (index, zone) in source.zones.iter().enumerate() {
            let stroke = strokes(sample)
                .into_iter()
                .find(|s| leading_id(s) == Some(zone.global_id));
            let ours = rebuilt
                .iter()
                .find(|s| leading_id(s) == Some(zone.global_id));
            let (Some(stroke), Some(ours)) = (stroke, ours) else {
                self.changed(
                    Field::Zone {
                        index,
                        field: ZoneField::Stream,
                    },
                    zone.audio.fields.len(),
                    Reason::Recoded,
                );
                continue;
            };
            let header = self
                .from
                .layout
                .header_len()
                .min(stroke.len())
                .min(ours.len());
            let unnamed: Vec<(usize, usize)> = (0..header)
                .filter(|at| !STROKE_HEADER_FIELDS.iter().any(|r| r.contains(at)))
                .map(|at| (at, at))
                .collect();
            for range in differing(&stroke[..header], &ours[..header], unnamed) {
                let value = hex(&stroke[range.clone()]);
                self.dropped(
                    bytes(&format!("zones[{index}].stk"), range, None),
                    value,
                    Reason::Unmodelled,
                );
            }
            if stroke[header..] != ours[header..] {
                self.changed(
                    Field::Zone {
                        index,
                        field: ZoneField::Stream,
                    },
                    zone.audio.fields.len(),
                    Reason::Recoded,
                );
            }
        }
    }
}

/// Stroke header bytes that hold a field or the word directory, which a stream laid
/// out again rewrites; the rest the writer holds constant.
const STROKE_HEADER_FIELDS: [Range<usize>; 9] = [
    0..4,
    5..6,
    8..9,
    9..16,
    20..22,
    29..31,
    38..40,
    47..49,
    57..66,
];

fn line(field: Field, value: impl ToString, reason: Reason) -> Line {
    Line {
        field,
        value: value.to_string(),
        reason,
    }
}

fn bytes(section: &str, range: Range<usize>, known_as: Option<&'static str>) -> Field {
    Field::Bytes {
        section: section.to_string(),
        range,
        known_as,
    }
}

fn hex(bytes: &[u8]) -> String {
    const SHOWN: usize = 16;
    let shown: String = bytes
        .iter()
        .take(SHOWN)
        .map(|b| format!("{b:02x}"))
        .collect();
    match bytes.len() > SHOWN {
        true => format!("{shown}…"),
        false => shown,
    }
}

fn header_of(sample: &Sample) -> &crate::cbin::Header {
    match sample {
        Sample::V2(file) => &file.header,
        Sample::V3(file) => &file.header,
    }
}

/// Every section as `(tag, version, payload)`, in file order.
fn sections(sample: &Sample) -> Vec<(String, u32, Vec<u8>)> {
    match sample {
        Sample::V2(file) => file
            .body
            .sections
            .iter()
            .map(|s| (s.tag_str(), u32::from(s.version), s.payload.clone()))
            .collect(),
        Sample::V3(file) => file
            .body
            .sections
            .iter()
            .map(|s| (s.tag_str(), s.version, s.payload.clone()))
            .collect(),
    }
}

fn strokes(sample: &Sample) -> Vec<&[u8]> {
    sample
        .stroke_streams()
        .into_iter()
        .map(|(_, s)| s)
        .collect()
}

fn leading_id(stroke: &[u8]) -> Option<u32> {
    stroke.first_chunk().map(|b| u32::from_be_bytes(*b))
}

/// Runs of source bytes that differ from `ours`, where `mapped` pairs each source
/// position with the position in `ours` that states the same thing; a position `ours`
/// does not reach differs. Runs a few bytes apart are reported as one.
fn differing(source: &[u8], ours: &[u8], mapped: Vec<(usize, usize)>) -> Vec<Range<usize>> {
    const JOINED: usize = 4;
    let mut runs: Vec<Range<usize>> = Vec::new();
    for (at, theirs) in mapped {
        if source.get(at) == ours.get(theirs) {
            continue;
        }
        match runs.last_mut() {
            Some(run) if at - run.end < JOINED => run.end = at + 1,
            _ => runs.push(at..at + 1),
        }
    }
    runs
}

/// Byte regions of a section that are known to hold something, though no field reads
/// it here, so a difference inside one is reported as the whole region.
fn known(tag: &str, layout: Layout, payload: &[u8]) -> Vec<(Range<usize>, &'static str)> {
    let keys = nsmp::keymap::KEYS;
    match (tag, layout) {
        ("sty", Layout::V2) => vec![],
        ("sty", Layout::V3) => vec![
            (12..13, "dynamics_response"),
            (14..15, "dynamics_curve"),
            (16..17, "dynamics_response"),
        ],
        ("sty", Layout::V4) => vec![
            (4..5, "dynamics_curve"),
            (45..79, "eq"),
            (85..88, "dynamics_response"),
        ],
        ("map", Layout::V3 | Layout::V4) => {
            let stride = match payload.len() > nsmp::zone::Wide::V21.count_at().unwrap_or(0) {
                true => nsmp::keymap::RECORD_LEN + 4,
                false => nsmp::keymap::RECORD_LEN,
            };
            let at = nsmp::keymap::RECORD_LEN;
            vec![(at..at + keys * stride, "per-key table")]
        }
        _ => vec![],
    }
}

/// Repeat the loop's own fields past the end until its mark sits at `to`, keeping its
/// length.
fn push(audio: &mut Lattice, to: usize) {
    let Some(at) = audio.mark else {
        return;
    };
    let length = audio.fields.len() - at;
    for _ in at..to {
        let repeated = audio.fields[audio.fields.len() - length];
        audio.fields.push(repeated);
    }
    audio.mark = Some(to);
}

/// The narrow chain's floor pushes a loop that starts near the resync point, where a
/// wide one lets the mark sit nearer. Undo as much of that push as the stream's own
/// repetition shows and the wide floor at `floor` allows, which leaves playback as it
/// was. Returns where the mark moved to.
fn pull(audio: &mut Lattice, floor: usize) -> Option<usize> {
    let at = audio.mark?;
    let channels = usize::from(audio.channels);
    let fields = audio.fields.len();
    let length = fields - at;
    let repeated = (0..at.saturating_sub(audio.resync_at))
        .take_while(|&k| {
            fields > k + length
                && audio.fields[fields - 1 - k] == audio.fields[fields - 1 - k - length]
        })
        .count()
        / channels
        * channels;
    let start = at - repeated;
    let to = (start + encode::LOOP_LEAD * channels).max(floor);
    if to >= at {
        return None;
    }
    audio.fields.truncate(fields - (at - to));
    audio.mark = Some(to);
    Some(to)
}

/// Scale every field by `factor` at its stored shift, and statistic B with them.
/// Returns how many fields then dequantize past 16 bits.
fn bake(audio: &mut Lattice, factor: f64) -> usize {
    let scale = |v: i64| (v as f64 * factor).round() as i64;
    let shift = audio.shift;
    let mut clipped = 0;
    for field in &mut audio.fields {
        let scaled = scale(i64::from(*field));
        let wide = match shift >= 0 {
            true => scaled << shift,
            false => scaled >> -shift,
        };
        clipped += usize::from(i64::from(i16::MIN) > wide || wide > i64::from(i16::MAX));
        *field = scaled.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    }
    audio.peak = match audio.peak {
        Peak::Magnitude(m) => Peak::Magnitude(scale(i64::from(m)).clamp(0, (1 << 24) - 1) as u32),
        Peak::Signed(s) => {
            Peak::Signed(scale(i64::from(s)).clamp(-(1 << 23), (1 << 23) - 1) as i32)
        }
    };
    clipped
}
