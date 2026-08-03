//! The human rendering of a decoded entity.
//!
//! Everything here is *data*, so it goes to stdout: `nord inspect x.ne5p | grep transpose`
//! has to work.

use nord_format::common::bank::Item;
use nord_format::electro5::{Instrument, OrganModel};
use nord_format::{Entity, Program, Settings, Song};

use crate::ui::Ui;

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

/// Format a library dependency id the way it is worth reading: hex, matching what
/// `nord program deps` reports for the same program.
fn dep_id(id: u32) -> String {
    match id {
        0 => "none".to_string(),
        id => format!("{id:#010x}"),
    }
}

/// Column the values line up in, counting the two-space indent.
const LABEL_WIDTH: usize = 11;
/// Same, for the effect-name column, whose longest entry is `reverb`.
const FX_WIDTH: usize = 7;
/// Same, for a setting's name, whose longest entry is `rotary rotor acceleration`.
const SETTING_WIDTH: usize = 27;

/// `label:     value`, label dimmed so the eye runs down the values. `indent` is 2 for
/// the file's own identity and 4 for anything sitting under a section heading.
fn field(ui: &Ui, indent: usize, label: &str, value: impl std::fmt::Display) -> String {
    let label = format!("{label}:");
    format!(
        "{:indent$}{}{value}",
        "",
        ui.dim(format!("{label:<LABEL_WIDTH$}"))
    )
}

/// Start a group: a blank line, then the heading.
fn section(ui: &Ui, name: &str) {
    ui.out("");
    ui.out(format!("  {}", ui.heading(name)));
}

