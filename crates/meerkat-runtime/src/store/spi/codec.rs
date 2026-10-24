//! Durable formats of the host store journal: scope names, heads, rows and
//! chunks. Every format carries a version and a load refuses an unknown one.
//!
//! - A **journaled scope** (a runtime, a mob, an identity, a realm record)
//!   has a JSON head (`ScopeHeadV1`): its family, its full logical key,
//!   its pinned entries (small values every boot needs, inline or by chunk),
//!   its row window and its byte counters. Its rows (`ROW_MAGIC`) carry the
//!   other entries as keyed changes, folded in order.
//! - A **chunk scope** (record kind, family `CHUNK_TAG`) carries one value
//!   too large for a row or a batch, as rows of at most [`MAX_ROW_BYTES`],
//!   under a JSON head (`ChunkHeadV1`) naming its owner scope, its length
//!   and its SHA-256. It is written, over as many batches as it needs, before
//!   the batch that references it; it is complete only once its head says so.
//!
//! Scope names are `<tag>:<key part>~<incarnation>`: the tag names the
//! family, the key part is `r` and the key itself when the key is short and
//! plain, or `h` and the key's SHA-256 otherwise, and the incarnation is
//! unique to the creating writer and batch, so a deleted name is never
//! created again. The head keeps the full key, and a load refuses a head
//! whose name does not encode it.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use meerkat_core::host_store::{MAX_ROW_BYTES, Row, ScopeName};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::SpiError;

/// The format version of every head this module writes.
pub(crate) const FORMAT_V1: u32 = 1;

/// The family tag of chunk scopes.
pub(crate) const CHUNK_TAG: &str = "ch";

/// The longest key kept verbatim in a scope name; longer keys are hashed.
const MAX_PLAIN_KEY_BYTES: usize = 200;

/// The magic and format of a journal row.
pub(crate) const ROW_MAGIC: &[u8; 4] = b"MKJ1";

const OP_DELETE: u8 = 0;
const OP_PUT: u8 = 1;
const OP_PUT_CHUNK: u8 = 2;
const OP_RESET: u8 = 3;
const OP_SPILL: u8 = 4;

/// Bytes a row spends before its entries.
pub(crate) const ROW_HEADER_BYTES: usize = ROW_MAGIC.len() + 4;

pub(crate) fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

pub(crate) fn unhex(text: &str) -> Result<Vec<u8>, SpiError> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return Err(SpiError::Corrupt("malformed hex".into()));
    }
    (0..text.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&text[index..index + 2], 16)
                .map_err(|_| SpiError::Corrupt("malformed hex".into()))
        })
        .collect()
}

pub(crate) fn unhex32(text: &str) -> Result<[u8; 32], SpiError> {
    unhex(text)?
        .try_into()
        .map_err(|_| SpiError::Corrupt("a digest is not 32 bytes".into()))
}

/// The key part of a scope name: injective over keys (`r` keeps a plain key,
/// `h` hashes any other), and bounded.
pub(crate) fn key_part(key: &str) -> String {
    if !key.is_empty() && key.len() <= MAX_PLAIN_KEY_BYTES && !key.chars().any(char::is_control) {
        format!("r{key}")
    } else {
        format!("h{}", hex(&sha256(key.as_bytes())))
    }
}

/// The name of one incarnation of a family's scope for `key`.
pub(crate) fn scope_name(tag: &str, key: &str, incarnation: &str) -> Result<ScopeName, SpiError> {
    ScopeName::try_new(format!("{tag}:{}~{incarnation}", key_part(key))).map_err(SpiError::Host)
}

/// Whether `name` is an incarnation of the family's scope for `key`.
pub(crate) fn name_encodes(name: &ScopeName, tag: &str, key: &str) -> bool {
    let prefix = format!("{tag}:{}~", key_part(key));
    name.as_str()
        .strip_prefix(&prefix)
        .is_some_and(|incarnation| !incarnation.is_empty() && !incarnation.contains('~'))
}

