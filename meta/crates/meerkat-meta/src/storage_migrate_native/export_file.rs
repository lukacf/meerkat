//! The reader of a continuity export file, format version 1: the projection
//! side of `meta_gateway storage-migrate-native --continuity-export`.
//!
//! The standalone export scripts in `tools/continuity-export` write the file
//! from a 0.9.3 host's continuity provider and verify it. This reader repeats
//! their checks in one streaming pass, so the migration never trusts a file
//! the scripts would refuse:
//!
//! - line 1 is the header, at most 65,536 bytes: exactly `format`, `version`
//!   (1), `writer`, `entries` (at least 1), `entries_length` (the bytes after
//!   the header line) and `entries_sha256` (their lowercase SHA-256);
//! - then one entry per line, at most [`MAX_ENTRY_LINE_BYTES`] bytes before
//!   its newline, in strictly ascending code point order of identity, each
//!   identity once and each session bound by at most one entry;
//! - a ready entry carries its record (the identity, the 0.9.3
//!   `agent_runtime_id`, the canonical session id, `generation`,
//!   `checkpoint_version`) and its session document (`length`, `sha256`,
//!   canonical padded `base64`); an uninitialized entry carries only its
//!   identity;
//! - every line is in the writers' compact form, with their key order.
//!
//! Every line is read, hashed and counted even after an entry fails its
//! checks, so a file the header no longer describes reports that damage
//! first, as the scripts do. A file that fails any check refuses whole.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;

use base64::Engine as _;
use meerkat_core::types::SessionId;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// The header's `format`.
pub(crate) const CONTINUITY_EXPORT_FORMAT: &str = "meerkat-meta.continuity-export";

/// The one format version this build reads.
pub(crate) const CONTINUITY_EXPORT_VERSION: u64 = 1;

/// The longest entry line, without its newline, either export script writes
/// or reads.
pub(crate) const MAX_ENTRY_LINE_BYTES: usize = 536_870_888;

/// The longest header line, without its newline.
const MAX_HEADER_LINE_BYTES: usize = 65_536;

const READY: &str = "ready";
const UNINITIALIZED: &str = "uninitialized";

/// Why a file is not a valid continuity export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExportFileError {
    pub(crate) reason: String,
}

impl ExportFileError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl std::fmt::Display for ExportFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason)
    }
}

/// A verified export file, without the session documents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExportFile {
    pub(crate) writer: String,
    /// The header's digest of the entry lines, which the apply journal
    /// records so a rerun can tell a different file.
    pub(crate) entries_sha256: String,
    pub(crate) entries: Vec<ExportEntry>,
}

/// One entry of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExportEntry {
    pub(crate) identity: String,
    /// The record of a ready entry; `None` for an uninitialized one.
    pub(crate) record: Option<ExportRecord>,
}

/// The 0.9.3 record of a ready entry and the digest of its document. The
/// 0.9.3 values (`agent_runtime_id`, `generation`, `checkpoint_version`)
/// decide nothing; reports print them for the operator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExportRecord {
    pub(crate) agent_runtime_id: String,
    pub(crate) session_id: SessionId,
    pub(crate) generation: u64,
    pub(crate) checkpoint_version: u64,
    pub(crate) document_length: u64,
    pub(crate) document_sha256: String,
}

impl ExportRecord {
    /// The record's 0.9.3 values, as every report detail ends.
    pub(crate) fn values_093(&self) -> String {
        format!(
            "0.9.3 record: agent_runtime_id {}, generation {}, checkpoint_version {}",
            self.agent_runtime_id, self.generation, self.checkpoint_version
        )
    }
}

// The wire shapes, field order as both writers emit them. Re-encoding a
// parsed line must give back its exact bytes (see `is_compact_form`).

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireHeader {
    format: String,
    version: u64,
    writer: String,
    entries: u64,
    entries_length: u64,
    entries_sha256: String,
}

