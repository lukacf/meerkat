//! The console audit log: one append-only row per console change, naming
//! who made it and how it ended.
//!
//! A row records the actor the console resolved from the connection (its
//! kind, its console principal, a person's verified issuer and subject, the
//! assurance, and a digest of the person's login, never token bytes), the
//! method, the change's target (the identifiers among its params, never
//! content), the outcome (`committed`, `refused` or `failed`, with the
//! JSON-RPC code or HTTP status and the typed kind of an error) and the
//! owner's receipt (the identifiers in its answer). A row is bounded
//! whatever the caller sends: at most 16 `id` fields in its target and in
//! its receipt, no key longer than 64 bytes, no value longer than 256 bytes,
//! and at most 4 KiB in all; a row that left anything out says so
//! (`truncated`). The console writes the row once the owner answered, and
//! nothing reads the log to decide anything: each owner's own record stays
//! the authority for what happened.
//!
//! The log lives in the realm's metadata store
//! ([`crate::runtime::PersistentMetadataStore`]) as pages of rows, one realm
//! document per page, and lasts as long as the store keeps its realm
//! documents: for the life of the process or across restarts, which every
//! read reports. A store that keeps no realm documents keeps no log
//! ([`ConsoleAuditLog::over`] answers `None`). Rows are numbered from 1, and
//! a store that keeps its documents never reuses a number: pruning never
//! removes the page the next row goes to. A log a process keeps in memory
//! starts again from 1 when the process does. The log is bounded by count and
//! by age ([`ConsoleAuditLimits`]); whole pages are pruned, so up to one page
//! more than a bound is kept. A row is read back exactly as it was stored,
//! so the rows a newer build wrote stay readable.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::runtime::{MetadataStoreError, PersistentMetadataStore, RecordRetention};

/// The key prefix of the log's pages among the realm documents: the page
/// number follows, zero-padded, so the keys sort as the pages do.
const PAGE_PREFIX: &str = "console-audit/page/";

/// Rows per page: page `n` holds the rows numbered `n * 64 + 1` to
/// `(n + 1) * 64`.
const ROWS_PER_PAGE: u64 = 64;

/// The page format this build writes and reads.
const PAGE_VERSION: u32 = 1;

/// The most rows one read answers.
pub(crate) const CONSOLE_AUDIT_MAX_PAGE: usize = 500;

/// The rows one read answers when it names no limit.
pub(crate) const CONSOLE_AUDIT_DEFAULT_PAGE: usize = 100;

/// The most `id` fields (`id` and `*_id`) a row's target or receipt keeps.
const MAX_ID_FIELDS: usize = 16;

/// The longest key a row keeps; a field with a longer one is left out.
const MAX_KEY_BYTES: usize = 64;

/// The longest value a row keeps; a longer one is cut.
const MAX_VALUE_BYTES: usize = 256;

/// The most bytes one row takes, encoded with its number and time.
const MAX_ROW_BYTES: usize = 4096;

/// The JSON-RPC codes of an owner that failed: whether anything changed is
/// not known. Every other error is a refusal.
const FAILURE_CODES: [i64; 2] = [-32603, -32000];

/// The bounds of the console audit log: the most rows it keeps and the
/// oldest a row may be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConsoleAuditLimits {
    /// The most rows the log keeps, give or take one page of 64 rows. At
    /// least 1: a runtime refuses a log that would keep no row when it is
    /// built.
    pub max_entries: u64,
    /// How old the newest row of a page may be for the log to keep the page.
    pub max_age: Duration,
}

impl ConsoleAuditLimits {
    /// Keep at most `max_entries` rows (at least 1), none older than
    /// `max_age` (each give or take one page of 64 rows).
    pub fn new(max_entries: u64, max_age: Duration) -> Self {
        Self {
            max_entries,
            max_age,
        }
    }
}

impl Default for ConsoleAuditLimits {
    /// 20000 rows, none older than 365 days.
    fn default() -> Self {
        Self::new(20_000, Duration::from_hours(365 * 24))
    }
}

/// The actor of one console change, as the console resolved the connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ConsoleAuditActor {
    /// `person`, `service`, `token_principal` or `anonymous`, as
    /// `meta/console/whoami` names it.
    pub(crate) kind: &'static str,
    /// The console principal the access policy keys (the token's email, or
    /// its subject); absent for an anonymous caller of an open console.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) principal: Option<String>,
    /// A person's verified OIDC issuer: with `subject`, the person's
    /// qualified principal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) issuer: Option<String>,
    /// A person's verified OIDC `sub`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) subject: Option<String>,
    /// The assurance the console established for the caller.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) assurance: Option<meerkat_core::ApprovalAssurance>,
    /// `sha256:` and a hex SHA-256 naming the person's login: the login the
    /// token's signed `sid` names, else the token itself. Never the login
    /// or the token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) login_scope_digest: Option<String>,
}

/// How a console change ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConsoleAuditOutcome {
    /// The owner answered with a result.
    Committed,
    /// The change was refused: a typed refusal, invalid params, a denied
    /// access check, a read-only console.
    Refused,
    /// The owner failed (`-32603`, `-32000`, an HTTP 5xx): whether anything
    /// changed is not known.
    Failed,
}

/// The identifiers a row keeps of a change's params or of its owner's
/// answer, and whether any were left out or cut.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Identifiers {
    pub(crate) fields: Map<String, Value>,
    pub(crate) truncated: bool,
}

