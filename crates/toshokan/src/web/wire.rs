//! The bytes a request and its result cross the worker boundary as: one buffer
//! each way, so a message carries no object graph to clone.
//!
//! Integers are little-endian `u64`, except tags, which are one byte. A string or
//! byte string is its length, then its bytes. Decoding refuses anything a
//! well-formed encoding cannot produce, trailing bytes included.

use crate::io::{
    Capabilities, Capability, DirEntry, Io, IoError, IoResult, Kind, Lock, Meta, Range, Reply,
};
use crate::path::RelPath;
use crate::Root;

/// Bytes that are not an encoding this module produces.
#[derive(Debug, PartialEq, Eq)]
pub struct Malformed(pub &'static str);

/// `io` as bytes. [`Io::Fill`] never crosses: the driver turns it into writes.
pub fn encode_request(io: &Io) -> Result<Vec<u8>, IoError> {
    let mut out = Out::default();
    match io {
        Io::List { root, dir } => out.tag(0).root(*root).path(dir),
        Io::Stat { root, path } => out.tag(1).root(*root).path(path),
        Io::ListStat { root, dir } => out.tag(2).root(*root).path(dir),
        Io::Read { root, path, range } => out.tag(3).root(*root).path(path).range(*range),
        Io::ReadMany { root, reads } => {
            out.tag(4).root(*root).count(reads.len());
            for (path, range) in reads {
                out.path(path).range(*range);
            }
            &mut out
        }
        Io::Create { root, path, bytes } => out.tag(5).root(*root).path(path).bytes(bytes),
        Io::Append { root, path, bytes } => out.tag(6).root(*root).path(path).bytes(bytes),
        Io::Write {
            root,
            path,
            offset,
            bytes,
        } => out.tag(7).root(*root).path(path).u64(*offset).bytes(bytes),
        Io::Rename { root, from, to } => out.tag(8).root(*root).path(from).path(to),
        Io::Remove { root, path } => out.tag(9).root(*root).path(path),
        Io::RemoveDir { root, path } => out.tag(10).root(*root).path(path),
        Io::MakeDir { root, path } => out.tag(11).root(*root).path(path),
        Io::Sync { root, path } => out.tag(12).root(*root).path(path),
        Io::Lock { name } => out.tag(13).path(name),
        Io::Unlock { name } => out.tag(14).path(name),
        Io::Fill { .. } => return Err(IoError::Other(crate::disk::FILLED_BY_DRIVERS.into())),
    };
    Ok(out.0)
}

pub fn decode_request(bytes: &[u8]) -> Result<Io, Malformed> {
    let mut input = In(bytes);
    let io = match input.u8()? {
        0 => Io::List {
            root: input.root()?,
            dir: input.path()?,
        },
        1 => Io::Stat {
            root: input.root()?,
            path: input.path()?,
        },
        2 => Io::ListStat {
            root: input.root()?,
            dir: input.path()?,
        },
        3 => Io::Read {
            root: input.root()?,
            path: input.path()?,
            range: input.range()?,
        },
        4 => {
            let root = input.root()?;
            let reads = input.many(|input| Ok((input.path()?, input.range()?)))?;
            Io::ReadMany { root, reads }
        }
        5 => Io::Create {
            root: input.root()?,
            path: input.path()?,
            bytes: input.bytes()?,
        },
        6 => Io::Append {
            root: input.root()?,
            path: input.path()?,
            bytes: input.bytes()?,
        },
        7 => Io::Write {
            root: input.root()?,
            path: input.path()?,
            offset: input.u64()?,
            bytes: input.bytes()?,
        },
        8 => Io::Rename {
            root: input.root()?,
            from: input.path()?,
            to: input.path()?,
        },
        9 => Io::Remove {
            root: input.root()?,
            path: input.path()?,
        },
        10 => Io::RemoveDir {
            root: input.root()?,
            path: input.path()?,
        },
        11 => Io::MakeDir {
            root: input.root()?,
            path: input.path()?,
        },
        12 => Io::Sync {
            root: input.root()?,
            path: input.path()?,
        },
        13 => Io::Lock {
            name: input.path()?,
        },
        14 => Io::Unlock {
            name: input.path()?,
        },
        _ => return Err(Malformed("an unknown request")),
    };
    input.end()?;
    Ok(io)
}

pub fn encode_result(result: &IoResult) -> Vec<u8> {
    let mut out = Out::default();
    match result {
        Ok(reply) => out.tag(0).reply(reply),
        Err(error) => out.tag(1).error(error),
    };
    out.0
}

/// The result `bytes` encode; malformed bytes are a failed request.
pub fn decode_result(bytes: &[u8]) -> IoResult {
    let mut input = In(bytes);
    let result = match input.u8() {
        Ok(0) => input.reply().map(Ok),
        Ok(1) => input.error().map(Err),
        Ok(_) => Err(Malformed("an unknown result")),
        Err(malformed) => Err(malformed),
    };
    match result.and_then(|result| input.end().map(|()| result)) {
        Ok(result) => result,
        Err(Malformed(what)) => Err(IoError::Other(format!("the worker replied with {what}"))),
    }
}

/// [`Capabilities`] as one bit each.
pub fn capability_bits(capabilities: Capabilities) -> u8 {
    let Capabilities {
        append,
        rename_file,
        no_replace,
        rename_dir,
        fsync,
    } = capabilities;
    [append, rename_file, no_replace, rename_dir, fsync]
        .into_iter()
        .enumerate()
        .fold(0, |bits, (bit, on)| bits | u8::from(on) << bit)
}

pub fn capabilities_from_bits(bits: u8) -> Capabilities {
    let on = |bit: u8| bits >> bit & 1 == 1;
    Capabilities {
        append: on(0),
        rename_file: on(1),
        no_replace: on(2),
        rename_dir: on(3),
        fsync: on(4),
    }
}

const KINDS: [Kind; 2] = [Kind::File, Kind::Directory];
const LOCKS: [Lock; 2] = [Lock::Acquired, Lock::Held];
const CAPABILITIES: [Capability; 4] = [
    Capability::Append,
    Capability::RenameFile,
    Capability::RenameDir,
    Capability::Fsync,
];

fn index_of<T: PartialEq>(table: &[T], value: &T) -> u8 {
    let index = table.iter().position(|entry| entry == value);
    index.expect("every value is in its table") as u8
}

fn from_table<T: Copy>(table: &[T], tag: u8, what: &'static str) -> Result<T, Malformed> {
    table.get(usize::from(tag)).copied().ok_or(Malformed(what))
}

#[derive(Default)]
struct Out(Vec<u8>);

impl Out {
    fn tag(&mut self, tag: u8) -> &mut Self {
        self.0.push(tag);
        self
    }

    fn u64(&mut self, value: u64) -> &mut Self {
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }

    fn count(&mut self, count: usize) -> &mut Self {
        self.u64(count as u64)
    }

    fn bytes(&mut self, bytes: &[u8]) -> &mut Self {
        self.count(bytes.len()).0.extend_from_slice(bytes);
        self
    }

    fn text(&mut self, text: &str) -> &mut Self {
        self.bytes(text.as_bytes())
    }

    fn path(&mut self, path: &RelPath) -> &mut Self {
        self.text(path.as_str())
    }

    fn root(&mut self, root: Root) -> &mut Self {
        self.tag(match root {
            Root::Folder => 0,
            Root::Local => 1,
        })
    }

    fn range(&mut self, range: Range) -> &mut Self {
        self.u64(range.offset).u64(range.len)
    }

    fn meta(&mut self, meta: &Meta) -> &mut Self {
        self.tag(index_of(&KINDS, &meta.kind)).u64(meta.len);
        match meta.modified {
            None => self.tag(0),
            Some(modified) => self.tag(1).u64(modified),
        }
    }

    fn reply(&mut self, reply: &Reply) -> &mut Self {
        match reply {
            Reply::Listed(entries) => {
                self.tag(0).count(entries.len());
                for entry in entries {
                    self.text(&entry.name).tag(index_of(&KINDS, &entry.kind));
                }
                self
            }
            Reply::Stat(None) => self.tag(1).tag(0),
            Reply::Stat(Some(meta)) => self.tag(1).tag(1).meta(meta),
            Reply::ListedStat(entries) => {
                self.tag(2).count(entries.len());
                for (name, meta) in entries {
                    self.text(name).meta(meta);
                }
                self
            }
            Reply::Bytes(bytes) => self.tag(3).bytes(bytes),
            Reply::ReadMany(reads) => {
                self.tag(4).count(reads.len());
                for read in reads {
                    match read {
                        Ok(bytes) => self.tag(0).bytes(bytes),
                        Err(error) => self.tag(1).error(error),
                    };
                }
                self
            }
            Reply::Lock(lock) => self.tag(5).tag(index_of(&LOCKS, lock)),
            Reply::Done => self.tag(6),
        }
    }

    fn error(&mut self, error: &IoError) -> &mut Self {
        match error {
            IoError::NotFound => self.tag(0),
            IoError::AlreadyExists => self.tag(1),
            IoError::IsDirectory => self.tag(2),
            IoError::NotDirectory => self.tag(3),
            IoError::NotEmpty => self.tag(4),
            IoError::NoSpace => self.tag(5),
            IoError::IntoItself => self.tag(6),
            IoError::SpliceRange => self.tag(7),
            IoError::Unsupported(capability) => {
                self.tag(8).tag(index_of(&CAPABILITIES, capability))
            }
            IoError::Crashed => self.tag(9),
            IoError::Other(text) => self.tag(10).text(text),
        }
    }
}

struct In<'a>(&'a [u8]);