#[derive(Deserialize)]
struct WireEntryProbe {
    identity: Option<serde_json::Value>,
    state: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireUninitialized {
    identity: String,
    state: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireReady {
    identity: String,
    state: String,
    record: WireRecord,
    document: WireDocument,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireRecord {
    identity: String,
    agent_runtime_id: String,
    session_id: String,
    generation: u64,
    checkpoint_version: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireDocument {
    length: u64,
    sha256: String,
    base64: String,
}

/// Read and verify the export file at `path`.
///
/// `on_document` receives each ready entry with the byte offset of its line
/// in the file and its document bytes (their length and SHA-256 already
/// checked) as the pass reaches it, so a caller can decode and check one
/// session at a time, or find the entry again with [`read_entry_at`]. When
/// the file then fails a check, whatever the caller derived from it must be
/// discarded: the result is the file's refusal.
pub(crate) fn read_export_file(
    path: &Path,
    on_document: impl FnMut(&ExportEntry, u64, Vec<u8>),
) -> Result<ExportFile, ExportFileError> {
    let file = std::fs::File::open(path)
        .map_err(|error| ExportFileError::new(format!("{}: {error}", path.display())))?;
    read_export(BufReader::new(file), MAX_ENTRY_LINE_BYTES, on_document)
}

fn read_export(
    mut reader: impl BufRead,
    entry_limit: usize,
    mut on_document: impl FnMut(&ExportEntry, u64, Vec<u8>),
) -> Result<ExportFile, ExportFileError> {
    let mut line = Vec::new();
    let header_length = read_line_bounded(&mut reader, MAX_HEADER_LINE_BYTES, &mut line)? as u64;
    if line.last() != Some(&b'\n') {
        if line.len() > MAX_HEADER_LINE_BYTES {
            return Err(ExportFileError::new(format!(
                "the header line is longer than {MAX_HEADER_LINE_BYTES} bytes"
            )));
        }
        return Err(ExportFileError::new(
            "the file must start with a header line that ends in a newline",
        ));
    }
    let header = parse_header(&line[..line.len() - 1])?;

    let mut digest = Sha256::new();
    let mut length: u64 = 0;
    let mut count: u64 = 0;
    let mut problem: Option<String> = None;
    let mut overlong = false;
    let mut entries: Vec<ExportEntry> = Vec::new();
    let mut sessions: std::collections::BTreeMap<SessionId, String> =
        std::collections::BTreeMap::new();
    loop {
        let read = read_line_bounded(&mut reader, entry_limit, &mut line)?;
        if read == 0 {
            break;
        }
        let offset = header_length + length;
        digest.update(&line);
        length = length.saturating_add(read as u64);
        let ends = line.last() == Some(&b'\n');
        if overlong {
            overlong = !ends;
            continue;
        }
        let index = count;
        count += 1;
        if !ends {
            overlong = line.len() > entry_limit;
            if problem.is_none() {
                problem = Some(if overlong {
                    format!("entry {index} is longer than {entry_limit} bytes")
                } else {
                    format!("entry {index} does not end in a newline")
                });
            }
            continue;
        }
        if problem.is_some() {
            continue;
        }
        match parse_entry(&line[..line.len() - 1], index) {
            Ok((entry, document)) => {
                if let Some(previous) = entries.last()
                    && previous.identity >= entry.identity
                {
                    problem = Some(format!(
                        "entry {index} ({:?}) breaks the identity order (strictly ascending by \
                         code point)",
                        entry.identity
                    ));
                    continue;
                }
                if let Some(record) = &entry.record {
                    if let Some(other) = sessions.get(&record.session_id) {
                        problem = Some(format!(
                            "entry {index} session {} is also bound to {other:?}",
                            record.session_id
                        ));
                        continue;
                    }
                    sessions.insert(record.session_id.clone(), entry.identity.clone());
                }
                if let Some(document) = document {
                    on_document(&entry, offset, document);
                }
                entries.push(entry);
            }
            Err(error) => problem = Some(error.reason),
        }
    }

    if length != header.entries_length {
        return Err(ExportFileError::new(format!(
            "the entries are {length} bytes, the header's entries_length says {}",
            header.entries_length
        )));
    }
    if hex_lower(&digest.finalize()) != header.entries_sha256 {
        return Err(ExportFileError::new(
            "the entries do not match the header's entries_sha256",
        ));
    }
    if count != header.entries {
        return Err(ExportFileError::new(format!(
            "the file holds {count} entry lines, the header says {}",
            header.entries
        )));
    }
    if let Some(problem) = problem {
        return Err(ExportFileError::new(problem));
    }
    Ok(ExportFile {
        writer: header.writer,
        entries_sha256: header.entries_sha256,
        entries,
    })
}

/// Read one line of at most `limit` bytes plus its newline into `line`
/// (cleared first). A longer line leaves `limit + 1` bytes without a newline;
/// the next call continues it.
fn read_line_bounded(
    reader: &mut impl BufRead,
    limit: usize,
    line: &mut Vec<u8>,
) -> Result<usize, ExportFileError> {
    line.clear();
    reader
        .by_ref()
        .take(limit as u64 + 1)
        .read_until(b'\n', line)
        .map_err(|error| ExportFileError::new(format!("read failed: {error}")))
}

/// Read the entry whose line starts at `offset` again, with every check the
/// file pass ran on it. The caller compares the entry with the one the pass
/// recorded there.
pub(crate) fn read_entry_at(
    path: &Path,
    offset: u64,
) -> Result<(ExportEntry, Option<Vec<u8>>), ExportFileError> {
    use std::io::{Seek as _, SeekFrom};
    let mut file = std::fs::File::open(path)
        .map_err(|error| ExportFileError::new(format!("{}: {error}", path.display())))?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| ExportFileError::new(format!("{}: {error}", path.display())))?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    read_line_bounded(&mut reader, MAX_ENTRY_LINE_BYTES, &mut line)?;
    if line.last() != Some(&b'\n') {
        return Err(ExportFileError::new(format!(
            "the entry at byte {offset} does not end in a newline"
        )));
    }
    line.pop();
    parse_entry(&line, offset)
}

fn parse_header(raw: &[u8]) -> Result<WireHeader, ExportFileError> {
    let text = std::str::from_utf8(raw)
        .map_err(|_| ExportFileError::new("the header is not valid UTF-8"))?;
    let header: WireHeader = serde_json::from_str(text).map_err(|error| {
        ExportFileError::new(format!(
            "the header is not a version {CONTINUITY_EXPORT_VERSION} export header (exactly \
             format, version, writer, entries, entries_length, entries_sha256): {error}"
        ))
    })?;
    if header.format != CONTINUITY_EXPORT_FORMAT {
        return Err(ExportFileError::new(format!(
            "the header format is {:?}, not {CONTINUITY_EXPORT_FORMAT:?}",
            header.format
        )));
    }
    if header.version != CONTINUITY_EXPORT_VERSION {
        return Err(ExportFileError::new(format!(
            "unsupported version {}; this build reads version {CONTINUITY_EXPORT_VERSION}",
            header.version
        )));
    }
    if !is_compact_form(&header, raw) {
        return Err(ExportFileError::new(
            "the header is not in the writers' compact form",
        ));
    }
    if header.writer.is_empty() {
        return Err(ExportFileError::new(
            "the header writer must be a non-empty string",
        ));
    }
    if header.entries == 0 {
        return Err(ExportFileError::new("the header lists no entries"));
    }
    if !is_sha256_hex(&header.entries_sha256) {
        return Err(ExportFileError::new(
            "the header entries_sha256 is not a lowercase SHA-256 hex digest",
        ));
    }
    Ok(header)
}

/// One entry line: the entry, and the document bytes of a ready one.
fn parse_entry(raw: &[u8], index: u64) -> Result<(ExportEntry, Option<Vec<u8>>), ExportFileError> {
    let fail = |reason: String| ExportFileError::new(format!("entry {index} {reason}"));
    let text = std::str::from_utf8(raw).map_err(|_| fail("is not valid UTF-8".to_owned()))?;
    let probe: WireEntryProbe = serde_json::from_str(text)
        .map_err(|error| fail(format!("is not a JSON object: {error}")))?;
    let Some(serde_json::Value::String(identity)) = probe.identity else {
        return Err(fail("identity is missing or not a string".to_owned()));
    };
    if let Some(problem) = identity_problem(&identity) {
        return Err(fail(format!("identity {identity:?} {problem}")));
    }
    match probe.state.as_ref().and_then(serde_json::Value::as_str) {
        Some(UNINITIALIZED) => {
            let wire: WireUninitialized = serde_json::from_str(text).map_err(|error| {
                fail(format!(
                    "keys must be exactly [\"identity\", \"state\"]: {error}"
                ))
            })?;
            if !is_compact_form(&wire, raw) {
                return Err(fail("is not in the writers' compact form".to_owned()));
            }
            Ok((
                ExportEntry {
                    identity,
                    record: None,
                },
                None,
            ))
        }
        Some(READY) => {
            let wire: WireReady = serde_json::from_str(text).map_err(|error| {
                fail(format!(
                    "is not a ready entry (exactly identity, state, record, document): {error}"
                ))
            })?;
            if !is_compact_form(&wire, raw) {
                return Err(fail("is not in the writers' compact form".to_owned()));
            }
            if wire.record.identity != identity {
                return Err(fail(format!(
                    "record names identity {:?}",
                    wire.record.identity
                )));
            }
            if let Some(problem) = identity_problem(&wire.record.agent_runtime_id) {
                return Err(fail(format!(
                    "record agent_runtime_id {:?} {problem}",
                    wire.record.agent_runtime_id
                )));
            }
            let session_id = SessionId::parse(&wire.record.session_id)
                .ok()
                .filter(|id| id.to_string() == wire.record.session_id)
                .ok_or_else(|| {
                    fail(format!(
                        "session_id {:?} is not in canonical form",
                        wire.record.session_id
                    ))
                })?;
            let document = base64::engine::general_purpose::STANDARD
                .decode(wire.document.base64.as_bytes())
                .map_err(|_| fail("document base64 is not in canonical padded form".to_owned()))?;
            if wire.document.length == 0 || document.len() as u64 != wire.document.length {
                return Err(fail(format!(
                    "document is {} bytes, its length says {}",
                    document.len(),
                    wire.document.length
                )));
            }
            if !is_sha256_hex(&wire.document.sha256) {
                return Err(fail(
                    "document sha256 is not a lowercase SHA-256 hex digest".to_owned(),
                ));
            }
            if hex_lower(&Sha256::digest(&document)) != wire.document.sha256 {
                return Err(fail("document bytes do not match their sha256".to_owned()));
            }
            Ok((
                ExportEntry {
                    identity,
                    record: Some(ExportRecord {
                        agent_runtime_id: wire.record.agent_runtime_id,
                        session_id,
                        generation: wire.record.generation,
                        checkpoint_version: wire.record.checkpoint_version,
                        document_length: wire.document.length,
                        document_sha256: wire.document.sha256,
                    }),
                },
                Some(document),
            ))
        }
        _ => Err(fail(format!("has unknown state {:?}", probe.state))),
    }
}

/// Whether `value`, encoded the way both writers encode it (compact, their
/// key order, the same escapes), is exactly `raw`. That one comparison
/// refuses repeated keys, whitespace, unneeded escapes and other spellings.
fn is_compact_form(value: &impl Serialize, raw: &[u8]) -> bool {
    let mut compare = CompareWriter {
        expected: raw,
        position: 0,
        mismatch: false,
    };
    serde_json::to_writer(&mut compare, value).is_ok()
        && !compare.mismatch
        && compare.position == raw.len()
}

/// A writer that compares what it is given with `expected`, without keeping
/// a second copy of a line that can be hundreds of MiB.
struct CompareWriter<'a> {
    expected: &'a [u8],
    position: usize,
    mismatch: bool,
}

impl Write for CompareWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let end = self.position.saturating_add(buf.len());
        if self.mismatch || self.expected.get(self.position..end) != Some(buf) {
            self.mismatch = true;
        }
        self.position = end;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Why `value` is not a valid identity (or 0.9.3 runtime id): empty,
/// holding whitespace, or holding a slash.
fn identity_problem(value: &str) -> Option<&'static str> {
    if value.is_empty() {
        Some("is empty")
    } else if value.chars().any(char::is_whitespace) {
        Some("contains whitespace")
    } else if value.contains('/') {
        Some("contains a slash")
    } else {
        None
    }
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    const FIXTURES: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/continuity_export_v1"
    );

