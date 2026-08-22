//! The project file's text tree: `name {` opens a block, `}` closes it, and
//! `key = value` is a field, every line indented two spaces per depth.
//!
//! The reader is strict — one of the three line shapes at exactly its depth's
//! indent, LF line ends, a single root block, nothing after it — and the
//! writer emits exactly that, so a tree read and written is the file it came
//! from byte-for-byte. Anything the editor would not have written is refused
//! rather than normalised away.

use crate::error::ParseError;
use std::fmt::Write as _;
use std::str::FromStr;

/// Spaces per level of depth.
const INDENT: usize = 2;

/// One `name { … }` block: its fields and child blocks, in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    pub entries: Vec<Entry>,
}

/// One line inside a block: a field, or a nested block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// `key = value`. The value is everything after the first ` = `, untrimmed
    /// — `m_buffer = ` carries an empty one.
    Field {
        key: String,
        value: String,
    },
    Block(Node),
}

impl Node {
    pub fn new(name: impl Into<String>) -> Node {
        Node {
            name: name.into(),
            entries: Vec::new(),
        }
    }

    /// The first field named `key`, as written.
    pub fn field(&self, key: &str) -> Option<&str> {
        self.entries.iter().find_map(|e| match e {
            Entry::Field { key: k, value } if k == key => Some(value.as_str()),
            _ => None,
        })
    }

    /// The first field named `key`, parsed. A missing or unparseable field is
    /// an error naming the block, so a caller reading a view sees which.
    pub fn get<T: FromStr>(&self, key: &str) -> Result<T, ParseError> {
        let raw = self
            .field(key)
            .ok_or_else(|| ParseError::AssertFail(format!("{} has no {key}", self.name)))?;
        raw.parse().map_err(|_| {
            ParseError::AssertFail(format!(
                "{}.{key} = {raw:?} is not a {}",
                self.name,
                std::any::type_name::<T>()
            ))
        })
    }

    /// Overwrite the first field named `key`. Errs if there is none: a view
    /// setter must not invent fields the editor never wrote.
    pub fn set_field(&mut self, key: &str, value: impl Into<String>) -> Result<(), ParseError> {
        let slot = self
            .entries
            .iter_mut()
            .find_map(|e| match e {
                Entry::Field { key: k, value } if k == key => Some(value),
                _ => None,
            })
            .ok_or_else(|| ParseError::AssertFail(format!("{} has no {key}", self.name)))?;
        *slot = value.into();
        Ok(())
    }

    pub fn push_field(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.entries.push(Entry::Field {
            key: key.into(),
            value: value.into(),
        });
    }

    pub fn push_block(&mut self, node: Node) {
        self.entries.push(Entry::Block(node));
    }

    /// Every child block named `name`, in file order.
    pub fn blocks<'a, 'n>(&'a self, name: &'n str) -> impl Iterator<Item = &'a Node> + use<'a, 'n> {
        self.entries.iter().filter_map(move |e| match e {
            Entry::Block(n) if n.name == name => Some(n),
            _ => None,
        })
    }

    pub fn blocks_mut<'a, 'n>(
        &'a mut self,
        name: &'n str,
    ) -> impl Iterator<Item = &'a mut Node> + use<'a, 'n> {
        self.entries.iter_mut().filter_map(move |e| match e {
            Entry::Block(n) if n.name == name => Some(n),
            _ => None,
        })
    }

    /// The first child block named `name`.
    pub fn block(&self, name: &str) -> Option<&Node> {
        self.blocks(name).next()
    }

    /// Like [`Node::block`], but a missing block is an error naming the parent.
    pub fn require(&self, name: &str) -> Result<&Node, ParseError> {
        self.block(name)
            .ok_or_else(|| ParseError::AssertFail(format!("{} has no {name} block", self.name)))
    }

    /// The tree as text, the root at `depth` levels of indent.
    pub fn render(&self, out: &mut String, depth: usize) {
        let pad = " ".repeat(depth * INDENT);
        let _ = writeln!(out, "{pad}{} {{", self.name);
        for entry in &self.entries {
            match entry {
                Entry::Field { key, value } => {
                    let _ = writeln!(out, "{pad}  {key} = {value}");
                }
                Entry::Block(node) => node.render(out, depth + 1),
            }
        }
        let _ = writeln!(out, "{pad}}}");
    }
}

