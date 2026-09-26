//! A rotary control for a continuous value, as on the panel.
//!
//! The instrument's knobs turn through three quarters of a circle, and so does this one:
//! the value's fraction of its range is its fraction of that sweep, and both the pointer
//! and the traveled arc are drawn from it. The rest is drawing.

use eframe::egui;

/// How far a knob turns, in radians: three quarters of a turn, as on the panel.
pub const SWEEP: f32 = 1.5 * std::f32::consts::PI;

/// Vertical drag, in points, that turns a knob from one stop to the other.
///
/// A full sweep in a short flick would make a 0..127 field unusable. This is about a
/// hand's travel for the full range, and finer for shorter ranges.
const DRAG_FOR_SWEEP: f32 = 220.0;

const DIAL: f32 = 42.0;

/// Where `value` sits between the stops, as 0..=1.
///
/// A range with one value reads as fully counterclockwise instead of dividing by zero.
pub fn fraction(value: i64, min: i64, max: i64) -> f32 {
    if max <= min {
        return 0.0;
    }
    let value = value.clamp(min, max);
    (value - min) as f32 / (max - min) as f32
}

/// The value a fraction of the sweep lands on, rounded to the nearest stop.
pub fn value_at(fraction: f32, min: i64, max: i64) -> i64 {
    if max <= min {
        return min;
    }
    let span = (max - min) as f32;
    let step = (fraction.clamp(0.0, 1.0) * span).round() as i64;
    (min + step).clamp(min, max)
}

/// The pointer's angle for a fraction, in radians clockwise from straight up.
///
/// Symmetrical about twelve o'clock, so a field's midpoint points straight up, the way a
/// panel is read at a glance.
pub fn angle(fraction: f32) -> f32 {
    (fraction.clamp(0.0, 1.0) - 0.5) * SWEEP
}

/// Where the lit arc starts: the bottom stop, or zero for a range that straddles zero.
///
/// A knob that runs either side of zero (transpose, an EQ cut and boost) is read as a
/// distance from zero, so an arc filling from the bottom stop would make zero look like
/// half of something.
pub fn origin(min: i64, max: i64) -> f32 {
    match min < 0 && max > 0 {
        true => fraction(0, min, max),
        false => 0.0,
    }
}

/// A point on the dial at `angle` clockwise from straight up.
fn on_dial(center: egui::Pos2, radius: f32, angle: f32) -> egui::Pos2 {
    egui::pos2(
        center.x + radius * angle.sin(),
        center.y - radius * angle.cos(),
    )
}

/// The arc between two fractions, as a polyline dense enough not to read as a polygon.
fn arc(center: egui::Pos2, radius: f32, from: f32, to: f32) -> Vec<egui::Pos2> {
    const STEPS: usize = 32;
    (0..=STEPS)
        .map(|step| {
            let at = from + (to - from) * step as f32 / STEPS as f32;
            on_dial(center, radius, angle(at))
        })
        .collect()
}

/// A knob for `value` in `min..=max`, with its number under it. Returns the value it was
/// turned or typed to, spelled as the field accepts it.
///
/// Drag up to increase and down to decrease; double-click to type a number. With focus,
/// the arrow keys step and Home/End jump to the stops.
///
/// ⚠️ A drag halts at the stops, but typed text is returned as typed, even past a stop or
/// not a number. The field refuses a value it cannot hold; a widget that substituted one
/// would write what nobody typed.
pub fn ui(ui: &mut egui::Ui, id_salt: &str, value: i64, min: i64, max: i64) -> Option<String> {
    let id = ui.make_persistent_id(("knob", id_salt));
    let mut moved = None;
    let mut typed = None;

    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        // ⚠️ Auto ids move when a preceding control is added, losing drag and focus state.
        let (rect, _) = ui.allocate_exact_size(egui::vec2(DIAL, DIAL), egui::Sense::hover());
        let response = ui.interact(rect, id, egui::Sense::click_and_drag());

        // Carry fractional travel so sub-step movement accumulates instead of rounding away.
        let held = id.with("held");
        if response.drag_started() {
            ui.data_mut(|d| d.insert_temp(held, fraction(value, min, max)));
        }
        if response.dragged() {
            let carried: f32 = ui
                .data(|d| d.get_temp(held))
                .unwrap_or_else(|| fraction(value, min, max));
            let turned = (carried - response.drag_delta().y / DRAG_FOR_SWEEP).clamp(0.0, 1.0);
            ui.data_mut(|d| d.insert_temp(held, turned));
            let want = value_at(turned, min, max);
            if want != value {
                moved = Some(want);
            }
        }
        if response.drag_stopped() {
            ui.data_mut(|d| d.remove::<f32>(held));
        }

        if response.has_focus() {
            let step = ui.input(|i| {
                let held = |key| i.key_pressed(key);
                match (
                    held(egui::Key::ArrowUp) || held(egui::Key::ArrowRight),
                    held(egui::Key::ArrowDown) || held(egui::Key::ArrowLeft),
                    held(egui::Key::PageUp),
                    held(egui::Key::PageDown),
                ) {
                    (true, _, _, _) => 1,
                    (_, true, _, _) => -1,
                    (_, _, true, _) => 10,
                    (_, _, _, true) => -10,
                    _ => 0,
                }
            });
            if step != 0 {
                moved = Some((value + step).clamp(min, max));
            }
            if ui.input(|i| i.key_pressed(egui::Key::Home)) {
                moved = Some(min);
            }
            if ui.input(|i| i.key_pressed(egui::Key::End)) {
                moved = Some(max);
            }
        }

        paint(ui, rect, &response, moved.unwrap_or(value), min, max);
        let typing = response.double_clicked();
        response.on_hover_text(format!("{min} … {max}: drag, or double-click to type"));

        typed = readout(ui, id, value, typing);
    });

    typed
        .or_else(|| moved.map(|want| want.to_string()))
        .filter(|want| *want != value.to_string())
}

