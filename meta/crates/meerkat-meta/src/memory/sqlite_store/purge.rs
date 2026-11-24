//! Purge of a tombstoned record's content: the store half of
//! `meta/agent_memory/purge`.
//!
//! A purge blanks text and keeps rows. The record named, and every earlier
//! version it replaced, keep their id, scope, kind, trust, status and
//! timestamps, so `supersedes`, `derived_from`, audit and ledger references
//! stay valid. Their title, description, body, tags and the free text of
//! the verification claim are blanked, the content digest column is set to
//! a fixed value, and the free text in their own audit rows (the op
//! rationale and a reviewer's rationale) is blanked. No status is added: a
//! purged record is a tombstoned (or superseded) record with empty text,
//! which a binary that knows nothing of purges reads as such.
//!
//! The record's id is kept, and an id made by the store was derived from
//! the title and body the record was created with. A purge also leaves
//! every other place the text can live: copies that are records of their
//! own (a release successor, a promotion, a derived record), proposals,
//! staged batches, the steward's notes, and the bytes the database file and
//! its write-ahead file still hold.

use std::collections::HashSet;

use rusqlite::{Connection, TransactionBehavior, params};

use super::{SqliteAgentMemoryStore, load_record, mint_token, now_ms, sql_err};
use crate::identity_first::agent_memory::{
    AgentMemoryError, AgentMemoryPurgeError, AgentMemoryPurgeResult,
};
use crate::memory::records::{MemoryAuthor, MemoryRecord, MemoryScope, RecordStatus};

/// The value the content digest column of a purged row holds: a fixed
/// value that is the digest of no content, so the exact-content guards (the
/// write-time duplicate check, the tombstone re-creation window) never
/// match it.
pub(crate) const PURGED_CONTENT_HASH: &str = "purged";

/// Bound on the walk over the versions a record replaced. A lineage this
/// long is not expected; the bound only keeps a damaged chain from looping.
const MAX_PURGE_CHAIN: usize = 4096;

/// Whether `record` is a purged row: a retired record with no text. The
/// write paths refuse an empty title or body, so only a purge leaves one.
pub(crate) fn is_purged(record: &MemoryRecord) -> bool {
    matches!(
        record.status,
        RecordStatus::Tombstoned | RecordStatus::Superseded { .. }
    ) && record.title.is_empty()
        && record.body.is_empty()
}

impl SqliteAgentMemoryStore {
    /// Blank the content of the tombstoned record `memory_id` of `scope`
    /// and of the earlier versions it replaced, in one transaction, with
    /// one audit row per record blanked.
    ///
    /// An unknown id and an id of another scope answer `purged: false`
    /// alike, so the call never confirms an id in a scope the caller did
    /// not name. A record that is not tombstoned is refused (the inner
    /// error): retiring a record and erasing it are two decisions.
    ///
    /// The transaction takes the write lock before it reads, as the
    /// quarantine review does, so what it checked is what it writes, also
    /// across processes that share the realm file.
    pub(super) fn purge_in_scope_blocking(
        &self,
        scope: &MemoryScope,
        memory_id: &str,
        author: &MemoryAuthor,
    ) -> Result<Result<AgentMemoryPurgeResult, AgentMemoryPurgeError>, AgentMemoryError> {
        let realm = scope.realm();
        self.with_realm_conn(realm, |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_err)?;
            let Some(head) = load_record(&tx, realm, memory_id)?.filter(|r| r.scope == *scope)
            else {
                return Ok(Ok(AgentMemoryPurgeResult {
                    memory_id: memory_id.to_string(),
                    purged: false,
                    purged_ids: Vec::new(),
                }));
            };
            if head.status != RecordStatus::Tombstoned {
                return Ok(Err(AgentMemoryPurgeError::NotTombstoned {
                    memory_id: memory_id.to_string(),
                    status: head.status.kind_str(),
                }));
            }
            let chain = replaced_versions(&tx, realm, scope, head)?;
            let author = serde_json::to_value(author)
                .map_err(|err| AgentMemoryError::Parse(err.to_string()))?;
            let token = mint_token("purge");
            let now = now_ms();
            let mut purged_ids = Vec::new();
            for record in &chain {
                if is_purged(record) {
                    continue;
                }
                let mut provenance = record.provenance.clone();
                provenance.verification = None;
                let provenance = serde_json::to_string(&provenance)
                    .map_err(|err| AgentMemoryError::Parse(err.to_string()))?;
                tx.execute(
                    "UPDATE records SET title = '', description = '', body = '', tags = '[]', \
                     provenance = ?1, content_hash = ?2 WHERE memory_id = ?3",
                    params![provenance, PURGED_CONTENT_HASH, record.id],
                )
                .map_err(sql_err)?;
                blank_audit_text(&tx, &record.id)?;
                let detail = serde_json::json!({
                    "op": "purge",
                    "author": author,
                    "rationale": null,
                    "purged_with": memory_id,
                });
                tx.execute(
                    "INSERT INTO audit (stage_token, op_index, op_kind, memory_id, detail, \
                     applied_at_ms) VALUES (?1, ?2, 'purge', ?3, ?4, ?5)",
                    params![
                        token,
                        purged_ids.len() as i64,
                        record.id,
                        detail.to_string(),
                        now as i64,
                    ],
                )
                .map_err(sql_err)?;
                purged_ids.push(record.id.clone());
            }
            tx.commit().map_err(sql_err)?;
            Ok(Ok(AgentMemoryPurgeResult {
                memory_id: memory_id.to_string(),
                purged: !purged_ids.is_empty(),
                purged_ids,
            }))
        })
    }
}