/// One console change to record: the row without its number and time.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ConsoleAuditEntry {
    pub(crate) actor: ConsoleAuditActor,
    /// The console method, or the route of a console change that is no
    /// JSON-RPC call (`/console/mcp-apps/call-tool`).
    pub(crate) method: String,
    /// The identifiers among the change's params ([`audit_target`]).
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub(crate) target: Map<String, Value>,
    pub(crate) outcome: ConsoleAuditOutcome,
    /// The JSON-RPC code (or the HTTP status of a route change) of a refusal
    /// or a failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) code: Option<i64>,
    /// The typed kind of a refusal or a failure (`error.data.kind`, or the
    /// `error` of a route's answer), when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) kind: Option<String>,
    /// The identifiers in the owner's answer ([`audit_receipt`]).
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub(crate) receipt: Map<String, Value>,
    /// The row left out or cut something the change or its answer named.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) truncated: bool,
}

impl ConsoleAuditEntry {
    /// The change `actor` asked for with `method` on `target`, ended by the
    /// JSON-RPC answer `answer`: committed with the receipt of its result, or
    /// refused or failed with its error's code and kind.
    pub(crate) fn answered(
        actor: ConsoleAuditActor,
        method: String,
        target: Identifiers,
        answer: &Value,
    ) -> Self {
        let (outcome, code, kind, receipt) =
            match answer.get("error").filter(|error| !error.is_null()) {
                None => (
                    ConsoleAuditOutcome::Committed,
                    None,
                    None,
                    answer.get("result").map(audit_receipt).unwrap_or_default(),
                ),
                Some(error) => {
                    let code = error.get("code").and_then(Value::as_i64);
                    let kind = error
                        .get("data")
                        .and_then(|data| data.get("kind"))
                        .and_then(Value::as_str)
                        .map(bounded);
                    let outcome = if code.is_some_and(|code| FAILURE_CODES.contains(&code)) {
                        ConsoleAuditOutcome::Failed
                    } else {
                        ConsoleAuditOutcome::Refused
                    };
                    (outcome, code, kind, Identifiers::default())
                }
            };
        Self {
            actor,
            method,
            target: target.fields,
            outcome,
            code,
            kind,
            truncated: target.truncated || receipt.truncated,
            receipt: receipt.fields,
        }
        .within_row_bound()
    }

    /// The change `actor` asked for on the route `method` with `target`,
    /// ended by an HTTP answer with `status` and the JSON body `body`:
    /// committed on a success, failed on a server error (whether anything
    /// changed is not known), refused otherwise, with the status as its code
    /// and the body's `error` as its kind.
    pub(crate) fn answered_http(
        actor: ConsoleAuditActor,
        method: String,
        target: Identifiers,
        status: u16,
        body: &Value,
    ) -> Self {
        let (outcome, code, kind) = if (200..300).contains(&status) {
            (ConsoleAuditOutcome::Committed, None, None)
        } else {
            let outcome = if status >= 500 {
                ConsoleAuditOutcome::Failed
            } else {
                ConsoleAuditOutcome::Refused
            };
            let kind = body.get("error").and_then(Value::as_str).map(bounded);
            (outcome, Some(i64::from(status)), kind)
        };
        Self {
            actor,
            method,
            target: target.fields,
            outcome,
            code,
            kind,
            receipt: Map::new(),
            truncated: target.truncated,
        }
        .within_row_bound()
    }

    /// This entry, made to fit [`MAX_ROW_BYTES`] with its number and time:
    /// the receipt gives way first, then the target, a field at a time from
    /// its last key, and the row says it was cut.
    fn within_row_bound(mut self) -> Self {
        /// The most a row's number and time add to its encoding.
        const NUMBER_AND_TIME_BYTES: usize = 64;
        loop {
            let size = serde_json::to_string(&self).map_or(usize::MAX, |encoded| encoded.len());
            if size.saturating_add(NUMBER_AND_TIME_BYTES) <= MAX_ROW_BYTES {
                return self;
            }
            let from = if self.receipt.is_empty() {
                &mut self.target
            } else {
                &mut self.receipt
            };
            let Some(last) = from.keys().next_back().cloned() else {
                // Only the actor, the method and the outcome are left, each
                // bounded on its own.
                return self;
            };
            from.remove(&last);
            self.truncated = true;
        }
    }
}

/// One row of the log as stored and read: its number, the time it was
/// written (epoch milliseconds) and its other fields as written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ConsoleAuditRow {
    pub(crate) seq: u64,
    pub(crate) at_ms: u64,
    #[serde(flatten)]
    pub(crate) fields: Map<String, Value>,
}

/// One read of the log: the rows after a cursor, oldest first.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ConsoleAuditPage {
    pub(crate) records: Vec<ConsoleAuditRow>,
    /// The cursor of the next read: the last row's `seq`, or the cursor this
    /// read started from when it found none.
    pub(crate) next_after_seq: u64,
    /// The oldest row the log keeps; `None` while it keeps none.
    pub(crate) oldest_available_seq: Option<u64>,
    /// The newest row the log has written, 0 before the first.
    pub(crate) latest_seq: u64,
    /// More rows follow this read.
    pub(crate) has_more: bool,
    /// The cursor named rows the log does not keep (pruned, or never
    /// written): this read starts at the oldest row kept, and the reader
    /// resynchronizes.
    pub(crate) gap: bool,
    /// How long the log keeps its rows: `process` (the metadata store keeps
    /// them for the life of the gateway process, so the log holds the rows
    /// since the gateway started) or `durable` (across restarts).
    pub(crate) retention: RecordRetention,
}