/// The number under the dial, which becomes an edit box when double-clicked. Returns
/// what was typed, unchanged.
///
/// ⚠️ Nothing here clamps or parses. The field refuses a number past a stop, or text that
/// is not a number, with its own message. Dropping them here would leave the dial on a
/// value the user did not type, with nothing on screen saying why.
fn readout(ui: &mut egui::Ui, id: egui::Id, value: i64, start_editing: bool) -> Option<String> {
    let editing = id.with("editing");
    let arming = id.with("arming");
    let mut buffer: Option<String> = ui.data(|d| d.get_temp(editing));
    if start_editing && buffer.is_none() {
        let text = value.to_string();
        // Opened with the number selected, like a value box anywhere else, so the first
        // keystroke replaces it.
        let mut state = egui::text_edit::TextEditState::default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(text.chars().count()),
            )));
        state.store(ui.ctx(), editing);
        buffer = Some(text);
        ui.data_mut(|d| d.insert_temp(arming, true));
    }

    let Some(mut text) = buffer else {
        ui.label(
            egui::RichText::new(value.to_string())
                .monospace()
                .small()
                .color(ui.visuals().strong_text_color()),
        );
        return None;
    };

    let box_ = ui.add(
        egui::TextEdit::singleline(&mut text)
            .id(editing)
            .desired_width(DIAL)
            .font(egui::TextStyle::Small)
            .horizontal_align(egui::Align::Center),
    );
    // ⚠️ egui reclaims focus on the opening press; keep requesting it until a later frame
    // gets it.
    if ui.data(|d| d.get_temp::<bool>(arming)).unwrap_or(false) {
        match box_.has_focus() {
            true => ui.data_mut(|d| d.remove::<bool>(arming)),
            false if !ui.input(|i| i.pointer.any_pressed()) => {
                ui.memory_mut(|m| m.request_focus(editing));
            }
            false => {}
        }
    }

    let escaped = ui.input(|i| i.key_pressed(egui::Key::Escape));
    // Enter commits, and so does clicking away: a number typed and then left is still
    // what the user meant. Only Escape discards it.
    let done = ui.input(|i| i.key_pressed(egui::Key::Enter)) || box_.lost_focus();

    if escaped {
        ui.data_mut(|d| {
            d.remove::<String>(editing);
            d.remove::<bool>(arming);
        });
        return None;
    }
    if !done {
        ui.data_mut(|d| d.insert_temp(editing, text));
        return None;
    }
    ui.data_mut(|d| {
        d.remove::<String>(editing);
        d.remove::<bool>(arming);
    });
    Some(text.trim().to_string())
}