/// Parse one file's text: a single root block, LF-terminated.
pub fn parse(text: &str) -> Result<Node, ParseError> {
    let fail = |line_no: usize, why: &str| {
        ParseError::AssertFail(format!("project line {line_no}: {why}"))
    };
    if !text.ends_with('\n') {
        return Err(ParseError::AssertFail(
            "project does not end in a newline".into(),
        ));
    }

    let mut stack: Vec<Node> = Vec::new();
    let mut root: Option<Node> = None;
    for (i, line) in text.split_inclusive('\n').enumerate() {
        let line_no = i + 1;
        let line = line.strip_suffix('\n').unwrap_or(line);
        if line.ends_with('\r') {
            return Err(fail(line_no, "CRLF line end"));
        }
        if root.is_some() {
            return Err(fail(line_no, "text after the root block"));
        }
        let body = line.trim_start_matches(' ');
        let indent = line.len() - body.len();
        // A close brace sits at its block's own indent, one level out from
        // the block's contents.
        let want = if body == "}" {
            stack.len().saturating_sub(1)
        } else {
            stack.len()
        } * INDENT;
        if indent != want {
            return Err(fail(line_no, &format!("expected {want} spaces of indent")));
        }

        if body == "}" {
            let done = stack
                .pop()
                .ok_or_else(|| fail(line_no, "a close brace with no block open"))?;
            match stack.last_mut() {
                Some(parent) => parent.push_block(done),
                None => root = Some(done),
            }
        } else if let Some((key, value)) = body.split_once(" = ") {
            let parent = stack
                .last_mut()
                .ok_or_else(|| fail(line_no, "a field outside any block"))?;
            parent.push_field(key, value);
        } else if let Some(name) = body.strip_suffix(" {") {
            if name.is_empty() || name.contains(' ') {
                return Err(fail(line_no, "a block with no name"));
            }
            stack.push(Node::new(name));
        } else {
            return Err(fail(line_no, "not a block, a field or a close brace"));
        }
    }
    if !stack.is_empty() {
        return Err(ParseError::AssertFail(format!(
            "project ends inside {}",
            stack.last().unwrap().name
        )));
    }
    root.ok_or_else(|| ParseError::AssertFail("project holds no block".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str =
        "root {\n  a = 1\n  child {\n    b = two words\n    empty = \n  }\n  c = 3\n}\n";

    fn rendered(node: &Node) -> String {
        let mut out = String::new();
        node.render(&mut out, 0);
        out
    }

    #[test]
    fn a_tree_reads_and_writes_byte_exactly() {
        let node = parse(SAMPLE).unwrap();
        assert_eq!(node.name, "root");
        assert_eq!(node.field("a"), Some("1"));
        let child = node.block("child").unwrap();
        assert_eq!(child.field("b"), Some("two words"));
        assert_eq!(child.field("empty"), Some(""));
        assert_eq!(node.get::<u8>("c").unwrap(), 3);
        assert_eq!(rendered(&node), SAMPLE);
    }

    #[test]
    fn a_set_field_lands_in_place() {
        let mut node = parse(SAMPLE).unwrap();
        node.set_field("a", "9").unwrap();
        assert!(node.set_field("nope", "9").is_err());
        assert_eq!(rendered(&node), SAMPLE.replace("a = 1", "a = 9"));
    }

    #[test]
    fn anything_the_editor_would_not_write_is_refused() {
        for bad in [
            "root {\n  a = 1\n}",          // no trailing newline
            "root {\r\n  a = 1\r\n}\r\n",  // CRLF
            "root {\n a = 1\n}\n",         // one space of indent
            "root {\n   a = 1\n}\n",       // three
            "root {\n  a = 1\n}\nx = 1\n", // text after the root
            "root {\n  a = 1\n",           // unterminated
            "a = 1\n",                     // no block
            "root {\n  junk\n}\n",         // no shape
            "}\n",                         // stray close
            "root {\n  {\n  }\n}\n",       // nameless block
            "",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} parsed");
        }
    }

    #[test]
    fn a_bad_number_names_its_block_and_key() {
        let node = parse("root {\n  a = x\n}\n").unwrap();
        let err = node.get::<u8>("a").unwrap_err().to_string();
        assert!(err.contains("root.a"), "{err}");
        let err = node.get::<u8>("b").unwrap_err().to_string();
        assert!(err.contains("root has no b"), "{err}");
    }
}
