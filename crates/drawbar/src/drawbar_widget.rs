//! The namesake: nine organ drawbars over a wide register field.
//!
//! The organ panel stores a registration as nine nibbles, high nibble first, and the
//! field is too wide to enumerate — so `nord-format` spells it as its stored bits
//! (`0x087654321`) and `nord inspect` prints the same digits (`888800000`). **Nibble n
//! is bar n in that printed order**, which is the one fact the widget has to get right:
//! everything else here is paint.

use eframe::egui;

/// Bars in a register, and the highest position one can be pulled to.
pub const BARS: usize = 9;
pub const MAX: u8 = 8;

/// Hammond's drawbar footages, in the order the panel lays them out, with the classic
/// stop colours: the two sub-octave bars brown, the mutations black, the unison and
/// octave ranks white.
///
/// Decoration only. What a bar *means* is its nibble index, which is what the encoding
/// fixes; the labels are the panel's convention, not something the file states.
const FOOTAGE: [&str; BARS] = ["16", "5⅓", "8", "4", "2⅔", "2", "1⅗", "1⅓", "1"];

/// Every rank, in register order.
pub const ALL_RANKS: [usize; BARS] = [0, 1, 2, 3, 4, 5, 6, 7, 8];

fn stop_colour(bar: usize) -> egui::Color32 {
    match bar {
        0 | 1 => egui::Color32::from_rgb(0x6b, 0x4a, 0x33),
        4 | 6 | 7 => egui::Color32::from_rgb(0x2a, 0x2a, 0x2e),
        _ => egui::Color32::from_rgb(0xd8, 0xd6, 0xd0),
    }
}

/// A stop standing for no rank in particular: neither one of the three stop colours nor
/// a footage, because nothing has said which rank it is.
const NO_RANK: egui::Color32 = egui::Color32::from_rgb(0x8a, 0x8a, 0x90);

/// The nine positions a stored register holds, bar 0 first.
pub fn bars(bits: u64) -> [u8; BARS] {
    std::array::from_fn(|n| {
        let shift = 4 * (BARS - 1 - n) as u32;
        ((bits >> shift) & 0xf) as u8
    })
}

/// The stored value nine positions spell. Positions above [`MAX`] are clamped: two bars
/// share a byte, so a wider one would silently walk into its neighbour.
pub fn bits(bars: [u8; BARS]) -> u64 {
    bars.iter()
        .fold(0u64, |bits, &bar| (bits << 4) | bar.min(MAX) as u64)
}

/// A stored register as `set_field` spells it back — the same form
/// `nord_format::fields::settable_form` produces, so an unedited field compares equal.
pub fn spell(bits: u64) -> String {
    format!("{bits:#x}")
}

/// Read a stored register out of the way a field prints it: `0x…` or decimal.
pub fn parse(text: &str) -> Option<u64> {
    let text = text.trim();
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

const STOP_H: f32 = 15.0;
const BAR_W: f32 = 21.0;
const TRACK_H: f32 = 104.0;

/// The positions as the panel groups them — `88 8000 000`: the two sub-octave bars,
/// the four foundation ranks, then the three upper mutations.
pub fn digits(positions: &[u8]) -> String {
    let mut out = String::with_capacity(BARS + 2);
    for (n, position) in positions.iter().enumerate() {
        if n == 2 || n == 6 {
            out.push(' ');
        }
        out.push(char::from_digit((*position).min(9) as u32, 10).unwrap_or('?'));
    }
    out
}

/// The first `ranks.len()` bars of `positions`, each labelled and coloured as the rank
/// it stands for. Returns the new positions when one has been pulled.
///
/// `live` false paints them dimmed and ignores input — for a registration the
/// instrument is not reading, where showing draggable bars would assert that moving
/// them does something.
///
/// ⚠️ The bass manual of b3+bass has two, and they are not the first two nibbles of a
/// nine-drawbar block — they are their own fields, at [`BASS_RANKS`]. Drawing nine
/// there would assert a registration that plays nothing.
pub fn ui_ranks(
    ui: &mut egui::Ui,
    positions: [u8; BARS],
    live: bool,
    ranks: &[usize],
) -> Option<[u8; BARS]> {
    let mut moved = positions;
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for (value, &rank) in moved.iter_mut().zip(ranks) {
            if bar(ui, Some(rank), value, live) {
                changed = true;
            }
        }
    });
    changed.then_some(moved)
}

/// One drawbar on its own, as the `rank`-th of a registration — the shape the Stage
/// bodies store, a field per bar. Returns the new position when it is pulled.
pub fn ui_one(ui: &mut egui::Ui, rank: Option<usize>, position: u8, live: bool) -> Option<u8> {
    let mut moved = position;
    bar(ui, rank, &mut moved, live).then_some(moved)
}

