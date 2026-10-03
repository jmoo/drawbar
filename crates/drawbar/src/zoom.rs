//! How large the whole window is drawn: one factor on every point the shell lays out.

use eframe::egui;

/// The zooms offered, in percent, smallest first.
const STEPS: [u16; 11] = [50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200];

/// Where a zoom nobody has picked stands: the size the shell is drawn for.
const DEFAULT: Zoom = Zoom(5);

/// A change of zoom a command asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    In,
    Out,
    Reset,
}

/// Why a step leads nowhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// There is no step that way, or the zoom is already where it leads.
    End,
    /// The next step down would draw a point smaller than a pixel of this display.
    Blur,
    /// The next step up would leave the window too small for the shell.
    Room,
}

/// One of [`STEPS`], by its index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Zoom(usize);

impl Default for Zoom {
    fn default() -> Zoom {
        DEFAULT
    }
}

impl Zoom {
    /// Where the zoom is kept between sessions, apart from the list of what is on this
    /// computer and its budget.
    pub(crate) const KEY: &'static str = "drawbar.zoom";

    /// A stored zoom, at the step nearest it. Anything that is not a percent is the
    /// default.
    pub(crate) fn read(text: &str) -> Zoom {
        let Ok(percent) = text.parse::<u16>() else {
            return DEFAULT;
        };
        let nearest = (0..STEPS.len()).min_by_key(|&index| STEPS[index].abs_diff(percent));
        nearest.map_or(DEFAULT, Zoom)
    }

    /// The factor in percent, which is also how it is stored and shown.
    pub(crate) fn percent(self) -> u16 {
        STEPS[self.0]
    }

    pub(crate) fn factor(self) -> f32 {
        f32::from(self.percent()) / 100.0
    }

    pub(crate) fn is_default(self) -> bool {
        self == DEFAULT
    }

    /// Where `step` leads in `room`. A step in only has to fit and a step out only has to
    /// stay sharp, since each direction only ever improves the other. A reset always
    /// leads home.
    pub(crate) fn after(self, step: Step, room: Room) -> Result<Zoom, Stop> {
        match step {
            Step::In => {
                let next = STEPS
                    .get(self.0 + 1)
                    .map(|_| Zoom(self.0 + 1))
                    .ok_or(Stop::End)?;
                room.fits(next).then_some(next).ok_or(Stop::Room)
            }
            Step::Out => {
                let next = self.0.checked_sub(1).map(Zoom).ok_or(Stop::End)?;
                room.sharp(next).then_some(next).ok_or(Stop::Blur)
            }
            Step::Reset => (!self.is_default()).then_some(DEFAULT).ok_or(Stop::End),
        }
    }
}

/// What the display and the window allow a zoom to be.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Room {
    /// The display's physical pixels per point at 100%.
    pub native: f32,
    /// The window in points at 100%.
    pub screen: egui::Vec2,
}

impl Room {
    /// The room the last frame had. Before the first frame a context knows neither, so
    /// it reads as one pixel per point and a window too large to limit anything.
    pub fn of(ctx: &egui::Context) -> Room {
        Room {
            native: ctx.native_pixels_per_point().unwrap_or(1.0),
            screen: ctx.screen_rect().size() * ctx.zoom_factor(),
        }
    }

    /// ⚠️ Below one pixel per point, egui draws a 1 pt rule as a faint smear and small
    /// text loses its shape. A 2× display stays sharp down to 50%, a 1× display only to
    /// 100%.
    fn sharp(self, zoom: Zoom) -> bool {
        f32::from(zoom.percent()) * self.native >= 100.0
    }