/// The family tag a scope name starts with.
pub(crate) fn name_tag(name: &ScopeName) -> Option<&str> {
    name.as_str().split_once(':').map(|(tag, _)| tag)
}

/// A reference to a chunk scope holding one value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChunkRef {
    pub name: ScopeName,
    pub len: u64,
    pub sha256: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChunkRefV1 {
    pub n: String,
    pub len: u64,
    pub sha: String,
}

impl ChunkRef {
    pub(crate) fn to_wire(&self) -> ChunkRefV1 {
        ChunkRefV1 {
            n: self.name.as_str().to_string(),
            len: self.len,
            sha: hex(&self.sha256),
        }
    }

    pub(crate) fn from_wire(wire: &ChunkRefV1) -> Result<Self, SpiError> {
        Ok(Self {
            name: ScopeName::try_new(wire.n.clone()).map_err(SpiError::Host)?,
            len: wire.len,
            sha256: unhex32(&wire.sha)?,
        })
    }
}

/// A pinned entry in a head: inline text, inline hex, or a chunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) enum PinnedV1 {
    #[serde(rename = "u")]
    Utf8(String),
    #[serde(rename = "x")]
    Hex(String),
    #[serde(rename = "c")]
    Chunk(ChunkRefV1),
}

/// The row window of a journaled scope: visible rows `s..e`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WindowV1 {
    pub s: u64,
    pub e: u64,
}

/// The head of a journaled scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScopeHeadV1 {
    /// Format version, [`FORMAT_V1`].
    pub f: u32,
    /// Family tag.
    pub fam: String,
    /// The full logical key.
    pub key: String,
    /// Pinned entries, by entry key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pin: BTreeMap<String, PinnedV1>,
    /// Visible rows.
    pub w: WindowV1,
    /// Encoded bytes of the live row entries.
    pub lb: u64,
    /// Row bytes appended since the last checkpoint.
    pub sb: u64,
}

impl ScopeHeadV1 {
    pub(crate) fn encode(&self) -> Result<Vec<u8>, SpiError> {
        serde_json::to_vec(self).map_err(|error| SpiError::Corrupt(error.to_string()))
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, SpiError> {
        // Peek at the version first, so a newer format refuses typed rather
        // than failing on an unknown field.
        #[derive(Deserialize)]
        struct Version {
            f: u32,
        }
        let version: Version = serde_json::from_slice(bytes)
            .map_err(|error| SpiError::Corrupt(format!("scope head: {error}")))?;
        if version.f != FORMAT_V1 {
            return Err(SpiError::Corrupt(format!(
                "scope head format {} is not supported (this build reads {FORMAT_V1})",
                version.f
            )));
        }
        serde_json::from_slice(bytes)
            .map_err(|error| SpiError::Corrupt(format!("scope head: {error}")))
    }
}

/// The head of a chunk scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChunkHeadV1 {
    pub f: u32,
    /// The name of the scope that references the chunk.
    pub owner: String,
    pub len: u64,
    pub sha: String,
    /// Rows the complete chunk has.
    pub n: u64,
    /// Whether every row is written.
    pub done: bool,
}

impl ChunkHeadV1 {
    pub(crate) fn encode(&self) -> Result<Vec<u8>, SpiError> {
        serde_json::to_vec(self).map_err(|error| SpiError::Corrupt(error.to_string()))
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, SpiError> {
        #[derive(Deserialize)]
        struct Version {
            f: u32,
        }
        let version: Version = serde_json::from_slice(bytes)
            .map_err(|error| SpiError::Corrupt(format!("chunk head: {error}")))?;
        if version.f != FORMAT_V1 {
            return Err(SpiError::Corrupt(format!(
                "chunk head format {} is not supported (this build reads {FORMAT_V1})",
                version.f
            )));
        }
        serde_json::from_slice(bytes)
            .map_err(|error| SpiError::Corrupt(format!("chunk head: {error}")))
    }
}

/// One operation of a journal row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RowOp {
    /// The entry no longer exists.
    Delete(String),
    /// The entry's value, inline.
    Put(String, Arc<Vec<u8>>),
    /// The entry's value, in a chunk.
    PutChunk(String, ChunkRef),
    /// A checkpoint starts: every earlier row entry is gone.
    Reset,
    /// The rows inside a chunk apply here, in order.
    Spill(ChunkRef),
}

