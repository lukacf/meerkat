//! Content digests, and the canonical JSON text this module digests.
//!
//! Every digest here is over bytes that are a pure function of their input:
//! object keys are written in byte order at every depth, whatever order a
//! map iterates in, so no digest depends on serde_json's map order or on a
//! feature that changes it.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

const SHA256_PREFIX: &str = "sha256:";

/// A content digest: `sha256:` and 64 lowercase hex digits, the form the
/// durable decision owner uses for subject and part digests.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sha256Digest(String);

impl Sha256Digest {
    /// The digest of `bytes`.
    pub fn of(bytes: &[u8]) -> Self {
        Self(format!("{SHA256_PREFIX}{:x}", Sha256::digest(bytes)))
    }

    /// `value` as a digest; `None` unless it is `sha256:` and 64 lowercase
    /// hex digits.
    pub fn parse(value: &str) -> Option<Self> {
        let hex = value.strip_prefix(SHA256_PREFIX)?;
        let well_formed = hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        well_formed.then(|| Self(value.to_owned()))
    }

    /// The digest's text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Sha256Digest {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value).ok_or_else(|| format!("not a sha256 digest: {value}"))
    }
}

impl From<Sha256Digest> for String {
    fn from(digest: Sha256Digest) -> Self {
        digest.0
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The canonical JSON text of `value`: object keys in byte order at every
/// depth, no insignificant whitespace, integers in decimal, any other number
/// as Rust prints the `f64` (the shortest text that reads back to it), and
/// strings with only `"`, `\` and control characters escaped.
pub(crate) fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical_json(&mut out, value);
    out
}

/// The digest of the canonical JSON text of `value`.
pub(crate) fn canonical_digest(value: &Value) -> Sha256Digest {
    Sha256Digest::of(canonical_json(value).as_bytes())
}

fn write_canonical_json(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => write_number(out, number),
        Value::String(text) => write_json_string(out, text),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical_json(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let sorted: BTreeMap<&str, &Value> =
                map.iter().map(|(key, item)| (key.as_str(), item)).collect();
            out.push('{');
            for (index, (key, item)) in sorted.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_json_string(out, key);
                out.push(':');
                write_canonical_json(out, item);
            }
            out.push('}');
        }
    }
}

/// The canonical pretty JSON text of `value`: the canonical text laid out
/// one member or element per line, two spaces per depth, with `": "` after
/// each key. Empty arrays and objects stay `[]` and `{}`. It has no trailing
/// newline.
pub(crate) fn canonical_json_pretty(value: &Value) -> String {
    let mut out = String::new();
    write_pretty(&mut out, value, 0);
    out
}

fn write_pretty(out: &mut String, value: &Value, depth: usize) {
    match value {
        Value::Array(items) if !items.is_empty() => {
            out.push_str("[\n");
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(",\n");
                }
                push_indent(out, depth + 1);
                write_pretty(out, item, depth + 1);
            }
            out.push('\n');
            push_indent(out, depth);
            out.push(']');
        }
        Value::Object(map) if !map.is_empty() => {
            let sorted: BTreeMap<&str, &Value> =
                map.iter().map(|(key, item)| (key.as_str(), item)).collect();
            out.push_str("{\n");
            for (index, (key, item)) in sorted.into_iter().enumerate() {
                if index > 0 {
                    out.push_str(",\n");
                }
                push_indent(out, depth + 1);
                write_json_string(out, key);
                out.push_str(": ");
                write_pretty(out, item, depth + 1);
            }
            out.push('\n');
            push_indent(out, depth);
            out.push('}');
        }
        other => write_canonical_json(out, other),
    }
}

fn push_indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn write_number(out: &mut String, number: &serde_json::Number) {
    if let Some(unsigned) = number.as_u64() {
        out.push_str(&unsigned.to_string());
    } else if let Some(signed) = number.as_i64() {
        out.push_str(&signed.to_string());
    } else if let Some(float) = number.as_f64() {
        out.push_str(&float.to_string());
    } else {
        out.push_str(&number.to_string());
    }
}

fn write_json_string(out: &mut String, text: &str) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            control if u32::from(control) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", u32::from(control)));
            }
            other => out.push(other),
        }
    }
    out.push('"');
}
