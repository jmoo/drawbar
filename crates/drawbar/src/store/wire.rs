//! The messages between the page and the worker that keeps a library in the browser: a
//! command, or the answers to one, as RON text, with the files and the larger byte
//! buffers it carries handed over beside the text rather than written into it.

use std::cell::RefCell;

use serde::de::Error as _;
use serde::ser::Error as _;
use serde::{Deserialize, Deserializer, Serializer};

use super::Outside;

/// What a message hands over beside its text.
#[derive(Debug)]
pub enum Attachment {
    Bytes(Vec<u8>),
    File(Outside),
}

/// One message: its text, which names each attachment by its place in `attached`.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Debug)]
pub struct Message {
    pub text: String,
    pub attached: Vec<Attachment>,
}

thread_local! {
    /// What the message being encoded hands over so far.
    static ATTACHING: RefCell<Option<Vec<Attachment>>> = const { RefCell::new(None) };
    /// What the message being decoded handed over, each taken as its text names it.
    static TAKING: RefCell<Option<Vec<Option<Attachment>>>> = const { RefCell::new(None) };
}

/// `value` as a message.
#[cfg(any(target_arch = "wasm32", test))]
pub fn encode<T: serde::Serialize>(value: &T) -> Result<Message, String> {
    ATTACHING.with(|held| *held.borrow_mut() = Some(Vec::new()));
    let text = ron::to_string(value);
    let attached = ATTACHING.with(|held| held.borrow_mut().take().unwrap_or_default());
    Ok(Message {
        text: text.map_err(|e| e.to_string())?,
        attached,
    })
}

/// The value `message` holds.
#[cfg(any(target_arch = "wasm32", test))]
pub fn decode<T: serde::de::DeserializeOwned>(message: Message) -> Result<T, String> {
    let attached = message.attached.into_iter().map(Some).collect();
    TAKING.with(|held| *held.borrow_mut() = Some(attached));
    let value = ron::from_str(&message.text);
    TAKING.with(|held| held.borrow_mut().take());
    value.map_err(|e| e.to_string())
}

fn attach<S: Serializer>(attachment: Attachment, serializer: S) -> Result<S::Ok, S::Error> {
    let at = ATTACHING.with(|held| {
        let mut held = held.borrow_mut();
        let held = held.as_mut()?;
        held.push(attachment);
        Some(held.len() - 1)
    });
    let at = at.ok_or_else(|| S::Error::custom("an attachment outside a message"))?;
    serializer.serialize_u64(at as u64)
}

fn take<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Attachment, D::Error> {
    let at = usize::deserialize(deserializer)?;
    let taken = TAKING.with(|held| {
        let mut held = held.borrow_mut();
        held.as_mut()?.get_mut(at)?.take()
    });
    taken.ok_or_else(|| D::Error::custom(format!("no attachment {at}")))
}

/// Bytes handed over beside a message's text, for `#[serde(with)]`.
pub mod bytes {
    use super::*;

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        attach(Attachment::Bytes(bytes.to_vec()), serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        match take(deserializer)? {
            Attachment::Bytes(bytes) => Ok(bytes),
            Attachment::File(_) => Err(D::Error::custom("a file where bytes were named")),
        }
    }
}

/// A file outside the library handed over beside a message's text, for
/// `#[serde(with)]`.
pub mod outside {
    use super::*;

    pub fn serialize<S: Serializer>(from: &Outside, serializer: S) -> Result<S::Ok, S::Error> {
        attach(Attachment::File(from.clone()), serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Outside, D::Error> {
        match take(deserializer)? {
            Attachment::File(from) => Ok(from),
            Attachment::Bytes(_) => Err(D::Error::custom("bytes where a file was named")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Cmd, Intent, LibPath, Part, Source};
    use super::*;

    fn path(text: &str) -> LibPath {
        LibPath::parse(text).unwrap()
    }

    /// A save's bytes and a copy's file go beside the text, and come back where the text
    /// names them.
    #[test]
    fn bytes_and_files_go_beside_the_text_and_come_back_in_place() {
        let outside = Outside::from("/music/Grand.npno");
        let cmd = Cmd::Intent(Intent::Batch(vec![
            Intent::Save {
                id: 1,
                path: path("a.ne5p"),
                bytes: b"saved bytes".to_vec(),
                expect: toshokan::Expect::Absent,
                came: None,
            },
            Intent::Import {
                id: 2,
                path: path("Grand.npno"),
                from: Source::Part(Part {
                    from: outside.clone(),
                    bytes: 4..9,
                    crc32: 7,
                }),
                expect: toshokan::Expect::Absent,
                came: None,
            },
        ]));
        let message = encode(&cmd).unwrap();
        assert!(!message.text.contains("saved bytes"), "{}", message.text);
        assert_eq!(message.attached.len(), 2, "{:?}", message.attached);
        let back: Cmd = decode(message).unwrap();
        let Cmd::Intent(Intent::Batch(parts)) = back else {
            panic!("{back:?}")
        };
        let [Intent::Save { bytes, .. }, Intent::Import { from, .. }] = &parts[..] else {
            panic!("{parts:?}")
        };
        assert_eq!(bytes, b"saved bytes");
        let Source::Part(part) = from else {
            panic!("{from:?}")
        };
        assert_eq!((&part.from, part.bytes.clone()), (&outside, 4..9));
    }

    /// A message that names an attachment it does not carry does not decode.
    #[test]
    fn a_message_naming_a_missing_attachment_does_not_decode() {
        let mut message = encode(&Cmd::Intent(Intent::Save {
            id: 1,
            path: path("a.ne5p"),
            bytes: b"bytes".to_vec(),
            expect: toshokan::Expect::Absent,
            came: None,
        }))
        .unwrap();
        message.attached.clear();
        assert!(decode::<Cmd>(message).is_err());
    }
}
