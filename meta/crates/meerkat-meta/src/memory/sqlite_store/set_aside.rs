//! The store's step of setting plain names' notes aside (see
//! [`crate::memory::set_aside`]).
//!
//! For each name the step forgets every record kept under the name's plain
//! identity scope (the existing tombstone, authored by the application),
//! moves the name's records, of every status, to the scope
//! `set-aside/<name>`, records that scope as private (the row
//! [`crate::memory::capabilities::StewardStore::record_private_retirement`]
//! writes), expires the gated
//! promotions of those records and settles the proposals still pending
//! under the name as rejected. Every name and the marker of the step are
//! one transaction: a step that stops part way leaves the store as it was,
//! and a step whose marker is in the store does nothing.
//!
//! No member's memory tool, no recall and no injection can name a
//! `set-aside/...` scope (a member name holds no `/`), and no record's
//! title, body, author or id changes: the records stay readable in the
//! memory panel by an access administrator.

use std::collections::BTreeSet;

use rusqlite::{OptionalExtension, TransactionBehavior, params};

use super::{
    AgentMemoryError, MemoryAuthor, SqliteAgentMemoryStore, StagedBatchKind, StagedMutationBatch,
    StagedOp, apply_batch_in_tx, expire_promotion_in_tx, json_string, mint_token, now_ms,
    pending_promotions_for, run_blocking, sql_err,
};
use crate::memory::staged::MAX_BATCH_OPS;

/// The first segment of the identity scope key set-aside notes are kept
/// under: `set-aside/<name>`.
pub(crate) const SET_ASIDE_SCOPE: &str = "set-aside";

/// The `audit` token of the marker row of one mob's step.
const MARKER_TOKEN: &str = "set-aside-first-start";

/// What the rows of a name's step say of it.
const SET_ASIDE_RATIONALE: &str =
    "set aside at the first start: kept under a member name that is no longer read";

/// The identity scope key the notes of the plain name `name` are set aside
/// under.
pub(crate) fn set_aside_scope_key(name: &str) -> String {
    format!("{SET_ASIDE_SCOPE}/{name}")
}

/// Whether `name` is a name the agent mob tools give a temporary council's
/// participant: `council-p` and a number.
fn is_council_participant_name(name: &str) -> bool {
    name.strip_prefix("council-p")
        .is_some_and(|order| !order.is_empty() && order.bytes().all(|byte| byte.is_ascii_digit()))
}

/// What one call of the step did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetAsideOutcome {
    /// The marker of the mob's step was in the store: nothing was read or
    /// written.
    AlreadyDone,
    /// The step ran and wrote its marker.
    Done {
        /// The names whose notes were set aside.
        names: usize,
        /// The records moved.
        records: usize,
    },
}

fn marker_token(mob: &str) -> String {
    format!("{MARKER_TOKEN}/{mob}")
}

