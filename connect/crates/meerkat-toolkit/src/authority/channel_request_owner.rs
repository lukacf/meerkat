//! Read-only source observation from the application's existing inbox.
//!
//! This adapter neither writes a root/cancel bit nor authenticates requester
//! strings. Native ingress consumes independently authenticated qualified
//! principals and the application's canonical native association separately.

use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use toolkit_package::canonical_json;

type Result<T> = std::result::Result<T, ChannelSourceError>;
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ChannelSourceError {
    #[error("channel source is unavailable")]
    Unavailable,
    #[error("channel source is inconsistent")]
    InvalidRecord,
    #[error("legacy signed request roots require native ingress migration")]
    MigrationRequired,
}

/// An exact existing channel/group location, selected by trusted composition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelRequestKey {
    channel: String,
    head: String,
}

impl ChannelRequestKey {
    pub fn new(channel: impl Into<String>, head: impl Into<String>) -> Result<Self> {
        let result = Self {
            channel: channel.into(),
            head: head.into(),
        };
        if result.channel.is_empty() || result.head.is_empty() {
            return Err(ChannelSourceError::InvalidRecord);
        }
        Ok(result)
    }
    pub fn channel(&self) -> &str {
        &self.channel
    }
    pub fn head(&self) -> &str {
        &self.head
    }
}

/// The historical source attestation, decoded from the same existing inbox row.
/// This contains no policy verdict and never authenticates a native principal or public RPC caller.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelSourceProvenance {
    version: u8,
    pub channel_id: String,
    pub extension_id: String,
    pub service_subject: String,
    pub provider_account: String,
    pub requester: String,
    pub sender_id: String,
    pub partition: String,
    pub policy_digest: String,
}

/// Exact source membership in persisted order, without message bodies.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ChannelSourceMember {
    pub source_event_id: String,
    pub envelope_sha256: String,
}

/// Historical source data, not authenticated native work or current custody.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelInboxObservation {
    pub key: ChannelRequestKey,
    pub target_identity: String,
    pub source: ChannelSourceProvenance,
    pub members: Vec<ChannelSourceMember>,
}

/// Reads existing application rows only. No schema changes, key ownership,
/// permission cache, cancellation writer, or runtime registration are installed.
pub struct SqliteChannelSourceReader {
    path: PathBuf,
}
impl SqliteChannelSourceReader {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if path.is_symlink() || !path.is_file() {
            return Err(ChannelSourceError::Unavailable);
        }
        let reader = Self {
            path: path.to_path_buf(),
        };
        reader.connect()?;
        Ok(reader)
    }
    fn connect(&self) -> Result<Connection> {
        Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(storage)
    }
    pub fn observe_exact(&self, key: &ChannelRequestKey) -> Result<ChannelInboxObservation> {
        let mut connection = self.connect()?;
        let tx = connection.transaction().map_err(storage)?;
        // An old authority envelope cannot be silently reclassified as native
        // evidence. The application must migrate through authenticated ingress.
        if columns(&tx, "channel_dispatch_groups")?
            .iter()
            .any(|c| c == "native_request_root")
        {
            let old: Option<String> = tx.query_row(
                "SELECT native_request_root FROM channel_dispatch_groups WHERE channel_id = ? AND head_source_event_id = ?",
                params![key.channel, key.head], |row| row.get(0),
            ).optional().map_err(storage)?.flatten();
            if old.is_some() {
                return Err(ChannelSourceError::MigrationRequired);
            }
        }
        read_group(&tx, key)
    }
}