    /// A read-back export file and its entries' documents.
    type ReadBack = (ExportFile, Vec<(String, Vec<u8>)>);

    fn read(bytes: &[u8]) -> Result<ReadBack, ExportFileError> {
        read_limited(bytes, MAX_ENTRY_LINE_BYTES)
    }

    fn read_limited(bytes: &[u8], limit: usize) -> Result<ReadBack, ExportFileError> {
        let mut documents = Vec::new();
        let file = read_export(bytes, limit, |entry, _offset, document| {
            documents.push((entry.identity.clone(), document));
        })?;
        Ok((file, documents))
    }

    /// An export file in the writers' form around `lines`.
    fn export(lines: &[&str]) -> Vec<u8> {
        let mut entries = Vec::new();
        for line in lines {
            entries.extend_from_slice(line.as_bytes());
            entries.push(b'\n');
        }
        let header = format!(
            "{{\"format\":\"{CONTINUITY_EXPORT_FORMAT}\",\"version\":1,\"writer\":\"test\",\
             \"entries\":{},\"entries_length\":{},\"entries_sha256\":\"{}\"}}\n",
            lines.len(),
            entries.len(),
            hex_lower(&Sha256::digest(&entries))
        );
        let mut file = header.into_bytes();
        file.extend_from_slice(&entries);
        file
    }