/// One stored page.
#[derive(Debug, Serialize, Deserialize)]
struct StoredPage {
    v: u32,
    rows: Vec<ConsoleAuditRow>,
}

/// The realm's console audit log over its metadata store.
pub(crate) struct ConsoleAuditLog {
    store: Arc<dyn PersistentMetadataStore>,
    /// How long the store keeps the log's pages.
    retention: RecordRetention,
    limits: ConsoleAuditLimits,
    /// The log's position, read from the store at first use. Appends and
    /// reads are serialized on it, so a read never sees a prune half done.
    state: tokio::sync::Mutex<AppendState>,
}

/// Where the log stands: the next row's number and the pages it pruned.
#[derive(Debug, Default)]
struct AppendState {
    /// The next row's number; `None` until it is read from the store.
    next_seq: Option<u64>,
    /// Every page below this one was pruned, or found removed, by this log.
    /// A store may still list them (a host store realm lists every key it
    /// wrote in the process that removed it): they count for nothing and
    /// are never removed again.
    pruned_below: u64,
}

impl std::fmt::Debug for ConsoleAuditLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsoleAuditLog")
            .field("retention", &self.retention)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

fn page_key(page: u64) -> String {
    format!("{PAGE_PREFIX}{page:020}")
}

fn page_of(seq: u64) -> u64 {
    seq.saturating_sub(1) / ROWS_PER_PAGE
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

impl ConsoleAuditLog {
    /// The log over `store`, bounded by `limits`; `None` when the store
    /// keeps no realm documents.
    pub(crate) fn over(
        store: Arc<dyn PersistentMetadataStore>,
        limits: ConsoleAuditLimits,
    ) -> Option<Self> {
        let retention = store.realm_document_retention()?;
        Some(Self {
            store,
            retention,
            limits,
            state: tokio::sync::Mutex::new(AppendState::default()),
        })
    }

    /// The page numbers the store lists, ascending, from `from` on.
    async fn pages(&self, from: u64) -> Result<Vec<u64>, MetadataStoreError> {
        let mut pages = self
            .store
            .realm_document_keys(PAGE_PREFIX)
            .await?
            .iter()
            .filter_map(|key| key.strip_prefix(PAGE_PREFIX)?.parse::<u64>().ok())
            .filter(|page| *page >= from)
            .collect::<Vec<_>>();
        pages.sort_unstable();
        pages.dedup();
        Ok(pages)
    }

    /// The rows of `page`; `None` when the store holds no such page.
    async fn stored_page(
        &self,
        page: u64,
    ) -> Result<Option<Vec<ConsoleAuditRow>>, MetadataStoreError> {
        let key = page_key(page);
        let Some(document) = self.store.realm_document(&key).await? else {
            return Ok(None);
        };
        let stored: StoredPage = serde_json::from_str(&document)
            .map_err(|error| MetadataStoreError::Decode(format!("{key}: {error}")))?;
        if stored.v != PAGE_VERSION {
            return Err(MetadataStoreError::Decode(format!(
                "{key}: page format {} is not {PAGE_VERSION}",
                stored.v
            )));
        }
        Ok(Some(stored.rows))
    }

    async fn read_page(&self, page: u64) -> Result<Vec<ConsoleAuditRow>, MetadataStoreError> {
        Ok(self.stored_page(page).await?.unwrap_or_default())
    }

    async fn write_page(
        &self,
        page: u64,
        rows: Vec<ConsoleAuditRow>,
    ) -> Result<(), MetadataStoreError> {
        let key = page_key(page);
        let document = serde_json::to_string(&StoredPage {
            v: PAGE_VERSION,
            rows,
        })
        .map_err(|error| MetadataStoreError::Io(format!("encode {key}: {error}")))?;
        self.store.set_realm_document(&key, Some(&document)).await
    }

    /// The next row's number, read from the store the first time.
    async fn next_seq(&self, state: &mut AppendState) -> Result<u64, MetadataStoreError> {
        if let Some(seq) = state.next_seq {
            return Ok(seq);
        }
        let mut newest_seq = None;
        for page in self.pages(state.pruned_below).await?.into_iter().rev() {
            if let Some(rows) = self.stored_page(page).await? {
                newest_seq = Some(
                    rows.iter()
                        .map(|row| row.seq)
                        .max()
                        .unwrap_or(page * ROWS_PER_PAGE),
                );
                break;
            }
        }
        let seq = newest_seq.map_or(1, |seq| seq.saturating_add(1));
        state.next_seq = Some(seq);
        Ok(seq)
    }

    /// Append `entry` as the next row. A row that cannot be written is
    /// logged and dropped: the change it records stands as its owner
    /// answered it.
    pub(crate) async fn record(&self, entry: ConsoleAuditEntry) -> Option<ConsoleAuditRow> {
        match self.append(entry, now_ms()).await {
            Ok(row) => Some(row),
            Err(error) => {
                tracing::warn!(%error, "a console audit row could not be written");
                None
            }
        }
    }

    /// Append `entry` as the next row, written at `at_ms`.
    async fn append(
        &self,
        entry: ConsoleAuditEntry,
        at_ms: u64,
    ) -> Result<ConsoleAuditRow, MetadataStoreError> {
        let fields = match serde_json::to_value(&entry) {
            Ok(Value::Object(fields)) => fields,
            Ok(other) => {
                return Err(MetadataStoreError::Io(format!(
                    "a console audit row encodes as {other}, not an object"
                )));
            }
            Err(error) => {
                return Err(MetadataStoreError::Io(format!(
                    "encode a console audit row: {error}"
                )));
            }
        };
        let mut state = self.state.lock().await;
        let seq = self.next_seq(&mut state).await?;
        let row = ConsoleAuditRow { seq, at_ms, fields };
        let page = page_of(seq);
        let mut rows = self.read_page(page).await?;
        rows.push(row.clone());
        self.write_page(page, rows).await?;
        state.next_seq = Some(seq.saturating_add(1));
        // A full page closes it: the bound is applied once per page.
        if seq % ROWS_PER_PAGE == 0
            && let Err(error) = self
                .prune(&mut state, page_of(seq.saturating_add(1)), at_ms)
                .await
        {
            tracing::warn!(%error, "the console audit log could not be pruned");
        }
        Ok(row)
    }

    /// Remove the oldest whole pages past the limits' count or age as of
    /// `now_ms`, never `open_page` (the page the next row goes to) nor a
    /// page after it. Pages go oldest first, so the pages from the oldest one
    /// kept on are all still held; a listed page the store no longer holds
    /// is skipped and never removed again.
    async fn prune(
        &self,
        state: &mut AppendState,
        open_page: u64,
        now_ms: u64,
    ) -> Result<(), MetadataStoreError> {
        let mut closed = self
            .pages(state.pruned_below)
            .await?
            .into_iter()
            .filter(|page| *page < open_page)
            .collect::<std::collections::VecDeque<_>>();
        // Listed pages the store no longer holds, oldest first: removed by an
        // earlier prune of this process, or listed stale.
        let mut oldest_rows = None;
        while let Some(&page) = closed.front() {
            match self.stored_page(page).await? {
                Some(rows) => {
                    oldest_rows = Some(rows);
                    break;
                }
                None => {
                    closed.pop_front();
                    state.pruned_below = page.saturating_add(1);
                }
            }
        }
        let kept_pages = self
            .limits
            .max_entries
            .max(1)
            .div_ceil(ROWS_PER_PAGE)
            .saturating_add(1);
        let over_count = (closed.len() as u64)
            .saturating_add(1)
            .saturating_sub(kept_pages);
        let cutoff_ms = now_ms
            .saturating_sub(u64::try_from(self.limits.max_age.as_millis()).unwrap_or(u64::MAX));
        for (index, page) in closed.into_iter().enumerate() {
            let rows = match oldest_rows.take() {
                Some(rows) => rows,
                None if (index as u64) < over_count => Vec::new(),
                None => self.read_page(page).await?,
            };
            let expired = (index as u64) < over_count
                || rows
                    .iter()
                    .map(|row| row.at_ms)
                    .max()
                    .is_some_and(|newest| newest < cutoff_ms);
            if !expired {
                break;
            }
            self.store.set_realm_document(&page_key(page), None).await?;
            state.pruned_below = page.saturating_add(1);
        }
        Ok(())
    }

    /// The rows after `after_seq` (from the oldest row kept without one),
    /// oldest first, at most `limit` of them (clamped to 1 to
    /// [`CONSOLE_AUDIT_MAX_PAGE`]). The read holds the append lock, so no
    /// append or prune changes the log under it.
    pub(crate) async fn page(
        &self,
        after_seq: Option<u64>,
        limit: usize,
    ) -> Result<ConsoleAuditPage, MetadataStoreError> {
        let limit = limit.clamp(1, CONSOLE_AUDIT_MAX_PAGE);
        let mut state = self.state.lock().await;
        let latest_seq = self.next_seq(&mut state).await?.saturating_sub(1);
        let pages = self.pages(state.pruned_below).await?;
        let mut oldest_available_seq = None;
        for page in &pages {
            if let Some(first) = self.read_page(*page).await?.first() {
                oldest_available_seq = Some(first.seq);
                break;
            }
        }
        // The cursor of the oldest row kept: the reads that start there miss
        // nothing the log keeps.
        let floor = oldest_available_seq.map_or(latest_seq, |oldest| oldest.saturating_sub(1));
        let gap = after_seq.is_some_and(|after| after < floor || after > latest_seq);
        let after = match after_seq {
            Some(after) if !gap => after,
            _ => floor,
        };
        let first_page = page_of(after.saturating_add(1));
        let mut records = Vec::new();
        let mut has_more = false;
        'pages: for page in pages.into_iter().filter(|page| *page >= first_page) {
            for row in self.read_page(page).await? {
                if row.seq <= after || row.seq > latest_seq {
                    continue;
                }
                if records.len() == limit {
                    has_more = true;
                    break 'pages;
                }
                records.push(row);
            }
        }
        drop(state);
        let next_after_seq = records.last().map_or(after, |row| row.seq);
        Ok(ConsoleAuditPage {
            records,
            next_after_seq,
            oldest_available_seq,
            latest_seq,
            has_more,
            gap,
            retention: self.retention,
        })
    }
}

