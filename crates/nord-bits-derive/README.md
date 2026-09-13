# nord-bits-derive

The proc-macro behind [`nord-format`](https://crates.io/crates/nord-format):
`#[bitbody(LEN)]` declares a bit-mapped binary structure once, leaf values at
bit ranges and nested bodies at byte ranges, and generates the byte-array
conversions both ways, preserving unclaimed bits verbatim through a re-encode.

**Do not depend on this crate directly.** The generated code names `nord-format`
internals (`crate::bits`, `crate::cbin`, …), so the macro only expands correctly
inside that crate; it is published only because crates.io requires it. Depend on
`nord-format`, which pins the exact matching version.

## Disclaimer

Not affiliated with, authorized, or endorsed by Clavia DMI AB. "Nord", "Clavia",
and "Electro" are trademarks of Clavia DMI AB, used here only to identify the
hardware these formats come from. All reverse engineering is of files produced by
Nord hardware, for interoperability.