impl RowOp {
    fn encoded_len(&self) -> usize {
        match self {
            Self::Delete(key) => 1 + 4 + key.len(),
            Self::Put(key, value) => 1 + 4 + key.len() + 4 + value.len(),
            Self::PutChunk(key, chunk) => 1 + 4 + key.len() + chunk_ref_len(chunk),
            Self::Reset => 1,
            Self::Spill(chunk) => 1 + chunk_ref_len(chunk),
        }
    }

    fn encode_into(&self, out: &mut Vec<u8>) -> Result<(), SpiError> {
        match self {
            Self::Delete(key) => {
                out.push(OP_DELETE);
                put_str(out, key)?;
            }
            Self::Put(key, value) => {
                out.push(OP_PUT);
                put_str(out, key)?;
                let len = u32::try_from(value.len())
                    .map_err(|_| SpiError::Misuse("an inline entry is too long".into()))?;
                out.extend_from_slice(&len.to_le_bytes());
                out.extend_from_slice(value);
            }
            Self::PutChunk(key, chunk) => {
                out.push(OP_PUT_CHUNK);
                put_str(out, key)?;
                put_chunk_ref(out, chunk)?;
            }
            Self::Reset => out.push(OP_RESET),
            Self::Spill(chunk) => {
                out.push(OP_SPILL);
                put_chunk_ref(out, chunk)?;
            }
        }
        Ok(())
    }
}

fn chunk_ref_len(chunk: &ChunkRef) -> usize {
    2 + chunk.name.as_str().len() + 8 + 32
}

fn put_str(out: &mut Vec<u8>, text: &str) -> Result<(), SpiError> {
    let len = u32::try_from(text.len())
        .map_err(|_| SpiError::Misuse("an entry key is too long".into()))?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(text.as_bytes());
    Ok(())
}

fn put_chunk_ref(out: &mut Vec<u8>, chunk: &ChunkRef) -> Result<(), SpiError> {
    let name = chunk.name.as_str();
    let len = u16::try_from(name.len())
        .map_err(|_| SpiError::Misuse("a chunk name is too long".into()))?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(&chunk.len.to_le_bytes());
    out.extend_from_slice(&chunk.sha256);
    Ok(())
}

/// The most bytes one operation may take, so that it fits one row.
pub(crate) const MAX_OP_BYTES: usize = MAX_ROW_BYTES - ROW_HEADER_BYTES;

