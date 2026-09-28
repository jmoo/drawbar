//! Anonymous usage and error reports from the browser build. The desktop build sends
//! nothing, and every function here is a no-op there.
//!
//! Each row is one [`Event`] of a kind [`KINDS`] declares. A row carries enums, codes and
//! versions only: never a name, a file, a byte of sound, or free text. No row carries
//! anything that ties it to another row, so a visit cannot be joined to a crash. The
//! collector (`telemetry/` at the repository root) accepts exactly these fields, read
//! from `telemetry.json` beside this crate's manifest, and adds the `edge` ones itself.
//! The privacy page lists all of them, and the collector's tests hold it to that.

use std::cell::RefCell;
use std::collections::VecDeque;

#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(target_arch = "wasm32")]
pub use web::{install, online, report_id, share, sharing, submit, visit, Sharing, Undelivered};

/// Where the collector listens.
pub const ENDPOINT: &str = "https://t.drawbar.app";

/// The only origin that reports. A local build, a fork's deployment and a test run send
/// nothing.
pub const ORIGIN: &str = "https://drawbar.app";

/// The privacy policy, relative to the app like the guide.
pub const PRIVACY: &str = "docs/privacy.html";

/// How a field is written on the wire.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ty {
    /// A short code: an enum value, a version, a location in the source.
    Text,
    Flag,
}

impl Ty {
    fn name(self) -> &'static str {
        match self {
            Ty::Text => "text",
            Ty::Flag => "flag",
        }
    }
}

/// One kind of row.
pub struct Kind {
    pub name: &'static str,
    /// What the app sends, in the order the collector stores them.
    pub fields: &'static [(&'static str, Ty)],
    /// What the collector adds from the request: the browser and operating system from
    /// the user agent, which it then drops, and the country from the edge.
    pub edge: &'static [&'static str],
}

const EDGE: &[&str] = &["browser", "os"];

/// Every kind of row. `start_failed` is sent by `index.html`, before the app exists.
pub const KINDS: &[Kind] = &[
    Kind {
        name: "visit",
        fields: &[
            ("version", Ty::Text),
            ("navigation", Ty::Text),
            ("first_day", Ty::Flag),
            ("first_month", Ty::Flag),
            ("webusb", Ty::Flag),
            ("fits", Ty::Flag),
            ("referrer", Ty::Text),
            ("language", Ty::Text),
        ],
        edge: &["browser", "os", "country"],
    },
    Kind {
        name: "start_failed",
        fields: &[
            ("version", Ty::Text),
            ("step", Ty::Text),
            ("error", Ty::Text),
        ],
        edge: EDGE,
    },
    Kind {
        name: "panic",
        fields: &[
            ("version", Ty::Text),
            ("location", Ty::Text),
            ("model", Ty::Text),
            ("firmware", Ty::Text),
        ],
        edge: EDGE,
    },
    Kind {
        name: "error",
        fields: &[
            ("version", Ty::Text),
            ("domain", Ty::Text),
            ("kind", Ty::Text),
            ("model", Ty::Text),
            ("firmware", Ty::Text),
        ],
        edge: EDGE,
    },
    Kind {
        name: "op",
        fields: &[
            ("version", Ty::Text),
            ("op", Ty::Text),
            ("class", Ty::Text),
            ("outcome", Ty::Text),
            ("took", Ty::Text),
            ("model", Ty::Text),
            ("firmware", Ty::Text),
        ],
        edge: EDGE,
    },
];

/// [`KINDS`] as the collector reads it from `crates/drawbar/telemetry.json`.
pub fn schema() -> String {
    let kinds: Vec<String> = KINDS
        .iter()
        .map(|kind| {
            let fields: Vec<String> = kind
                .fields
                .iter()
                .map(|(name, ty)| format!("      \"{name}\": \"{}\"", ty.name()))
                .collect();
            let edge: Vec<String> = kind.edge.iter().map(|name| format!("\"{name}\"")).collect();
            format!(
                "  \"{}\": {{\n    \"fields\": {{\n{}\n    }},\n    \"edge\": [{}]\n  }}",
                kind.name,
                fields.join(",\n"),
                edge.join(", ")
            )
        })
        .collect();
    format!("{{\n{}\n}}\n", kinds.join(",\n"))
}

/// The attached instrument as rows describe it. Empty fields when none is attached.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Instrument {
    /// The product name from its USB descriptor, which the manufacturer sets.
    pub model: String,
    /// `2.04`, from the firmware in hundredths.
    pub firmware: String,
}