/// The identifiers among `params` a row keeps as the change's target: each
/// top-level `id` or `*_id` field and the fields that name what a change is
/// of (an identity, a realm, a role, a principal, a group or rule name, a
/// decision, an action, the revision a checked edit expected), when their
/// value is a string, a number or a boolean, and the id of an access rule
/// an upsert carries. A checked access edit's fields are read inside its
/// `checked_v1` envelope. Content (messages, memory bodies, labels,
/// configurations, reasons) is never kept, and the identifiers are bounded
/// ([`identifier_fields`]).
pub(crate) fn audit_target(params: &Value) -> Identifiers {
    const NAMED: &[&str] = &[
        "identity",
        "agent_identity",
        "realm",
        "role",
        "principal",
        "name",
        "decision",
        "action",
        "operation",
        "enabled",
        "expected_revision",
    ];
    let params = params
        .get("checked_v1")
        .filter(|envelope| envelope.is_object())
        .unwrap_or(params);
    let mut target = identifier_fields(params, |key| NAMED.contains(&key));
    if let Some(rule_id) = params
        .get("rule")
        .and_then(|rule| rule.get("id"))
        .and_then(Value::as_str)
    {
        target.truncated |= rule_id.len() > MAX_VALUE_BYTES;
        target
            .fields
            .insert("rule_id".to_string(), Value::String(bounded(rule_id)));
    }
    target
}