    fn fits(self, zoom: Zoom) -> bool {
        !crate::shell::too_small(self.screen / zoom.factor())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2× display in a window big enough for every step.
    const ROOMY: Room = Room {
        native: 2.0,
        screen: egui::vec2(4000.0, 3000.0),
    };

    fn walk(from: &str, step: Step, room: Room) -> Vec<u16> {
        let mut zoom = Zoom::read(from);
        let mut seen = vec![zoom.percent()];
        while let Ok(next) = zoom.after(step, room) {
            zoom = next;
            seen.push(zoom.percent());
        }
        seen
    }

    #[test]
    fn with_room_to_spare_a_step_visits_every_zoom_once_and_stops_at_either_end() {
        assert_eq!(
            walk("50", Step::In, ROOMY),
            [50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200]
        );
        assert_eq!(Zoom::read("200").after(Step::In, ROOMY), Err(Stop::End));
        assert_eq!(Zoom::read("50").after(Step::Out, ROOMY), Err(Stop::End));
        for percent in STEPS.windows(2) {
            let (low, high) = (
                Zoom::read(&percent[0].to_string()),
                Zoom::read(&percent[1].to_string()),
            );
            assert_eq!(high.after(Step::Out, ROOMY), Ok(low), "{percent:?}");
        }
    }

    #[test]
    fn a_step_out_stops_where_a_point_would_be_smaller_than_a_pixel() {
        let on = |native: f32| Room { native, ..ROOMY };
        assert_eq!(walk("100", Step::Out, on(1.0)), [100]);
        assert_eq!(Zoom::read("100").after(Step::Out, on(1.0)), Err(Stop::Blur));
        assert_eq!(walk("100", Step::Out, on(1.25)), [100, 90, 80]);
        assert_eq!(walk("100", Step::Out, on(1.5)), [100, 90, 80, 75, 67]);
        assert_eq!(walk("100", Step::Out, on(2.0)).last(), Some(&50));
        // Stepping in from a blurred zoom is always a step toward sharp.
        assert_eq!(
            Zoom::read("50").after(Step::In, on(1.0)),
            Ok(Zoom::read("67"))
        );
    }

    #[test]
    fn a_step_in_stops_where_the_window_would_be_too_small_for_the_shell() {
        let least = crate::shell::LEAST;
        let window = |scale: f32| Room {
            screen: least * scale,
            ..ROOMY
        };
        assert_eq!(walk("100", Step::In, window(1.0)), [100]);
        assert_eq!(
            Zoom::read("100").after(Step::In, window(1.0)),
            Err(Stop::Room)
        );
        assert_eq!(walk("100", Step::In, window(1.25)), [100, 110, 125]);
        assert_eq!(walk("100", Step::In, window(2.0)).last(), Some(&200));
        // Stepping out of a zoom too large for the window is always a step toward fitting.
        assert_eq!(
            Zoom::read("200").after(Step::Out, window(1.0)),
            Ok(Zoom::read("175"))
        );
    }

    #[test]
    fn reset_leads_home_from_anywhere_else_whatever_the_room() {
        let cramped = Room {
            native: 1.0,
            screen: egui::Vec2::ZERO,
        };
        assert_eq!(Zoom::default().after(Step::Reset, ROOMY), Err(Stop::End));
        for percent in ["50", "200"] {
            assert_eq!(
                Zoom::read(percent).after(Step::Reset, cramped),
                Ok(Zoom::default()),
                "{percent}"
            );
        }
    }

    #[test]
    fn a_zoom_nobody_picked_is_actual_size() {
        assert!(Zoom::default().is_default());
        assert_eq!(Zoom::default().percent(), 100);
        assert_eq!(Zoom::default().factor(), 1.0);
    }

    #[test]
    fn a_stored_zoom_comes_back_at_the_nearest_step() {
        for percent in STEPS {
            assert_eq!(Zoom::read(&percent.to_string()).percent(), percent);
        }
        for (stored, step) in [
            ("105", 100),
            ("106", 110),
            ("66", 67),
            ("0", 50),
            ("65535", 200),
        ] {
            assert_eq!(Zoom::read(stored).percent(), step, "{stored}");
        }
        for text in ["", "1.1", "110%", "-80", "65536"] {
            assert_eq!(Zoom::read(text), Zoom::default(), "{text:?}");
        }
    }
}
