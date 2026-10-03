//! How large the whole window is drawn: one factor on every point the shell lays out.
//!
//! The reader sees steps from the default, never the factor behind them.

use std::cmp::Ordering;

/// The zooms offered, in percent, smallest first.
const STEPS: [u16; 8] = [80, 90, 100, 110, 125, 150, 175, 200];

/// Where a zoom nobody has picked stands: the size the shell is drawn for.
const DEFAULT: Zoom = Zoom(2);

/// A change of zoom a command asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    In,
    Out,
    Reset,
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

    /// A stored zoom. Anything that is not one of the steps is the default.
    pub(crate) fn read(text: &str) -> Zoom {
        text.parse::<u16>()
            .ok()
            .and_then(|percent| STEPS.iter().position(|&step| step == percent))
            .map_or(DEFAULT, Zoom)
    }

    /// "Default", or how many steps from it, signed with a true minus.
    pub(crate) fn label(self) -> String {
        match self.0.cmp(&DEFAULT.0) {
            Ordering::Equal => "Default".to_string(),
            Ordering::Greater => format!("+{}", self.0 - DEFAULT.0),
            Ordering::Less => format!("\u{2212}{}", DEFAULT.0 - self.0),
        }
    }

    /// The factor in percent, which is also how it is stored.
    pub(crate) fn percent(self) -> u16 {
        STEPS[self.0]
    }

    pub(crate) fn factor(self) -> f32 {
        f32::from(self.percent()) / 100.0
    }

    /// Where `step` leads, or nothing where it would change nothing.
    pub(crate) fn after(self, step: Step) -> Option<Zoom> {
        let index = match step {
            Step::In => self.0 + 1,
            Step::Out => self.0.checked_sub(1)?,
            Step::Reset => DEFAULT.0,
        };
        (index < STEPS.len() && index != self.0).then_some(Zoom(index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stepping_up_from_the_smallest_visits_every_zoom_once_and_stops_at_the_largest() {
        let mut zoom = Zoom::read("80");
        assert_eq!(zoom.after(Step::Out), None, "nothing below the smallest");
        let mut seen = vec![zoom.percent()];
        while let Some(larger) = zoom.after(Step::In) {
            assert_eq!(
                larger.after(Step::Out),
                Some(zoom),
                "a step down undoes a step up"
            );
            zoom = larger;
            seen.push(zoom.percent());
        }
        assert_eq!(seen, [80, 90, 100, 110, 125, 150, 175, 200]);
    }

    #[test]
    fn each_zoom_is_named_by_its_steps_from_the_default() {
        let labels: Vec<String> = STEPS
            .iter()
            .map(|percent| Zoom::read(&percent.to_string()).label())
            .collect();
        assert_eq!(
            labels,
            [
                "\u{2212}2",
                "\u{2212}1",
                "Default",
                "+1",
                "+2",
                "+3",
                "+4",
                "+5"
            ]
        );
    }

    #[test]
    fn reset_leads_to_the_default_from_anywhere_but_the_default() {
        assert_eq!(Zoom::default().after(Step::Reset), None);
        for percent in [80, 200] {
            let zoom = Zoom::read(&percent.to_string());
            assert_eq!(zoom.after(Step::Reset), Some(Zoom::default()), "{percent}");
        }
    }

    #[test]
    fn a_zoom_nobody_picked_is_actual_size() {
        assert_eq!(Zoom::default().percent(), 100);
        assert_eq!(Zoom::default().factor(), 1.0);
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