/// Encode operations into rows of at most [`MAX_ROW_BYTES`], in order.
///
/// # Errors
/// [`SpiError::Misuse`] when one operation alone does not fit a row; the
/// planner keeps every inline value far below that.
pub(crate) fn encode_rows(ops: &[RowOp]) -> Result<Vec<Row>, SpiError> {
    let mut rows = Vec::new();
    let mut body: Vec<u8> = Vec::new();
    let mut count: u32 = 0;
    let flush =
        |body: &mut Vec<u8>, count: &mut u32, rows: &mut Vec<Row>| -> Result<(), SpiError> {
            if *count == 0 {
                return Ok(());
            }
            let mut row = Vec::with_capacity(ROW_HEADER_BYTES + body.len());
            row.extend_from_slice(ROW_MAGIC);
            row.extend_from_slice(&count.to_le_bytes());
            row.extend_from_slice(body);
            rows.push(Row::try_new(row).map_err(SpiError::Host)?);
            body.clear();
            *count = 0;
            Ok(())
        };
    for op in ops {
        let len = op.encoded_len();
        if len > MAX_OP_BYTES {
            return Err(SpiError::Misuse(format!(
                "a journal operation of {len} bytes does not fit one row"
            )));
        }
        if ROW_HEADER_BYTES + body.len() + len > MAX_ROW_BYTES {
            flush(&mut body, &mut count, &mut rows)?;
        }
        op.encode_into(&mut body)?;
        count += 1;
    }
    flush(&mut body, &mut count, &mut rows)?;
    Ok(rows)
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], SpiError> {
        if self.bytes.len() < n {
            return Err(SpiError::Corrupt("a journal row ends early".into()));
        }
        let (head, rest) = self.bytes.split_at(n);
        self.bytes = rest;
        Ok(head)
    }

    fn u16(&mut self) -> Result<u16, SpiError> {
        let raw = self.take(2)?;
        Ok(u16::from_le_bytes([raw[0], raw[1]]))
    }

    fn u32(&mut self) -> Result<u32, SpiError> {
        let raw = self.take(4)?;
        Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
    }

    fn u64(&mut self) -> Result<u64, SpiError> {
        let raw = self.take(8)?;
        let mut array = [0_u8; 8];
        array.copy_from_slice(raw);
        Ok(u64::from_le_bytes(array))
    }

    fn string(&mut self) -> Result<String, SpiError> {
        let len = self.u32()? as usize;
        String::from_utf8(self.take(len)?.to_vec())
            .map_err(|_| SpiError::Corrupt("a journal key is not UTF-8".into()))
    }

    fn chunk_ref(&mut self) -> Result<ChunkRef, SpiError> {
        let name_len = self.u16()? as usize;
        let name = String::from_utf8(self.take(name_len)?.to_vec())
            .map_err(|_| SpiError::Corrupt("a chunk name is not UTF-8".into()))?;
        let len = self.u64()?;
        let mut sha256 = [0_u8; 32];
        sha256.copy_from_slice(self.take(32)?);
        Ok(ChunkRef {
            name: ScopeName::try_new(name).map_err(SpiError::Host)?,
            len,
            sha256,
        })
    }
}

/// Decode one journal row.
pub(crate) fn decode_row(row: &[u8]) -> Result<Vec<RowOp>, SpiError> {
    let mut reader = Reader { bytes: row };
    if reader.take(ROW_MAGIC.len())? != ROW_MAGIC {
        return Err(SpiError::Corrupt(
            "a journal row has an unknown format".into(),
        ));
    }
    let count = reader.u32()?;
    let mut ops = Vec::with_capacity(count.min(4096) as usize);
    for _ in 0..count {
        let op = reader.take(1)?[0];
        ops.push(match op {
            OP_DELETE => RowOp::Delete(reader.string()?),
            OP_PUT => {
                let key = reader.string()?;
                let len = reader.u32()? as usize;
                RowOp::Put(key, Arc::new(reader.take(len)?.to_vec()))
            }
            OP_PUT_CHUNK => {
                let key = reader.string()?;
                RowOp::PutChunk(key, reader.chunk_ref()?)
            }
            OP_RESET => RowOp::Reset,
            OP_SPILL => RowOp::Spill(reader.chunk_ref()?),
            other => {
                return Err(SpiError::Corrupt(format!(
                    "a journal row has an unknown operation {other}"
                )));
            }
        });
    }
    if !reader.bytes.is_empty() {
        return Err(SpiError::Corrupt("a journal row has trailing bytes".into()));
    }
    Ok(ops)
}

/// Pack rows into the content of a spill chunk: each row's length, then
/// the row.
pub(crate) fn pack_rows(rows: &[Row]) -> Vec<u8> {
    let total = rows.iter().map(|row| 4 + row.len()).sum();
    let mut out = Vec::with_capacity(total);
    for row in rows {
        // A row is at most MAX_ROW_BYTES, far below u32::MAX.
        out.extend_from_slice(&(row.len() as u32).to_le_bytes());
        out.extend_from_slice(row.as_bytes());
    }
    out
}