impl<'a> In<'a> {
    fn take(&mut self, len: u64) -> Result<&'a [u8], Malformed> {
        let len = usize::try_from(len).map_err(|_| Malformed("a length past memory"))?;
        if len > self.0.len() {
            return Err(Malformed("a length past its end"));
        }
        let (taken, rest) = self.0.split_at(len);
        self.0 = rest;
        Ok(taken)
    }

    fn u8(&mut self) -> Result<u8, Malformed> {
        Ok(self.take(1)?[0])
    }

    fn u64(&mut self) -> Result<u64, Malformed> {
        let bytes = self.take(8)?.try_into().expect("eight bytes taken");
        Ok(u64::from_le_bytes(bytes))
    }

    fn bytes(&mut self) -> Result<Vec<u8>, Malformed> {
        let len = self.u64()?;
        Ok(self.take(len)?.to_vec())
    }

    fn text(&mut self) -> Result<String, Malformed> {
        String::from_utf8(self.bytes()?).map_err(|_| Malformed("text that is not UTF-8"))
    }

    fn path(&mut self) -> Result<RelPath, Malformed> {
        RelPath::new(&self.text()?).map_err(|_| Malformed("an invalid path"))
    }

    fn root(&mut self) -> Result<Root, Malformed> {
        match self.u8()? {
            0 => Ok(Root::Folder),
            1 => Ok(Root::Local),
            _ => Err(Malformed("an unknown root")),
        }
    }

    fn range(&mut self) -> Result<Range, Malformed> {
        Ok(Range {
            offset: self.u64()?,
            len: self.u64()?,
        })
    }

    fn flag(&mut self) -> Result<bool, Malformed> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Malformed("a flag that is neither 0 nor 1")),
        }
    }

    fn kind(&mut self) -> Result<Kind, Malformed> {
        from_table(&KINDS, self.u8()?, "an unknown kind")
    }

    /// `count` items read by `item`; the count is checked against the bytes left,
    /// since every item takes at least one.
    fn many<T>(
        &mut self,
        mut item: impl FnMut(&mut Self) -> Result<T, Malformed>,
    ) -> Result<Vec<T>, Malformed> {
        let count = self.u64()?;
        if count > self.0.len() as u64 {
            return Err(Malformed("a count past its end"));
        }
        (0..count).map(|_| item(self)).collect()
    }

    fn meta(&mut self) -> Result<Meta, Malformed> {
        Ok(Meta {
            kind: self.kind()?,
            len: self.u64()?,
            modified: match self.flag()? {
                false => None,
                true => Some(self.u64()?),
            },
        })
    }

    fn reply(&mut self) -> Result<Reply, Malformed> {
        Ok(match self.u8()? {
            0 => Reply::Listed(self.many(|input| {
                Ok(DirEntry {
                    name: input.text()?,
                    kind: input.kind()?,
                })
            })?),
            1 => Reply::Stat(match self.flag()? {
                false => None,
                true => Some(self.meta()?),
            }),
            2 => Reply::ListedStat(self.many(|input| Ok((input.text()?, input.meta()?)))?),
            3 => Reply::Bytes(self.bytes()?),
            4 => Reply::ReadMany(self.many(|input| match input.flag()? {
                false => input.bytes().map(Ok),
                true => input.error().map(Err),
            })?),
            5 => Reply::Lock(from_table(&LOCKS, self.u8()?, "an unknown lock")?),
            6 => Reply::Done,
            _ => return Err(Malformed("an unknown reply")),
        })
    }

    fn error(&mut self) -> Result<IoError, Malformed> {
        Ok(match self.u8()? {
            0 => IoError::NotFound,
            1 => IoError::AlreadyExists,
            2 => IoError::IsDirectory,
            3 => IoError::NotDirectory,
            4 => IoError::NotEmpty,
            5 => IoError::NoSpace,
            6 => IoError::IntoItself,
            7 => IoError::SpliceRange,
            8 => IoError::Unsupported(from_table(
                &CAPABILITIES,
                self.u8()?,
                "an unknown capability",
            )?),
            9 => IoError::Crashed,
            10 => IoError::Other(self.text()?),
            _ => return Err(Malformed("an unknown error")),
        })
    }

    fn end(self) -> Result<(), Malformed> {
        match self.0.is_empty() {
            true => Ok(()),
            false => Err(Malformed("trailing bytes")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::Content;

    fn path(text: &str) -> RelPath {
        RelPath::new(text).unwrap()
    }

    fn every_request() -> Vec<Io> {
        let (root, p) = (Root::Local, path("a/b"));
        let range = Range {
            offset: u64::MAX - 1,
            len: 7,
        };
        vec![
            Io::List {
                root: Root::Folder,
                dir: RelPath::ROOT,
            },
            Io::Stat {
                root,
                path: p.clone(),
            },
            Io::ListStat {
                root,
                dir: p.clone(),
            },
            Io::Read {
                root,
                path: p.clone(),
                range,
            },
            Io::ReadMany {
                root,
                reads: vec![(p.clone(), range), (path("ü"), Range { offset: 0, len: 0 })],
            },
            Io::ReadMany {
                root,
                reads: Vec::new(),
            },
            Io::Create {
                root,
                path: p.clone(),
                bytes: b"new".to_vec(),
            },
            Io::Append {
                root,
                path: p.clone(),
                bytes: Vec::new(),
            },
            Io::Write {
                root,
                path: p.clone(),
                offset: 1 << 40,
                bytes: vec![0, 255],
            },
            Io::Rename {
                root,
                from: p.clone(),
                to: path("c"),
            },
            Io::Remove {
                root,
                path: p.clone(),
            },
            Io::RemoveDir {
                root,
                path: p.clone(),
            },
            Io::MakeDir {
                root,
                path: p.clone(),
            },
            Io::Sync {
                root,
                path: p.clone(),
            },
            Io::Lock { name: p.clone() },
            Io::Unlock { name: p },
        ]
    }

    fn every_result() -> Vec<IoResult> {
        let meta = |kind, modified| Meta {
            kind,
            len: 3,
            modified,
        };
        let mut results = vec![
            Ok(Reply::Listed(vec![
                DirEntry {
                    name: "a".into(),
                    kind: Kind::File,
                },
                DirEntry {
                    name: "d".into(),
                    kind: Kind::Directory,
                },
            ])),
            Ok(Reply::Stat(None)),
            Ok(Reply::Stat(Some(meta(Kind::File, Some(u64::MAX))))),
            Ok(Reply::ListedStat(vec![
                ("a".into(), meta(Kind::File, Some(0))),
                ("d".into(), meta(Kind::Directory, None)),
            ])),
            Ok(Reply::Bytes(b"bytes".to_vec())),
            Ok(Reply::ReadMany(vec![
                Ok(Vec::new()),
                Err(IoError::NotFound),
            ])),
            Ok(Reply::Lock(Lock::Acquired)),
            Ok(Reply::Lock(Lock::Held)),
            Ok(Reply::Done),
            Err(IoError::NotFound),
            Err(IoError::AlreadyExists),
            Err(IoError::IsDirectory),
            Err(IoError::NotDirectory),
            Err(IoError::NotEmpty),
            Err(IoError::NoSpace),
            Err(IoError::IntoItself),
            Err(IoError::SpliceRange),
            Err(IoError::Crashed),
            Err(IoError::Other("why".into())),
        ];
        results.extend(CAPABILITIES.map(|c| Err(IoError::Unsupported(c))));
        results
    }

    #[test]
    fn every_request_and_result_decodes_to_itself() {
        for io in every_request() {
            let bytes = encode_request(&io).unwrap();
            assert_eq!(decode_request(&bytes), Ok(io));
        }
        for result in every_result() {
            assert_eq!(decode_result(&encode_result(&result)), result);
        }
    }

    #[test]
    fn a_fill_does_not_cross() {
        let fill = Io::Fill {
            root: Root::Folder,
            path: path("f"),
            content: Content(0),
        };
        assert!(encode_request(&fill).is_err());
    }

    #[test]
    fn every_cut_short_or_extended_encoding_is_refused() {
        for io in every_request() {
            let bytes = encode_request(&io).unwrap();
            for end in 0..bytes.len() {
                assert!(
                    decode_request(&bytes[..end]).is_err(),
                    "{io:?} cut at {end}"
                );
            }
            let mut longer = bytes.clone();
            longer.push(0);
            assert_eq!(decode_request(&longer), Err(Malformed("trailing bytes")));
        }
        for result in every_result() {
            let bytes = encode_result(&result);
            for end in 0..bytes.len() {
                let decoded = decode_result(&bytes[..end]);
                assert!(
                    matches!(&decoded, Err(IoError::Other(why)) if why.starts_with("the worker replied")),
                    "{result:?} cut at {end}: {decoded:?}"
                );
            }
        }
    }

    #[test]
    fn a_huge_count_is_refused_before_anything_is_allocated() {
        let mut bytes = vec![4, 0];
        bytes.extend_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(
            decode_request(&bytes),
            Err(Malformed("a count past its end"))
        );
        let mut bytes = vec![5, 0];
        bytes.extend_from_slice(&u64::MAX.to_le_bytes());
        assert!(decode_request(&bytes).is_err());
    }

    #[test]
    fn invalid_paths_tags_and_text_are_refused() {
        let with_path = |text: &[u8]| {
            let mut bytes = vec![1, 0];
            bytes.extend_from_slice(&(text.len() as u64).to_le_bytes());
            bytes.extend_from_slice(text);
            bytes
        };
        assert_eq!(
            decode_request(&with_path(b"a/../b")),
            Err(Malformed("an invalid path"))
        );
        assert_eq!(
            decode_request(&with_path(&[0xff])),
            Err(Malformed("text that is not UTF-8"))
        );
        assert_eq!(decode_request(&[15]), Err(Malformed("an unknown request")));
        assert_eq!(
            decode_request(&with_path(b"a")[..1]),
            Err(Malformed("a length past its end"))
        );
        let mut bad_root = with_path(b"a");
        bad_root[1] = 2;
        assert_eq!(decode_request(&bad_root), Err(Malformed("an unknown root")));
    }

    #[test]
    fn capabilities_survive_their_bits() {
        for bits in 0..32 {
            assert_eq!(capability_bits(capabilities_from_bits(bits)), bits);
        }
        assert_eq!(capability_bits(Capabilities::ALL), 0b11111);
        assert_eq!(capabilities_from_bits(0), Capabilities::NONE);
    }
}