/// The identifiers in an owner's answer a row keeps as its receipt: each
/// top-level `id` or `*_id` field, a revision or generation, the record a
/// memory update supersedes, and the audit token of a quarantine decision,
/// bounded as a target is.
pub(crate) fn audit_receipt(result: &Value) -> Identifiers {
    const NAMED: &[&str] = &["revision", "generation", "agent_identity", "supersedes"];
    let mut receipt = identifier_fields(result, |key| NAMED.contains(&key));
    if let Some(token) = result
        .get("decision")
        .and_then(|decision| decision.get("audit_token"))
        .and_then(Value::as_str)
    {
        receipt.truncated |= token.len() > MAX_VALUE_BYTES;
        receipt
            .fields
            .insert("audit_token".to_string(), Value::String(bounded(token)));
    }
    receipt
}

/// A target naming each field of `fields` that has a value, bounded as every
/// target is: for a console change that is no JSON-RPC call (an MCP app tool
/// call names its member and its tool).
pub(crate) fn target_of(fields: &[(&str, Option<&str>)]) -> Identifiers {
    let mut target = Identifiers::default();
    for (key, value) in fields {
        let Some(value) = value else {
            continue;
        };
        target.truncated |= value.len() > MAX_VALUE_BYTES;
        target
            .fields
            .insert((*key).to_string(), Value::String(bounded(value)));
    }
    target
}

/// The top-level fields of `value` a row keeps: `id` and each `*_id` field
/// (at most [`MAX_ID_FIELDS`] of them, in key order) and the fields `named`
/// names, when their value is a string, a number or a boolean. A field whose
/// key is longer than [`MAX_KEY_BYTES`] is left out and a string longer than
/// [`MAX_VALUE_BYTES`] is cut; either marks the result truncated.
fn identifier_fields(value: &Value, named: impl Fn(&str) -> bool) -> Identifiers {
    let mut kept = Identifiers::default();
    let Some(object) = value.as_object() else {
        return kept;
    };
    let mut ids = 0_usize;
    for (key, value) in object {
        let is_id = key == "id" || key.ends_with("_id");
        if !is_id && !named(key) {
            continue;
        }
        let value = match value {
            Value::String(text) => {
                kept.truncated |= text.len() > MAX_VALUE_BYTES;
                Value::String(bounded(text))
            }
            Value::Number(_) | Value::Bool(_) => value.clone(),
            _ => continue,
        };
        if key.len() > MAX_KEY_BYTES || (is_id && ids == MAX_ID_FIELDS) {
            kept.truncated = true;
            continue;
        }
        if is_id {
            ids += 1;
        }
        kept.fields.insert(key.clone(), value);
    }
    kept
}

