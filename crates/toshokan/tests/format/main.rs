//! toshokan's on-disk format as a contract. The fixture states under
//! `tests/fixtures` were written by an earlier build; every later build must read
//! them as their oracles say, and write what this build writes.

mod fixture;
mod generate;
mod oracle;
mod read;
mod stable;
