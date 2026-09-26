//! How large the whole window is drawn: one factor on every point the shell lays out.

/// The zooms offered, in percent, smallest first.
const STEPS: [u16; 8] = [80, 90, 100, 110, 125, 150, 175, 200];

/// Where a zoom nobody has picked stands. The shell's text is 11.5 pt, small beside a
/// desktop's own, so it starts a step up.
const DEFAULT: Zoom = Zoom(3);

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

    /// A stored zoom. Anything that is not one of the steps is the default.
    pub(crate) fn read(text: &str) -> Zoom {
        text.parse::<u16>()
            .ok()
            .and_then(|percent| STEPS.iter().position(|&step| step == percent))
            .map_or(DEFAULT, Zoom)
    }

    pub(crate) fn percent(self) -> u16 {
        STEPS[self.0]
    }

    pub(crate) fn factor(self) -> f32 {
        f32::from(self.percent()) / 100.0
    }

    /// The next step up, or nothing at the largest.
    pub(crate) fn larger(self) -> Option<Zoom> {
        (self.0 + 1 < STEPS.len()).then_some(Zoom(self.0 + 1))
    }

    /// The next step down, or nothing at the smallest.
    pub(crate) fn smaller(self) -> Option<Zoom> {
        self.0.checked_sub(1).map(Zoom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stepping_up_from_the_smallest_visits_every_zoom_once_and_stops_at_the_largest() {
        let mut zoom = Zoom::read("80");
        assert_eq!(zoom.smaller(), None, "nothing below the smallest");
        let mut seen = vec![zoom.percent()];
        while let Some(larger) = zoom.larger() {
            assert_eq!(larger.smaller(), Some(zoom), "a step down undoes a step up");
            zoom = larger;
            seen.push(zoom.percent());
        }
        assert_eq!(seen, [80, 90, 100, 110, 125, 150, 175, 200]);
    }

    #[test]
    fn a_zoom_nobody_picked_is_a_step_above_actual_size() {
        assert_eq!(Zoom::default().percent(), 110);
        assert_eq!(Zoom::default().factor(), 1.1);
    }

    #[test]
    fn a_stored_zoom_is_read_back_and_anything_else_is_the_default() {
        for percent in STEPS {
            assert_eq!(Zoom::read(&percent.to_string()).percent(), percent);
        }
        for text in ["", "105", "1.1", "110%", "-80", "65616"] {
            assert_eq!(Zoom::read(text), Zoom::default(), "{text:?}");
        }
    }
}