/// `head` and the earlier versions it replaced, newest first. A prior joins
/// the chain only when it is of the same scope and was superseded by the
/// version before it in the chain: the prior of a quarantined update is
/// still the active record, and a prior that another, live successor
/// replaced belongs to that successor's history, so neither is touched.
fn replaced_versions(
    conn: &Connection,
    realm: &str,
    scope: &MemoryScope,
    head: MemoryRecord,
) -> Result<Vec<MemoryRecord>, AgentMemoryError> {
    let mut seen = HashSet::new();
    let mut chain = Vec::new();
    let mut cursor = Some(head);
    while let Some(record) = cursor {
        if !seen.insert(record.id.clone()) || chain.len() >= MAX_PURGE_CHAIN {
            break;
        }
        cursor = match record.supersedes.as_deref() {
            Some(prior_id) => load_record(conn, realm, prior_id)?.filter(|prior| {
                prior.scope == *scope
                    && matches!(&prior.status, RecordStatus::Superseded { by } if *by == record.id)
            }),
            None => None,
        };
        chain.push(record);
    }
    Ok(chain)
}

/// Blank the free text in the audit rows of `memory_id`: the op rationale
/// (the steward's words, or a reviewer's as the review wrote them into the
/// op), the reviewer's rationale a quarantine review keeps under `review`,
/// and the content digest that review was bound to. Each can quote the
/// record. The rows and their other fields stay (who decided what, and the
/// gate's reason), so a review's replay evidence still parses.
fn blank_audit_text(conn: &Connection, memory_id: &str) -> Result<(), AgentMemoryError> {
    let rows: Vec<(i64, String)> = {
        let mut stmt = conn
            .prepare("SELECT audit_id, detail FROM audit WHERE memory_id = ?1")
            .map_err(sql_err)?;
        stmt.query_map(params![memory_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(sql_err)?
            .collect::<Result<_, _>>()
            .map_err(sql_err)?
    };
    for (audit_id, detail) in rows {
        let Ok(mut detail) = serde_json::from_str::<serde_json::Value>(&detail) else {
            // Not the store's typed JSON: left as it is rather than guessed at.
            continue;
        };
        let mut changed = blank_text_field(detail.get_mut("rationale"));
        if let Some(review) = detail.get_mut("review") {
            changed |= blank_text_field(review.get_mut("rationale"));
            if let Some(hash) = review.get_mut("expected_content_hash")
                && hash.as_str() != Some(PURGED_CONTENT_HASH)
            {
                *hash = serde_json::Value::String(PURGED_CONTENT_HASH.to_string());
                changed = true;
            }
        }
        if changed {
            conn.execute(
                "UPDATE audit SET detail = ?1 WHERE audit_id = ?2",
                params![detail.to_string(), audit_id],
            )
            .map_err(sql_err)?;
        }
    }
    Ok(())
}

/// Set a free-text field of an audit detail to null. True when it held
/// something.
fn blank_text_field(field: Option<&mut serde_json::Value>) -> bool {
    match field {
        Some(value) if !value.is_null() => {
            *value = serde_json::Value::Null;
            true
        }
        _ => false,
    }
}
