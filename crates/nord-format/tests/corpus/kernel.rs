//! A `.kernel.tsv` table: the sample codec's 512-phase interpolation kernel as the
//! encoder stores it in float32, one `phase tap value class` row per point, measured
//! outside this crate. `unique` and `excluded` points must match bit for bit,
//! `ideal` ones within one ulp, and `zero` ones must be zero.

use crate::Context;
use nord_format::formats::nsmp;
use std::collections::BTreeSet;
use std::path::Path;

pub fn check(path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path).context("read")?;
    let bank = nsmp::kernel::taps();
    let mut points = BTreeSet::new();
    for (index, line) in text.lines().enumerate() {
        if line.starts_with('#') || line.starts_with("k\t") {
            continue;
        }
        let at = format!("line {}", index + 1);
        let cols: Vec<&str> = line.split('\t').collect();
        let [k, m, g, class, ..] = cols[..] else {
            return Err(format!("{at}: fewer than four columns"));
        };
        let k: usize = k.parse().context(&at)?;
        let m: i64 = m.parse().context(&at)?;
        let g: f32 = g.parse().context(&at)?;
        ensure!(k < nsmp::kernel::PHASES, "{at}: phase {k}");
        ensure!((-16..=15).contains(&m), "{at}: phase {k} tap {m}");
        ensure!(points.insert((k, m)), "{at}: duplicate phase {k} tap {m}");
        let ours = match usize::try_from(m + 15) {
            Ok(j) if j < nsmp::kernel::TAPS => bank[k][j],
            _ => 0.0,
        };
        let ulps = if ours.is_sign_negative() == g.is_sign_negative() {
            i64::from(ours.to_bits()).abs_diff(i64::from(g.to_bits()))
        } else {
            u64::MAX
        };
        let agrees = match class {
            "unique" | "excluded" => ours.to_bits() == g.to_bits(),
            "ideal" => ulps <= 1,
            "zero" => ours == 0.0,
            other => return Err(format!("{at}: class {other}")),
        };
        ensure!(
            agrees,
            "{at}: {class} phase {k} tap {m}: ours {ours}, the table's {g}"
        );
    }
    ensure!(
        points.len() == nsmp::kernel::PHASES * (nsmp::kernel::TAPS + 2),
        "{} phase and tap points",
        points.len()
    );
    Ok(())
}
