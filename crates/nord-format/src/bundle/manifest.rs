//! `meta.xml`, the manifest a bundle carries beside its members.
//!
//! Inferred from specimens; not confirmed on hardware. The manifest is one `<bundle>`
//! element with five attributes, holding one `<file>` element per member that depends
//! on others. Each names the member's archive path, a `depCnt`, and then `dep0`, `dep1`,
//! … as archive paths. A member with no dependencies has no element. The layout is
//! fixed down to the whitespace, and anything else is refused, so a read manifest
//! writes back byte for byte.

use crate::error::ParseError;

/// The manifest's archive path.
pub const PATH: &str = "meta.xml";

const DECLARATION: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n";

/// A bundle's manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    /// `1` on every specimen.
    pub version: u32,
    /// The instrument model code. Inferred from specimens; not confirmed on hardware:
    /// `39` on every Electro 5 bundle.
    pub product: u32,
    /// The firmware version of the instrument the bundle was made from, as `204` for
    /// v2.04.
    pub product_version: u32,
    /// Unexplained: `1` on every specimen.
    pub content_version: u32,
    /// Unexplained: `-1` on every specimen.
    pub source: i32,
    /// Each member with dependencies, in the order the manifest lists them.
    pub files: Vec<Dependencies>,
}

/// One member and the archive paths of the members it depends on, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dependencies {
    pub name: String,
    pub deps: Vec<String>,
}

impl Manifest {
    /// A manifest for an Electro 5 bundle made from firmware `product_version`, with the
    /// values every specimen holds for the rest.
    pub fn electro5(product_version: u32, files: Vec<Dependencies>) -> Manifest {
        Manifest {
            version: 1,
            product: 39,
            product_version,
            content_version: 1,
            source: -1,
            files,
        }
    }

    pub fn parse(bytes: &[u8]) -> Result<Manifest, ParseError> {
        let text = std::str::from_utf8(bytes).map_err(|_| malformed("text that is not UTF-8"))?;
        let mut rest = text
            .strip_prefix(DECLARATION)
            .ok_or_else(|| malformed("a missing XML declaration"))?;
        let attributes = element(&mut rest, "<bundle", ">\n")?;
        let [version, product, product_version, content_version, source] = attributes
            .try_into()
            .map_err(|_| malformed("a bundle element without five attributes"))?;
        let mut files = Vec::new();
        while rest.starts_with("  <file") {
            files.push(file(element(&mut rest, "  <file", "/>\n")?)?);
        }
        if rest != "</bundle>\n" {
            return Err(malformed("text after the last file element"));
        }
        Ok(Manifest {
            version: number(version, "version")?,
            product: number(product, "product")?,
            product_version: number(product_version, "product_version")?,
            content_version: number(content_version, "content_version")?,
            source: number(source, "source")?,
            files,
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = String::from(DECLARATION);
        out += &format!(
            "<bundle version=\"{}\" product=\"{}\" product_version=\"{}\" content_version=\"{}\" source=\"{}\">\n",
            self.version, self.product, self.product_version, self.content_version, self.source
        );
        for file in &self.files {
            out += &format!(
                "  <file name=\"{}\" depCnt=\"{}\"",
                escape(&file.name),
                file.deps.len()
            );
            for (i, dep) in file.deps.iter().enumerate() {
                out += &format!(" dep{i}=\"{}\"", escape(dep));
            }
            out += "/>\n";
        }
        out += "</bundle>\n";
        out.into_bytes()
    }

    /// The members `name` depends on, or none when the manifest lists it without any.
    pub fn deps(&self, name: &str) -> &[String] {
        self.files
            .iter()
            .find(|file| file.name == name)
            .map_or(&[], |file| &file.deps)
    }
}

fn malformed(what: &str) -> ParseError {
    ParseError::AssertFail(format!("meta.xml: {what}"))
}

/// The `(name, value)` attributes of the element `rest` starts with, after `open`, up to
/// `close`. Each attribute is one space, a name, `="`, a value and `"`.
fn element<'a>(
    rest: &mut &'a str,
    open: &str,
    close: &str,
) -> Result<Vec<(&'a str, String)>, ParseError> {
    let mut text = rest
        .strip_prefix(open)
        .ok_or_else(|| malformed("an unexpected element"))?;
    let mut attributes = Vec::new();
    while let Some(after) = text.strip_prefix(' ') {
        let (name, after) = after
            .split_once("=\"")
            .ok_or_else(|| malformed("an attribute without a quoted value"))?;
        let (value, after) = after
            .split_once('"')
            .ok_or_else(|| malformed("an unterminated attribute"))?;
        attributes.push((name, unescape(value)?));
        text = after;
    }
    *rest = text
        .strip_prefix(close)
        .ok_or_else(|| malformed("an element not closed where expected"))?;
    Ok(attributes)
}

fn number<T: std::str::FromStr + ToString>(
    (name, value): (&str, String),
    expected: &str,
) -> Result<T, ParseError> {
    if name != expected {
        return Err(malformed(&format!(
            "attribute {name} where {expected} belongs"
        )));
    }
    let parsed: T = value
        .parse()
        .map_err(|_| malformed(&format!("{name}=\"{value}\" is not a number")))?;
    // A number written another way, such as `01`, would not write back the same.
    if parsed.to_string() != value {
        return Err(malformed(&format!(
            "{name}=\"{value}\" is not in its plain form"
        )));
    }
    Ok(parsed)
}

fn file(attributes: Vec<(&str, String)>) -> Result<Dependencies, ParseError> {
    let mut attributes = attributes.into_iter();
    let mut take = |expected: &str| match attributes.next() {
        Some((name, value)) if name == expected => Ok(value),
        _ => Err(malformed(&format!(
            "a file element without {expected} in place"
        ))),
    };
    let name = take("name")?;
    let count = take("depCnt")?;
    let count: usize = count
        .parse()
        .ok()
        .filter(|n: &usize| n.to_string() == count)
        .ok_or_else(|| malformed(&format!("depCnt=\"{count}\"")))?;
    let deps = (0..count)
        .map(|i| take(&format!("dep{i}")))
        .collect::<Result<Vec<_>, _>>()?;
    if attributes.next().is_some() {
        return Err(malformed(&format!(
            "{name} has more dependencies than its depCnt"
        )));
    }
    Ok(Dependencies { name, deps })
}

/// The characters an attribute value escapes. Inferred from specimens' writer, which
/// leaves `'` as it is; not confirmed against a name that holds one.
const ENTITIES: [(char, &str); 4] = [
    ('&', "&amp;"),
    ('<', "&lt;"),
    ('>', "&gt;"),
    ('"', "&quot;"),
];

fn escape(value: &str) -> String {
    value.chars().fold(String::new(), |mut out, c| {
        match ENTITIES.iter().find(|(raw, _)| *raw == c) {
            Some((_, entity)) => out += entity,
            None => out.push(c),
        }
        out
    })
}

/// The value an attribute's text holds. Only text [`escape`] writes is accepted, so the
/// value escapes back to the same text.
fn unescape(text: &str) -> Result<String, ParseError> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        match ENTITIES.iter().find(|(_, entity)| rest.starts_with(entity)) {
            Some((raw, entity)) => {
                out.push(*raw);
                rest = &rest[entity.len()..];
            }
            None if ENTITIES.iter().any(|(raw, _)| *raw == c) => {
                return Err(malformed(&format!("an unescaped {c} in an attribute")));
            }
            None => {
                out.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    Ok(out)
}