impl Instrument {
    pub fn of(card: &crate::device::DeviceCard) -> Instrument {
        Instrument {
            model: card.product.clone(),
            firmware: card
                .firmware
                .map(|hundredths| format!("{}.{:02}", hundredths / 100, hundredths % 100))
                .unwrap_or_default(),
        }
    }

    fn values(&self) -> [(&'static str, Value<'_>); 2] {
        [
            ("model", Value::Text(&self.model)),
            ("firmware", Value::Text(&self.firmware)),
        ]
    }
}

/// One instrument operation, as [`crate::device::DeviceCmd::metric`] names it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Op {
    pub name: &'static str,
    pub class: Option<u32>,
    /// The operator asked for it, so a success is worth a row too. The walks and reads
    /// the app makes on its own report failures only.
    pub asked: bool,
}

/// One row.
#[derive(Clone, PartialEq, Debug)]
pub enum Event {
    Visit {
        /// `navigate`, `reload`, `back_forward` or `prerender`, as the browser reports the
        /// page load.
        navigation: String,
        first_day: bool,
        first_month: bool,
        webusb: bool,
        /// The window is large enough for the app, not the small-screen notice.
        fits: bool,
        /// The linking page's host alone, or empty.
        referrer: String,
        /// The primary language subtag alone: `de`, not `de-AT`.
        language: String,
    },
    Panic {
        /// `file:line:column`. The message is never sent: it can hold formatted content.
        location: String,
        instrument: Instrument,
    },
    Error {
        domain: &'static str,
        kind: &'static str,
        instrument: Instrument,
    },
    Op {
        op: Op,
        /// `ok`, or the failure's kind as `nord_usb::error::ErrKind` spells it, or
        /// `refused` for a refusal the app made itself.
        outcome: String,
        took: &'static str,
        instrument: Instrument,
    },
}

enum Value<'a> {
    Text(&'a str),
    Owned(String),
    Flag(bool),
}

impl Event {
    pub fn kind(&self) -> &'static str {
        match self {
            Event::Visit { .. } => "visit",
            Event::Panic { .. } => "panic",
            Event::Error { .. } => "error",
            Event::Op { .. } => "op",
        }
    }

    fn values(&self) -> Vec<(&'static str, Value<'_>)> {
        let version = ("version", Value::Text(crate::sheet::VERSION));
        match self {
            Event::Visit {
                navigation,
                first_day,
                first_month,
                webusb,
                fits,
                referrer,
                language,
            } => vec![
                version,
                ("navigation", Value::Text(navigation)),
                ("first_day", Value::Flag(*first_day)),
                ("first_month", Value::Flag(*first_month)),
                ("webusb", Value::Flag(*webusb)),
                ("fits", Value::Flag(*fits)),
                ("referrer", Value::Text(referrer)),
                ("language", Value::Text(language)),
            ],
            Event::Panic {
                location,
                instrument,
            } => {
                let mut values = vec![version, ("location", Value::Text(location))];
                values.extend(instrument.values());
                values
            }
            Event::Error {
                domain,
                kind,
                instrument,
            } => {
                let mut values = vec![
                    version,
                    ("domain", Value::Text(domain)),
                    ("kind", Value::Text(kind)),
                ];
                values.extend(instrument.values());
                values
            }
            Event::Op {
                op,
                outcome,
                took,
                instrument,
            } => {
                let class = op.class.map(|class| class.to_string()).unwrap_or_default();
                let mut values = vec![
                    version,
                    ("op", Value::Text(op.name)),
                    ("class", Value::Owned(class)),
                    ("outcome", Value::Text(outcome)),
                    ("took", Value::Text(took)),
                ];
                values.extend(instrument.values());
                values
            }
        }
    }

    /// The row as the collector reads it: one JSON object.
    pub fn json(&self) -> String {
        let mut out = format!("{{\"event\":{}", quoted(self.kind()));
        for (name, value) in self.values() {
            let value = match value {
                Value::Text(text) => quoted(text),
                Value::Owned(text) => quoted(&text),
                Value::Flag(flag) => flag.to_string(),
            };
            out.push_str(&format!(",{}:{value}", quoted(name)));
        }
        out.push('}');
        out
    }

    /// One line for a report's recent errors, or `None` for a row that is not a fault.
    fn fault(&self) -> Option<String> {
        match self {
            Event::Visit { .. } => None,
            Event::Op { outcome, .. } if outcome == "ok" => None,
            Event::Panic { location, .. } => Some(format!("panic at {location}")),
            Event::Error { domain, kind, .. } => Some(format!("{domain}: {kind}")),
            Event::Op { op, outcome, .. } => Some(format!("{}: {outcome}", op.name)),
        }
    }
}