/// A drawbar position as a block whose height is how far the bar is pulled out.
///
/// `0` is a dot rather than the shortest block: a registration is read by which bars are
/// *out*, and an eighth-block reads as a small value instead of none.
fn level(position: u8) -> char {
    const LEVELS: [char; 9] = ['·', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    LEVELS[(position as usize).min(8)]
}

fn digits(positions: &[u8]) -> String {
    positions.iter().map(u8::to_string).collect()
}

/// Nine drawbars as a chart, with the digits — the data — kept alongside it.
///
/// ⚠️ Without unicode this must return exactly the digits and nothing else: a pipe
/// carries them in that form.
fn drawbars(ui: &Ui, positions: &[u8]) -> String {
    let digits = digits(positions);
    if !ui.unicode() {
        return digits;
    }
    let chart: String = positions.iter().map(|&p| level(p)).collect();
    format!("{chart}  {}", ui.dim(digits))
}

pub fn print(ui: &Ui, entity: &Entity) {
    match entity {
        Entity::Program(Program::Electro5(p)) => {
            let l = p.location();
            let split = if p.schema.center_panel.split {
                format!("yes @ {:?}", p.schema.center_panel.split_point)
            } else {
                "no".to_string()
            };
            ui.out(field(ui, 2, "type", "Electro 5 program (ne5p)"));
            ui.out(field(ui, 2, "location", location(l.x(), l.y())));

            section(ui, "Keyboard");
            for (name, part, octave, sustain, control) in [
                (
                    "lower",
                    p.schema.center_panel.lower_part,
                    p.schema.center_panel.lower_octave_shift.inner(),
                    p.schema.center_panel.lower_sustain,
                    p.schema.center_panel.lower_control,
                ),
                (
                    "upper",
                    p.schema.center_panel.upper_part,
                    p.schema.center_panel.upper_octave_shift.inner(),
                    p.schema.center_panel.upper_sustain,
                    p.schema.center_panel.upper_control,
                ),
            ] {
                ui.out(field(
                    ui,
                    4,
                    name,
                    format!(
                        "{part:?}  {} {octave:+}  {} {}  {} {}",
                        ui.dim("octave"),
                        ui.dim("sustain"),
                        yn(sustain),
                        ui.dim("control"),
                        yn(control),
                    ),
                ));
            }
            ui.out(field(ui, 4, "split", split));
            // The enable is a separate field from the value, so it is shown as the panel
            // shows it — a light that is on or off, not a yes/no answer to "transpose".
            let transpose = format!("{:+}", p.schema.center_panel.transpose.inner());
            ui.out(field(
                ui,
                4,
                "transpose",
                if p.schema.center_panel.transpose_enabled {
                    format!("{transpose}  {}", ui.dim("(on)"))
                } else {
                    ui.dim(format!("{transpose}  (off)"))
                },
            ));
            ui.out(field(
                ui,
                4,
                "part mix",
                format!(
                    "{} {}",
                    p.schema.center_panel.part_mix.as_string(),
                    ui.dim("(lower/upper %)")
                ),
            ));
            ui.out(field(ui, 4, "gain", p.schema.center_panel.gain));

            section(ui, "Voices");
            let (piano, sample) = (&p.schema.piano_panel, &p.schema.sample_panel);
            ui.out(field(
                ui,
                4,
                "piano",
                format!(
                    "{}  {} {}  {} {}  {} {}  {} {}  {} {}",
                    piano.category,
                    ui.dim("model"),
                    piano.piano_model.as_u8(),
                    ui.dim("clav"),
                    piano.clav_model.as_u8(),
                    ui.dim("acoustics"),
                    piano.acoustics.as_u8(),
                    ui.dim("touch"),
                    piano.touch.as_u8(),
                    ui.dim("mono"),
                    yn(piano.mono),
                ),
            ));
            ui.out(field(
                ui,
                4,
                "sample",
                format!(
                    "{} {}  {} {}  {} {}  {} {}  {} {}",
                    ui.dim("number"),
                    sample.number,
                    ui.dim("attack"),
                    sample.attack,
                    ui.dim("decay/rel"),
                    sample.decay_release,
                    ui.dim("dynamics"),
                    sample.dynamics.as_u8(),
                    ui.dim("filter"),
                    yn(sample.filter),
                ),
            ));
            // The file stores only the ids. `nord program deps` reports the same ids for
            // this program with names attached, which is the only way to resolve them.
            ui.out(field(
                ui,
                4,
                "depends",
                format!(
                    "{} {}  {} {}",
                    ui.dim("piano"),
                    dep_id(piano.id),
                    ui.dim("sample"),
                    dep_id(sample.id),
                ),
            ));

            // Effects. Values are printed as stored (0..127); the panel shows most of
            // them on a 0..10 scale, and the two do not map linearly for delay tempo,
            // so rescaling here would invent precision the file does not carry.
            let fx = &p.schema.effects_panel;
            section(ui, "Effects");
            ui.out(format!(
                "    {}",
                ui.dim("stored value, with the panel's 0-10 reading where it applies")
            ));
            // An off effect is printed dimmed rather than skipped.
            let off = |name: &str, value: &dyn std::fmt::Display| {
                ui.out(ui.dim(format!("    {name:<FX_WIDTH$}{value}")))
            };
            match fx.fx1.part() {
                Some(part) => ui.out(format!(
                    "    {:<FX_WIDTH$}{part:<5}  {:<9}  {} {}  {} {}",
                    "fx1",
                    fx.fx1_type,
                    ui.dim("rate"),
                    fx.fx1_rate,
                    ui.dim("control"),
                    yn(fx.fx1_control),
                )),
                None => off("fx1", &fx.fx1),
            }
            match fx.fx2.part() {
                Some(part) => ui.out(format!(
                    "    {:<FX_WIDTH$}{part:<5}  {:<9}  {} {}  {} {}",
                    "fx2",
                    fx.fx2_type,
                    ui.dim("rate"),
                    fx.fx2_rate.as_u8(),
                    ui.dim("deep"),
                    yn(fx.fx2_deep),
                )),
                None => off("fx2", &fx.fx2),
            }
            match fx.fx3.part() {
                Some(part) => ui.out(format!(
                    "    {:<FX_WIDTH$}{part:<5}  {:<9}  {} {}",
                    "fx3",
                    fx.fx3_type,
                    ui.dim("compression"),
                    fx.fx3_compression,
                )),
                None => off("fx3", &fx.fx3),
            }
            match fx.fx4.part() {
                Some(part) => ui.out(format!(
                    "    {:<FX_WIDTH$}{part:<5}  {} {}  {} {}  {} {}  {} {}",
                    "delay",
                    ui.dim("feedback"),
                    fx.fx4_feedback.as_u8(),
                    ui.dim("tempo"),
                    fx.fx4_tempo.as_u8(),
                    ui.dim("wet"),
                    fx.fx4_moisture,
                    ui.dim("ping-pong"),
                    yn(fx.fx4_ping_pong),
                )),
                None => off("delay", &fx.fx4),
            }
            // ⚠️ Inferred from specimens; not confirmed on hardware. `fx5` set is read as
            // reverb *on*: every `fx5_1xx` specimen holds it set, and the opposite sense
            // would make all of them captures of a disabled reverb whose type and wet
            // level were varied.
            if fx.fx5 {
                ui.out(format!(
                    "    {:<FX_WIDTH$}{:<15}  {} {}",
                    "reverb",
                    fx.fx5_type,
                    ui.dim("wet"),
                    fx.fx5_moisture,
                ));
            } else {
                off("reverb", &"off");
            }
            // `0` for the EQ routing means *lower*, not off, so the enable has to be
            // checked first.
            if fx.equalizer_on {
                let part = fx.equalizer_part;
                ui.out(format!(
                    "    {:<FX_WIDTH$}{part:<11}  {} {}  {} {}  {} {}  {} {}",
                    "eq",
                    ui.dim("bass"),
                    fx.equalizer_bass.as_u8(),
                    ui.dim("freq"),
                    fx.equalizer_freq.as_u8(),
                    ui.dim("gain"),
                    fx.equalizer_freq_gain.as_u8(),
                    ui.dim("treble"),
                    fx.equalizer_treble.as_u8(),
                ));
            } else {
                off("eq", &"off");
            }
            ui.out(format!(
                "    {:<FX_WIDTH$}{} {}  {} {}",
                "rotary",
                ui.dim("speed"),
                if fx.rotary_speed { "fast" } else { "slow" },
                ui.dim("stop"),
                if fx.rotary_stop { "on" } else { "off" },
            ));

            // The file keeps the full state of all four models, and b3+bass cannot be
            // read without preset 1 and preset 2 side by side, so every row is printed.
            if p.schema.center_panel.lower_part == Instrument::Organ
                || p.schema.center_panel.upper_part == Instrument::Organ
            {
                let o = &p.schema.organ_panel;
                let selected = p.schema.center_panel.organ_type;
                // b3+bass shares the B3's storage, so it marks the B3 rows.
                let sel_model = selected.storage();
                section(ui, "Organ");
                ui.out(format!(
                    "    {}",
                    ui.dim(format!(
                        "{selected} selected (*), active preset (<), drawbar positions 0-8"
                    ))
                ));
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
                        let bars = if selected.is_b3_bass()
                            && model == OrganModel::B3
                            && preset == 1
                        {
                            // Only two of the nine positions exist on the bass manual;
                            // dots for the rest so it lines up with the other rows and
                            // cannot be misread as a nine-drawbar registration.
                            let b = o.b3_bass_drawbars();
                            let plain = format!("{}{}.......", b[0], b[1]);
                            if ui.unicode() {
                                format!("{}{}·······  {}", level(b[0]), level(b[1]), ui.dim(plain))
                            } else {
                                plain
                            }
                        } else if model == OrganModel::Farfisa {
                            // Farfisa's drawbars are on/off tabs on the panel, but the
                            // file still stores nine positions and the low bits of each
                            // vary independently of the on/off threshold. Show both, or
                            // the display silently discards them.
                            let (on, off) = if ui.unicode() {
                                ('█', '·')
                            } else {
                                ('|', '.')
                            };
                            let tabs: String = o
                                .farfisa_tabs(preset)
                                .iter()
                                .map(|t| if *t { on } else { off })
                                .collect();
                            let pos = digits(&o.drawbars(model, preset)[..]);
                            if ui.unicode() {
                                format!("{tabs}  {}", ui.dim(format!("({pos})")))
                            } else {
                                format!("{tabs} ({pos})")
                            }
                        } else {
                            drawbars(ui, &o.drawbars(model, preset)[..])
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
                        // Padded before styling: an escape sequence inside a width spec
                        // is counted as characters and the column stops lining up.
                        let name = format!("{label:<5}");
                        let name = if Some(model) == sel_model {
                            ui.bold(name)
                        } else {
                            name
                        };
                        ui.out(format!("   {mark}{name} p{preset}{live} {bars}{vib}{perc}"));
                    }
                }
            }
        }
        Entity::Song(Song::Electro5(s)) => {
            let l = s.location();
            ui.out(field(ui, 2, "type", "Electro 5 song / set (ne5t)"));
            ui.out(field(ui, 2, "location", location(l.x(), l.y())));
            section(ui, "Programs");
            for slot in 0..4u16 {
                let p = s.get(slot);
                ui.out(field(
                    ui,
                    4,
                    &format!("slot {}", slot + 1),
                    location(p.x(), p.y()),
                ));
            }
        }
        Entity::Settings(Settings::Electro5(s)) => {
            ui.out(field(ui, 2, "type", "Electro 5 settings (ne5s)"));
            // Grouped and ordered by the instrument's own menus, which is not the order
            // the fields sit in the file.
            for (menu, fields) in s.schema.panel.by_menu() {
                section(ui, menu.title());
                for f in fields {
                    ui.out(format!(
                        "    {}{}",
                        ui.dim(format!("{:<SETTING_WIDTH$}", f.name.replace('_', " "))),
                        f.value,
                    ));
                }
            }
        }
        Entity::Piano(_) => {
            ui.out(field(
                ui,
                2,
                "type",
                format!("piano (npno) {} header/reference only", ui.dash()),
            ));
        }
        Entity::Sample(_) => {
            ui.out(field(
                ui,
                2,
                "type",
                format!("sample (nsmp) {} header/reference only", ui.dash()),
            ));
        }
        Entity::Bundle(nord_format::Bundle::Electro5(b)) => {
            ui.out(field(ui, 2, "type", "backup bundle (zip)"));
            if let Some(name) = b.name() {
                ui.out(field(ui, 2, "name", name));
            }
            ui.out(field(
                ui,
                2,
                "note",
                ui.dim("use --raw to list contained programs/songs"),
            ));
            // Chatter, not data: a partial read is something the operator should see,
            // but a pipe consuming the summary should not.
            for (name, why) in b.skipped() {
                ui.warn(format!("bundle entry skipped: {name}: {why}"));
            }
            let _ = (b.programs(), b.songs()); // decoded; shown via --raw
        }
    }
}
