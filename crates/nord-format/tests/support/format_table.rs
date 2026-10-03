//! `(tag, body length, a version the reader accepts)` for every CBIN format except
//! those in [`UNTABLED`], each length taken from its format module. A library whose
//! body varies in length from file to file has none.
//!
//! ⚠️ Not a test target. Each test target that includes this module compiles its
//! own copy.
#![allow(dead_code)]

use nord_format::formats::{
    nc2, nc2d, nd2, nd3, ne3, ne4, ne5, ne6, ne7, ng2, nl4, nla1, no3, np, np2, np3, np4, np5,
    npip, npno, ns2, ns3, ns4, nsclassic, nsmp, nw, nw2,
};

/// The dispatched formats [`formats`] leaves out: the Electro 5 formats and the
/// sample and piano libraries.
pub const UNTABLED: &[&str] = &[
    ne5::program::FORMAT,
    ne5::live::FORMAT,
    ne5::song::FORMAT,
    ne5::settings::FORMAT,
    nsmp::FORMAT,
    npno::FORMAT,
];

pub fn formats() -> Vec<(&'static str, Option<usize>, u32)> {
    vec![
        (nc2::program::FORMAT, Some(nc2::program::BODY_LEN), 100),
        (nc2::settings::FORMAT, Some(nc2::settings::BODY_LEN), 100),
        (nc2d::program::FORMAT, Some(nc2d::program::BODY_LEN), 100),
        (nc2d::settings::FORMAT, Some(nc2d::settings::BODY_LEN), 100),
        (nd2::program::FORMAT, Some(nd2::program::BODY_LEN), 3),
        (nd3::kit::FORMAT, Some(nd3::kit::BODY_LEN), 1),
        (ne3::program::FORMAT, Some(ne3::program::BODY_LEN), 101),
        (
            ne3::organ_preset::FORMAT,
            Some(ne3::organ_preset::BODY_LEN),
            100,
        ),
        (ne4::program::FORMAT, Some(ne4::program::BODY_LEN), 103),
        (ne4::live::FORMAT, Some(ne4::live::BODY_LEN), 103),
        (ne4::settings::FORMAT, Some(ne4::settings::BODY_LEN), 100),
        (ne6::program::FORMAT, Some(ne6::program::BODY_LEN), 204),
        (ne6::live::FORMAT, Some(ne6::live::BODY_LEN), 204),
        (ne6::settings::FORMAT, Some(ne6::settings::BODY_LEN), 200),
        (ne7::program::FORMAT, Some(ne7::program::BODY_LEN), 110),
        (ne7::live::FORMAT, Some(ne7::live::BODY_LEN), 110),
        (ne7::settings::FORMAT, Some(ne7::settings::BODY_LEN), 301),
        (ng2::program::FORMAT, Some(ng2::program::BODY_LEN), 102),
        (ng2::live::FORMAT, Some(ng2::live::BODY_LEN), 102),
        (ng2::settings::FORMAT, Some(ng2::settings::BODY_LEN), 102),
        (nl4::program::FORMAT, Some(nl4::program::BODY_LEN), 7),
        (
            nl4::performance::FORMAT,
            Some(nl4::performance::BODY_LEN),
            7,
        ),
        (nl4::settings::FORMAT, Some(nl4::settings::BODY_LEN), 3),
        (nla1::program::FORMAT, Some(nla1::program::BODY_LEN), 6),
        (
            nla1::performance::FORMAT,
            Some(nla1::performance::BODY_LEN),
            6,
        ),
        (nla1::settings::FORMAT, Some(nla1::settings::BODY_LEN), 0),
        (no3::program::FORMAT, Some(no3::program::BODY_LEN), 200),
        (no3::settings::FORMAT, Some(no3::settings::BODY_LEN), 200),
        (np::program::FORMAT, Some(np::program::BODY_LEN), 103),
        (np::live::FORMAT, Some(np::live::BODY_LEN), 104),
        (np::settings::FORMAT, Some(np::settings::BODY_LEN), 100),
        (np2::program::FORMAT, Some(np2::program::BODY_LEN), 1),
        (np2::live::FORMAT, Some(np2::live::BODY_LEN), 0),
        (np2::settings::FORMAT, Some(np2::settings::BODY_LEN), 1),
        (np3::program::FORMAT, Some(np3::program::BODY_LEN), 4),
        (np3::live::FORMAT, Some(np3::live::BODY_LEN), 4),
        (np3::settings::FORMAT, Some(np3::settings::BODY_LEN), 0),
        (np4::program::FORMAT, Some(np4::program::BODY_LEN), 100),
        (np4::live::FORMAT, Some(np4::live::BODY_LEN), 100),
        (np4::settings::FORMAT, Some(np4::settings::BODY_LEN), 100),
        (np5::program::FORMAT, Some(np5::program::BODY_LEN), 101),
        (np5::live::FORMAT, Some(np5::live::BODY_LEN), 101),
        (np5::settings::FORMAT, Some(np5::settings::BODY_LEN), 100),
        (ns2::program::FORMAT, Some(ns2::program::BODY_LEN), 6),
        (ns2::live::FORMAT, Some(ns2::program::BODY_LEN), 6),
        (ns2::synth::FORMAT, Some(ns2::synth::BODY_LEN), 6),
        (ns2::settings::FORMAT, Some(ns2::settings::BODY_LEN), 4),
        (ns3::program::FORMAT, Some(ns3::program::BODY_LEN), 304),
        (ns3::live::FORMAT, Some(ns3::program::BODY_LEN), 304),
        (ns3::song::FORMAT, Some(ns3::song::BODY_LEN), 300),
        (ns3::synth::FORMAT, Some(ns3::synth::BODY_LEN), 300),
        (ns3::settings::FORMAT, Some(ns3::settings::BODY_LEN), 300),
        (ns4::program::FORMAT, Some(ns4::program::BODY_LEN), 313),
        (ns4::live::FORMAT, Some(ns4::program::BODY_LEN), 313),
        (ns4::synth::FORMAT, Some(ns4::synth::BODY_LEN), 208),
        (
            ns4::piano_preset::FORMAT,
            Some(ns4::piano_preset::BODY_LEN),
            203,
        ),
        (
            ns4::organ_preset::FORMAT,
            Some(ns4::organ_preset::BODY_LEN),
            205,
        ),
        (ns4::settings::FORMAT, Some(ns4::settings::BODY_LEN), 106),
        (
            nsclassic::program::FORMAT,
            Some(nsclassic::program::BODY_LEN),
            316,
        ),
        (
            nsclassic::synth::FORMAT,
            Some(nsclassic::synth::BODY_LEN),
            100,
        ),
        (nsclassic::piano_library::FORMAT, None, 210),
        (npip::pipe_library::FORMAT, None, 100),
        (nw::program::FORMAT, Some(nw::program::BODY_LEN), 8),
        (nw::settings::FORMAT, Some(nw::settings::BODY_LEN), 5),
        (nw2::program::FORMAT, Some(nw2::program::BODY_LEN), 301),
        (nw2::live::FORMAT, Some(nw2::live::BODY_LEN), 301),
        (nw2::settings::FORMAT, Some(nw2::settings::BODY_LEN), 300),
    ]
}
