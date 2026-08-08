#![cfg(feature = "corpus")]
//! Whole-corpus sweep: every specimen of every model parses, classifies to the
//! right entity, and re-encodes byte-exactly — the breadth counterpart to the
//! Electro 5 depth suite in `tests/ne5.rs`.
//!
//! ```sh
//! NORD_CORPUS_ROOT=/path/to/nord-corpus cargo test -p nord-format --features corpus --test corpus
//! ```

use nord_format::Entity;
use std::collections::BTreeMap;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

/// Root of the corpus checkout: `NORD_CORPUS_ROOT`, or the parent of the
/// Electro 5 suite's `NORD_CORPUS_DIR` so one variable can serve both.
fn corpus_root() -> PathBuf {
    if let Some(root) = std::env::var_os("NORD_CORPUS_ROOT") {
        return PathBuf::from(root);
    }
    std::env::var_os("NORD_CORPUS_DIR")
        .map(|d| {
            PathBuf::from(&d)
                .parent()
                .expect("NORD_CORPUS_DIR has no parent")
                .to_path_buf()
        })
        .expect("set NORD_CORPUS_ROOT to a nord-corpus checkout for --features corpus")
}

/// Extensions the sweep reads as entities. Everything else in the corpus —
/// captures, sidecars, manifests, documentation — is deliberately not a format.
fn wanted(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    // `.ne5t.body` files are bare bodies with no container; `.skip.` marks a
    // specimen the suite is told to ignore.
    let name = path.file_name().unwrap().to_string_lossy();
    if name.contains(".skip.") || name.ends_with(".body") {
        return false;
    }
    !matches!(
        ext,
        "md" | "json"
            | "xml"
            | "nix"
            | "lock"
            | "tsv"
            | "bin"
            | "txt"
            | "pcapng"
            | "nsmpproj"
            | "html"
            | "pdf"
            | "gitignore"
            // The ZIP banks get their own bundle-gated test below.
            | "nd2_bank"
            | "nd3_kitbank"
    )
}

