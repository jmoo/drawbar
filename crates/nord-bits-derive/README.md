# nord-bits-derive

The proc-macro behind [`nord-format`](https://crates.io/crates/nord-format):
`#[bitbody(LEN)]` declares a bit-mapped binary structure once — leaf values at
bit ranges, nested bodies at byte ranges — and generates the byte-array
conversions both ways, preserving unclaimed bits verbatim through a re-encode.

**Don't depend on this crate directly.** The generated code names `nord-format`
internals (`crate::bits`, `crate::cbin`, …), so the macro only expands correctly
inside that crate; it is published only because crates.io requires it. Depend on
`nord-format`, which pins the exact matching version.