/// Unpack the rows of a spill chunk.
pub(crate) fn unpack_rows(bytes: &[u8]) -> Result<Vec<Vec<u8>>, SpiError> {
    let mut reader = Reader { bytes };
    let mut rows = Vec::new();
    while !reader.bytes.is_empty() {
        let len = reader.u32()? as usize;
        if len > MAX_ROW_BYTES {
            return Err(SpiError::Corrupt("a spilled row is too long".into()));
        }
        rows.push(reader.take(len)?.to_vec());
    }
    Ok(rows)
}

/// Encode a pinned value for a head.
pub(crate) fn pin_inline(value: &[u8]) -> PinnedV1 {
    match std::str::from_utf8(value) {
        Ok(text) => PinnedV1::Utf8(text.to_string()),
        Err(_) => PinnedV1::Hex(hex(value)),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    // Unrun until the candidate's validation.
    #[test]
    fn rows_round_trip_every_operation_and_split_at_the_row_limit() {
        let chunk = ChunkRef {
            name: ScopeName::try_new("ch:rx~w.1").expect("name"),
            len: 7,
            sha256: [9; 32],
        };
        let big = Arc::new(vec![1_u8; MAX_ROW_BYTES / 2]);
        let ops = vec![
            RowOp::Reset,
            RowOp::Put("a".into(), Arc::new(b"one".to_vec())),
            RowOp::Put("b".into(), Arc::clone(&big)),
            RowOp::Put("c".into(), Arc::clone(&big)),
            RowOp::PutChunk("d".into(), chunk.clone()),
            RowOp::Delete("a".into()),
            RowOp::Spill(chunk),
        ];
        let rows = encode_rows(&ops).expect("encode");
        assert!(rows.len() >= 2, "two half-row values cannot share one row");
        assert!(rows.iter().all(|row| row.len() <= MAX_ROW_BYTES));
        let decoded: Vec<RowOp> = rows
            .iter()
            .flat_map(|row| decode_row(row.as_bytes()).expect("decode"))
            .collect();
        assert_eq!(decoded, ops);
        let packed = pack_rows(&rows);
        let unpacked = unpack_rows(&packed).expect("unpack");
        assert_eq!(unpacked.len(), rows.len());
    }

    // Unrun until the candidate's validation.
    #[test]
    fn scope_names_are_injective_bounded_and_verified() {
        let long = "k".repeat(1000);
        let plain = scope_name("rt", "abc", "w.1").expect("plain");
        let hashed = scope_name("rt", &long, "w.2").expect("hashed");
        assert!(name_encodes(&plain, "rt", "abc"));
        assert!(!name_encodes(&plain, "rt", "abd"));
        assert!(!name_encodes(&plain, "ck", "abc"));
        assert!(name_encodes(&hashed, "rt", &long));
        assert!(hashed.as_str().len() < 200);
        // A plain key that looks like a hash never meets a hashed key.
        let lookalike = format!("h{}", hex(&sha256(long.as_bytes())));
        assert_ne!(key_part(&lookalike), key_part(&long));
    }

    // Unrun until the candidate's validation.
    #[test]
    fn heads_refuse_an_unknown_format() {
        let head = ScopeHeadV1 {
            f: FORMAT_V1,
            fam: "rt".into(),
            key: "k".into(),
            pin: BTreeMap::new(),
            w: WindowV1::default(),
            lb: 0,
            sb: 0,
        };
        let bytes = head.encode().expect("encode");
        assert_eq!(ScopeHeadV1::decode(&bytes).expect("decode"), head);
        let newer = br#"{"f":2,"fam":"rt","key":"k","w":{"s":0,"e":0},"lb":0,"sb":0,"x":1}"#;
        assert!(matches!(
            ScopeHeadV1::decode(newer),
            Err(SpiError::Corrupt(_))
        ));
    }
}