/// Every wanted file under the corpus, excluding the un-triaged staging area.
fn specimens(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if path.is_dir() {
                if !matches!(name.as_str(), "pending" | "tools" | ".git") {
                    stack.push(path);
                }
            } else if wanted(&path) {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

#[test]
fn every_specimen_parses_and_round_trips() {
    let root = corpus_root();
    let mut by_format: BTreeMap<String, usize> = BTreeMap::new();
    let mut failures: Vec<String> = Vec::new();

    for path in specimens(&root) {
        let bytes = fs::read(&path).unwrap();
        let entity = match nord_format::from_stream(&mut Cursor::new(&bytes)) {
            Ok(e) => e,
            Err(e) => {
                failures.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        *by_format
            .entry(entity.identity().format.replace('\0', "␀"))
            .or_default() += 1;

        match nord_format::to_bytes(&entity) {
            Ok(back) if back == bytes => {}
            Ok(_) => failures.push(format!("{}: re-encode changed the bytes", path.display())),
            Err(e) => failures.push(format!("{}: re-encode: {e}", path.display())),
        }
    }

    assert!(
        failures.is_empty(),
        "{} specimens failed:\n{}",
        failures.len(),
        failures.join("\n")
    );

    let total: usize = by_format.values().sum();
    println!("{total} specimens across {} formats:", by_format.len());
    for (format, n) in &by_format {
        println!("  {format:<6} {n}");
    }
    // The corpus holds ~9,900 readable specimens; a large drop means a whole
    // model directory silently stopped being read.
    assert!(
        total > 9000,
        "only {total} specimens read — corpus present?"
    );
    // Every CBIN tag the corpus ships plus the three carriers, minus the two
    // R2-only libraries (`nsp`, `npip`) that have no in-git specimen.
    assert!(
        by_format.len() >= 50,
        "only {} formats seen: {by_format:?}",
        by_format.len()
    );
}

/// The stub modules' observed body lengths hold across every specimen.
#[test]
fn observed_body_lengths_match_the_documented_constants() {
    use nord_format::formats::{
        nc2, nc2d, ne3, ne4, ne6, ne7, ng2, nl4, nla1, no3, np, np2, np3, np4, np5, ns2, ns3, ns4,
        nsclassic, nw, nw2,
    };

    let expected: BTreeMap<&str, u64> = BTreeMap::from([
        (nc2::program::FORMAT, nc2::program::BODY_LEN),
        (nc2::settings::FORMAT, nc2::settings::BODY_LEN),
        (nc2d::program::FORMAT, nc2d::program::BODY_LEN),
        (nc2d::settings::FORMAT, nc2d::settings::BODY_LEN),
        (ne3::program::FORMAT, ne3::program::BODY_LEN),
        (ne3::organ_preset::FORMAT, ne3::organ_preset::BODY_LEN),
        (ne4::program::FORMAT, ne4::program::BODY_LEN),
        (ne4::live::FORMAT, ne4::live::BODY_LEN),
        (ne4::settings::FORMAT, ne4::settings::BODY_LEN),
        (ne6::program::FORMAT, ne6::program::BODY_LEN),
        (ne6::live::FORMAT, ne6::live::BODY_LEN),
        (ne6::settings::FORMAT, ne6::settings::BODY_LEN),
        (ne7::program::FORMAT, ne7::program::BODY_LEN),
        (ne7::live::FORMAT, ne7::live::BODY_LEN),
        (ne7::settings::FORMAT, ne7::settings::BODY_LEN),
        (ng2::program::FORMAT, ng2::program::BODY_LEN),
        (ng2::live::FORMAT, ng2::live::BODY_LEN),
        (ng2::settings::FORMAT, ng2::settings::BODY_LEN),
        (nl4::program::FORMAT, nl4::program::BODY_LEN),
        (nl4::performance::FORMAT, nl4::performance::BODY_LEN),
        (nl4::settings::FORMAT, nl4::settings::BODY_LEN),
        (nla1::program::FORMAT, nla1::program::BODY_LEN),
        (nla1::performance::FORMAT, nla1::performance::BODY_LEN),
        (nla1::settings::FORMAT, nla1::settings::BODY_LEN),
        (no3::program::FORMAT, no3::program::BODY_LEN),
        (no3::settings::FORMAT, no3::settings::BODY_LEN),
        (np::program::FORMAT, np::program::BODY_LEN),
        (np::live::FORMAT, np::live::BODY_LEN),
        (np::settings::FORMAT, np::settings::BODY_LEN),
        (np2::program::FORMAT, np2::program::BODY_LEN),
        (np2::live::FORMAT, np2::live::BODY_LEN),
        (np2::settings::FORMAT, np2::settings::BODY_LEN),
        (np3::program::FORMAT, np3::program::BODY_LEN),
        (np3::live::FORMAT, np3::live::BODY_LEN),
        (np3::settings::FORMAT, np3::settings::BODY_LEN),
        (np4::program::FORMAT, np4::program::BODY_LEN),
        (np4::live::FORMAT, np4::live::BODY_LEN),
        (np4::settings::FORMAT, np4::settings::BODY_LEN),
        (np5::program::FORMAT, np5::program::BODY_LEN),
        (np5::live::FORMAT, np5::live::BODY_LEN),
        (np5::settings::FORMAT, np5::settings::BODY_LEN),
        (ns2::program::FORMAT, ns2::program::BODY_LEN as u64),
        (ns2::live::FORMAT, ns2::program::BODY_LEN as u64),
        (ns2::synth::FORMAT, ns2::synth::BODY_LEN),
        (ns2::settings::FORMAT, ns2::settings::BODY_LEN),
        (ns3::program::FORMAT, ns3::program::BODY_LEN as u64),
        (ns3::live::FORMAT, ns3::program::BODY_LEN as u64),
        (ns3::song::FORMAT, ns3::song::BODY_LEN),
        (ns3::synth::FORMAT, ns3::synth::BODY_LEN),
        (ns3::settings::FORMAT, ns3::settings::BODY_LEN),
        (ns4::program::FORMAT, ns4::program::BODY_LEN),
        (ns4::live::FORMAT, ns4::live::BODY_LEN),
        (ns4::synth::FORMAT, ns4::synth::BODY_LEN),
        (ns4::piano_preset::FORMAT, ns4::piano_preset::BODY_LEN),
        (ns4::organ_preset::FORMAT, ns4::organ_preset::BODY_LEN),
        (ns4::settings::FORMAT, ns4::settings::BODY_LEN),
        (nsclassic::program::FORMAT, nsclassic::program::BODY_LEN),
        (nsclassic::synth::FORMAT, nsclassic::synth::BODY_LEN),
        (nw::program::FORMAT, nw::program::BODY_LEN),
        (nw::settings::FORMAT, nw::settings::BODY_LEN),
        (nw2::program::FORMAT, nw2::program::BODY_LEN),
        (nw2::live::FORMAT, nw2::live::BODY_LEN),
        (nw2::settings::FORMAT, nw2::settings::BODY_LEN),
    ]);

    let mut checked = 0usize;
    for path in specimens(&corpus_root()) {
        let mut file = std::fs::File::open(&path).unwrap();
        let Ok(info) = nord_format::cbin::inspect(&mut file) else {
            continue; // not CBIN — the sweep test already classified it
        };
        let tag = String::from_utf8_lossy(&info.header.tag).into_owned();
        if let Some(&want) = expected.get(tag.as_str()) {
            assert_eq!(
                info.body_len,
                want,
                "{}: {tag} body is {} bytes where every prior specimen held {want}",
                path.display(),
                info.body_len,
            );
            checked += 1;
        }
    }
    assert!(checked > 8000, "only {checked} bodies measured");
}

/// The Stage globals decode reads values the panel could actually show, across
/// every factory program of both models. No oracle exists for these — the
/// filename is just a program name — so range-sanity over 1,500 files is the
/// strongest available check that the bit placements are right.
#[test]
fn stage_globals_decode_to_panel_values() {
    use nord_format::{Live, Program};

    let root = corpus_root();
    let mut ns2_seen = 0usize;
    let mut ns3_seen = 0usize;
    let mut ns3_split_on = 0usize;
    let mut ns3_at_default_clock = 0usize;

    for path in specimens(&root) {
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        match ext {
            "ns2p" | "ns2l" => {
                let entity = nord_format::from_path(&path).unwrap();
                let p = match &entity {
                    Entity::Program(Program::Stage2(p)) => p,
                    Entity::Live(Live::Stage2(p)) => p,
                    other => panic!("{}: decoded to {other:?}", path.display()),
                };
                // The Stage 2 EX factory live buffers are all-ones — a slot the
                // instrument never wrote — so every field is legitimately
                // out-of-table there. Real content must stay in the tables.
                let raw = <[u8; 521]>::from(&p.body);
                if raw.iter().all(|&b| b == 0xff) {
                    continue;
                }
                assert!(!p.split_low_note.is_unknown(), "{}", path.display());
                assert!(!p.split_high_note.is_unknown(), "{}", path.display());
                ns2_seen += 1;
            }
            "ns3f" | "ns3l" => {
                let entity = nord_format::from_path(&path).unwrap();
                let p = match &entity {
                    Entity::Program(Program::Stage3(p)) => p,
                    Entity::Live(Live::Stage3(p)) => p,
                    other => panic!("{}: decoded to {other:?}", path.display()),
                };
                assert!(!p.panel_enable.is_unknown(), "{}", path.display());
                assert!(!p.split_low_note.is_unknown(), "{}", path.display());
                assert!(!p.split_mid_note.is_unknown(), "{}", path.display());
                assert!(!p.split_high_note.is_unknown(), "{}", path.display());
                assert!(!p.split_low_width.is_unknown(), "{}", path.display());
                assert!(!p.split_mid_width.is_unknown(), "{}", path.display());
                assert!(!p.split_high_width.is_unknown(), "{}", path.display());
                ns3_split_on += usize::from(p.split_enabled);
                ns3_at_default_clock += usize::from(p.master_clock.bpm() == 120);
                ns3_seen += 1;
            }
            _ => {}
        }
    }

    assert!(ns2_seen > 700, "only {ns2_seen} Stage 2 programs read");
    assert!(ns3_seen > 290, "only {ns3_seen} Stage 3 programs read");
    // A decode where no factory program ever splits is reading the wrong bits,
    // and one where the master clock is not overwhelmingly at its 120 bpm
    // default is reading the wrong bits shifted — either failure moves these.
    assert!(ns3_split_on > 0, "no ns3f decodes with a split enabled");
    assert!(
        ns3_at_default_clock * 2 > ns3_seen,
        "only {ns3_at_default_clock}/{ns3_seen} programs read the 120 bpm default"
    );
}

/// The drum banks: every member of every bank parses and the counts match the
/// devices' bank sizes.
#[cfg(feature = "bundle")]
#[test]
fn drum_banks_walk_to_their_members() {
    use nord_format::Bundle;

    let root = corpus_root();
    let mut banks = 0usize;
    for dir in ["nd2/factory/banks", "nd3p/factory/kitbanks"] {
        let dir = root.join(dir);
        for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let path = entry.unwrap().path();
            if path
                .extension()
                .is_none_or(|e| e != "nd2_bank" && e != "nd3_kitbank")
            {
                continue;
            }
            match nord_format::from_path(&path).unwrap() {
                Entity::Bundle(Bundle::Drum2Bank(b)) => assert_eq!(b.programs.len(), 50),
                Entity::Bundle(Bundle::Drum3KitBank(b)) => assert_eq!(b.kits.len(), 50),
                other => panic!("{}: decoded to {other:?}", path.display()),
            }
            banks += 1;
        }
    }
    assert_eq!(banks, 8, "the corpus ships four banks per drum model");
}