/// `text` as a JSON string.
pub fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// How long an operation took, bucketed so a row cannot carry a precise timing.
pub fn took(ms: f64) -> &'static str {
    match ms {
        ms if ms < 1_000.0 => "<1s",
        ms if ms < 10_000.0 => "<10s",
        ms if ms < 60_000.0 => "<60s",
        _ => "60s+",
    }
}

/// Whether this is the browser's first visit today and this month, given its last.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn firsts(last: Option<&str>, today: &str) -> (bool, bool) {
    match last {
        None => (true, true),
        Some(last) => (last != today, last.get(..7) != today.get(..7)),
    }
}

/// The primary subtag alone, lowercased: `de` for `de-AT`.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn language(tag: Option<&str>) -> String {
    tag.and_then(|tag| tag.split(['-', '_']).next())
        .filter(|primary| primary.len() <= 3 && primary.chars().all(|c| c.is_ascii_alphabetic()))
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

/// The report's fields as one JSON object. Absent parts are empty strings.
pub fn report_json(fields: &[(&str, &str)]) -> String {
    let body: Vec<String> = fields
        .iter()
        .map(|(name, value)| format!("{}:{}", quoted(name), quoted(value)))
        .collect();
    format!("{{{}}}", body.join(","))
}

/// How many faults a report can list.
const RECENT: usize = 10;

thread_local! {
    static ATTACHED: RefCell<Instrument> = RefCell::default();
    /// The newest faults, whether or not sharing is on: they leave only in a report the
    /// operator sends.
    static FAULTS: RefCell<VecDeque<String>> = RefCell::default();
}

/// Note the instrument that is now attached, or that none is.
pub fn attached(card: Option<&crate::device::DeviceCard>) {
    let instrument = card.map(Instrument::of).unwrap_or_default();
    ATTACHED.with(|attached| *attached.borrow_mut() = instrument);
}

/// The attached instrument. Empty while a panic holds the cell.
pub fn instrument() -> Instrument {
    ATTACHED.with(|attached| {
        attached
            .try_borrow()
            .map(|held| held.clone())
            .unwrap_or_default()
    })
}

/// The newest faults, oldest first.
pub fn recent() -> Vec<String> {
    FAULTS.with(|faults| faults.borrow().iter().cloned().collect())
}

fn record(event: Event) {
    if let Some(line) = event.fault() {
        FAULTS.with(|faults| {
            if let Ok(mut faults) = faults.try_borrow_mut() {
                if faults.len() == RECENT {
                    faults.pop_front();
                }
                faults.push_back(line);
            }
        });
    }
    #[cfg(target_arch = "wasm32")]
    web::queue(&event);
}

/// Something went wrong outside an instrument operation.
pub fn fault(domain: &'static str, kind: &'static str) {
    record(Event::Error {
        domain,
        kind,
        instrument: instrument(),
    });
}

/// An instrument operation finished: `failed` is `None` for a success, else its kind.
pub fn op(op: Op, failed: Option<String>, ms: f64) {
    if failed.is_none() && !op.asked {
        return;
    }
    record(Event::Op {
        op,
        outcome: failed.unwrap_or_else(|| "ok".to_string()),
        took: took(ms),
        instrument: instrument(),
    });
}

