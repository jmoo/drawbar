//! The namesake: nine organ drawbars over a wide register field.
//!
//! The organ panel stores a registration as nine nibbles, high nibble first. The field
//! is too wide to enumerate, so `nord-format` spells it as its stored bits
//! (`0x087654321`) and `nord inspect` prints the same digits (`888800000`). Nibble n is
//! bar n in that printed order; the rest of this module is drawing.

use eframe::egui;

/// Bars in a register, and the highest position one can be pulled to.
pub const BARS: usize = 9;
pub const MAX: u8 = 8;

/// Hammond's drawbar footages, in the order the panel lays them out, with the classic
/// stop colors: the two sub-octave bars brown, the mutations black, the unison and
/// octave ranks white.
///
/// Decoration only. The encoding fixes a bar's nibble index; the footage labels are the
/// panel's convention and are not stored in the file.
const FOOTAGE: [&str; BARS] = ["16", "5⅓", "8", "4", "2⅔", "2", "1⅗", "1⅓", "1"];

/// Every rank, in register order.
pub const ALL_RANKS: [usize; BARS] = [0, 1, 2, 3, 4, 5, 6, 7, 8];

fn stop_color(visuals: &egui::Visuals, bar: usize) -> egui::Color32 {
    match bar {
        0 | 1 => egui::Color32::from_rgb(0x6b, 0x4a, 0x33),
        4 | 6 | 7 => crate::app::stop_black(visuals),
        _ => crate::app::stop_white(visuals),
    }
}

/// The color of a stop whose rank is unknown. It matches none of the three rank colors,
/// and the stop gets no footage label.
const NO_RANK: egui::Color32 = egui::Color32::from_rgb(0x8a, 0x8a, 0x90);

/// The nine positions a stored register holds, bar 0 first.
pub fn bars(bits: u64) -> [u8; BARS] {
    std::array::from_fn(|n| {
        let shift = 4 * (BARS - 1 - n) as u32;
        ((bits >> shift) & 0xf) as u8
    })
}

/// `bits` with each moved bar written into its nibble, or `None` if one was moved past
/// [`MAX`].
///
/// ⚠️ Only moved nibbles are written. A stored nibble above [`MAX`] is no position this
/// widget can show, and rewriting it would edit a bar nobody pulled.
pub fn written(bits: u64, moved: [u8; BARS]) -> Option<u64> {
    let mut out = bits;
    for (n, (was, now)) in bars(bits).into_iter().zip(moved).enumerate() {
        if was == now {
            continue;
        }
        if now > MAX {
            return None;
        }
        let shift = 4 * (BARS - 1 - n) as u32;
        out = (out & !(0xf << shift)) | (u64::from(now) << shift);
    }
    Some(out)
}

/// A stored register in the form `set_field` accepts. It matches the form
/// `nord_format::fields::settable_form` produces, so an unedited field compares equal.
pub fn spell(bits: u64) -> String {
    format!("{bits:#x}")
}

/// Parse a stored register as a field prints it: `0x…` or decimal.
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

/// The positions in the panel's grouping, `88 8000 000`: the two sub-octave bars, the
/// four foundation ranks, then the three upper mutations.
///
/// A stored position above [`MAX`] is not a drawbar position, so it reads as `?`.
pub fn digits(positions: &[u8]) -> String {
    let mut out = String::with_capacity(BARS + 2);
    for (n, &position) in positions.iter().enumerate() {
        if n == 2 || n == 6 {
            out.push(' ');
        }
        out.push(match position {
            0..=MAX => (b'0' + position) as char,
            _ => '?',
        });
    }
    out
}

/// The first `ranks.len()` bars of `positions`, each labeled and colored for its rank.
/// Returns the new positions when a bar is pulled.
///
/// With `live` false the bars are dimmed and ignore input. That is for a registration
/// the instrument is not reading, where draggable bars would suggest that moving them
/// does something.
///
/// ⚠️ The bass manual of B3 + bass has two drawbars, and they are not the first two
/// nibbles of a nine-drawbar block: they are separate fields, `b3_bass_bar1` and
/// `b3_bass_bar2`. Drawing nine there would show a registration that plays nothing.
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

/// A single drawbar, the `rank`-th of a registration, for the Stage bodies, which store
/// one field per bar. Returns the new position when it is pulled.
pub fn ui_one(ui: &mut egui::Ui, rank: Option<usize>, position: u8, live: bool) -> Option<u8> {
    let mut moved = position;
    bar(ui, rank, &mut moved, live).then_some(moved)
}