/// `text` cut to at most [`MAX_VALUE_BYTES`] bytes, at a character boundary.
pub(crate) fn bounded(text: &str) -> String {
    if text.len() <= MAX_VALUE_BYTES {
        return text.to_string();
    }
    let mut end = MAX_VALUE_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::runtime::InMemoryMetadataStore;

    const DAY_MS: u64 = 24 * 60 * 60 * 1000;

    fn entry(method: &str) -> ConsoleAuditEntry {
        ConsoleAuditEntry::answered(
            ConsoleAuditActor {
                kind: "person",
                principal: Some("alice@example.test".to_string()),
                issuer: Some("https://issuer.example.test".to_string()),
                subject: Some("sub-1".to_string()),
                assurance: Some(meerkat_core::ApprovalAssurance::PersonAuthenticated),
                login_scope_digest: Some("sha256:00".to_string()),
            },
            method.to_string(),
            Identifiers::default(),
            &json!({ "jsonrpc": "2.0", "id": 1, "result": { "accepted": true } }),
        )
    }

    fn audit_log(
        store: &Arc<InMemoryMetadataStore>,
        limits: ConsoleAuditLimits,
    ) -> ConsoleAuditLog {
        ConsoleAuditLog::over(
            Arc::clone(store) as Arc<dyn PersistentMetadataStore>,
            limits,
        )
        .expect("the in-memory store keeps realm documents")
    }

    async fn append_rows(log: &ConsoleAuditLog, count: u64, at_ms: u64) {
        for _ in 0..count {
            log.append(entry("meta/mob_labels/set"), at_ms)
                .await
                .expect("append");
        }
    }

    fn seqs(page: &ConsoleAuditPage) -> Vec<u64> {
        page.records.iter().map(|row| row.seq).collect()
    }

    /// Rows are numbered from 1 and read in order, a page of rows at a time;
    /// a new log over the same store numbers its rows after the stored ones.
    #[tokio::test]
    async fn rows_are_numbered_in_order_and_read_after_a_cursor() {
        let store = Arc::new(InMemoryMetadataStore::new());
        let first = audit_log(&store, ConsoleAuditLimits::default());
        append_rows(&first, 150, 1_000).await;

        let head = first.page(None, 100).await.expect("read");
        assert_eq!(seqs(&head), (1..=100).collect::<Vec<_>>());
        assert!(head.has_more && !head.gap);
        assert_eq!(head.next_after_seq, 100);
        assert_eq!(head.latest_seq, 150);
        assert_eq!(head.oldest_available_seq, Some(1));
        assert_eq!(
            head.retention,
            RecordRetention::Process,
            "an in-memory store keeps the log for the process"
        );
        let tail = first.page(Some(100), 100).await.expect("read");
        assert_eq!(seqs(&tail), (101..=150).collect::<Vec<_>>());
        assert!(!tail.has_more && !tail.gap);
        let empty = first.page(Some(150), 100).await.expect("read");
        assert!(empty.records.is_empty());
        assert_eq!(empty.next_after_seq, 150);

        let second = audit_log(&store, ConsoleAuditLimits::default());
        let row = second
            .append(entry("meta/reset"), 2_000)
            .await
            .expect("append");
        assert_eq!(
            row.seq, 151,
            "a restarted log numbers after the stored rows"
        );
        assert_eq!(row.fields["method"], json!("meta/reset"));
        assert_eq!(row.fields["outcome"], json!("committed"));
    }

    /// The count bound prunes whole pages, oldest first, never the page the
    /// next row goes to; a cursor before the oldest row kept reads from
    /// there and says so, and so does a cursor past the newest row.
    #[tokio::test]
    async fn pruned_rows_leave_a_gap_the_reader_is_told_of() {
        let store = Arc::new(InMemoryMetadataStore::new());
        let log = audit_log(
            &store,
            ConsoleAuditLimits::new(64, Duration::from_hours(365 * 24)),
        );
        append_rows(&log, 4 * ROWS_PER_PAGE + 1, now_ms()).await;

        let page = log.page(Some(3), 10).await.expect("read");
        assert!(page.gap, "rows 1 to 192 were pruned: {page:?}");
        assert_eq!(page.oldest_available_seq, Some(3 * ROWS_PER_PAGE + 1));
        assert_eq!(
            page.records.first().map(|row| row.seq),
            Some(3 * ROWS_PER_PAGE + 1)
        );
        let current = log.page(Some(4 * ROWS_PER_PAGE), 10).await.expect("read");
        assert!(!current.gap);
        assert_eq!(seqs(&current), vec![4 * ROWS_PER_PAGE + 1]);
        let future = log.page(Some(10_000), 10).await.expect("read");
        assert!(future.gap, "a cursor past the newest row resynchronizes");
        assert_eq!(
            future.records.first().map(|row| row.seq),
            Some(3 * ROWS_PER_PAGE + 1)
        );
    }

    /// The age bound prunes the pages whose newest row is older than it,
    /// and keeps every page from the first one that is not.
    #[tokio::test]
    async fn pages_older_than_the_age_bound_are_pruned() {
        let store = Arc::new(InMemoryMetadataStore::new());
        let log = audit_log(
            &store,
            ConsoleAuditLimits::new(20_000, Duration::from_hours(24)),
        );
        let now = 100 * DAY_MS;
        append_rows(&log, ROWS_PER_PAGE, now - 3 * DAY_MS).await;
        append_rows(&log, ROWS_PER_PAGE, now - 2 * DAY_MS).await;
        // The third page's last row closes it, and prunes as of now.
        append_rows(&log, ROWS_PER_PAGE, now).await;

        let page = log.page(None, 500).await.expect("read");
        assert_eq!(page.oldest_available_seq, Some(2 * ROWS_PER_PAGE + 1));
        assert_eq!(page.latest_seq, 3 * ROWS_PER_PAGE);
    }

    /// A row keeps the identifiers of a change and of its receipt, never its
    /// content, and an error is a refusal unless the owner failed.
    #[test]
    fn a_row_keeps_identifiers_and_the_outcome_only() {
        let target = audit_target(&json!({
            "realm": "meta",
            "identity": "beta",
            "memory_id": "mem-1",
            "title": "secret plan",
            "body": "the body",
            "labels": { "team": "ops" },
            "rule": { "id": "ops-rule", "subjects": ["a"] },
        }));
        assert_eq!(
            Value::Object(target.fields),
            json!({
                "realm": "meta",
                "identity": "beta",
                "memory_id": "mem-1",
                "rule_id": "ops-rule",
            })
        );
        let receipt = audit_receipt(&json!({
            "pending_id": "gate-pending-1",
            "decision": { "audit_token": "tok-1", "reason": "fine" },
            "summary": "text",
        }));
        assert_eq!(
            Value::Object(receipt.fields),
            json!({ "pending_id": "gate-pending-1", "audit_token": "tok-1" })
        );
        let actor = entry("meta/reset").actor;
        let refused = ConsoleAuditEntry::answered(
            actor.clone(),
            "meta/reset".to_string(),
            Identifiers::default(),
            &json!({ "error": { "code": -32040, "message": "no", "data": { "kind": "access_denied" } } }),
        );
        assert_eq!(refused.outcome, ConsoleAuditOutcome::Refused);
        assert_eq!(refused.code, Some(-32040));
        assert_eq!(refused.kind.as_deref(), Some("access_denied"));
        let failed = ConsoleAuditEntry::answered(
            actor,
            "meta/reset".to_string(),
            Identifiers::default(),
            &json!({ "error": { "code": -32603, "message": "store down" } }),
        );
        assert_eq!(failed.outcome, ConsoleAuditOutcome::Failed);
        assert!(failed.receipt.is_empty());
    }

    /// A checked access edit carries its fields in its `checked_v1`
    /// envelope: the row keeps the identifiers there and the revision the
    /// edit expected, never the owner instance or the configuration.
    #[test]
    fn a_checked_access_edit_keeps_the_identifiers_of_its_envelope() {
        let target = audit_target(&json!({
            "checked_v1": {
                "owner_instance": "0123abcd",
                "expected_revision": 4,
                "rule": { "id": "ops-view", "subjects": ["svc:robot"] },
            }
        }));
        assert_eq!(
            Value::Object(target.fields),
            json!({ "expected_revision": 4, "rule_id": "ops-view" })
        );
        let target = audit_target(&json!({
            "checked_v1": {
                "owner_instance": "0123abcd",
                "expected_revision": 5,
                "name": "ops",
                "group": { "members": ["svc:robot"] },
            }
        }));
        assert_eq!(
            Value::Object(target.fields),
            json!({ "expected_revision": 5, "name": "ops" })
        );
    }

    /// A row keeps a bounded part of whatever a caller sends: at most 16
    /// `id` fields, no key longer than 64 bytes and no value longer than 256
    /// bytes, in a row of at most 4 KiB, and it says when it left anything
    /// out.
    #[tokio::test]
    async fn a_row_keeps_a_bounded_target_and_says_it_was_cut() {
        let mut params = Map::new();
        for n in 0..5_000 {
            params.insert(format!("note{n}_id"), json!("x".repeat(1_000)));
        }
        params.insert(format!("{}_id", "k".repeat(65_536)), json!("free text"));
        let store = Arc::new(InMemoryMetadataStore::new());
        let log = audit_log(&store, ConsoleAuditLimits::default());
        let row = log
            .append(
                ConsoleAuditEntry::answered(
                    entry("meta/reset").actor,
                    "meta/reset".to_string(),
                    audit_target(&Value::Object(params)),
                    &json!({ "result": { "accepted": true } }),
                ),
                now_ms(),
            )
            .await
            .expect("append");
        let encoded = serde_json::to_string(&row).expect("encode");
        assert!(
            encoded.len() <= 4096,
            "the row takes {} bytes",
            encoded.len()
        );
        let target = row.fields["target"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        assert!(
            target.keys().filter(|key| key.ends_with("_id")).count() <= 16,
            "the row keeps {} fields",
            target.len()
        );
        assert!(target.keys().all(|key| key.len() <= 64));
        assert!(
            target
                .values()
                .all(|value| value.as_str().is_none_or(|text| text.len() <= 256))
        );
        assert_eq!(row.fields["truncated"], json!(true));
    }

    fn listing_log(
        store: &Arc<test_support::ListingStore>,
        limits: ConsoleAuditLimits,
    ) -> ConsoleAuditLog {
        ConsoleAuditLog::over(
            Arc::clone(store) as Arc<dyn PersistentMetadataStore>,
            limits,
        )
        .expect("the store keeps realm documents")
    }

    /// The age bound keeps pruning on a store that still lists the pages it
    /// removed, as a host store realm does in the process that removed them.
    #[tokio::test]
    async fn the_age_bound_holds_on_a_store_that_lists_removed_pages() {
        let store = Arc::new(test_support::ListingStore::default());
        let log = listing_log(
            &store,
            ConsoleAuditLimits::new(20_000, Duration::from_hours(24)),
        );
        let now = 100 * DAY_MS;
        append_rows(&log, ROWS_PER_PAGE, now - 10 * DAY_MS).await;
        // Closing the second page prunes the first.
        append_rows(&log, ROWS_PER_PAGE, now - 5 * DAY_MS).await;
        // Closing the third page prunes the second, past the first's key.
        append_rows(&log, ROWS_PER_PAGE, now - 2 * DAY_MS).await;

        let page = log.page(None, 500).await.expect("read");
        assert_eq!(page.oldest_available_seq, Some(2 * ROWS_PER_PAGE + 1));
        assert_eq!(page.latest_seq, 3 * ROWS_PER_PAGE);
    }

    /// The count bound removes each page once, on a store that still lists
    /// the pages it removed.
    #[tokio::test]
    async fn a_count_prune_removes_each_page_once() {
        let store = Arc::new(test_support::ListingStore::default());
        let log = listing_log(
            &store,
            ConsoleAuditLimits::new(ROWS_PER_PAGE, Duration::from_hours(365 * 24)),
        );
        append_rows(&log, 6 * ROWS_PER_PAGE, now_ms()).await;

        let removals = store.removals();
        assert_eq!(
            removals,
            (0..5)
                .map(|page| (page_key(page), 1))
                .collect::<std::collections::BTreeMap<_, _>>(),
            "pages 0 to 4 are removed, each once"
        );
        let page = log.page(None, 500).await.expect("read");
        assert_eq!(page.oldest_available_seq, Some(5 * ROWS_PER_PAGE + 1));
    }
}

/// Test support: a metadata store that keeps realm documents and lists every
/// key it ever wrote, removed ones included, as a host store realm does in
/// the process that removed them. It counts the removals of each key, and
/// can hold every realm document write for a while.
#[cfg(test)]
pub(crate) mod test_support {
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::{Mutex, PoisonError};
    use std::time::Duration;

    use crate::runtime::{MetadataStoreError, PersistentMetadataStore, RecordRetention};

    #[derive(Default)]
    pub(crate) struct ListingStore {
        documents: Mutex<BTreeMap<String, String>>,
        listed: Mutex<BTreeSet<String>>,
        removals: Mutex<BTreeMap<String, usize>>,
        write_delay: Option<Duration>,
    }

    impl ListingStore {
        /// A store whose every realm document write takes `delay`.
        pub(crate) fn holding_writes_for(delay: Duration) -> Self {
            Self {
                write_delay: Some(delay),
                ..Self::default()
            }
        }

        /// How often each key was removed.
        pub(crate) fn removals(&self) -> BTreeMap<String, usize> {
            self.removals
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    #[async_trait::async_trait]
    impl PersistentMetadataStore for ListingStore {
        async fn get_subscription_cursor(
            &self,
            _mob_id: &str,
        ) -> Result<Option<u64>, MetadataStoreError> {
            Ok(None)
        }
        async fn set_subscription_cursor(
            &self,
            _mob_id: &str,
            _cursor: u64,
        ) -> Result<(), MetadataStoreError> {
            Ok(())
        }
        async fn load_member_idle_retire_overrides(
            &self,
        ) -> Result<Vec<crate::MemberIdleRetireOverrideRecord>, MetadataStoreError> {
            Ok(Vec::new())
        }
        async fn set_member_idle_retire_override(
            &self,
            _record: &crate::MemberIdleRetireOverrideRecord,
        ) -> Result<(), MetadataStoreError> {
            Ok(())
        }
        async fn identity_tool_record(
            &self,
            _mob_id: &str,
            _identity: &str,
        ) -> Result<Option<crate::IdentityToolRecord>, MetadataStoreError> {
            Ok(None)
        }
        async fn set_identity_tool_record(
            &self,
            _record: &crate::IdentityToolRecord,
        ) -> Result<(), MetadataStoreError> {
            Ok(())
        }
        async fn load_identity_rotation_intents(
            &self,
            _mob_id: &str,
        ) -> Result<Vec<crate::IdentityRotationIntent>, MetadataStoreError> {
            Ok(Vec::new())
        }
        async fn set_identity_rotation_intent(
            &self,
            _intent: &crate::IdentityRotationIntent,
        ) -> Result<(), MetadataStoreError> {
            Ok(())
        }
        async fn clear_identity_rotation_intent(
            &self,
            _mob_id: &str,
            _identity: &str,
        ) -> Result<(), MetadataStoreError> {
            Ok(())
        }
        async fn lifecycle_projection_cursor(
            &self,
            _mob_id: &str,
        ) -> Result<Option<crate::LifecycleProjectionCursor>, MetadataStoreError> {
            Ok(None)
        }
        async fn set_lifecycle_projection_cursor(
            &self,
            _cursor: &crate::LifecycleProjectionCursor,
        ) -> Result<(), MetadataStoreError> {
            Ok(())
        }
        async fn lifecycle_effect_intent(
            &self,
            _mob_id: &str,
            _key: &crate::LifecycleIntentKey,
        ) -> Result<Option<crate::LifecycleEffectIntent>, MetadataStoreError> {
            Ok(None)
        }
        async fn set_lifecycle_effect_intent(
            &self,
            _intent: &crate::LifecycleEffectIntent,
        ) -> Result<(), MetadataStoreError> {
            Ok(())
        }
        async fn clear_lifecycle_effect_intent(
            &self,
            _mob_id: &str,
            _key: &crate::LifecycleIntentKey,
        ) -> Result<(), MetadataStoreError> {
            Ok(())
        }
        async fn lifecycle_effect_intents(
            &self,
            _mob_id: &str,
        ) -> Result<Vec<crate::LifecycleEffectIntent>, MetadataStoreError> {
            Ok(Vec::new())
        }
        async fn record_lifecycle_effect_failure(
            &self,
            _failure: &crate::LifecycleEffectFailure,
        ) -> Result<(), MetadataStoreError> {
            Ok(())
        }
        async fn lifecycle_effect_failures(
            &self,
            _mob_id: &str,
            _skip: usize,
            _limit: usize,
        ) -> Result<Vec<crate::LifecycleEffectFailure>, MetadataStoreError> {
            Ok(Vec::new())
        }
        async fn prune_lifecycle_effect_failures(
            &self,
            _mob_id: &str,
            _keep: usize,
        ) -> Result<usize, MetadataStoreError> {
            Ok(0)
        }
        fn realm_document_retention(&self) -> Option<RecordRetention> {
            Some(RecordRetention::Durable)
        }
        async fn realm_document(&self, key: &str) -> Result<Option<String>, MetadataStoreError> {
            Ok(self
                .documents
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(key)
                .cloned())
        }
        async fn set_realm_document(
            &self,
            key: &str,
            document: Option<&str>,
        ) -> Result<(), MetadataStoreError> {
            if let Some(delay) = self.write_delay {
                tokio::time::sleep(delay).await;
            }
            let mut documents = self
                .documents
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            match document {
                Some(document) => {
                    documents.insert(key.to_string(), document.to_string());
                    self.listed
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert(key.to_string());
                }
                None => {
                    documents.remove(key);
                    *self
                        .removals
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .entry(key.to_string())
                        .or_default() += 1;
                }
            }
            Ok(())
        }
        async fn realm_document_keys(
            &self,
            prefix: &str,
        ) -> Result<Vec<String>, MetadataStoreError> {
            Ok(self
                .listed
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .filter(|key| key.starts_with(prefix))
                .cloned()
                .collect())
        }
    }
}