/// Milliseconds on a clock that only differences are taken of.
///
/// ⚠️ `std::time::Instant::now()` traps on `wasm32-unknown-unknown`.
pub fn now() -> f64 {
    #[cfg(target_arch = "wasm32")]
    return js_sys::Date::now();
    #[cfg(not(target_arch = "wasm32"))]
    return 0.0;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declared(kind: &str) -> Vec<&'static str> {
        KINDS
            .iter()
            .find(|declared| declared.name == kind)
            .unwrap_or_else(|| panic!("{kind} is not in KINDS"))
            .fields
            .iter()
            .map(|(name, _)| *name)
            .collect()
    }

    fn samples() -> Vec<Event> {
        let instrument = Instrument {
            model: "Nord Electro 5D".into(),
            firmware: "2.04".into(),
        };
        vec![
            Event::Visit {
                navigation: "navigate".into(),
                first_day: true,
                first_month: false,
                webusb: true,
                fits: true,
                referrer: "example.com".into(),
                language: "de".into(),
            },
            Event::Panic {
                location: "src/app.rs:1:2".into(),
                instrument: instrument.clone(),
            },
            Event::Error {
                domain: "usb",
                kind: "lost",
                instrument: instrument.clone(),
            },
            Event::Op {
                op: Op {
                    name: "put",
                    class: Some(1),
                    asked: true,
                },
                outcome: "device-status 0x15".into(),
                took: "<1s",
                instrument,
            },
        ]
    }

    #[test]
    fn every_event_sends_exactly_the_fields_its_kind_declares_in_order() {
        for event in samples() {
            let sent: Vec<&str> = event.values().iter().map(|(name, _)| *name).collect();
            assert_eq!(sent, declared(event.kind()), "{}", event.kind());
        }
    }

    #[test]
    fn a_row_is_one_json_object_naming_its_kind_first() {
        let op = &samples()[3];
        assert_eq!(
            op.json(),
            format!(
                "{{\"event\":\"op\",\"version\":\"{}\",\"op\":\"put\",\"class\":\"1\",\
                 \"outcome\":\"device-status 0x15\",\"took\":\"<1s\",\
                 \"model\":\"Nord Electro 5D\",\"firmware\":\"2.04\"}}",
                crate::sheet::VERSION
            )
        );
    }

    #[test]
    fn quoting_escapes_what_json_requires() {
        assert_eq!(quoted("a\"b\\c\nd\u{1}"), "\"a\\\"b\\\\c\\nd\\u0001\"");
    }

    #[test]
    fn firmware_is_written_in_the_panel_s_form() {
        let card = crate::device::DeviceCard {
            product: "Nord Stage 4".into(),
            manufacturer: None,
            vendor_id: 0,
            product_id: 0,
            serial: Some("never sent".into()),
            interface: None,
            firmware: Some(204),
            build: None,
            kind: None,
            max_transfer: None,
        };
        let instrument = Instrument::of(&card);
        assert_eq!(instrument.firmware, "2.04");
        assert_eq!(instrument.model, "Nord Stage 4");
    }

    #[test]
    fn timings_are_bucketed() {
        assert_eq!(took(999.0), "<1s");
        assert_eq!(took(1_000.0), "<10s");
        assert_eq!(took(59_999.0), "<60s");
        assert_eq!(took(600_000.0), "60s+");
    }

    #[test]
    fn a_success_the_operator_did_not_ask_for_sends_nothing() {
        let walk = Op {
            name: "scan-bank",
            class: Some(1),
            asked: false,
        };
        let before = recent().len();
        op(walk, None, 5.0);
        op(walk, Some("transport".into()), 5.0);
        assert_eq!(recent().len(), before + 1);
        assert_eq!(
            recent().last().map(String::as_str),
            Some("scan-bank: transport")
        );
    }

    #[test]
    fn the_collector_s_schema_is_this_one() {
        let committed = include_str!("../../telemetry.json");
        assert_eq!(
            committed,
            schema(),
            "crates/drawbar/telemetry.json must hold exactly KINDS; write schema() there"
        );
    }

    #[test]
    fn the_page_s_start_failure_row_sends_what_its_kind_declares() {
        let page = include_str!("../../index.html");
        for name in declared("start_failed") {
            assert!(
                page.contains(&format!("{name}:")),
                "index.html's start_failed row lacks `{name}`"
            );
        }
    }

    #[test]
    fn a_browser_with_no_last_visit_is_new_today_and_this_month() {
        assert_eq!(firsts(None, "2026-09-28"), (true, true));
    }

    #[test]
    fn a_second_visit_the_same_day_is_neither() {
        assert_eq!(firsts(Some("2026-09-28"), "2026-09-28"), (false, false));
    }

    #[test]
    fn a_visit_on_a_later_day_of_the_same_month_is_new_today_only() {
        assert_eq!(firsts(Some("2026-09-01"), "2026-09-28"), (true, false));
    }

    #[test]
    fn language_keeps_the_primary_subtag_only() {
        assert_eq!(language(Some("de-AT")), "de");
        assert_eq!(language(Some("EN")), "en");
        assert_eq!(language(Some("x-klingon-long")), "x");
        assert_eq!(language(Some("")), "");
        assert_eq!(language(None), "");
    }
}