fn read_group(tx: &Transaction<'_>, key: &ChannelRequestKey) -> Result<ChannelInboxObservation> {
    let row = tx.query_row("SELECT identity, member_count, source_provenance FROM channel_dispatch_groups WHERE channel_id = ? AND head_source_event_id = ?", params![key.channel, key.head], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, Option<String>>(2)?))).optional().map_err(storage)?.ok_or(ChannelSourceError::Unavailable)?;
    let encoded = row.2.ok_or(ChannelSourceError::Unavailable)?;
    let provenance: ChannelSourceProvenance = serde_json::from_str(&encoded).map_err(invalid)?;
    // This outer codec contains only strings plus the fixed integer version.
    // Its canonical encoding is shared with Python without parsing the numeric
    // tokens inside the partition string. A merely equivalent attestation is
    // not the exact original image accepted by InboxSourceProvenance.decode.
    if canonical_json(&provenance).map_err(invalid)? != encoded.as_bytes() {
        return Err(ChannelSourceError::InvalidRecord);
    }
    if provenance.version != 1
        || provenance.channel_id != key.channel
        || [
            &provenance.channel_id,
            &provenance.extension_id,
            &provenance.service_subject,
            &provenance.provider_account,
            &provenance.requester,
            &provenance.sender_id,
            &provenance.partition,
            &provenance.policy_digest,
        ]
        .iter()
        .any(|value| value.is_empty())
        || row.1 <= 0
        || row.1 > 64
        || row.0.is_empty()
    {
        return Err(ChannelSourceError::InvalidRecord);
    }
    // Hash exact original bytes, not cross-language JSON re-serialization.
    // In particular Python and Rust differ in numeric canonical spellings.
    let mut statement = tx.prepare("SELECT m.source_event_id, m.ordinal, m.envelope_digest, i.envelope, i.source_provenance, i.dispatch_continuity_version, i.state FROM channel_dispatch_members m JOIN channel_inbox i ON i.channel_id = m.channel_id AND i.source_event_id = m.source_event_id WHERE m.channel_id = ? AND m.head_source_event_id = ? ORDER BY m.ordinal").map_err(storage)?;
    let rows = statement
        .query_map(params![key.channel, key.head], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .map_err(storage)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(storage)?;
    if rows.len() != usize::try_from(row.1).map_err(invalid)?
        || rows.first().map(|row| &row.0) != Some(&key.head)
    {
        return Err(ChannelSourceError::InvalidRecord);
    }
    let mut members = Vec::with_capacity(rows.len());
    for (index, member) in rows.iter().enumerate() {
        if member.1 != i64::try_from(index).map_err(invalid)?
            || member.2 != hex::encode(Sha256::digest(member.3.as_bytes()))
            || member.4.as_deref() != Some(encoded.as_str())
            || member.5 != 1
            || !matches!(
                member.6.as_str(),
                "accepted" | "dispatched" | "failed" | "rejected_known"
            )
        {
            return Err(ChannelSourceError::InvalidRecord);
        }
        // The Python owner freezes canonical envelope bytes before publishing
        // membership. Validate the top-level object framing and unique keys,
        // preserving the exact nested values rather than inventing another
        // Python number codec. The original member digest remains mandatory.
        let envelope = frozen_envelope(&member.3)?;
        let sender: String = serde_json::from_str(
            envelope
                .get("sender_id")
                .ok_or(ChannelSourceError::InvalidRecord)?
                .get(),
        )
        .map_err(invalid)?;
        if sender != provenance.sender_id {
            return Err(ChannelSourceError::InvalidRecord);
        }
        // Derive the complete exact three-field encoding from the frozen
        // original envelope. Comparing the entire string rejects duplicate or
        // extra partition keys, alternate framing, and changed raw tokens.
        // Missing fields have the same null spelling as Python envelope.get.
        let partition: BTreeMap<&str, Option<&RawValue>> =
            ["group_id", "recipient_or_chat_id", "thread_id"]
                .into_iter()
                .map(|field| (field, envelope.get(field).map(|value| value.as_ref())))
                .collect();
        if serde_json::to_string(&partition).map_err(invalid)? != provenance.partition {
            return Err(ChannelSourceError::InvalidRecord);
        }
        members.push(ChannelSourceMember {
            source_event_id: member.0.clone(),
            envelope_sha256: member.2.clone(),
        });
    }
    Ok(ChannelInboxObservation {
        key: key.clone(),
        target_identity: row.0,
        source: provenance,
        members,
    })
}

/// Read the original frozen top-level provider object without loss of numeric
/// spelling. The original Python owner remains responsible for nested-value
/// canonicalization before membership publication; this observer binds those
/// raw values to the exact member digest and the exact selected partition.
fn frozen_envelope(encoded: &str) -> Result<BTreeMap<String, Box<RawValue>>> {
    struct UniqueObject(BTreeMap<String, Box<RawValue>>);
    impl<'de> Deserialize<'de> for UniqueObject {
        fn deserialize<D: serde::Deserializer<'de>>(
            deserializer: D,
        ) -> std::result::Result<Self, D::Error> {
            struct UniqueObjectVisitor;
            impl<'de> serde::de::Visitor<'de> for UniqueObjectVisitor {
                type Value = UniqueObject;

                fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    formatter.write_str("a frozen provider object with unique keys")
                }

                fn visit_map<A: serde::de::MapAccess<'de>>(
                    self,
                    mut access: A,
                ) -> std::result::Result<Self::Value, A::Error> {
                    let mut object = BTreeMap::new();
                    while let Some((key, value)) = access.next_entry::<String, Box<RawValue>>()? {
                        if object.insert(key, value).is_some() {
                            return Err(serde::de::Error::custom("duplicate frozen provider key"));
                        }
                    }
                    Ok(UniqueObject(object))
                }
            }
            deserializer.deserialize_map(UniqueObjectVisitor)
        }
    }
    let UniqueObject(object) = serde_json::from_str(encoded).map_err(invalid)?;
    if serde_json::to_string(&object).map_err(invalid)? != encoded {
        return Err(ChannelSourceError::InvalidRecord);
    }
    Ok(object)
}

fn columns(connection: &Connection, table: &str) -> Result<Vec<String>> {
    // Callers supply only the fixed internal table names above.
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(storage)?;
    let rows = statement
        .query_map([], |row| row.get(1))
        .map_err(storage)?
        .collect::<std::result::Result<Vec<String>, _>>()
        .map_err(storage)?;
    Ok(rows)
}
fn storage(_: rusqlite::Error) -> ChannelSourceError {
    ChannelSourceError::Unavailable
}
fn invalid<E>(_: E) -> ChannelSourceError {
    ChannelSourceError::InvalidRecord
}

#[cfg(test)]
mod tests;