/// One drawbar. Pull down to increase, as on the instrument.
fn bar(ui: &mut egui::Ui, rank: Option<usize>, value: &mut u8, live: bool) -> bool {
    let size = egui::vec2(BAR_W, TRACK_H + 16.0);
    let sense = match live {
        true => egui::Sense::click_and_drag(),
        false => egui::Sense::hover(),
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);

    let track = egui::Rect::from_min_size(rect.min, egui::vec2(BAR_W, TRACK_H));
    // The stop's center is at the top of its travel at position 0 and at the bottom at
    // MAX. Everything below uses this one mapping in both directions.
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

    // A stored position above MAX is painted at the bottom: the readout shows that it
    // is out of range, and a stop drawn off the end of its track would not.
    let center = top + travel * (f32::from((*value).min(MAX)) / MAX as f32);
    let stop = egui::Rect::from_center_size(
        egui::pos2(track.center().x, center),
        egui::vec2(BAR_W - 2.0, STOP_H),
    );
    let color = dim(rank.map_or(NO_RANK, |rank| stop_color(ui.visuals(), rank)));
    painter.rect_filled(stop, 2.0, color);
    painter.text(
        stop.center(),
        egui::Align2::CENTER_CENTER,
        value.to_string(),
        egui::FontId::monospace(10.0),
        match color.intensity() > 0.5 {
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

    /// Getting this backwards would silently mirror every registration.
    #[test]
    fn nibble_n_is_bar_n_left_to_right() {
        assert_eq!(bars(0x0_8765_4321), [0, 8, 7, 6, 5, 4, 3, 2, 1]);
        assert_eq!(bars(0x8_8880_0000), [8, 8, 8, 8, 0, 0, 0, 0, 0]);
        assert_eq!(bars(0), [0; BARS]);
    }

    #[test]
    fn positions_and_stored_bits_are_inverses() {
        for value in [0u64, 0x0_8765_4321, 0x8_8880_0000, 0x8_8888_8888] {
            assert_eq!(written(value, bars(value)), Some(value));
        }
        let positions = [1, 2, 3, 4, 5, 6, 7, 8, 0];
        assert_eq!(
            bars(written(0, positions).expect("every bar is a stop")),
            positions
        );
    }

    /// A bar pulled past the end would spill into its neighbor's nibble, since two share
    /// a byte.
    #[test]
    fn a_position_past_the_top_is_no_register() {
        assert_eq!(written(0, [9, 0, 0, 0, 0, 0, 0, 0, 0]), None);
        assert_eq!(written(0, [0, 15, 0, 0, 0, 0, 0, 0, 0]), None);
    }

    #[test]
    fn pulling_one_bar_leaves_a_neighbor_out_of_range_alone() {
        let stored = 0xf_0000_0000u64;
        let mut moved = bars(stored);
        assert_eq!(moved[0], 0xf);
        moved[1] = 1;
        assert_eq!(written(stored, moved), Some(0xf_1000_0000));
        assert_eq!(digits(&bars(0xf_1000_0000)), "?1 0000 000");
    }

    /// Returning a bar to where it was is not a change.
    #[test]
    fn the_spelling_matches_what_the_field_reads_back() {
        let value = 0x8_8880_0000u64;
        assert_eq!(spell(value), "0x888800000");
        assert_eq!(parse(&spell(value)), Some(value));
        assert_eq!(parse("2290649224"), Some(2290649224));
        assert_eq!(parse("nonsense"), None);
    }

    #[test]
    fn the_digits_read_out_in_the_panels_groups() {
        assert_eq!(digits(&bars(0x8_8880_0000)), "88 8800 000");
        assert_eq!(digits(&bars(0x0_8765_4321)), "08 7654 321");
        assert_eq!(digits(&[4, 0]), "40");
    }

    /// The stops and the piano keys share two colors, defined in one place.
    #[test]
    fn the_stops_are_painted_in_the_key_colors() {
        for visuals in [egui::Visuals::dark(), egui::Visuals::light()] {
            assert_eq!(stop_color(&visuals, 2), crate::app::stop_white(&visuals));
            assert_eq!(stop_color(&visuals, 8), crate::app::stop_white(&visuals));
            assert_eq!(stop_color(&visuals, 4), crate::app::stop_black(&visuals));
            assert_eq!(stop_color(&visuals, 6), crate::app::stop_black(&visuals));
            // The sub-octave pair is brown, which is neither.
            assert_ne!(stop_color(&visuals, 0), crate::app::stop_white(&visuals));
            assert_ne!(stop_color(&visuals, 0), crate::app::stop_black(&visuals));
        }
        let (dark, light) = (egui::Visuals::dark(), egui::Visuals::light());
        assert_eq!(
            crate::app::stop_white(&dark),
            crate::app::stop_white(&light)
        );
        assert_eq!(
            crate::app::stop_black(&dark),
            crate::app::stop_black(&light)
        );
    }

    #[test]
    fn a_register_is_nine_bars_of_nibble() {
        assert_eq!(bars(u64::MAX >> (64 - 4 * BARS as u32)), [0xf; BARS]);
    }
}