impl SqliteAgentMemoryStore {
    /// Whether the step of the mob `mob` has run on the store of `realm`:
    /// its marker row is there.
    pub(crate) async fn plain_names_were_set_aside(
        &self,
        realm: &str,
        mob: &str,
    ) -> Result<bool, AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        let token = marker_token(mob);
        run_blocking(move || {
            store.with_realm_conn(&realm, |conn| {
                conn.query_row(
                    "SELECT 1 FROM audit WHERE stage_token = ?1 LIMIT 1",
                    params![token],
                    |_| Ok(()),
                )
                .optional()
                .map(|marker| marker.is_some())
                .map_err(sql_err)
            })
        })
        .await
    }

    /// Set aside the notes kept under each plain name of `names`, and under
    /// every name of a temporary council's participant, in the store of
    /// `realm`, for the mob `mob`: the records created before
    /// `created_before_ms`. Once per mob: a call that finds the mob's marker
    /// does nothing.
    pub(crate) async fn set_plain_names_aside(
        &self,
        realm: &str,
        mob: &str,
        names: BTreeSet<String>,
        created_before_ms: u64,
    ) -> Result<SetAsideOutcome, AgentMemoryError> {
        let store = self.clone();
        let realm = realm.to_string();
        let mob = mob.to_string();
        run_blocking(move || {
            store.set_plain_names_aside_blocking(
                &realm,
                &mob,
                &names,
                created_before_ms,
                &mut |_| Ok(()),
            )
        })
        .await
    }

    /// [`Self::set_plain_names_aside`]; `before_name` is called before each
    /// name that holds something to set aside, and its error ends the step
    /// with nothing written.
    pub(crate) fn set_plain_names_aside_blocking(
        &self,
        realm: &str,
        mob: &str,
        names: &BTreeSet<String>,
        created_before_ms: u64,
        before_name: &mut dyn FnMut(&str) -> Result<(), AgentMemoryError>,
    ) -> Result<SetAsideOutcome, AgentMemoryError> {
        let marker = marker_token(mob);
        self.with_realm_conn(realm, |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_err)?;
            let done: Option<i64> = tx
                .query_row(
                    "SELECT 1 FROM audit WHERE stage_token = ?1 LIMIT 1",
                    params![marker],
                    |row| row.get(0),
                )
                .optional()
                .map_err(sql_err)?;
            if done.is_some() {
                return Ok(SetAsideOutcome::AlreadyDone);
            }
            let now = now_ms();
            let bound = created_before_ms as i64;
            let mut names: BTreeSet<String> = names
                .iter()
                .filter(|name| !name.is_empty() && !name.contains('/'))
                .cloned()
                .collect();
            {
                let mut stmt = tx
                    .prepare(
                        "SELECT DISTINCT scope_key FROM records \
                         WHERE scope_kind = 'identity' AND scope_key LIKE 'council-p%'",
                    )
                    .map_err(sql_err)?;
                let keys = stmt
                    .query_map([], |row| row.get::<_, String>(0))
                    .map_err(sql_err)?;
                for key in keys {
                    let key = key.map_err(sql_err)?;
                    if is_council_participant_name(&key) {
                        names.insert(key);
                    }
                }
            }
            let mut names_set_aside = 0usize;
            let mut records_moved = 0usize;
            for name in &names {
                let records: Vec<(String, String)> = {
                    let mut stmt = tx
                        .prepare(
                            "SELECT memory_id, status_kind FROM records \
                             WHERE scope_kind = 'identity' AND scope_key = ?1 \
                             AND created_at_ms < ?2 ORDER BY memory_id",
                        )
                        .map_err(sql_err)?;
                    let rows = stmt
                        .query_map(params![name, bound], |row| Ok((row.get(0)?, row.get(1)?)))
                        .map_err(sql_err)?;
                    rows.collect::<Result<_, _>>().map_err(sql_err)?
                };
                let author = json_string(&MemoryAuthor::Agent {
                    identity: name.clone(),
                })?;
                let proposals: Vec<String> = {
                    let mut stmt = tx
                        .prepare(
                            "SELECT proposal_id FROM proposals \
                             WHERE status IN ('pending', 'held') AND author = ?1 \
                             AND created_at_ms < ?2 ORDER BY proposal_id",
                        )
                        .map_err(sql_err)?;
                    let rows = stmt
                        .query_map(params![author, bound], |row| row.get(0))
                        .map_err(sql_err)?;
                    rows.collect::<Result<_, _>>().map_err(sql_err)?
                };
                if records.is_empty() && proposals.is_empty() {
                    continue;
                }
                before_name(name)?;
                let live: Vec<&str> = records
                    .iter()
                    .filter(|(_, status)| status != "tombstoned")
                    .map(|(id, _)| id.as_str())
                    .collect();
                for chunk in live.chunks(MAX_BATCH_OPS) {
                    let batch = StagedMutationBatch {
                        kind: StagedBatchKind::FreshWrite,
                        realm: realm.to_string(),
                        author: MemoryAuthor::Application,
                        ops: chunk
                            .iter()
                            .map(|id| StagedOp::Tombstone {
                                id: (*id).to_string(),
                                rationale: Some(SET_ASIDE_RATIONALE.to_string()),
                            })
                            .collect(),
                    };
                    apply_batch_in_tx(&tx, &batch, None, &mint_token("set-aside"), now, None)?;
                }
                // A gated promotion of a set-aside record, or of a proposal
                // settled here, commits nothing later: its mapping expires
                // and its staged batch is discarded.
                for subject in records.iter().map(|(id, _)| id).chain(proposals.iter()) {
                    for promotion in pending_promotions_for(&tx, subject)? {
                        expire_promotion_in_tx(&tx, &promotion, now)?;
                    }
                }
                for proposal in &proposals {
                    tx.execute(
                        "UPDATE proposals SET status = 'rejected' WHERE proposal_id = ?1",
                        params![proposal],
                    )
                    .map_err(sql_err)?;
                }
                let key = set_aside_scope_key(name);
                if !records.is_empty() {
                    let moved = tx
                        .execute(
                            "UPDATE records SET scope_key = ?1 \
                             WHERE scope_kind = 'identity' AND scope_key = ?2 \
                             AND created_at_ms < ?3",
                            params![key, name, bound],
                        )
                        .map_err(sql_err)?;
                    records_moved += moved;
                    tx.execute(
                        "INSERT INTO pending_harvests \
                         (identity, session_key, cause, retired_at_ms, status) \
                         VALUES (?1, NULL, 'set_aside', ?2, 'private') \
                         ON CONFLICT (identity, retired_at_ms) DO UPDATE SET status = 'private'",
                        params![key, now as i64],
                    )
                    .map_err(sql_err)?;
                }
                names_set_aside += 1;
                let detail = serde_json::json!({
                    "op": "set_aside",
                    "author": MemoryAuthor::Application,
                    "rationale": SET_ASIDE_RATIONALE,
                    "name": name,
                    "scope_key": key,
                    "records": records.len(),
                    "proposals": proposals.len(),
                });
                tx.execute(
                    "INSERT INTO audit (stage_token, op_index, op_kind, memory_id, detail, \
                     applied_at_ms) VALUES (?1, ?2, 'set_aside', NULL, ?3, ?4)",
                    params![
                        marker,
                        names_set_aside as i64,
                        detail.to_string(),
                        now as i64
                    ],
                )
                .map_err(sql_err)?;
            }
            let detail = serde_json::json!({
                "op": "set_aside_summary",
                "author": MemoryAuthor::Application,
                "mob": mob,
                "names": names_set_aside,
                "records": records_moved,
            });
            tx.execute(
                "INSERT INTO audit (stage_token, op_index, op_kind, memory_id, detail, \
                 applied_at_ms) VALUES (?1, 0, 'set_aside_summary', NULL, ?2, ?3)",
                params![marker, detail.to_string(), now as i64],
            )
            .map_err(sql_err)?;
            tx.commit().map_err(sql_err)?;
            Ok(SetAsideOutcome::Done {
                names: names_set_aside,
                records: records_moved,
            })
        })
    }
}
