//! Where the private specimen corpus lives.

use std::path::PathBuf;

/// The corpus checkout: `NORD_CORPUS_ROOT`.
pub fn root() -> PathBuf {
    std::env::var_os("NORD_CORPUS_ROOT")
        .map(PathBuf::from)
        .expect("set NORD_CORPUS_ROOT to a nord-corpus checkout for --features corpus")
}

/// The Electro 5 tree — the parameter sweeps, the settings captures and the
/// USB recordings all sit under it.
pub fn ne5() -> PathBuf {
    root().join("ne5")
}