    fn ready(identity: &str, session: &str, document: &[u8]) -> String {
        format!(
            "{{\"identity\":\"{identity}\",\"state\":\"ready\",\"record\":{{\"identity\":\
             \"{identity}\",\"agent_runtime_id\":\"rt-{}\",\"session_id\":\"{session}\",\
             \"generation\":1,\"checkpoint_version\":2}},\"document\":{{\"length\":{},\
             \"sha256\":\"{}\",\"base64\":\"{}\"}}}}",
            identity.replace(':', "-"),
            document.len(),
            hex_lower(&Sha256::digest(document)),
            base64::engine::general_purpose::STANDARD.encode(document)
        )
    }

    fn uninitialized(identity: &str) -> String {
        format!("{{\"identity\":\"{identity}\",\"state\":\"uninitialized\"}}")
    }

    const S1: &str = "019ecb20-4899-7292-a2b4-c81bc7a44258";
    const S2: &str = "019f0000-0000-7000-8000-000000000001";

    #[test]
    fn both_writers_goldens_read_with_their_documents() {
        for writer in ["python", "node"] {
            let path = Path::new(FIXTURES).join(format!("written_by_{writer}.jsonl"));
            let mut documents = Vec::new();
            let mut offsets = Vec::new();
            let file = read_export_file(&path, |entry, offset, document| {
                offsets.push(offset);
                documents.push((entry.identity.clone(), document));
            })
            .unwrap_or_else(|error| panic!("{writer} golden: {error}"));
            assert_eq!(file.writer, writer);
            let identities: Vec<_> = file.entries.iter().map(|e| e.identity.as_str()).collect();
            assert_eq!(
                identities,
                [
                    "agent:alpha",
                    "agent:beta",
                    "agent:\u{ff61}",
                    "agent:\u{1f600}"
                ]
            );
            let ready: Vec<_> = file
                .entries
                .iter()
                .filter_map(|entry| entry.record.as_ref())
                .collect();
            assert_eq!(ready.len(), 2);
            assert_eq!(ready[0].session_id.to_string(), S1);
            assert_eq!(ready[0].generation, 2);
            assert_eq!(ready[0].checkpoint_version, 7);
            assert_eq!(documents.len(), 2);
            assert_eq!(documents[1].1, (0..=255).collect::<Vec<u8>>());
            for (offset, (identity, document)) in offsets.iter().zip(&documents) {
                let (entry, again) = read_entry_at(&path, *offset).expect("read the entry again");
                assert_eq!(&entry.identity, identity);
                assert_eq!(again.as_ref(), Some(document));
            }
        }
    }