/// One drawbar. Pull down to increase, the way the real thing works.
fn bar(ui: &mut egui::Ui, rank: Option<usize>, value: &mut u8, live: bool) -> bool {
    let size = egui::vec2(BAR_W, TRACK_H + 16.0);
    let sense = match live {
        true => egui::Sense::click_and_drag(),
        false => egui::Sense::hover(),
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);

    let track = egui::Rect::from_min_size(rect.min, egui::vec2(BAR_W, TRACK_H));
    // Centre of the stop at position 0 sits at the top of its travel; at MAX, the
    // bottom. Everything below is that one mapping, forwards and backwards.
    let top = track.top() + STOP_H / 2.0;
    let travel = TRACK_H - STOP_H;

    let mut changed = false;
    if live && response.dragged() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let fraction = ((pointer.y - top) / travel).clamp(0.0, 1.0);
            let want = (fraction * MAX as f32).round() as u8;
            changed = want != *value;
            *value = want;
        }
    }

    let painter = ui.painter();
    let dim = |c: egui::Color32| match live {
        true => c,
        false => c.gamma_multiply(0.4),
    };
    painter.rect_filled(track, 3.0, dim(egui::Color32::from_rgb(0x11, 0x11, 0x13)));

    let centre = top + travel * (*value as f32 / MAX as f32);
    let stop = egui::Rect::from_center_size(
        egui::pos2(track.center().x, centre),
        egui::vec2(BAR_W - 2.0, STOP_H),
    );
    let colour = dim(rank.map_or(NO_RANK, stop_colour));
    painter.rect_filled(stop, 2.0, colour);
    painter.text(
        stop.center(),
        egui::Align2::CENTER_CENTER,
        value.to_string(),
        egui::FontId::monospace(10.0),
        match colour.intensity() > 0.5 {
            true => egui::Color32::BLACK,
            false => egui::Color32::WHITE,
        },
    );
    if let Some(rank) = rank {
        painter.text(
            egui::pos2(track.center().x, track.bottom() + 8.0),
            egui::Align2::CENTER_CENTER,
            FOOTAGE[rank],
            egui::FontId::proportional(9.0),
            ui.visuals().weak_text_color(),
        );
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nibble n is bar n, in the order the value is written and printed. This is the
    /// whole contract; getting it backwards silently mirrors every registration.
    #[test]
    fn nibble_n_is_bar_n_left_to_right() {
        assert_eq!(bars(0x0_8765_4321), [0, 8, 7, 6, 5, 4, 3, 2, 1]);
        assert_eq!(bars(0x8_8880_0000), [8, 8, 8, 8, 0, 0, 0, 0, 0]);
        assert_eq!(bars(0), [0; BARS]);
    }

    #[test]
    fn positions_and_stored_bits_are_inverses() {
        for value in [0u64, 0x0_8765_4321, 0x8_8880_0000, 0x8_8888_8888] {
            assert_eq!(bits(bars(value)), value);
        }
        let positions = [1, 2, 3, 4, 5, 6, 7, 8, 0];
        assert_eq!(bars(bits(positions)), positions);
    }

    /// A bar pulled past the end would walk into its neighbour's nibble, since two
    /// share a byte.
    #[test]
    fn a_position_past_the_top_is_clamped_not_wrapped() {
        assert_eq!(
            bars(bits([9, 15, 0, 0, 0, 0, 0, 0, 0])),
            [8, 8, 0, 0, 0, 0, 0, 0, 0]
        );
    }

    /// The widget writes back the same spelling the field reads out, so parking a bar
    /// where it already was is not a change.
    #[test]
    fn the_spelling_matches_what_the_field_reads_back() {
        let value = 0x8_8880_0000u64;
        assert_eq!(spell(value), "0x888800000");
        assert_eq!(parse(&spell(value)), Some(value));
        assert_eq!(parse("2290649224"), Some(2290649224));
        assert_eq!(parse("nonsense"), None);
    }

    /// The readout is the panel's own grouping, so it can be read back onto the bars.
    #[test]
    fn the_digits_read_out_in_the_panels_groups() {
        assert_eq!(digits(&bars(0x8_8880_0000)), "88 8800 000");
        assert_eq!(digits(&bars(0x0_8765_4321)), "08 7654 321");
        assert_eq!(digits(&[4, 0]), "40");
    }

    /// A whole register is nine nibbles wide, which is the encoding this widget rests
    /// on: nibble n is bar n in the printed order.
    #[test]
    fn a_register_is_nine_bars_of_nibble() {
        assert_eq!(bars(u64::MAX >> (64 - 4 * BARS as u32)), [0xf; BARS]);
    }
}