fn paint(
    ui: &egui::Ui,
    rect: egui::Rect,
    response: &egui::Response,
    value: i64,
    min: i64,
    max: i64,
) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    let visuals = ui.visuals();
    let widget = ui.style().interact(response);
    let center = rect.center();
    let radius = rect.width() / 2.0;
    let at = fraction(value, min, max);
    let painter = ui.painter();

    // The traveled arc over a faint full sweep: the lit part shows how far the knob is
    // turned, like the panel's own scale.
    let track = egui::Stroke::new(2.0_f32, crate::app::unlit(visuals));
    painter.add(egui::Shape::line(
        arc(center, radius - 1.0, 0.0, 1.0),
        track,
    ));
    let from = origin(min, max);
    if (at - from).abs() > f32::EPSILON {
        let lit = egui::Stroke::new(2.5_f32, crate::app::accent(visuals));
        painter.add(egui::Shape::line(arc(center, radius - 1.0, from, at), lit));
    }

    let body = radius - 5.0;
    painter.circle_filled(center, body, widget.bg_fill);
    let rim = match response.has_focus() {
        true => visuals.selection.stroke,
        false => widget.bg_stroke,
    };
    painter.circle_stroke(center, body, rim);

    let pointer = angle(at);
    painter.line_segment(
        [
            on_dial(center, body * 0.30, pointer),
            on_dial(center, body * 0.86, pointer),
        ],
        egui::Stroke::new(2.5_f32, widget.fg_stroke.color),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A knob at the bottom of its range is fully counterclockwise, at the top fully
    /// clockwise, and halfway points straight up.
    #[test]
    fn a_value_sits_where_its_share_of_the_range_puts_it() {
        assert_eq!(fraction(0, 0, 127), 0.0);
        assert_eq!(fraction(127, 0, 127), 1.0);
        assert!((fraction(64, 0, 128) - 0.5).abs() < 1e-6);
        // A signed range is no different: transpose runs -6 ..= 6 and zero is the middle.
        assert!((fraction(0, -6, 6) - 0.5).abs() < 1e-6);
        assert_eq!(fraction(-6, -6, 6), 0.0);
        assert_eq!(fraction(6, -6, 6), 1.0);
    }

    #[test]
    fn the_sweep_is_symmetrical_about_twelve_oclock() {
        assert!((angle(0.5)).abs() < 1e-6);
        assert!((angle(0.0) + SWEEP / 2.0).abs() < 1e-6);
        assert!((angle(1.0) - SWEEP / 2.0).abs() < 1e-6);
        // 270° of travel, no more: the stops are at 7:30 and 4:30, never overlapping.
        assert!((angle(1.0) - angle(0.0) - SWEEP).abs() < 1e-6);
    }

    /// A mapping that rounded the wrong way would leave a knob unable to reach one of its
    /// stops.
    #[test]
    fn a_value_survives_the_trip_to_the_dial_and_back() {
        for range in [(0i64, 127i64), (-6, 6), (0, 1), (0, 31)] {
            let (min, max) = range;
            for value in min..=max {
                assert_eq!(
                    value_at(fraction(value, min, max), min, max),
                    value,
                    "{value} in {min}..={max}"
                );
            }
        }
    }

    /// A drag passes the stop long before the pointer stops moving, and the value must
    /// hold still at the stop.
    #[test]
    fn past_the_stops_is_the_stops() {
        assert_eq!(value_at(-1.0, 0, 127), 0);
        assert_eq!(value_at(2.0, 0, 127), 127);
        assert_eq!(fraction(-40, 0, 127), 0.0);
        assert_eq!(fraction(999, 0, 127), 1.0);
        assert!((angle(-1.0) + SWEEP / 2.0).abs() < 1e-6);
        assert!((angle(9.0) - SWEEP / 2.0).abs() < 1e-6);
    }

    /// A knob that runs either side of zero lights from zero; one that starts at zero
    /// lights from its bottom stop.
    #[test]
    fn a_bipolar_range_lights_from_zero() {
        assert_eq!(origin(0, 127), 0.0);
        assert_eq!(origin(0, 1), 0.0);
        assert!((origin(-6, 6) - 0.5).abs() < 1e-6);
        // The arc starts at zero, so a knob at zero lights nothing.
        assert!((origin(-6, 6) - fraction(0, -6, 6)).abs() < 1e-6);
        // An asymmetric range still starts at zero, not at its middle value.
        assert!((origin(-3, 9) - fraction(0, -3, 9)).abs() < 1e-6);
        assert!(origin(-3, 9) < 0.5);
    }

    /// A field with one legal value has nowhere to turn, and dividing by its own width
    /// would be a NaN painted as an arc.
    #[test]
    fn a_range_with_nothing_in_it_does_not_divide_by_zero() {
        assert_eq!(fraction(5, 5, 5), 0.0);
        assert!(fraction(5, 5, 5).is_finite());
        assert_eq!(value_at(0.7, 5, 5), 5);
        assert_eq!(value_at(0.7, 9, 4), 9);
    }

    /// Dragging up by half of `DRAG_FOR_SWEEP` covers half the range.
    #[test]
    fn a_drag_up_turns_the_knob_open() {
        let ctx = egui::Context::default();
        let mut value = 0i64;
        // The dial is the first thing in the panel, so it sits under the panel's margin.
        let on_dial = egui::pos2(28.0, 28.0);
        let travel = DRAG_FOR_SWEEP / 2.0;

        let frames: [Vec<egui::Event>; 4] = [
            vec![egui::Event::PointerMoved(on_dial)],
            vec![egui::Event::PointerButton {
                pos: on_dial,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            }],
            vec![egui::Event::PointerMoved(on_dial - egui::vec2(0.0, travel))],
            vec![egui::Event::PointerButton {
                pos: on_dial - egui::vec2(0.0, travel),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            }],
        ];
        for events in frames {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    if let Some(moved) = ui_for(ui, "gain", value, 0, 127) {
                        value = moved.parse().expect("a drag lands on a stop");
                    }
                });
            });
        }
        // Half the drag that spans the sweep is half the range, give or take a step.
        assert!((60..=68).contains(&value), "half a sweep landed on {value}");
    }

    #[test]
    fn a_typed_number_is_handed_on_as_it_was_spelled() {
        for (typed, expected) in [
            ("96", Some("96")),
            ("400", Some("400")),
            ("zero", Some("zero")),
        ] {
            let ctx = egui::Context::default();
            let on_dial = egui::pos2(28.0, 28.0);
            let mut got = None;
            let click = || {
                vec![
                    egui::Event::PointerButton {
                        pos: on_dial,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::default(),
                    },
                    egui::Event::PointerButton {
                        pos: on_dial,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::default(),
                    },
                ]
            };
            let frames: [Vec<egui::Event>; 6] = [
                vec![egui::Event::PointerMoved(on_dial)],
                click(),
                click(),
                // The box takes the focus on the frame after the one that opened it.
                Vec::new(),
                vec![egui::Event::Text(typed.to_string())],
                vec![egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::default(),
                }],
            ];
            // Two whole clicks, a frame apart: that is what egui counts as a double
            // click, and one press-and-release is not.
            for events in frames {
                let input = egui::RawInput {
                    events,
                    ..Default::default()
                };
                let _ = ctx.run(input, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        if let Some(moved) = ui_for(ui, "gain", 0, 0, 127) {
                            got = Some(moved);
                        }
                    });
                });
            }
            assert_eq!(got.as_deref(), expected, "typing {typed:?}");
        }
    }

    #[test]
    fn the_arrows_step_a_focused_knob() {
        let ctx = egui::Context::default();
        let mut value = 40i64;
        let mut focused = false;
        for _ in 0..4 {
            let mut events = vec![egui::Event::Key {
                key: egui::Key::ArrowUp,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            }];
            if !focused {
                events.clear();
            }
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    if !focused {
                        // The dial uses the knob's own id, so this is where Tab would
                        // land.
                        let id = ui.make_persistent_id(("knob", "gain"));
                        ui.memory_mut(|m| m.request_focus(id));
                    }
                    if let Some(moved) = ui_for(ui, "gain", value, 0, 127) {
                        value = moved.parse().expect("an arrow steps to a stop");
                    }
                });
            });
            focused = true;
        }
        assert!(value > 40, "the arrows moved it: {value}");
    }

    /// The knob as the document draws it, with an id of its own.
    fn ui_for(ui: &mut egui::Ui, salt: &str, value: i64, min: i64, max: i64) -> Option<String> {
        super::ui(ui, salt, value, min, max)
    }

    /// The dial's geometry: straight up is the top of the circle, and the two stops fall
    /// either side of the bottom.
    #[test]
    fn the_dial_puts_its_points_where_the_angle_says() {
        let center = egui::pos2(0.0, 0.0);
        let up = on_dial(center, 10.0, 0.0);
        assert!(up.x.abs() < 1e-5 && (up.y + 10.0).abs() < 1e-5);
        let right = on_dial(center, 10.0, std::f32::consts::FRAC_PI_2);
        assert!((right.x - 10.0).abs() < 1e-5 && right.y.abs() < 1e-5);
        // Both stops sit below the center, one to each side.
        let low = on_dial(center, 10.0, angle(0.0));
        let high = on_dial(center, 10.0, angle(1.0));
        assert!(low.y > 0.0 && high.y > 0.0);
        assert!(low.x < 0.0 && high.x > 0.0);
    }
}