    #[test]
    fn a_well_formed_file_reads_every_entry_in_order() {
        let bytes = export(&[
            &ready("agent:a", S1, b"one"),
            &uninitialized("agent:b"),
            &ready("agent:c", S2, b"two"),
        ]);
        let (file, documents) = read(&bytes).expect("reads");
        assert_eq!(file.entries.len(), 3);
        assert!(file.entries[1].record.is_none());
        assert_eq!(
            documents,
            vec![
                ("agent:a".to_owned(), b"one".to_vec()),
                ("agent:c".to_owned(), b"two".to_vec())
            ]
        );
    }

    #[test]
    fn the_header_must_describe_the_entries() {
        let mut bytes = export(&[&uninitialized("agent:a")]);
        bytes.extend_from_slice(b"{}\n");
        let error = read(&bytes).expect_err("an extra line");
        assert!(error.reason.contains("entries_length"), "{error}");

        let bytes = export(&[&uninitialized("agent:a")]);
        let tampered = String::from_utf8(bytes)
            .expect("utf8")
            .replace("agent:a", "agent:b");
        let error = read(tampered.as_bytes()).expect_err("a changed entry");
        assert!(error.reason.contains("entries_sha256"), "{error}");
    }

    #[test]
    fn entry_problems_refuse_the_whole_file() {
        let cases: [(&[String], &str); 6] = [
            (
                &[uninitialized("agent:b"), uninitialized("agent:a")],
                "breaks the identity order",
            ),
            (
                &[uninitialized("agent:a"), uninitialized("agent:a")],
                "breaks the identity order",
            ),
            (
                &[ready("agent:a", S1, b"x"), ready("agent:b", S1, b"y")],
                "is also bound to",
            ),
            (
                &[ready("agent:a", &S1.to_uppercase(), b"x")],
                "is not in canonical form",
            ),
            (&[uninitialized("agent:a b")], "contains whitespace"),
            (
                &["{\"identity\":\"agent:a\",\"state\":\"gone\"}".to_owned()],
                "unknown state",
            ),
        ];
        for (lines, expected) in cases {
            let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
            let error = read(&export(&lines)).expect_err(expected);
            assert!(error.reason.contains(expected), "{expected}: {error}");
        }
    }

    #[test]
    fn only_the_writers_compact_form_reads() {
        let spaced = "{\"identity\": \"agent:a\",\"state\":\"uninitialized\"}";
        let reordered = "{\"state\":\"uninitialized\",\"identity\":\"agent:a\"}";
        let repeated =
            "{\"identity\":\"agent:a\",\"identity\":\"agent:a\",\"state\":\"uninitialized\"}";
        let escaped = "{\"identity\":\"agent:\\u0061\",\"state\":\"uninitialized\"}";
        for line in [spaced, reordered, escaped] {
            let error = read(&export(&[line])).expect_err(line);
            assert!(error.reason.contains("compact form"), "{line}: {error}");
        }
        assert!(read(&export(&[repeated])).is_err());
    }

    #[test]
    fn a_document_must_match_its_length_digest_and_padding() {
        let good = ready("agent:a", S1, b"hello");
        let wrong_length = good.replace("\"length\":5", "\"length\":6");
        let error = read(&export(&[&wrong_length])).expect_err("length");
        assert!(error.reason.contains("its length says 6"), "{error}");

        let digest = hex_lower(&Sha256::digest(b"hello"));
        let wrong_digest = good.replace(&digest, &"0".repeat(64));
        let error = read(&export(&[&wrong_digest])).expect_err("digest");
        assert!(
            error.reason.contains("do not match their sha256"),
            "{error}"
        );

        let unpadded = good.replace("aGVsbG8=", "aGVsbG8");
        let error = read(&export(&[&unpadded])).expect_err("padding");
        assert!(error.reason.contains("canonical padded form"), "{error}");
    }

    #[test]
    fn an_overlong_entry_refuses_after_the_whole_file_is_hashed() {
        let bytes = export(&[
            &ready("agent:a", S1, &[7_u8; 600]),
            &uninitialized("agent:b"),
        ]);
        let error = read_limited(&bytes, 200).expect_err("overlong");
        assert_eq!(error.reason, "entry 0 is longer than 200 bytes");
    }

    #[test]
    fn a_header_that_is_not_one_refuses() {
        let error = read(b"not json\n").expect_err("not json");
        assert!(error.reason.contains("export header"), "{error}");
        let error = read(b"").expect_err("empty");
        assert!(error.reason.contains("header line"), "{error}");
        let bytes = String::from_utf8(export(&[&uninitialized("agent:a")]))
            .expect("utf8")
            .replacen("\"version\":1", "\"version\":2", 1);
        let error = read(bytes.as_bytes()).expect_err("version");
        assert!(error.reason.contains("unsupported version 2"), "{error}");
    }
}
