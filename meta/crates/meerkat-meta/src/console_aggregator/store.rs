use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use super::types::{
    AppendDisposition, AppendOutcome, ConsoleCursor, ConsoleDeltaCompaction, ConsoleFrame,
    ConsoleFrameSource, ConsoleFrameSourceKind, ConsoleFrameStatus, ConsoleTimelineMode,
    ConsoleTimelinePage, ConsoleTimelineQuery, ConsoleTimelineWindowPage,
    ConsoleTimelineWindowQuery, NewConsoleFrame, streamed_block_payload,
};
use super::{OperationServesTheDefaultMobAlone, SendState, SendTransition};
use crate::member_key::DefaultMob;

pub type ConsoleLogResult<T> = Result<T, ConsoleLogError>;

pub type ConsoleLogError = Box<dyn std::error::Error + Send + Sync>;

/// How long a store keeps the send dedupe records behind
/// [`ConsoleLogStore::frame_by_dedupe_key`]. A client may resend an
/// uncertain send with its original idempotency key only when the record
/// outlives a gateway restart; otherwise the resend can be admitted twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleSendDedupeDurability {
    /// Records persist across gateway restarts and are never expired.
    Durable,
    /// Records live only as long as this process (or are not known to
    /// outlive it).
    ProcessLifetime,
}

impl ConsoleSendDedupeDurability {
    #[must_use]
    pub fn is_durable(self) -> bool {
        matches!(self, Self::Durable)
    }
}

/// The frame sources that record an identity acting: live runtime events,
/// console sends and tool applications.
const ACTIVITY_SOURCE_KINDS: [ConsoleFrameSourceKind; 3] = [
    ConsoleFrameSourceKind::ConsoleEvent,
    ConsoleFrameSourceKind::Send,
    ConsoleFrameSourceKind::ToolApplication,
];

/// Live event kinds that record something done to an identity, not the
/// identity acting: operator lifecycle records and interaction failures.
const NON_ACTIVITY_EVENT_KINDS: [&str; 5] = [
    "identity_respawned",
    "identity_retired",
    "identity_reset",
    "identity_deleted",
    "interaction_failed",
];

/// The kind prefix of member memory events, which record the memory
/// pipeline's work rather than the member acting.
const MEMORY_EVENT_KIND_PREFIX: &str = "memory.";

fn is_activity_source(kind: &ConsoleFrameSourceKind) -> bool {
    ACTIVITY_SOURCE_KINDS.contains(kind)
}

/// Whether a frame records its identity acting, told apart by the frame's
/// source and kind columns alone (no payload is read). The one rule: the
/// SQL of [`latest_activity_sql`] is built from the same lists, and the
/// aggregator's in-memory last activity
/// ([`super::last_activity::LastActivityCache`]) records an appended frame
/// only when this says it acts.
pub(super) fn is_activity_frame(frame: &ConsoleFrame) -> bool {
    is_activity_source(&frame.source.kind)
        && !NON_ACTIVITY_EVENT_KINDS.contains(&frame.kind.as_str())
        && !frame.kind.starts_with(MEMORY_EVENT_KIND_PREFIX)
}

/// Live event kinds that record an identity leaving: retired or deleted. The
/// aggregator drops a departed identity's in-memory last activity
/// ([`super::last_activity::LastActivityCache`]), and its next read walks the
/// log again. Each is also a [`NON_ACTIVITY_EVENT_KINDS`] kind: a departure is
/// never the identity acting.
const DEPARTURE_EVENT_KINDS: [&str; 2] = ["identity_retired", "identity_deleted"];

/// Whether a frame records its identity leaving: a live runtime event of a
/// [`DEPARTURE_EVENT_KINDS`] kind.
pub(super) fn is_departure_frame(frame: &ConsoleFrame) -> bool {
    frame.source.kind == ConsoleFrameSourceKind::ConsoleEvent
        && DEPARTURE_EVENT_KINDS.contains(&frame.kind.as_str())
}

/// A comma-separated list of SQL string literals, for fixed values only.
fn sql_string_list<'a>(values: impl IntoIterator<Item = &'a str>) -> String {
    values
        .into_iter()
        .map(|value| format!("'{value}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A frame's `mob_id` column: empty for a member of the host's default mob,
/// the form every row stored before the column existed has.
fn mob_column(mob_id: Option<&str>) -> &str {
    mob_id.unwrap_or("")
}

/// The refusal of a provided operation that names a mob, when the mob it
/// names, as a stored frame names it, is a mob beside the host's default
/// mob ([`OperationServesTheDefaultMobAlone`]); none for the default mob (no
/// mob, or an empty id, as on a frame).
fn refusal_of_another_mob(
    mob_id: Option<&meerkat_mob::MobId>,
    operation: &'static str,
) -> Option<ConsoleLogError> {
    let mob = mob_id.filter(|mob_id| !mob_id.as_str().is_empty())?;
    Some(Box::new(OperationServesTheDefaultMobAlone {
        mob: mob.clone(),
        operation,
    }))
}

/// Whether the frame `frame` is of the mob a store keeps as `mob` (none
/// for the default mob).
fn frame_is_of_mob(frame: &ConsoleFrame, mob: Option<&str>) -> bool {
    mob_column(frame.mob_id.as_ref().map(meerkat_mob::MobId::as_str)) == mob_column(mob)
}

/// The mob a frame row names: none for an empty `mob_id` column.
fn mob_of_column(column: String) -> Option<meerkat_mob::MobId> {
    (!column.is_empty()).then(|| meerkat_mob::MobId::from(column))
}

/// Whether `frame` is of the member a read names by `mob_id` (none for the
/// default mob) and `identity`.
fn frame_is_of_member(frame: &ConsoleFrame, mob_id: Option<&str>, identity: &str) -> bool {
    frame.identity == identity
        && mob_column(frame.mob_id.as_ref().map(meerkat_mob::MobId::as_str)) == mob_column(mob_id)
}

/// Refuse a frame whose mob is only white space. Such a mob names no mob
/// and is not the default mob either (a frame of the default mob names
/// none): stored, it would be a second spelling of "no mob" that no read
/// finds, and it would move a SQLite log.
fn refuse_a_blank_frame_mob(frame: &NewConsoleFrame) -> ConsoleLogResult<()> {
    match frame.mob_id.as_ref().map(meerkat_mob::MobId::as_str) {
        Some(mob_id) if !mob_id.is_empty() && mob_id.trim().is_empty() => Err(boxed_error(
            "a console frame's mob is only white space: it names no mob",
        )),
        _ => Ok(()),
    }
}

/// The host's default mob as a console log store was told it
/// ([`ConsoleLogStore::bind_default_mob`]), and the one place a store reads
/// the mob a frame, a read or a compaction names.
#[derive(Default)]
struct BoundDefaultMob(std::sync::OnceLock<DefaultMob>);

impl BoundDefaultMob {
    fn bind(&self, default_mob: &DefaultMob) -> ConsoleLogResult<()> {
        if default_mob.mob_id().as_str().is_empty() {
            return Err(boxed_error(
                "a console log store is told its default mob by the mob's id; a blank id names no mob",
            ));
        }
        let bound = self.0.get_or_init(|| default_mob.clone());
        if bound == default_mob {
            Ok(())
        } else {
            Err(boxed_error(format!(
                "this console log store was told the default mob `{}` and cannot be told `{}`: \
                 a console log is one host's",
                bound.mob_id().as_str(),
                default_mob.mob_id().as_str()
            )))
        }
    }

    /// The mob a caller names, as the store keeps it: none for the default
    /// mob, whether the caller names no mob, a blank one or, once the store
    /// was told its default mob, that mob's id. So a member of the default
    /// mob has one stored key.
    fn stored<'a>(&self, mob_id: Option<&'a meerkat_mob::MobId>) -> Option<&'a str> {
        self.stored_id(mob_id.map(meerkat_mob::MobId::as_str))
    }

    fn stored_id<'a>(&self, mob_id: Option<&'a str>) -> Option<&'a str> {
        let mob_id = mob_id.filter(|mob_id| !mob_id.is_empty())?;
        let is_default = self
            .0
            .get()
            .is_some_and(|default_mob| default_mob.mob_id().as_str() == mob_id);
        (!is_default).then_some(mob_id)
    }

    /// The mob a removal of one mob's frames names, as the store keeps it.
    /// The default mob's frames are the host's own and are never removed: a
    /// blank id is refused, and so is the id of the default mob the store
    /// was told.
    fn removable<'a>(&self, mob_id: &'a meerkat_mob::MobId) -> ConsoleLogResult<&'a str> {
        self.stored(Some(mob_id)).ok_or_else(|| {
            boxed_error(
                "the frames of the host's default mob are not removed: a removal names another \
                 mob by its id",
            )
        })
    }
}

/// Refuse a console log that was moved past the newest schema version this
/// build reads. Each operation of the store that reads or writes the log
/// takes this look first, with the connection held, at the log's own ledger
/// row: a log that a later build moved on while this one holds it open is
/// refused as an open of it is, and the operation runs no statement. An
/// operation whose look fails fails; it never runs a statement on a guess.
fn refuse_a_later_version(conn: &Connection) -> ConsoleLogResult<()> {
    let version: i64 = conn
        .prepare_cached("SELECT version FROM meerkat_schema WHERE domain = ?1")
        .and_then(|mut stamp| stamp.query_row(params![META_CONSOLE_DOMAIN.name], |row| row.get(0)))
        .map_err(into_boxed)?;
    let supported = META_CONSOLE_DOMAIN.supported_version();
    if version > supported {
        return Err(into_boxed(
            meerkat_sqlite::SqliteStoreError::SchemaFromTheFuture {
                domain: META_CONSOLE_DOMAIN.name.to_string(),
                found: version,
                supported,
            },
        ));
    }
    Ok(())
}

/// The page read of a timeline query: the frames of a cursor range
/// (`?1`, `?2`), of one identity when `by_identity`, of one mob when
/// `by_mob`, of one conversation when `by_conversation`, then the row
/// limit. A member is its identity and its mob, never the name alone: a
/// read by identity is always by mob too. The read of one mob is by mob
/// without an identity. The statement names the frame's mob column, in its
/// mob term and as its last selected column, by [`with_member_sql`]'s
/// placeholder: the look at the log and the statement are one.
fn timeline_page_sql(
    by_identity: bool,
    by_mob: bool,
    by_conversation: bool,
    mode: &ConsoleTimelineMode,
) -> String {
    let mut sql = String::from(
        "SELECT cursor_seq, id, dedupe_key, timestamp_ms, runtime_key, identity,
                    conversation_id, session_id, kind, status, frame_version, updated_at_ms, payload_json,
                    source_kind, source_cursor, source_event_id, interaction_id,
                    parent_frame_id, caused_by_frame_id, turn_id, run_id, COALESCE(member_provenance_json,
                    (SELECT payload_json FROM console_member_provenance WHERE id = member_provenance_id)), member_provenance_id, update_marker_shares_provenance, {mob}
             FROM console_frames WHERE cursor_seq > ?1 AND cursor_seq < ?2",
    );
    let mut next_param = 3usize;
    if by_identity {
        sql.push_str(" AND identity = ?");
        sql.push_str(&next_param.to_string());
        next_param += 1;
    }
    if by_mob {
        // A page of a conversation alone finds its rows by the
        // conversation's index: the unary plus keeps the statement on it,
        // as on a log without the column, and checks the found rows' mob.
        sql.push_str(if by_conversation && !by_identity {
            " AND +{mob} = ?"
        } else {
            " AND {mob} = ?"
        });
        sql.push_str(&next_param.to_string());
        next_param += 1;
    }
    if by_conversation {
        sql.push_str(" AND conversation_id = ?");
        sql.push_str(&next_param.to_string());
        next_param += 1;
    }
    match mode {
        ConsoleTimelineMode::Since => sql.push_str(" ORDER BY cursor_seq ASC LIMIT ?"),
        ConsoleTimelineMode::Recent => sql.push_str(" ORDER BY cursor_seq DESC LIMIT ?"),
    }
    sql.push_str(&next_param.to_string());
    sql
}

/// One member's newest activity time: a walk of the member's own frames,
/// newest first, that stops at the first acting frame, never a scan of the
/// log. The walk reads `idx_console_frames_mob_identity_cursor` on a log
/// with the mob column and `idx_console_frames_identity_cursor` on a log
/// without it. It applies [`is_activity_frame`] in SQL, and names the
/// frame's mob column by [`with_member_sql`]'s placeholder.
fn latest_activity_sql() -> String {
    let sources = sql_string_list(
        ACTIVITY_SOURCE_KINDS
            .iter()
            .map(ConsoleFrameSourceKind::as_str),
    );
    let excluded = sql_string_list(NON_ACTIVITY_EVENT_KINDS);
    format!(
        "SELECT timestamp_ms FROM console_frames
         WHERE identity = ?1
           AND {{mob}} = ?2
           AND source_kind IN ({sources})
           AND kind NOT IN ({excluded})
           AND kind NOT GLOB '{MEMORY_EVENT_KIND_PREFIX}*'
         ORDER BY cursor_seq DESC LIMIT 1"
    )
}

/// Storage retains complete frames, including private member provenance, across
/// append, lookup, status changes and reopen. Serde frame serialization is the
/// storage representation; use aggregator projections for transport output.
#[async_trait::async_trait]
pub trait ConsoleLogStore: Send + Sync {
    async fn append_if_absent(&self, frame: NewConsoleFrame) -> ConsoleLogResult<AppendOutcome>;

    /// Set the status of the default mob's frame `frame_id`. The operation
    /// names no mob, so it changes a frame of the host's default mob alone:
    /// a frame of another mob is not found by it
    /// ([`Self::update_frame_status_of_mob`] changes one).
    async fn update_frame_status(
        &self,
        frame_id: &str,
        status: ConsoleFrameStatus,
    ) -> ConsoleLogResult<Option<ConsoleFrame>>;

    /// [`Self::update_frame_status`] for the frame `frame_id` of the mob
    /// `mob_id` (none for the host's default mob, as a stored frame names
    /// it): a frame of any other mob under that id is not found, and
    /// answers `None` with nothing changed.
    ///
    /// The provided form asks [`Self::update_frame_status`] for the default
    /// mob and refuses any other mob. A store that serves a mob beside the
    /// default mob ([`Self::reads_one_mob`]) implements it, as it does each
    /// of the five other operations that name a mob
    /// ([`Self::transition_send_frame_of_mob`],
    /// [`Self::take_send_frame_hold_of_mob`],
    /// [`Self::frame_by_dedupe_key_of_mob`],
    /// [`Self::send_frame_by_interaction_of_mob`],
    /// [`Self::unsettled_send_frames_of_mob`]); a store that wraps another
    /// forwards all six.
    async fn update_frame_status_of_mob(
        &self,
        mob_id: Option<&meerkat_mob::MobId>,
        frame_id: &str,
        status: ConsoleFrameStatus,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        if let Some(refusal) = refusal_of_another_mob(mob_id, "change a frame's status") {
            return Err(refusal);
        }
        self.update_frame_status(frame_id, status).await
    }

    /// Apply the send machine's `transition` to a send frame's own current
    /// state, atomically. When the machine accepts it from the state the
    /// frame's status records, the status becomes the next state's, `note`
    /// (a payload field and its value, such as the `delivery_hold` of a held
    /// dispatch) is recorded, and the updated frame is answered. A
    /// transition the machine refuses from that state, or a frame that is no
    /// send's, answers `None` and changes nothing: a hold is never recorded
    /// over a send that was submitted or settled. A store that cannot change
    /// a frame answers an error. The operation names no mob: it moves a
    /// send of the host's default mob alone
    /// ([`Self::transition_send_frame_of_mob`] moves one of another mob).
    async fn transition_send_frame(
        &self,
        frame_id: &str,
        transition: SendTransition,
        note: Option<(&'static str, serde_json::Value)>,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        let _ = (frame_id, transition, note);
        Err(boxed_error(
            "this console log store does not move a send through its states",
        ))
    }

    /// [`Self::transition_send_frame`] for the send frame `frame_id` of the
    /// mob `mob_id` (none for the host's default mob): a frame of any other
    /// mob under that id answers `None` and changes nothing. The provided
    /// form asks [`Self::transition_send_frame`] for the default mob and
    /// refuses any other mob ([`Self::update_frame_status_of_mob`] says who
    /// implements it).
    async fn transition_send_frame_of_mob(
        &self,
        mob_id: Option<&meerkat_mob::MobId>,
        frame_id: &str,
        transition: SendTransition,
        note: Option<(&'static str, serde_json::Value)>,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        if let Some(refusal) = refusal_of_another_mob(mob_id, "move a send through its states") {
            return Err(refusal);
        }
        self.transition_send_frame(frame_id, transition, note).await
    }

    /// Whether this store implements [`Self::transition_send_frame`] and
    /// [`Self::take_send_frame_hold`]: a governed Console send moves through
    /// both, so a Console whose store answers `false` refuses every governed
    /// send before anything is reserved. By default `false`; a store that
    /// implements both answers `true`, and a wrapper forwards all three.
    fn supports_send_transitions(&self) -> bool {
        false
    }

    /// Take a send frame's `delivery_hold`, as a compare-and-set on the
    /// hold: while the frame records one, clear it and answer the updated
    /// frame; when it records none (another dispatch took it, or none was
    /// recorded), answer `None` and change nothing. A dispatch again of a
    /// held send takes the hold first, so of concurrent takers exactly one
    /// gets it. A store that cannot change a frame's payload answers an
    /// error. The operation names no mob: it takes the hold of a send of the
    /// host's default mob alone ([`Self::take_send_frame_hold_of_mob`] takes
    /// one of another mob).
    async fn take_send_frame_hold(&self, frame_id: &str) -> ConsoleLogResult<Option<ConsoleFrame>> {
        let _ = frame_id;
        Err(boxed_error(
            "this console log store does not record a send's delivery hold",
        ))
    }

    /// [`Self::take_send_frame_hold`] for the send frame `frame_id` of the
    /// mob `mob_id` (none for the host's default mob): a frame of any other
    /// mob under that id answers `None` and keeps its hold. The provided
    /// form asks [`Self::take_send_frame_hold`] for the default mob and
    /// refuses any other mob ([`Self::update_frame_status_of_mob`] says who
    /// implements it).
    async fn take_send_frame_hold_of_mob(
        &self,
        mob_id: Option<&meerkat_mob::MobId>,
        frame_id: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        if let Some(refusal) = refusal_of_another_mob(mob_id, "take a send's delivery hold") {
            return Err(refusal);
        }
        self.take_send_frame_hold(frame_id).await
    }

    async fn query_frames(
        &self,
        query: ConsoleTimelineQuery,
    ) -> ConsoleLogResult<ConsoleTimelinePage>;

    async fn query_windowed_frames(
        &self,
        query: ConsoleTimelineWindowQuery,
    ) -> ConsoleLogResult<ConsoleTimelineWindowPage> {
        if query.mode != ConsoleTimelineMode::Since || query.before.is_some() {
            return Err(std::io::Error::other(
                "console log store must implement query_windowed_frames for v0.4 timeline windows",
            )
            .into());
        }
        let page = self
            .query_frames(ConsoleTimelineQuery {
                identity: query.identity,
                mob_id: query.mob_id,
                conversation_id: query.conversation_id,
                after: query.after,
                limit: query.limit,
            })
            .await?;
        Ok(ConsoleTimelineWindowPage {
            latest_cursor: page.next_cursor.clone(),
            exhausted: page.frames.is_empty(),
            frames: page.frames,
            next_cursor: page.next_cursor,
        })
    }

    /// [`Self::query_windowed_frames`] over the frames of ONE mob, whoever
    /// their member: the mob `query.mob_id` names (none for the host's
    /// default mob) selects with or without `query.identity`. The stores of
    /// this module answer the plain read the same way: a read that names no
    /// mob takes the default mob's frames alone, never the frames of every
    /// mob. A door that serves one mob reads its timeline with this method.
    /// A store that cannot answer it keeps the default, which refuses, and
    /// is never asked it: a host serves the default mob alone over such a
    /// store ([`Self::reads_one_mob`]). A store that wraps another must
    /// forward this method.
    async fn query_windowed_frames_of_mob(
        &self,
        query: ConsoleTimelineWindowQuery,
    ) -> ConsoleLogResult<ConsoleTimelineWindowPage> {
        let _ = query;
        Err(std::io::Error::other(
            "console log store must implement query_windowed_frames_of_mob to read one mob's frames",
        )
        .into())
    }

    /// Whether this store reads one mob's frames itself
    /// ([`Self::query_windowed_frames_of_mob`]). A store that keeps the
    /// default does not, and serves the host's default mob alone: the
    /// console events of a mob beside the default mob are not registered
    /// over it, so no frame of such a mob is handed to it, and a read of
    /// such a mob's timeline is refused, never answered as a page. The
    /// default mob's timeline is read from its plain pages, of which the
    /// reader keeps the frames that name no mob, so such a store is never
    /// asked the read it refuses. A store that wraps another and is to
    /// serve a mob beside the default mob forwards this method and
    /// [`Self::query_windowed_frames_of_mob`].
    fn reads_one_mob(&self) -> bool {
        false
    }

    /// The default mob's frame that holds `dedupe_key`. The read names no
    /// mob: a key only a frame of another mob holds answers none
    /// ([`Self::frame_by_dedupe_key_of_mob`] reads one).
    async fn frame_by_dedupe_key(&self, dedupe_key: &str)
    -> ConsoleLogResult<Option<ConsoleFrame>>;

    /// [`Self::frame_by_dedupe_key`] among the frames of the mob `mob_id`
    /// (none for the host's default mob): a key a frame of any other mob
    /// holds answers none. The provided form asks
    /// [`Self::frame_by_dedupe_key`] for the default mob and refuses any
    /// other mob ([`Self::update_frame_status_of_mob`] says who implements
    /// it).
    async fn frame_by_dedupe_key_of_mob(
        &self,
        mob_id: Option<&meerkat_mob::MobId>,
        dedupe_key: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        if let Some(refusal) = refusal_of_another_mob(mob_id, "read a frame by its key") {
            return Err(refusal);
        }
        self.frame_by_dedupe_key(dedupe_key).await
    }

    /// The send record of the console send whose interaction id is
    /// `interaction_id`: its `user_input` frame, read newest first, so a
    /// recent send is found after the frames written since. A send's
    /// interaction id is unique (it is derived from the send's dedupe key).
    /// Stores that cannot answer keep the default and report none: a reply
    /// to such a send then has no thread to go to. A store that wraps
    /// another must forward this method. The read names no mob: it answers
    /// a send of the host's default mob alone
    /// ([`Self::send_frame_by_interaction_of_mob`] reads one of another mob).
    async fn send_frame_by_interaction(
        &self,
        _interaction_id: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        Ok(None)
    }

    /// [`Self::send_frame_by_interaction`] among the sends of the mob
    /// `mob_id` (none for the host's default mob): a send of any other mob
    /// is not answered. The provided form asks
    /// [`Self::send_frame_by_interaction`] for the default mob and refuses
    /// any other mob ([`Self::update_frame_status_of_mob`] says who
    /// implements it).
    async fn send_frame_by_interaction_of_mob(
        &self,
        mob_id: Option<&meerkat_mob::MobId>,
        interaction_id: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        if let Some(refusal) = refusal_of_another_mob(mob_id, "read a send by its interaction") {
            return Err(refusal);
        }
        self.send_frame_by_interaction(interaction_id).await
    }

    /// Console sends of the host's default mob that have not settled
    /// (`user_input` frames of a send, still accepted, dispatching,
    /// submitted, or redacted at acceptance), oldest first. Bounded: only
    /// the newest `window` frames of the default mob are examined (another
    /// mob's frames are not counted in the window and never move a send
    /// out of it), and at most `limit` are returned. Stores that cannot
    /// answer keep the default and report none, so nothing is reconciled
    /// from them.
    async fn unsettled_send_frames(
        &self,
        _window: usize,
        _limit: usize,
    ) -> ConsoleLogResult<Vec<ConsoleFrame>> {
        Ok(Vec::new())
    }

    /// [`Self::unsettled_send_frames`] of the mob `mob_id` (none for the
    /// host's default mob): that mob's unsettled sends among the newest
    /// `window` of that mob's own frames, oldest first, at most `limit`.
    /// The provided form asks [`Self::unsettled_send_frames`] for the
    /// default mob and refuses any other mob
    /// ([`Self::update_frame_status_of_mob`] says who implements it).
    async fn unsettled_send_frames_of_mob(
        &self,
        mob_id: Option<&meerkat_mob::MobId>,
        window: usize,
        limit: usize,
    ) -> ConsoleLogResult<Vec<ConsoleFrame>> {
        if let Some(refusal) = refusal_of_another_mob(mob_id, "list a mob's unsettled sends") {
            return Err(refusal);
        }
        self.unsettled_send_frames(window, limit).await
    }

    /// The newest activity time of each named identity, in milliseconds since
    /// the epoch: the `timestamp_ms` of its newest frame that records the
    /// identity acting (a live runtime event, a console send or a tool
    /// application), however many of its frames are newer. Frames rebuilt
    /// from session history or synthesized by the console are stamped when the
    /// console observes them, so they do not count; nor do operator lifecycle
    /// records (respawn, retire, reset, delete), interaction failures and
    /// member memory events, which record something done to the identity. An
    /// identity without such a frame is absent. The read walks each named
    /// identity's frames newest first and stops at its first acting frame.
    /// The console aggregator reads an identity here on its first read, a
    /// chunk of identities per call, and again only after an operator's
    /// retire or delete, its capacity's eviction or a clear drops the
    /// identity's entry; in between it keeps the time in memory from the
    /// frames it appends. Stores that cannot answer keep the default and
    /// report none, so an identity's activity reads as unknown until the
    /// aggregator appends a frame of it acting. A store that wraps another
    /// must forward this method, or the console silently reads every agent's
    /// earlier activity as unknown. The identities are members of the host's
    /// default mob: a frame of a member of the same name in another mob
    /// never counts ([`Self::latest_activity_times_in_mob`] reads those).
    async fn latest_activity_times(
        &self,
        _identities: &[String],
    ) -> ConsoleLogResult<HashMap<String, u64>> {
        Ok(HashMap::new())
    }

    /// [`Self::latest_activity_times`] for identities of the mob `mob_id`,
    /// a mob other than the host's default mob, as the frames name it
    /// ([`ConsoleFrame::mob_id`]). A store that was told the host's default
    /// mob ([`Self::bind_default_mob`]) reads that mob's id as the default
    /// mob. Stores that cannot answer keep the default and report none.
    async fn latest_activity_times_in_mob(
        &self,
        _mob_id: &str,
        _identities: &[String],
    ) -> ConsoleLogResult<HashMap<String, u64>> {
        Ok(HashMap::new())
    }

    /// Whether the dedupe records above survive a gateway restart. Stores
    /// that cannot vouch for it keep the default, which never claims
    /// durability.
    fn send_dedupe_durability(&self) -> ConsoleSendDedupeDurability {
        ConsoleSendDedupeDurability::ProcessLifetime
    }

    async fn latest_cursor(&self) -> ConsoleLogResult<Option<ConsoleCursor>>;

    /// An opaque witness that existing frame provenance and cursor positions
    /// have not been replaced or removed. Appending rows may preserve it, and
    /// so may removing superseded delta rows
    /// ([`Self::compact_message_deltas`]): no prefix reuse depends on them.
    /// Stores without this guarantee return None, disabling prefix reuse.
    async fn history_prefix_revision(&self) -> ConsoleLogResult<Option<String>> {
        Ok(None)
    }

    /// Remove one streamed block's delta rows once the block's complete row
    /// is stored: the `kind`'s console-event delta rows of `runtime_key` and
    /// `identity` whose payload names `assistant_message_id` and carries a
    /// source sequence, after `first_delta` and before the `complete` row,
    /// and only while that complete row is stored in this store. The row at
    /// `first_delta` stays: it holds the block's place in the timeline. The
    /// other rows keep their cursors; nothing is renumbered. Returns how
    /// many rows were removed.
    ///
    /// Stores that cannot remove rows keep the default, which removes none:
    /// their logs keep every delta row, as before.
    async fn compact_message_deltas(
        &self,
        _compaction: &ConsoleDeltaCompaction,
    ) -> ConsoleLogResult<usize> {
        Ok(0)
    }

    /// Remove every frame of the mob `mob_id`, a mob other than the host's
    /// default mob, whoever their member, and answer how many went. A host
    /// calls it when that mob is destroyed, and again before a mob created
    /// under the same id serves: a mob created again under an id reads no
    /// frame of the mob that had the id before. Every other frame stays as
    /// it is and keeps its cursor; nothing is renumbered, and a cursor that
    /// was given out is never given to a later frame. A log that never
    /// stored a frame of another mob has none to remove: the answer is zero
    /// and the log is not moved.
    ///
    /// The default mob's frames are the host's own and no call removes
    /// them: a blank id is refused, and so is the id of the default mob the
    /// store was told ([`Self::bind_default_mob`]). A store that cannot
    /// remove a mob's frames keeps the default, which refuses. A store that
    /// wraps another must forward this method.
    async fn remove_frames_of_mob(&self, mob_id: &meerkat_mob::MobId) -> ConsoleLogResult<u64> {
        let _ = mob_id;
        Err(boxed_error(
            "console log store must implement remove_frames_of_mob to remove one mob's frames",
        ))
    }

    async fn clear_frames(&self) -> ConsoleLogResult<()>;

    async fn record_source_watermark(
        &self,
        runtime_key: &str,
        source_kind: ConsoleFrameSourceKind,
        source_cursor: &str,
    ) -> ConsoleLogResult<()>;

    async fn source_watermark(
        &self,
        runtime_key: &str,
        source_kind: ConsoleFrameSourceKind,
    ) -> ConsoleLogResult<Option<String>>;

    /// Tell the store the host's default mob: the mob its launch names
    /// ([`DefaultMob`]).
    ///
    /// The rule every caller of a store keeps, with or without this method:
    /// a member of the default mob is named with no mob
    /// ([`DefaultMob::stored`]), on a frame, a member read, a last-activity
    /// read and a delta compaction alike ([`ConsoleFrame::mob_id`]). That
    /// rule alone gives a member of the default mob one stored key in every
    /// store, also in one that cannot be told.
    ///
    /// A store that was told is the second line behind that rule. It reads
    /// the default mob's id as no mob on each of those paths, so a caller
    /// that names the default mob by its id still reaches the default mob's
    /// member, and a frame appended under the default mob's id is a frame
    /// of the default mob. Telling the same mob again changes nothing; a
    /// store that was told one mob refuses another, and it refuses a blank
    /// id. A store is told before it is given a frame: a frame it stored
    /// under the mob's id earlier is not rewritten, and it stays that id's:
    /// its key appended again after the telling, which then names the
    /// default mob, is refused as a key that a frame of another mob holds.
    ///
    /// A console log is one host's, so a store has one default mob. What is
    /// told is this handle's and is not kept in the log: the first mob a
    /// handle is told is the one it reads as no mob for the life of the
    /// process, a second handle on the same log is told separately, and the
    /// same mob must be told first on every start.
    ///
    /// An `Ok` answer means the store was told and reads the default mob's
    /// id as no mob. Stores that keep the default cannot be told and answer
    /// an error. Such a store rests on the callers' rule alone: it keeps a
    /// frame under the mob the frame names, so a caller that named the
    /// default mob by its id would give it a second key that no read of the
    /// default mob finds. A store that wraps another must forward this
    /// method.
    fn bind_default_mob(&self, default_mob: &DefaultMob) -> ConsoleLogResult<()> {
        let _ = default_mob;
        Err(boxed_error(
            "this console log store does not learn the host's default mob",
        ))
    }
}

pub struct InMemoryConsoleLogStore {
    state: Mutex<InMemoryState>,
    default_mob: BoundDefaultMob,
}

impl Default for InMemoryConsoleLogStore {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Default)]
struct InMemoryState {
    next_seq: u64,
    prefix_revision: uuid::Uuid,
    frames: BTreeMap<u64, ConsoleFrame>,
    dedupe_to_seq: HashMap<String, u64>,
    id_to_seq: HashMap<String, u64>,
    identity_to_seqs: HashMap<String, BTreeSet<u64>>,
    conversation_to_seqs: HashMap<String, BTreeSet<u64>>,
    watermarks: HashMap<(String, String), String>,
}

impl InMemoryState {
    /// The frame `frame_id` of the mob this store keeps as `mob` (none for
    /// the default mob), to change: a frame of another mob under the id is
    /// not found.
    fn frame_of_mob_mut(&mut self, mob: Option<&str>, frame_id: &str) -> Option<&mut ConsoleFrame> {
        let seq = self.id_to_seq.get(frame_id).copied()?;
        self.frames
            .get_mut(&seq)
            .filter(|frame| frame_is_of_mob(frame, mob))
    }
}

impl InMemoryConsoleLogStore {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(InMemoryState {
                next_seq: 1,
                prefix_revision: uuid::Uuid::new_v4(),
                frames: BTreeMap::new(),
                dedupe_to_seq: HashMap::new(),
                id_to_seq: HashMap::new(),
                identity_to_seqs: HashMap::new(),
                conversation_to_seqs: HashMap::new(),
                watermarks: HashMap::new(),
            }),
            default_mob: BoundDefaultMob::default(),
        }
    }
}

impl InMemoryConsoleLogStore {
    /// The newest activity time of each of `identities`, members of the mob
    /// `mob_id` names (none for the host's default mob): see
    /// [`ConsoleLogStore::latest_activity_times`].
    fn member_activity_times(
        &self,
        mob_id: Option<&str>,
        identities: &[String],
    ) -> ConsoleLogResult<HashMap<String, u64>> {
        let mut times = HashMap::new();
        for identity in identities {
            // The lock is held for one identity's walk at a time, as in
            // SQLite, so appends interleave with a large roster's read.
            let state = self
                .state
                .lock()
                .map_err(|_| boxed_error("console log lock poisoned"))?;
            let Some(seqs) = state.identity_to_seqs.get(identity) else {
                continue;
            };
            let newest = seqs
                .iter()
                .rev()
                .filter_map(|seq| state.frames.get(seq))
                .filter(|frame| frame_is_of_member(frame, mob_id, identity))
                .find(|frame| is_activity_frame(frame));
            if let Some(frame) = newest {
                times.insert(identity.clone(), frame.timestamp_ms);
            }
        }
        Ok(times)
    }
}

impl InMemoryConsoleLogStore {
    /// The window read of this store, for the plain read
    /// ([`ConsoleLogStore::query_windowed_frames`]) and the read of one mob
    /// ([`ConsoleLogStore::query_windowed_frames_of_mob`]) alike: every
    /// page is of the one mob the query names, the default mob when it
    /// names none, whoever the member; with an identity it is that
    /// member's page.
    fn windowed_frames(
        &self,
        query: ConsoleTimelineWindowQuery,
    ) -> ConsoleLogResult<ConsoleTimelineWindowPage> {
        let after_seq = query.after.as_ref().map(cursor_seq).transpose()?;
        let before_seq = query.before.as_ref().map(cursor_seq).transpose()?;
        let limit = normalize_limit(query.limit);
        let scan_limit = limit.saturating_add(1);
        let state = self
            .state
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        // The mob the query names, as this store keeps it: none for the
        // default mob, also when the query names it by the id the store was
        // told.
        let query_mob = self.default_mob.stored(query.mob_id.as_ref());
        let frame_matches = |seq: u64, frame: &ConsoleFrame| -> bool {
            if after_seq.is_some_and(|after| seq <= after) {
                return false;
            }
            if before_seq.is_some_and(|before| seq >= before) {
                return false;
            }
            // Every frame a page answers is of the mob the query names,
            // the default mob when it names none, whoever its member.
            if mob_column(frame.mob_id.as_ref().map(meerkat_mob::MobId::as_str))
                != mob_column(query_mob)
            {
                return false;
            }
            if let Some(identity) = query.identity.as_deref()
                && !frame_is_of_member(frame, query_mob, identity)
            {
                return false;
            }
            if let Some(conversation_id) = query.conversation_id.as_deref()
                && frame.conversation_id.as_deref() != Some(conversation_id)
            {
                return false;
            }
            true
        };
        // Every read walks a cursor range of the ordered indexes, so it starts
        // at the requested window instead of at the first frame ever stored.
        let Some(window) = in_memory_seq_window(after_seq, before_seq) else {
            return Ok(empty_window());
        };
        if let (Some(identity), Some(conversation_id)) =
            (query.identity.as_deref(), query.conversation_id.as_deref())
        {
            let identity_seqs = state.identity_to_seqs.get(identity);
            let conversation_seqs = state.conversation_to_seqs.get(conversation_id);
            return match (identity_seqs, conversation_seqs) {
                (Some(left), Some(right)) if left.len() <= right.len() => {
                    Ok(in_memory_window_from_seq_iters(
                        &state,
                        query.mode,
                        (limit, scan_limit),
                        left.range(window)
                            .copied()
                            .filter(|seq| right.contains(seq)),
                        left.range(window)
                            .rev()
                            .copied()
                            .filter(|seq| right.contains(seq)),
                        left.range(window)
                            .rev()
                            .copied()
                            .filter(|seq| right.contains(seq)),
                        &frame_matches,
                    ))
                }
                (Some(left), Some(right)) => Ok(in_memory_window_from_seq_iters(
                    &state,
                    query.mode,
                    (limit, scan_limit),
                    right
                        .range(window)
                        .copied()
                        .filter(|seq| left.contains(seq)),
                    right
                        .range(window)
                        .rev()
                        .copied()
                        .filter(|seq| left.contains(seq)),
                    right
                        .range(window)
                        .rev()
                        .copied()
                        .filter(|seq| left.contains(seq)),
                    &frame_matches,
                )),
                _ => Ok(empty_window()),
            };
        }
        if let Some(identity) = query.identity.as_deref() {
            let Some(seqs) = state.identity_to_seqs.get(identity) else {
                return Ok(empty_window());
            };
            return Ok(in_memory_window_from_seq_iters(
                &state,
                query.mode,
                (limit, scan_limit),
                seqs.range(window).copied(),
                seqs.range(window).rev().copied(),
                seqs.range(window).rev().copied(),
                &frame_matches,
            ));
        }
        if let Some(conversation_id) = query.conversation_id.as_deref() {
            let Some(seqs) = state.conversation_to_seqs.get(conversation_id) else {
                return Ok(empty_window());
            };
            return Ok(in_memory_window_from_seq_iters(
                &state,
                query.mode,
                (limit, scan_limit),
                seqs.range(window).copied(),
                seqs.range(window).rev().copied(),
                seqs.range(window).rev().copied(),
                &frame_matches,
            ));
        }
        Ok(in_memory_window_from_seq_iters(
            &state,
            query.mode,
            (limit, scan_limit),
            state.frames.range(window).map(|(seq, _)| *seq),
            state.frames.range(window).rev().map(|(seq, _)| *seq),
            state.frames.range(window).rev().map(|(seq, _)| *seq),
            &frame_matches,
        ))
    }
}

#[async_trait::async_trait]
impl ConsoleLogStore for InMemoryConsoleLogStore {
    fn send_dedupe_durability(&self) -> ConsoleSendDedupeDurability {
        ConsoleSendDedupeDurability::ProcessLifetime
    }

    fn supports_send_transitions(&self) -> bool {
        true
    }

    fn bind_default_mob(&self, default_mob: &DefaultMob) -> ConsoleLogResult<()> {
        self.default_mob.bind(default_mob)
    }

    async fn append_if_absent(&self, frame: NewConsoleFrame) -> ConsoleLogResult<AppendOutcome> {
        refuse_a_blank_frame_mob(&frame)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        if let Some(seq) = state.dedupe_to_seq.get(&frame.dedupe_key).copied()
            && let Some(existing) = state.frames.get(&seq)
        {
            // A key is one frame's in the whole log. The frame's own mob
            // finds its frame; an append of another mob under the key is
            // refused, as the SQLite store's unique index refuses it, and
            // is never answered another mob's frame.
            if existing.mob_id.as_ref().map(meerkat_mob::MobId::as_str)
                != self.default_mob.stored(frame.mob_id.as_ref())
            {
                return Err(boxed_error(
                    "the dedupe key is held by a frame of another mob",
                ));
            }
            return Ok(AppendOutcome {
                disposition: AppendDisposition::Existing,
                frame: existing.clone(),
            });
        }

        let id = frame
            .id
            .unwrap_or_else(|| stable_frame_id(&frame.dedupe_key));
        // A frame id is one frame's in the whole log too: an append under
        // an id a stored frame holds is refused, and takes no cursor.
        if state.id_to_seq.contains_key(&id) {
            return Err(boxed_error("the frame id is held by another frame"));
        }
        let seq = state.next_seq;
        state.next_seq = state.next_seq.saturating_add(1);
        let frame = ConsoleFrame {
            id: id.clone(),
            cursor: ConsoleCursor::from_seq(seq),
            dedupe_key: frame.dedupe_key,
            timestamp_ms: frame.timestamp_ms,
            runtime_key: frame.runtime_key,
            identity: frame.identity,
            mob_id: frame
                .mob_id
                .filter(|mob_id| self.default_mob.stored(Some(mob_id)).is_some()),
            conversation_id: frame.conversation_id,
            session_id: frame.session_id,
            kind: frame.kind,
            status: frame.status,
            frame_version: 1,
            updated_at_ms: None,
            payload: frame.payload,
            source: frame.source,
            source_event_id: frame.source_event_id,
            interaction_id: frame.interaction_id,
            turn_id: frame.turn_id,
            run_id: frame.run_id,
            parent_frame_id: frame.parent_frame_id,
            caused_by_frame_id: frame.caused_by_frame_id,
        };
        state.dedupe_to_seq.insert(frame.dedupe_key.clone(), seq);
        state.id_to_seq.insert(id, seq);
        state
            .identity_to_seqs
            .entry(frame.identity.clone())
            .or_default()
            .insert(seq);
        if let Some(conversation_id) = frame.conversation_id.as_ref() {
            state
                .conversation_to_seqs
                .entry(conversation_id.clone())
                .or_default()
                .insert(seq);
        }
        state.frames.insert(seq, frame.clone());
        Ok(AppendOutcome {
            disposition: AppendDisposition::Inserted,
            frame,
        })
    }

    async fn update_frame_status(
        &self,
        frame_id: &str,
        status: ConsoleFrameStatus,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.update_frame_status_of_mob(None, frame_id, status)
            .await
    }

    async fn update_frame_status_of_mob(
        &self,
        mob_id: Option<&meerkat_mob::MobId>,
        frame_id: &str,
        status: ConsoleFrameStatus,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        let mob = self.default_mob.stored(mob_id);
        let mut state = self
            .state
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        let Some(frame) = state.frame_of_mob_mut(mob, frame_id) else {
            return Ok(None);
        };
        frame.status = status;
        frame.frame_version = frame.frame_version.saturating_add(1);
        frame.updated_at_ms = Some(current_time_ms());
        Ok(Some(frame.clone()))
    }

    async fn transition_send_frame(
        &self,
        frame_id: &str,
        transition: SendTransition,
        note: Option<(&'static str, serde_json::Value)>,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.transition_send_frame_of_mob(None, frame_id, transition, note)
            .await
    }

    async fn transition_send_frame_of_mob(
        &self,
        mob_id: Option<&meerkat_mob::MobId>,
        frame_id: &str,
        transition: SendTransition,
        note: Option<(&'static str, serde_json::Value)>,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        let mob = self.default_mob.stored(mob_id);
        let mut state = self
            .state
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        let Some(frame) = state.frame_of_mob_mut(mob, frame_id) else {
            return Ok(None);
        };
        let Some(status) = send_frame_transition(frame.status, transition) else {
            return Ok(None);
        };
        if let Some((field, value)) = note {
            set_payload_field(&mut frame.payload, field, Some(value))?;
        }
        frame.status = status;
        frame.frame_version = frame.frame_version.saturating_add(1);
        frame.updated_at_ms = Some(current_time_ms());
        Ok(Some(frame.clone()))
    }

    async fn take_send_frame_hold(&self, frame_id: &str) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.take_send_frame_hold_of_mob(None, frame_id).await
    }

    async fn take_send_frame_hold_of_mob(
        &self,
        mob_id: Option<&meerkat_mob::MobId>,
        frame_id: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        let mob = self.default_mob.stored(mob_id);
        let mut state = self
            .state
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        let Some(frame) = state.frame_of_mob_mut(mob, frame_id) else {
            return Ok(None);
        };
        if frame.payload.get("delivery_hold").is_none() {
            return Ok(None);
        }
        set_payload_field(&mut frame.payload, "delivery_hold", None)?;
        frame.frame_version = frame.frame_version.saturating_add(1);
        frame.updated_at_ms = Some(current_time_ms());
        Ok(Some(frame.clone()))
    }

    async fn query_frames(
        &self,
        query: ConsoleTimelineQuery,
    ) -> ConsoleLogResult<ConsoleTimelinePage> {
        let page = self.query_windowed_frames(query.into()).await?;
        Ok(ConsoleTimelinePage {
            frames: page.frames,
            next_cursor: page.next_cursor,
        })
    }

    async fn query_windowed_frames(
        &self,
        query: ConsoleTimelineWindowQuery,
    ) -> ConsoleLogResult<ConsoleTimelineWindowPage> {
        self.windowed_frames(query)
    }

    async fn query_windowed_frames_of_mob(
        &self,
        query: ConsoleTimelineWindowQuery,
    ) -> ConsoleLogResult<ConsoleTimelineWindowPage> {
        self.windowed_frames(query)
    }

    fn reads_one_mob(&self) -> bool {
        true
    }

    async fn frame_by_dedupe_key(
        &self,
        dedupe_key: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.frame_by_dedupe_key_of_mob(None, dedupe_key).await
    }

    async fn frame_by_dedupe_key_of_mob(
        &self,
        mob_id: Option<&meerkat_mob::MobId>,
        dedupe_key: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        let mob = self.default_mob.stored(mob_id);
        let state = self
            .state
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        let Some(seq) = state.dedupe_to_seq.get(dedupe_key).copied() else {
            return Ok(None);
        };
        Ok(state
            .frames
            .get(&seq)
            .filter(|frame| frame_is_of_mob(frame, mob))
            .cloned())
    }

    async fn send_frame_by_interaction(
        &self,
        interaction_id: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.send_frame_by_interaction_of_mob(None, interaction_id)
            .await
    }

    async fn send_frame_by_interaction_of_mob(
        &self,
        mob_id: Option<&meerkat_mob::MobId>,
        interaction_id: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        let mob = self.default_mob.stored(mob_id);
        let state = self
            .state
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        Ok(state
            .frames
            .values()
            .rev()
            .find(|frame| frame_is_of_mob(frame, mob) && is_send_record_of(frame, interaction_id))
            .cloned())
    }

    async fn unsettled_send_frames(
        &self,
        window: usize,
        limit: usize,
    ) -> ConsoleLogResult<Vec<ConsoleFrame>> {
        self.unsettled_send_frames_of_mob(None, window, limit).await
    }

    async fn unsettled_send_frames_of_mob(
        &self,
        mob_id: Option<&meerkat_mob::MobId>,
        window: usize,
        limit: usize,
    ) -> ConsoleLogResult<Vec<ConsoleFrame>> {
        let mob = self.default_mob.stored(mob_id);
        let state = self
            .state
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        // The window is the newest `window` frames of the mob itself, as in
        // SQLite: another mob's frames are not counted in it.
        let mut in_window: Vec<&ConsoleFrame> = state
            .frames
            .values()
            .rev()
            .filter(|frame| frame_is_of_mob(frame, mob))
            .take(window)
            .collect();
        in_window.reverse();
        Ok(in_window
            .into_iter()
            .filter(|frame| is_unsettled_send_frame(frame))
            .take(limit)
            .cloned()
            .collect())
    }

    async fn latest_activity_times(
        &self,
        identities: &[String],
    ) -> ConsoleLogResult<HashMap<String, u64>> {
        self.member_activity_times(None, identities)
    }

    async fn latest_activity_times_in_mob(
        &self,
        mob_id: &str,
        identities: &[String],
    ) -> ConsoleLogResult<HashMap<String, u64>> {
        self.member_activity_times(self.default_mob.stored_id(Some(mob_id)), identities)
    }

    async fn compact_message_deltas(
        &self,
        compaction: &ConsoleDeltaCompaction,
    ) -> ConsoleLogResult<usize> {
        let first = cursor_seq(&compaction.first_delta)?;
        let complete = cursor_seq(&compaction.complete)?;
        if first >= complete {
            return Ok(0);
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        let member_mob = self.default_mob.stored(compaction.mob_id.as_ref());
        let block_row = |frame: &ConsoleFrame, kind: &str| {
            frame.runtime_key == compaction.runtime_key
                && frame_is_of_member(frame, member_mob, &compaction.identity)
                && frame.kind == kind
                && frame.source.kind == ConsoleFrameSourceKind::ConsoleEvent
                && streamed_block_payload(&frame.payload)
                    .is_some_and(|(message, _)| message == compaction.assistant_message_id.as_str())
        };
        if !state
            .frames
            .get(&complete)
            .is_some_and(|frame| block_row(frame, compaction.kind.complete_frame_kind()))
        {
            return Ok(0);
        }
        let removable: Vec<u64> = state
            .identity_to_seqs
            .get(&compaction.identity)
            .map(|seqs| {
                seqs.range((Bound::Excluded(first), Bound::Excluded(complete)))
                    .copied()
                    .filter(|seq| {
                        state.frames.get(seq).is_some_and(|frame| {
                            block_row(frame, compaction.kind.delta_frame_kind())
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        for seq in &removable {
            remove_in_memory_frame(&mut state, *seq);
        }
        Ok(removable.len())
    }

    async fn remove_frames_of_mob(&self, mob_id: &meerkat_mob::MobId) -> ConsoleLogResult<u64> {
        let mob = self.default_mob.removable(mob_id)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        let removable: Vec<u64> = state
            .frames
            .iter()
            .filter(|(_, frame)| {
                mob_column(frame.mob_id.as_ref().map(meerkat_mob::MobId::as_str)) == mob
            })
            .map(|(seq, _)| *seq)
            .collect();
        for seq in &removable {
            remove_in_memory_frame(&mut state, *seq);
        }
        // Frames with their member witnesses went: no prefix read before
        // stands. The next cursor is untouched, so none is given out again.
        if !removable.is_empty() {
            state.prefix_revision = uuid::Uuid::new_v4();
        }
        Ok(removable.len() as u64)
    }

    async fn latest_cursor(&self) -> ConsoleLogResult<Option<ConsoleCursor>> {
        let state = self
            .state
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        Ok(state
            .frames
            .keys()
            .next_back()
            .copied()
            .map(ConsoleCursor::from_seq))
    }

    async fn history_prefix_revision(&self) -> ConsoleLogResult<Option<String>> {
        let state = self
            .state
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        Ok(Some(state.prefix_revision.to_string()))
    }

    async fn clear_frames(&self) -> ConsoleLogResult<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        state.frames.clear();
        state.dedupe_to_seq.clear();
        state.id_to_seq.clear();
        state.identity_to_seqs.clear();
        state.conversation_to_seqs.clear();
        state.next_seq = 1;
        state.prefix_revision = uuid::Uuid::new_v4();
        Ok(())
    }

    async fn record_source_watermark(
        &self,
        runtime_key: &str,
        source_kind: ConsoleFrameSourceKind,
        source_cursor: &str,
    ) -> ConsoleLogResult<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        state.watermarks.insert(
            (runtime_key.to_string(), source_kind.as_str().to_string()),
            source_cursor.to_string(),
        );
        Ok(())
    }

    async fn source_watermark(
        &self,
        runtime_key: &str,
        source_kind: ConsoleFrameSourceKind,
    ) -> ConsoleLogResult<Option<String>> {
        let state = self
            .state
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        Ok(state
            .watermarks
            .get(&(runtime_key.to_string(), source_kind.as_str().to_string()))
            .cloned())
    }
}

/// Remove one frame and its index entries; the other frames keep their
/// sequences.
fn remove_in_memory_frame(state: &mut InMemoryState, seq: u64) {
    let Some(frame) = state.frames.remove(&seq) else {
        return;
    };
    state.dedupe_to_seq.remove(&frame.dedupe_key);
    state.id_to_seq.remove(&frame.id);
    if let Some(seqs) = state.identity_to_seqs.get_mut(&frame.identity) {
        seqs.remove(&seq);
        if seqs.is_empty() {
            state.identity_to_seqs.remove(&frame.identity);
        }
    }
    if let Some(conversation_id) = frame.conversation_id.as_ref()
        && let Some(seqs) = state.conversation_to_seqs.get_mut(conversation_id)
    {
        seqs.remove(&seq);
        if seqs.is_empty() {
            state.conversation_to_seqs.remove(conversation_id);
        }
    }
}

/// The sequences strictly after `after` and strictly before `before`, as a
/// range of the ordered indexes; `None` when no sequence lies between them.
fn in_memory_seq_window(
    after: Option<u64>,
    before: Option<u64>,
) -> Option<(Bound<u64>, Bound<u64>)> {
    if let (Some(after), Some(before)) = (after, before)
        && after.saturating_add(1) >= before
    {
        return None;
    }
    Some((
        after.map_or(Bound::Unbounded, Bound::Excluded),
        before.map_or(Bound::Unbounded, Bound::Excluded),
    ))
}

fn empty_window() -> ConsoleTimelineWindowPage {
    ConsoleTimelineWindowPage {
        frames: Vec::new(),
        next_cursor: None,
        latest_cursor: None,
        exhausted: true,
    }
}

fn in_memory_window_from_seq_iters<IForward, IReverse, ILatest, F>(
    state: &InMemoryState,
    mode: ConsoleTimelineMode,
    limits: (usize, usize),
    forward_iter: IForward,
    reverse_iter: IReverse,
    mut latest_iter: ILatest,
    frame_matches: &F,
) -> ConsoleTimelineWindowPage
where
    IForward: Iterator<Item = u64>,
    IReverse: Iterator<Item = u64>,
    ILatest: Iterator<Item = u64>,
    F: Fn(u64, &ConsoleFrame) -> bool,
{
    let (limit, scan_limit) = limits;
    let mut frames = match mode {
        ConsoleTimelineMode::Since => forward_iter
            .filter_map(|seq| {
                let frame = state.frames.get(&seq)?;
                frame_matches(seq, frame).then(|| frame.clone())
            })
            .take(scan_limit)
            .collect::<Vec<_>>(),
        ConsoleTimelineMode::Recent => {
            let mut frames = reverse_iter
                .filter_map(|seq| {
                    let frame = state.frames.get(&seq)?;
                    frame_matches(seq, frame).then(|| frame.clone())
                })
                .take(scan_limit)
                .collect::<Vec<_>>();
            frames.reverse();
            frames
        }
    };
    let exhausted = frames.len() <= limit;
    if frames.len() > limit {
        match mode {
            ConsoleTimelineMode::Since => frames.truncate(limit),
            ConsoleTimelineMode::Recent => {
                frames.remove(0);
            }
        }
    }
    let next_cursor = frames.last().map(|frame| frame.cursor.clone());
    let latest_cursor = match mode {
        ConsoleTimelineMode::Since => latest_iter.find_map(|seq| {
            let frame = state.frames.get(&seq)?;
            frame_matches(seq, frame).then(|| frame.cursor.clone())
        }),
        ConsoleTimelineMode::Recent => next_cursor.clone(),
    };
    ConsoleTimelineWindowPage {
        frames,
        next_cursor,
        latest_cursor,
        exhausted,
    }
}

pub struct SqliteConsoleLogStore {
    conn: Arc<Mutex<Connection>>,
    watermarks: Arc<Mutex<HashMap<(String, String), String>>>,
    prefix_revision: Mutex<uuid::Uuid>,
    /// Database file path; `:memory:` for in-memory stores (where the
    /// per-operation fence guard degrades to a no-op).
    db_path: PathBuf,
    /// Durable for a file-backed database, process-lifetime for
    /// [`SqliteConsoleLogStore::in_memory`].
    dedupe_durability: ConsoleSendDedupeDurability,
    default_mob: BoundDefaultMob,
}

/// The console log's schema domain while the log holds no frame of another
/// mob: schema versions 1 to 3, the domain of every build before frames
/// named a mob. A host that names no mob keeps its log here, so such a
/// build still opens it. Migration 0001 is the historical two-table +
/// two-index DDL.
const META_CONSOLE_UNKEYED_DOMAIN: meerkat_sqlite::SchemaDomain = meerkat_sqlite::SchemaDomain {
    name: "meta-console",
    migrations: &[
        meerkat_sqlite::Migration {
            version: 1,
            name: "base-schema",
            apply: migration_0001_console_schema,
        },
        meerkat_sqlite::Migration {
            version: 2,
            name: "member-provenance",
            apply: migration_0002_member_provenance,
        },
        meerkat_sqlite::Migration {
            version: 3,
            name: "normalized-member-provenance",
            apply: migration_0003_normalized_member_provenance,
        },
    ],
    initialize_current: initialize_v3_console_schema,
    allowed_existing_versions: &[1, 2, 3],
    // Unledgered meta files are refused at open (below the 0.8.8 ledger
    // floor) and meta never runs the offline bridge, so no source
    // version is inferable.
    bridge_recoverable_versions: &[],
    released_predecessors: &[
        meerkat_sqlite::SchemaPredecessor {
            version: 1,
            verify: verify_released_v1_console_schema,
        },
        meerkat_sqlite::SchemaPredecessor {
            version: 2,
            verify: verify_development_v2_console_schema,
        },
    ],
    owned_objects: &[
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "console_frames",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "console_source_watermarks",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "idx_console_frames_identity_cursor",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "idx_console_frames_conversation_cursor",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "console_member_provenance",
        },
    ],
    retired_objects: &[],
};

/// The console log's schema domain once the log stores a frame of a member
/// of another mob: a frame names its mob (`mob_id`, empty for the host's
/// default mob), an index by mob and identity takes the place of the index
/// by identity, and an index by mob keeps each mob's frames in cursor order
/// ([`migration_0004_frame_mob`]). A log that was moved is verified against
/// this domain when it is opened, and a log at a later version is refused.
/// Version 4 is the only version the domain accepts in a file, so it cannot
/// move a log: [`apply_console_domain`] gives it a log at version 4 or
/// above alone, and it would refuse a log below that. The move has a
/// declaration of its own ([`META_CONSOLE_MOVING_DOMAIN`]).
const META_CONSOLE_DOMAIN: meerkat_sqlite::SchemaDomain = meerkat_sqlite::SchemaDomain {
    name: "meta-console",
    migrations: &[
        meerkat_sqlite::Migration {
            version: 1,
            name: "base-schema",
            apply: migration_0001_console_schema,
        },
        meerkat_sqlite::Migration {
            version: 2,
            name: "member-provenance",
            apply: migration_0002_member_provenance,
        },
        meerkat_sqlite::Migration {
            version: 3,
            name: "normalized-member-provenance",
            apply: migration_0003_normalized_member_provenance,
        },
        meerkat_sqlite::Migration {
            version: 4,
            name: "frame-mob",
            apply: migration_0004_frame_mob,
        },
    ],
    initialize_current: initialize_v4_console_schema,
    allowed_existing_versions: &[4],
    // Unledgered meta files are refused at open (below the 0.8.8 ledger
    // floor) and meta never runs the offline bridge, so no source
    // version is inferable.
    bridge_recoverable_versions: &[],
    released_predecessors: &[],
    owned_objects: &[
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "console_frames",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "console_source_watermarks",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "idx_console_frames_conversation_cursor",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "console_member_provenance",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "idx_console_frames_mob_identity_cursor",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "idx_console_frames_mob_cursor",
        },
    ],
    // Version 4 reads a member by its mob and identity, never by the
    // identity alone: the member index takes the identity index's place.
    retired_objects: &[meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Index,
        name: "idx_console_frames_identity_cursor",
    }],
};

/// The move of a log to schema version 4: the version 4 domain with the
/// earlier versions among those it accepts in a file, each checked against
/// what its release wrote. [`ensure_mob_keyed`] hands a log to it when the
/// log is about to store the first frame of another mob, and nothing else
/// does: an open never does ([`apply_console_domain`]).
const META_CONSOLE_MOVING_DOMAIN: meerkat_sqlite::SchemaDomain = meerkat_sqlite::SchemaDomain {
    allowed_existing_versions: &[1, 2, 3, 4],
    released_predecessors: &[
        meerkat_sqlite::SchemaPredecessor {
            version: 1,
            verify: verify_released_v1_console_schema,
        },
        meerkat_sqlite::SchemaPredecessor {
            version: 2,
            verify: verify_development_v2_console_schema,
        },
        meerkat_sqlite::SchemaPredecessor {
            version: 3,
            verify: verify_released_v3_console_schema,
        },
    ],
    ..META_CONSOLE_DOMAIN
};

fn migration_0001_console_schema(tx: &rusqlite::Transaction<'_>) -> Result<(), rusqlite::Error> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS console_frames (
            cursor_seq INTEGER PRIMARY KEY AUTOINCREMENT,
            id TEXT NOT NULL UNIQUE,
            dedupe_key TEXT NOT NULL UNIQUE,
            timestamp_ms INTEGER NOT NULL,
            runtime_key TEXT NOT NULL,
            identity TEXT NOT NULL,
            conversation_id TEXT,
            session_id TEXT,
            kind TEXT NOT NULL,
            status TEXT NOT NULL,
            frame_version INTEGER NOT NULL DEFAULT 1,
            updated_at_ms INTEGER,
            payload_json TEXT NOT NULL,
            source_kind TEXT NOT NULL,
            source_cursor TEXT,
            source_event_id TEXT,
            interaction_id TEXT,
            parent_frame_id TEXT,
            caused_by_frame_id TEXT,
            turn_id TEXT,
            run_id TEXT
        );
        CREATE TABLE IF NOT EXISTS console_source_watermarks (
            runtime_key TEXT NOT NULL,
            source_kind TEXT NOT NULL,
            source_cursor TEXT NOT NULL,
            last_ingested_at_ms INTEGER NOT NULL,
            PRIMARY KEY(runtime_key, source_kind)
        );
        CREATE INDEX IF NOT EXISTS idx_console_frames_identity_cursor
            ON console_frames(identity, cursor_seq);
        CREATE INDEX IF NOT EXISTS idx_console_frames_conversation_cursor
            ON console_frames(conversation_id, cursor_seq);",
    )
}

fn migration_0002_member_provenance(tx: &rusqlite::Transaction<'_>) -> Result<(), rusqlite::Error> {
    tx.execute_batch("ALTER TABLE console_frames ADD COLUMN member_provenance_json TEXT")
}

fn initialize_v3_console_schema(tx: &rusqlite::Transaction<'_>) -> Result<(), rusqlite::Error> {
    initialize_v2_console_schema(tx)?;
    migration_0003_normalized_member_provenance(tx)
}

/// The schema of a log at version 4, built whole: what a log that was moved
/// there is verified against when it is opened. No log is created with it.
fn initialize_v4_console_schema(tx: &rusqlite::Transaction<'_>) -> Result<(), rusqlite::Error> {
    initialize_v3_console_schema(tx)?;
    migration_0004_frame_mob(tx)
}

/// The mob a frame's identity is a member of, beside `identity`: empty for a
/// member of the host's default mob. Every row stored before the column has
/// that value, so no row is rewritten and each reads as the default mob's.
/// The index is the member's own: a read of one member walks that member's
/// frames in cursor order, not the frames of every member of that name. It
/// takes the place of the identity index, which no read uses once every
/// read by identity names the mob (and which the query planner would
/// otherwise keep choosing, as the narrower of the two). The second index
/// is one mob's frames in cursor order, whoever their member: the read of
/// one mob with no identity walks it, where the member index alone would
/// make each page sort every frame of the mob.
///
/// The text of these statements is part of the stored form: a build opens
/// a log stamped with version 4 by comparing each object's stored text with
/// what its own copy of this migration produces. It does not change by a
/// character other than white space.
fn migration_0004_frame_mob(tx: &rusqlite::Transaction<'_>) -> Result<(), rusqlite::Error> {
    tx.execute_batch(
        "ALTER TABLE console_frames ADD COLUMN mob_id TEXT NOT NULL DEFAULT '';
         DROP INDEX idx_console_frames_identity_cursor;
         CREATE INDEX idx_console_frames_mob_identity_cursor
            ON console_frames(mob_id, identity, cursor_seq);
         CREATE INDEX idx_console_frames_mob_cursor
            ON console_frames(mob_id, cursor_seq);",
    )
}

fn initialize_v2_console_schema(tx: &rusqlite::Transaction<'_>) -> Result<(), rusqlite::Error> {
    migration_0001_console_schema(tx)?;
    migration_0002_member_provenance(tx)
}

fn migration_0003_normalized_member_provenance(
    tx: &rusqlite::Transaction<'_>,
) -> Result<(), rusqlite::Error> {
    tx.execute_batch("CREATE TABLE console_member_provenance (
        id INTEGER PRIMARY KEY, payload_json TEXT NOT NULL UNIQUE
    );
    ALTER TABLE console_frames ADD COLUMN member_provenance_id INTEGER;
    ALTER TABLE console_frames ADD COLUMN update_marker_shares_provenance INTEGER NOT NULL DEFAULT 0;")
}

fn verify_released_v1_console_schema(conn: &rusqlite::Connection) -> Result<(), String> {
    meerkat_sqlite::verify_released_schema_fingerprint(
        conn,
        &META_CONSOLE_UNKEYED_DOMAIN,
        &META_CONSOLE_UNKEYED_DOMAIN.owned_objects[..4],
        migration_0001_console_schema,
    )
}

fn verify_released_v3_console_schema(conn: &rusqlite::Connection) -> Result<(), String> {
    meerkat_sqlite::verify_released_schema_fingerprint(
        conn,
        &META_CONSOLE_UNKEYED_DOMAIN,
        META_CONSOLE_UNKEYED_DOMAIN.owned_objects,
        initialize_v3_console_schema,
    )
}

fn verify_development_v2_console_schema(conn: &rusqlite::Connection) -> Result<(), String> {
    meerkat_sqlite::verify_released_schema_fingerprint(
        conn,
        &META_CONSOLE_UNKEYED_DOMAIN,
        &META_CONSOLE_UNKEYED_DOMAIN.owned_objects[..4],
        initialize_v2_console_schema,
    )
}

/// Bring the console log behind `conn` to the schema it needs: the domain
/// without the frame's mob for a log that holds no frame of another mob (a
/// fresh log too), the domain with it for a log that was moved there. So
/// opening a log never moves it to schema version 4.
fn apply_console_domain(conn: &mut Connection) -> ConsoleLogResult<()> {
    let keyed = meerkat_sqlite::domain_version(conn, META_CONSOLE_DOMAIN.name)
        .map_err(into_boxed)?
        .is_some_and(|version| version > META_CONSOLE_UNKEYED_DOMAIN.supported_version());
    let domain = if keyed {
        &META_CONSOLE_DOMAIN
    } else {
        &META_CONSOLE_UNKEYED_DOMAIN
    };
    meerkat_sqlite::apply_domain_migrations(conn, domain).map_err(into_boxed)?;
    Ok(())
}

/// Whether the log behind `conn` has the frame's mob column (schema version
/// 4). Read from the schema itself on every use, so it holds whichever
/// connection moved the log.
fn mob_keyed(conn: &Connection) -> bool {
    let keyed = conn
        .prepare_cached("SELECT mob_id FROM console_frames LIMIT 0")
        .is_ok();
    #[cfg(test)]
    mob_check_probe::fire();
    keyed
}

/// A test's hook on [`mob_keyed`]: what another connection to the log does
/// right after this one read whether the log has the mob column, before the
/// statement shaped by that answer runs.
#[cfg(test)]
mod mob_check_probe {
    use std::cell::RefCell;

    type Probe = Box<dyn FnOnce()>;

    thread_local! {
        static ARMED: RefCell<Option<(usize, Probe)>> = const { RefCell::new(None) };
    }

    /// Run `probe` once, on this thread, after the check that follows
    /// `skip` further checks.
    pub(super) fn arm(skip: usize, probe: impl FnOnce() + 'static) {
        ARMED.with(|armed| *armed.borrow_mut() = Some((skip, Box::new(probe))));
    }

    /// Take the probe back; whether it had not run.
    pub(super) fn disarm() -> bool {
        ARMED.with(|armed| armed.borrow_mut().take().is_some())
    }

    pub(super) fn fire() {
        let due = ARMED.with(|armed| {
            let mut armed = armed.borrow_mut();
            match armed.as_mut() {
                Some((0, _)) => armed.take().map(|(_, probe)| probe),
                Some((skip, _)) => {
                    *skip -= 1;
                    None
                }
                None => None,
            }
        });
        if let Some(probe) = due {
            probe();
        }
    }
}

/// Move the log behind `conn` to schema version 4, if it is not there: the
/// log is about to store a frame of a member of another mob.
fn ensure_mob_keyed(conn: &mut Connection) -> ConsoleLogResult<()> {
    if !mob_keyed(conn) {
        meerkat_sqlite::apply_domain_migrations(conn, &META_CONSOLE_MOVING_DOMAIN)
            .map_err(into_boxed)?;
    }
    Ok(())
}

/// The newest cursor among the default mob's frames: what an operation
/// that names no mob reaches. It names the frame's mob by the placeholder
/// ([`with_member_sql`]): on a log without the column every frame is the
/// default mob's.
const NEWEST_OF_THE_DEFAULT_MOB: &str =
    "SELECT cursor_seq FROM console_frames WHERE {mob} = '' ORDER BY cursor_seq DESC LIMIT 1";

/// The frame's mob column as a statement names it on a log without the
/// column: the empty mob, the value every frame of such a log has, written
/// as an expression that names the column. While `console_frames` has no
/// `mob_id` the name resolves to the one row of `unkeyed`, and no frame is
/// read for it (`keyed` is the frames table cut to no row). Once the log
/// has the column the name is ambiguous between the two and the statement
/// does not compile. So the look at the log and the statement are one: a
/// statement shaped for a log without the column never runs on a log with
/// it, whichever connection moved the log and whenever.
const UNKEYED_MOB: &str = "(SELECT mob_id FROM (SELECT '' AS mob_id) AS unkeyed \
     LEFT JOIN (SELECT * FROM console_frames LIMIT 0) AS keyed)";

/// `sql` as a log takes it: one with the mob column when `keyed`. A
/// statement names the frame's mob column by the placeholder `{mob}`
/// (`{superseding.mob}` for the column of the row aliased `superseding`),
/// and nothing else in its text is touched. On a log with the column the
/// placeholder is the column. A log without it holds only frames of the
/// default mob, so the placeholder is the value every such frame has
/// ([`UNKEYED_MOB`]) and a read that names another mob finds nothing. A
/// statement that names the column outright does not run on a log without
/// it, and one rendered for a log without it does not run on a log with it.
fn render_member_sql(sql: &str, keyed: bool) -> String {
    let (mob, superseding_mob) = if keyed {
        ("mob_id", "superseding.mob_id")
    } else {
        (UNKEYED_MOB, UNKEYED_MOB)
    };
    sql.replace("{superseding.mob}", superseding_mob)
        .replace("{mob}", mob)
}

/// `sql` as the log behind `conn` takes it now ([`render_member_sql`]).
#[cfg(test)]
fn member_sql(conn: &Connection, sql: &str) -> String {
    render_member_sql(sql, mob_keyed(conn))
}

/// Run `statement` with `sql` as the log behind `conn` takes it
/// ([`render_member_sql`]). Another connection may move the log to schema
/// version 4 between this connection's look at it and the statement. The
/// text for a log without the mob column does not compile on a log with
/// it, so the statement then fails before it reads or changes a row, and it
/// is run again as the moved log takes it. A log never loses the column, so
/// that second run is the last.
fn with_member_sql<T>(
    conn: &Connection,
    sql: &str,
    statement: impl Fn(&str) -> rusqlite::Result<T>,
) -> rusqlite::Result<T> {
    let keyed = mob_keyed(conn);
    match statement(&render_member_sql(sql, keyed)) {
        Err(_) if !keyed && mob_keyed(conn) => statement(&render_member_sql(sql, true)),
        answer => answer,
    }
}

impl SqliteConsoleLogStore {
    pub fn open(path: impl AsRef<Path>) -> ConsoleLogResult<Self> {
        let path = path.as_ref().to_path_buf();
        let mut conn = meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::PRIMARY)
            .map_err(into_boxed)?;
        apply_console_domain(&mut conn)?;
        Self::from_connection(conn, path, ConsoleSendDedupeDurability::Durable)
    }

    pub fn in_memory() -> ConsoleLogResult<Self> {
        let mut conn = Connection::open_in_memory().map_err(into_boxed)?;
        apply_console_domain(&mut conn)?;
        Self::from_connection(
            conn,
            PathBuf::from(":memory:"),
            ConsoleSendDedupeDurability::ProcessLifetime,
        )
    }

    fn from_connection(
        conn: Connection,
        db_path: PathBuf,
        dedupe_durability: ConsoleSendDedupeDurability,
    ) -> ConsoleLogResult<Self> {
        let watermarks = load_source_watermarks(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            watermarks: Arc::new(Mutex::new(watermarks)),
            prefix_revision: Mutex::new(uuid::Uuid::new_v4()),
            db_path,
            dedupe_durability,
            default_mob: BoundDefaultMob::default(),
        })
    }

    /// Per-operation maintenance-fence guard: the connection is held for
    /// the store's lifetime, so the fence cannot ride the open — every
    /// database-touching operation takes its own shared guard.
    fn operation_fence(&self) -> ConsoleLogResult<meerkat_sqlite::OperationGuard> {
        meerkat_sqlite::OperationGuard::for_database(&self.db_path).map_err(into_boxed)
    }
}

impl SqliteConsoleLogStore {
    /// The newest activity time of each of `identities`, members of the mob
    /// `mob_id` names (none for the host's default mob): see
    /// [`ConsoleLogStore::latest_activity_times`].
    fn member_activity_times(
        &self,
        mob_id: Option<&str>,
        identities: &[String],
    ) -> ConsoleLogResult<HashMap<String, u64>> {
        if identities.is_empty() {
            return Ok(HashMap::new());
        }
        let sql = latest_activity_sql();
        let _fence = self.operation_fence()?;
        let mut times = HashMap::with_capacity(identities.len());
        for identity in identities {
            // The connection is held for one identity at a time, so frame
            // appends interleave with a large roster's read instead of
            // waiting for all of it.
            let timestamp: Option<i64> = {
                let conn = self
                    .conn
                    .lock()
                    .map_err(|_| boxed_error("console log lock poisoned"))?;
                refuse_a_later_version(&conn)?;
                with_member_sql(&conn, &sql, |sql| {
                    conn.prepare_cached(sql)?
                        .query_row(params![identity, mob_column(mob_id)], |row| row.get(0))
                        .optional()
                })
                .map_err(into_boxed)?
            };
            if let Some(timestamp) = timestamp.and_then(|value| u64::try_from(value).ok()) {
                times.insert(identity.clone(), timestamp);
            }
        }
        Ok(times)
    }
}

impl SqliteConsoleLogStore {
    /// The window read of this store, for the plain read
    /// ([`ConsoleLogStore::query_windowed_frames`]) and the read of one mob
    /// ([`ConsoleLogStore::query_windowed_frames_of_mob`]) alike: every
    /// page is of the one mob the query names, the default mob when it
    /// names none, whoever the member; with an identity it is that
    /// member's page.
    fn windowed_frames(
        &self,
        query: ConsoleTimelineWindowQuery,
    ) -> ConsoleLogResult<ConsoleTimelineWindowPage> {
        let after_seq = query.after.as_ref().map(cursor_seq_i64).transpose()?;
        let before_seq = query.before.as_ref().map(cursor_seq_i64).transpose()?;
        let limit = normalize_limit(query.limit);
        let scan_limit = limit.saturating_add(1);
        let _fence = self.operation_fence()?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        refuse_a_later_version(&conn)?;
        // Every page is one mob's: the mob the query names, the default mob
        // when it names none, as this store keeps it (none for the default
        // mob, also when the query names it by the id the store was told).
        // With an identity it is the member's page.
        let mob = mob_column(self.default_mob.stored(query.mob_id.as_ref()));
        let sql = timeline_page_sql(
            query.identity.is_some(),
            true,
            query.conversation_id.is_some(),
            &query.mode,
        );

        let after = after_seq.unwrap_or(0);
        let before = before_seq.unwrap_or(i64::MAX);
        let mut values = vec![
            rusqlite::types::Value::Integer(after),
            rusqlite::types::Value::Integer(before),
        ];
        if let Some(identity) = query.identity.as_ref() {
            values.push(rusqlite::types::Value::Text(identity.clone()));
        }
        values.push(rusqlite::types::Value::Text(mob.to_string()));
        if let Some(conversation_id) = query.conversation_id.as_ref() {
            values.push(rusqlite::types::Value::Text(conversation_id.clone()));
        }
        values.push(rusqlite::types::Value::Integer(scan_limit as i64));
        let mut frames = query_sql_frames(&conn, &sql, rusqlite::params_from_iter(values.iter()))?;
        if query.mode == ConsoleTimelineMode::Recent {
            frames.reverse();
        }
        let exhausted = frames.len() <= limit;
        if frames.len() > limit {
            match query.mode {
                ConsoleTimelineMode::Since => frames.truncate(limit),
                ConsoleTimelineMode::Recent => {
                    frames.remove(0);
                }
            }
        }
        let latest_cursor = latest_matching_cursor(
            &conn,
            after,
            before,
            query.identity.as_deref(),
            Some(mob),
            query.conversation_id.as_deref(),
        )?;
        let next_cursor = frames.last().map(|frame| frame.cursor.clone());
        Ok(ConsoleTimelineWindowPage {
            frames,
            next_cursor,
            latest_cursor,
            exhausted,
        })
    }
}

#[async_trait::async_trait]
impl ConsoleLogStore for SqliteConsoleLogStore {
    fn send_dedupe_durability(&self) -> ConsoleSendDedupeDurability {
        self.dedupe_durability
    }

    fn supports_send_transitions(&self) -> bool {
        true
    }

    fn bind_default_mob(&self, default_mob: &DefaultMob) -> ConsoleLogResult<()> {
        self.default_mob.bind(default_mob)
    }

    async fn append_if_absent(&self, frame: NewConsoleFrame) -> ConsoleLogResult<AppendOutcome> {
        refuse_a_blank_frame_mob(&frame)?;
        let _fence = self.operation_fence()?;
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        refuse_a_later_version(&conn)?;
        // The look for the key is among the frames of the frame's own mob. A
        // key that only a frame of another mob holds answers no frame here;
        // the insert below then fails on the key's unique index, and the
        // append is refused with nothing stored.
        if let Some(existing) = select_frame_by_dedupe(
            &conn,
            mob_column(self.default_mob.stored(frame.mob_id.as_ref())),
            &frame.dedupe_key,
        )? {
            return Ok(AppendOutcome {
                disposition: AppendDisposition::Existing,
                frame: existing,
            });
        }

        let id = frame
            .id
            .clone()
            .unwrap_or_else(|| stable_frame_id(&frame.dedupe_key));
        let mut payload = frame.payload.clone();
        let mut update_marker_shares_provenance = false;
        // Update markers share the outer witness. Hydrate the complete nested
        // frame on read, keeping the public store contract unchanged.
        if frame.kind == "frame_updated"
            && frame.source.member_provenance.is_some()
            && let Some(source) = payload
                .get_mut("frame")
                .and_then(|value| value.get_mut("source"))
            && source.get("member_provenance")
                == frame
                    .source
                    .member_provenance
                    .as_ref()
                    .and_then(|provenance| serde_json::to_value(provenance).ok())
                    .as_ref()
            && let Some(source) = source.as_object_mut()
        {
            source.remove("member_provenance");
            update_marker_shares_provenance = true;
        }
        let payload_json = serde_json::to_string(&payload).map_err(into_boxed)?;
        let member_provenance_json = frame
            .source
            .member_provenance
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(into_boxed)?;
        // A frame of a member of another mob needs the mob column: the
        // first one moves the log to schema version 4. Until then the log
        // stores frames of the default mob only, in the schema it had. A
        // frame under the default mob's id is a frame of the default mob.
        let frame_mob = mob_column(self.default_mob.stored(frame.mob_id.as_ref()));
        if !frame_mob.is_empty() {
            ensure_mob_keyed(&mut conn)?;
        }
        // The statement without the column stores only a frame of the
        // default mob, and such a frame is stored the same on a log another
        // connection moves before the statement runs: the column's default
        // is the empty mob.
        let (mob_name, mob_value) = if mob_keyed(&conn) {
            (", mob_id", ", ?21")
        } else {
            ("", " WHERE ?21 = ''")
        };
        let tx = conn.transaction().map_err(into_boxed)?;
        let member_provenance_id = if let Some(provenance) = member_provenance_json {
            tx.execute(
                "INSERT OR IGNORE INTO console_member_provenance(payload_json) VALUES (?1)",
                params![provenance],
            )
            .map_err(into_boxed)?;
            Some(
                tx.query_row(
                    "SELECT id FROM console_member_provenance WHERE payload_json = ?1",
                    params![provenance],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(into_boxed)?,
            )
        } else {
            None
        };
        let inserted = tx.execute(
            &format!(
                "INSERT INTO console_frames (
                id, dedupe_key, timestamp_ms, runtime_key, identity,
                conversation_id, session_id, kind, status, frame_version, updated_at_ms, payload_json,
                source_kind, source_cursor, source_event_id, interaction_id,
                parent_frame_id, caused_by_frame_id, turn_id, run_id, member_provenance_id, update_marker_shares_provenance{mob_name}
            ) SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, NULL, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20{mob_value}"
            ),
            params![
                id,
                frame.dedupe_key,
                frame.timestamp_ms as i64,
                frame.runtime_key,
                frame.identity,
                frame.conversation_id,
                frame.session_id,
                frame.kind,
                frame.status.as_str(),
                payload_json,
                frame.source.kind.as_str(),
                frame.source.source_cursor,
                frame.source_event_id,
                frame.interaction_id,
                frame.parent_frame_id,
                frame.caused_by_frame_id,
                frame.turn_id,
                frame.run_id,
                member_provenance_id,
                update_marker_shares_provenance,
                frame_mob,
            ],
        )
        .map_err(into_boxed)?;
        if inserted != 1 {
            return Err(boxed_error(
                "a console log without the mob column stores frames of the default mob only",
            ));
        }
        tx.commit().map_err(into_boxed)?;
        let inserted = select_frame_by_dedupe(&conn, frame_mob, &frame.dedupe_key)?
            .ok_or_else(|| boxed_error("inserted console frame was not readable"))?;
        Ok(AppendOutcome {
            disposition: AppendDisposition::Inserted,
            frame: inserted,
        })
    }

    async fn update_frame_status(
        &self,
        frame_id: &str,
        status: ConsoleFrameStatus,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.update_frame_status_of_mob(None, frame_id, status)
            .await
    }

    async fn update_frame_status_of_mob(
        &self,
        mob_id: Option<&meerkat_mob::MobId>,
        frame_id: &str,
        status: ConsoleFrameStatus,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        let mob = mob_column(self.default_mob.stored(mob_id));
        let _fence = self.operation_fence()?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        refuse_a_later_version(&conn)?;
        with_member_sql(
            &conn,
            "UPDATE console_frames SET status = ?1, frame_version = frame_version + 1, updated_at_ms = ?2 WHERE id = ?3 AND +{mob} = ?4",
            |sql| {
                conn.execute(
                    sql,
                    params![status.as_str(), current_time_ms() as i64, frame_id, mob],
                )
            },
        )
        .map_err(into_boxed)?;
        select_frame_by_id(&conn, mob, frame_id)
    }

    async fn transition_send_frame(
        &self,
        frame_id: &str,
        transition: SendTransition,
        note: Option<(&'static str, serde_json::Value)>,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.transition_send_frame_of_mob(None, frame_id, transition, note)
            .await
    }

    async fn transition_send_frame_of_mob(
        &self,
        mob_id: Option<&meerkat_mob::MobId>,
        frame_id: &str,
        transition: SendTransition,
        note: Option<(&'static str, serde_json::Value)>,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        let mob = mob_column(self.default_mob.stored(mob_id));
        let _fence = self.operation_fence()?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        refuse_a_later_version(&conn)?;
        let Some((payload_json, status)) = with_member_sql(
            &conn,
            "SELECT payload_json, status FROM console_frames WHERE id = ?1 AND +{mob} = ?2",
            |sql| {
                conn.query_row(sql, params![frame_id, mob], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .optional()
            },
        )
        .map_err(into_boxed)?
        else {
            return Ok(None);
        };
        let Some(next) = send_frame_transition(ConsoleFrameStatus::from_str(&status), transition)
        else {
            return Ok(None);
        };
        let mut payload: serde_json::Value =
            serde_json::from_str(&payload_json).map_err(into_boxed)?;
        if let Some((field, value)) = note {
            set_payload_field(&mut payload, field, Some(value))?;
        }
        let next_json = serde_json::to_string(&payload).map_err(into_boxed)?;
        // A compare-and-set on the state read above: a writer on another
        // connection that moved the send first leaves nothing to apply.
        let changed = with_member_sql(
            &conn,
            "UPDATE console_frames SET status = ?1, payload_json = ?2, frame_version = frame_version + 1, updated_at_ms = ?3 WHERE id = ?4 AND status = ?5 AND payload_json = ?6 AND +{mob} = ?7",
            |sql| {
                conn.execute(
                    sql,
                    params![
                        next.as_str(),
                        next_json,
                        current_time_ms() as i64,
                        frame_id,
                        status,
                        payload_json,
                        mob
                    ],
                )
            },
        )
        .map_err(into_boxed)?;
        if changed == 0 {
            return Ok(None);
        }
        select_frame_by_id(&conn, mob, frame_id)
    }

    async fn take_send_frame_hold(&self, frame_id: &str) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.take_send_frame_hold_of_mob(None, frame_id).await
    }

    async fn take_send_frame_hold_of_mob(
        &self,
        mob_id: Option<&meerkat_mob::MobId>,
        frame_id: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        let mob = mob_column(self.default_mob.stored(mob_id));
        let _fence = self.operation_fence()?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        refuse_a_later_version(&conn)?;
        let Some(held_json) = with_member_sql(
            &conn,
            "SELECT payload_json FROM console_frames WHERE id = ?1 AND +{mob} = ?2",
            |sql| {
                conn.query_row(sql, params![frame_id, mob], |row| row.get::<_, String>(0))
                    .optional()
            },
        )
        .map_err(into_boxed)?
        else {
            return Ok(None);
        };
        let mut payload: serde_json::Value =
            serde_json::from_str(&held_json).map_err(into_boxed)?;
        if payload.get("delivery_hold").is_none() {
            return Ok(None);
        }
        set_payload_field(&mut payload, "delivery_hold", None)?;
        let taken_json = serde_json::to_string(&payload).map_err(into_boxed)?;
        // The compare-and-set: the row changes only while it still holds the
        // payload read above, its hold included. A writer on another
        // connection that took or changed it first leaves nothing to take.
        let changed = with_member_sql(
            &conn,
            "UPDATE console_frames SET payload_json = ?1, frame_version = frame_version + 1, updated_at_ms = ?2 WHERE id = ?3 AND payload_json = ?4 AND +{mob} = ?5",
            |sql| {
                conn.execute(
                    sql,
                    params![
                        taken_json,
                        current_time_ms() as i64,
                        frame_id,
                        held_json,
                        mob
                    ],
                )
            },
        )
        .map_err(into_boxed)?;
        if changed == 0 {
            return Ok(None);
        }
        select_frame_by_id(&conn, mob, frame_id)
    }

    async fn query_frames(
        &self,
        query: ConsoleTimelineQuery,
    ) -> ConsoleLogResult<ConsoleTimelinePage> {
        let page = self.query_windowed_frames(query.into()).await?;
        Ok(ConsoleTimelinePage {
            frames: page.frames,
            next_cursor: page.next_cursor,
        })
    }

    async fn query_windowed_frames(
        &self,
        query: ConsoleTimelineWindowQuery,
    ) -> ConsoleLogResult<ConsoleTimelineWindowPage> {
        self.windowed_frames(query)
    }

    async fn query_windowed_frames_of_mob(
        &self,
        query: ConsoleTimelineWindowQuery,
    ) -> ConsoleLogResult<ConsoleTimelineWindowPage> {
        self.windowed_frames(query)
    }

    fn reads_one_mob(&self) -> bool {
        true
    }

    async fn frame_by_dedupe_key(
        &self,
        dedupe_key: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        let _fence = self.operation_fence()?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        refuse_a_later_version(&conn)?;
        // The read names no mob: the default mob's frame, or none.
        select_frame_by_dedupe(&conn, "", dedupe_key)
    }

    async fn frame_by_dedupe_key_of_mob(
        &self,
        mob_id: Option<&meerkat_mob::MobId>,
        dedupe_key: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        let mob = mob_column(self.default_mob.stored(mob_id));
        let _fence = self.operation_fence()?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        refuse_a_later_version(&conn)?;
        select_frame_by_dedupe(&conn, mob, dedupe_key)
    }

    async fn send_frame_by_interaction(
        &self,
        interaction_id: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.send_frame_by_interaction_of_mob(None, interaction_id)
            .await
    }

    async fn send_frame_by_interaction_of_mob(
        &self,
        mob_id: Option<&meerkat_mob::MobId>,
        interaction_id: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        let mob = mob_column(self.default_mob.stored(mob_id));
        let _fence = self.operation_fence()?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        refuse_a_later_version(&conn)?;
        // Newest first, down the rowid: a recent send is found after the
        // rows written since it, without reading older ones.
        let frames = query_sql_frames(
            &conn,
            "SELECT cursor_seq, id, dedupe_key, timestamp_ms, runtime_key, identity,
                    conversation_id, session_id, kind, status, frame_version, updated_at_ms, payload_json,
                    source_kind, source_cursor, source_event_id, interaction_id,
                    parent_frame_id, caused_by_frame_id, turn_id, run_id, COALESCE(member_provenance_json,
                    (SELECT payload_json FROM console_member_provenance WHERE id = member_provenance_id)), member_provenance_id, update_marker_shares_provenance, {mob}
             FROM console_frames
             WHERE kind = 'user_input' AND source_kind = 'send' AND interaction_id = ?1
               AND {mob} = ?2
             ORDER BY cursor_seq DESC LIMIT 1",
            params![interaction_id, mob],
        )?;
        Ok(frames
            .into_iter()
            .find(|frame| is_send_record_of(frame, interaction_id)))
    }

    async fn unsettled_send_frames(
        &self,
        window: usize,
        limit: usize,
    ) -> ConsoleLogResult<Vec<ConsoleFrame>> {
        self.unsettled_send_frames_of_mob(None, window, limit).await
    }

    async fn unsettled_send_frames_of_mob(
        &self,
        mob_id: Option<&meerkat_mob::MobId>,
        window: usize,
        limit: usize,
    ) -> ConsoleLogResult<Vec<ConsoleFrame>> {
        if window == 0 {
            return Ok(Vec::new());
        }
        let mob = mob_column(self.default_mob.stored(mob_id));
        // The oldest frame of the window is the `window`-th newest frame of
        // the mob: OFFSET counts from the newest.
        let window_offset = i64::try_from(window - 1).unwrap_or(i64::MAX);
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let _fence = self.operation_fence()?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        refuse_a_later_version(&conn)?;
        // The window is the newest `window` frames of the mob itself, read
        // down the mob's own cursor index: another mob's frames are not
        // counted in it, wherever they lie. A mob with fewer frames than
        // the window has every frame in it.
        let frames = query_sql_frames(
            &conn,
            "SELECT cursor_seq, id, dedupe_key, timestamp_ms, runtime_key, identity,
                    conversation_id, session_id, kind, status, frame_version, updated_at_ms, payload_json,
                    source_kind, source_cursor, source_event_id, interaction_id,
                    parent_frame_id, caused_by_frame_id, turn_id, run_id, COALESCE(member_provenance_json,
                    (SELECT payload_json FROM console_member_provenance WHERE id = member_provenance_id)), member_provenance_id, update_marker_shares_provenance, {mob}
             FROM console_frames
             WHERE cursor_seq >= COALESCE((SELECT cursor_seq FROM console_frames WHERE {mob} = ?3
                                           ORDER BY cursor_seq DESC LIMIT 1 OFFSET ?1), 0)
               AND kind = 'user_input' AND source_kind = 'send'
               AND status IN ('accepted', 'dispatching', 'submitted', 'redacted')
               AND {mob} = ?3
             ORDER BY cursor_seq ASC LIMIT ?2",
            params![window_offset, limit, mob],
        )?;
        Ok(frames.into_iter().filter(is_unsettled_send_frame).collect())
    }

    async fn latest_activity_times(
        &self,
        identities: &[String],
    ) -> ConsoleLogResult<HashMap<String, u64>> {
        self.member_activity_times(None, identities)
    }

    async fn latest_activity_times_in_mob(
        &self,
        mob_id: &str,
        identities: &[String],
    ) -> ConsoleLogResult<HashMap<String, u64>> {
        self.member_activity_times(self.default_mob.stored_id(Some(mob_id)), identities)
    }

    async fn latest_cursor(&self) -> ConsoleLogResult<Option<ConsoleCursor>> {
        let _fence = self.operation_fence()?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        refuse_a_later_version(&conn)?;
        // The default mob's newest frame: what a read that names no mob
        // reaches.
        let seq: Option<i64> = with_member_sql(&conn, NEWEST_OF_THE_DEFAULT_MOB, |sql| {
            conn.query_row(sql, [], |row| row.get(0)).optional()
        })
        .map_err(into_boxed)?;
        Ok(seq.map(|value| ConsoleCursor::from_seq(value as u64)))
    }

    async fn history_prefix_revision(&self) -> ConsoleLogResult<Option<String>> {
        let _fence = self.operation_fence()?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        // Other connections can replace an old prefix without changing its
        // final cursor. Conservatively invalidate for any external commit.
        let external_revision: i64 = conn
            .query_row("PRAGMA data_version", [], |row| row.get(0))
            .map_err(into_boxed)?;
        let local_revision = self
            .prefix_revision
            .lock()
            .map_err(|_| boxed_error("console prefix revision lock poisoned"))?;
        Ok(Some(format!("{local_revision}:{external_revision}")))
    }

    async fn compact_message_deltas(
        &self,
        compaction: &ConsoleDeltaCompaction,
    ) -> ConsoleLogResult<usize> {
        let first = cursor_seq_i64(&compaction.first_delta)?;
        let complete = cursor_seq_i64(&compaction.complete)?;
        if first >= complete {
            return Ok(0);
        }
        let _fence = self.operation_fence()?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        refuse_a_later_version(&conn)?;
        // One statement: the rows go only while the complete row is stored,
        // and the cursor range keeps the scan to the block's own span. The
        // first delta row stays. A row takes part only when its payload
        // carries the message id and a source sequence, as the projection
        // requires (`streamed_block_payload`).
        with_member_sql(
            &conn,
            "DELETE FROM console_frames
             WHERE cursor_seq > ?1 AND cursor_seq < ?2
               AND runtime_key = ?3 AND identity = ?4 AND {mob} = ?9 AND kind = ?5
               AND source_kind = ?6
               AND json_extract(payload_json, '$.assistant_message_id') = ?7
               AND json_type(payload_json, '$.source_sequence') = 'integer'
               AND json_extract(payload_json, '$.source_sequence') >= 0
               AND EXISTS (
                   SELECT 1 FROM console_frames AS superseding
                   WHERE superseding.cursor_seq = ?2
                     AND superseding.runtime_key = ?3 AND superseding.identity = ?4
                     AND {superseding.mob} = ?9
                     AND superseding.kind = ?8 AND superseding.source_kind = ?6
                     AND json_extract(superseding.payload_json, '$.assistant_message_id') = ?7
                     AND json_type(superseding.payload_json, '$.source_sequence') = 'integer'
                     AND json_extract(superseding.payload_json, '$.source_sequence') >= 0
               )",
            |sql| {
                conn.execute(
                    sql,
                    params![
                        first,
                        complete,
                        compaction.runtime_key,
                        compaction.identity,
                        compaction.kind.delta_frame_kind(),
                        ConsoleFrameSourceKind::ConsoleEvent.as_str(),
                        compaction.assistant_message_id,
                        compaction.kind.complete_frame_kind(),
                        mob_column(self.default_mob.stored(compaction.mob_id.as_ref())),
                    ],
                )
            },
        )
        .map_err(into_boxed)
    }

    async fn remove_frames_of_mob(&self, mob_id: &meerkat_mob::MobId) -> ConsoleLogResult<u64> {
        let mob = self.default_mob.removable(mob_id)?;
        let _fence = self.operation_fence()?;
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        refuse_a_later_version(&conn)?;
        let tx = conn.transaction().map_err(into_boxed)?;
        // The statement names the mob column by the placeholder. A log
        // without the column holds no frame of another mob: the placeholder
        // is the empty mob there, so the statement removes nothing and the
        // log is not moved. The cursor sequence is the table's own and is
        // not touched: no cursor is given out again.
        let removed = with_member_sql(&tx, "DELETE FROM console_frames WHERE {mob} = ?1", |sql| {
            tx.execute(sql, params![mob])
        })
        .map_err(into_boxed)?;
        if removed > 0 {
            // The member records only the removed frames named go with
            // them.
            tx.execute(
                "DELETE FROM console_member_provenance
                 WHERE id NOT IN (
                     SELECT member_provenance_id FROM console_frames
                     WHERE member_provenance_id IS NOT NULL
                 )",
                [],
            )
            .map_err(into_boxed)?;
        }
        tx.commit().map_err(into_boxed)?;
        // Frames with their member witnesses went: no prefix read before
        // stands.
        if removed > 0 {
            *self
                .prefix_revision
                .lock()
                .map_err(|_| boxed_error("console prefix revision lock poisoned"))? =
                uuid::Uuid::new_v4();
        }
        Ok(removed as u64)
    }

    async fn clear_frames(&self) -> ConsoleLogResult<()> {
        let _fence = self.operation_fence()?;
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        refuse_a_later_version(&conn)?;
        let tx = conn.transaction().map_err(into_boxed)?;
        // The clear names no mob: the default mob's frames go, and the
        // member records no frame names any more. Another mob's frames stay
        // with the records they name. On a log without the mob column every
        // frame is the default mob's.
        with_member_sql(&tx, "DELETE FROM console_frames WHERE {mob} = ''", |sql| {
            tx.execute(sql, [])
        })
        .map_err(into_boxed)?;
        tx.execute(
            "DELETE FROM console_member_provenance WHERE id NOT IN (
                 SELECT member_provenance_id FROM console_frames
                 WHERE member_provenance_id IS NOT NULL
             )",
            [],
        )
        .map_err(into_boxed)?;
        if !mob_keyed(&tx) {
            // A log that names no mob is empty now and its cursor sequence
            // starts again, as before. On a log that names mobs the sequence
            // stays: a later frame is stored past every cursor the log has
            // given out. The delete above holds the log for this
            // transaction, so no other connection moves it before this look.
            tx.execute(
                "DELETE FROM sqlite_sequence WHERE name = 'console_frames'",
                [],
            )
            .ok();
        }
        tx.commit().map_err(into_boxed)?;
        *self
            .prefix_revision
            .lock()
            .map_err(|_| boxed_error("console prefix revision lock poisoned"))? =
            uuid::Uuid::new_v4();
        Ok(())
    }

    async fn record_source_watermark(
        &self,
        runtime_key: &str,
        source_kind: ConsoleFrameSourceKind,
        source_cursor: &str,
    ) -> ConsoleLogResult<()> {
        let _fence = self.operation_fence()?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| boxed_error("console log lock poisoned"))?;
        refuse_a_later_version(&conn)?;
        conn.execute(
            "INSERT INTO console_source_watermarks (
                runtime_key, source_kind, source_cursor, last_ingested_at_ms
            ) VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(runtime_key, source_kind) DO UPDATE SET
                source_cursor = excluded.source_cursor,
                last_ingested_at_ms = excluded.last_ingested_at_ms",
            params![
                runtime_key,
                source_kind.as_str(),
                source_cursor,
                current_time_ms() as i64,
            ],
        )
        .map_err(into_boxed)?;
        self.watermarks
            .lock()
            .map_err(|_| boxed_error("console watermark lock poisoned"))?
            .insert(
                (runtime_key.to_string(), source_kind.as_str().to_string()),
                source_cursor.to_string(),
            );
        Ok(())
    }

    async fn source_watermark(
        &self,
        runtime_key: &str,
        source_kind: ConsoleFrameSourceKind,
    ) -> ConsoleLogResult<Option<String>> {
        let watermarks = self
            .watermarks
            .lock()
            .map_err(|_| boxed_error("console watermark lock poisoned"))?;
        Ok(watermarks
            .get(&(runtime_key.to_string(), source_kind.as_str().to_string()))
            .cloned())
    }
}

fn load_source_watermarks(
    conn: &Connection,
) -> ConsoleLogResult<HashMap<(String, String), String>> {
    let mut stmt = conn
        .prepare(
            "SELECT runtime_key, source_kind, source_cursor
             FROM console_source_watermarks",
        )
        .map_err(into_boxed)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                (row.get::<_, String>(0)?, row.get::<_, String>(1)?),
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(into_boxed)?;
    let mut watermarks = HashMap::new();
    for row in rows {
        let (key, cursor) = row.map_err(into_boxed)?;
        watermarks.insert(key, cursor);
    }
    Ok(watermarks)
}

fn query_sql_frames<P: rusqlite::Params + Clone>(
    conn: &Connection,
    sql: &str,
    params: P,
) -> ConsoleLogResult<Vec<ConsoleFrame>> {
    with_member_sql(conn, sql, |sql| {
        let mut statement = conn.prepare(sql)?;
        statement.query_map(params.clone(), row_to_frame)?.collect()
    })
    .map_err(into_boxed)
}

/// The frame that holds `dedupe_key` among the frames of the mob whose
/// `mob_id` column is `mob` (empty: the default mob). A key that only a
/// frame of another mob holds answers no frame. The unary plus keeps the
/// statement on the key's own index and checks the found row's mob.
fn select_frame_by_dedupe(
    conn: &Connection,
    mob: &str,
    dedupe_key: &str,
) -> ConsoleLogResult<Option<ConsoleFrame>> {
    with_member_sql(
        conn,
        "SELECT cursor_seq, id, dedupe_key, timestamp_ms, runtime_key, identity,
                conversation_id, session_id, kind, status, frame_version, updated_at_ms, payload_json,
                source_kind, source_cursor, source_event_id, interaction_id,
                parent_frame_id, caused_by_frame_id, turn_id, run_id, COALESCE(member_provenance_json,
                    (SELECT payload_json FROM console_member_provenance WHERE id = member_provenance_id)), member_provenance_id, update_marker_shares_provenance, {mob}
         FROM console_frames WHERE dedupe_key = ?1 AND +{mob} = ?2",
        |sql| {
            conn.query_row(sql, params![dedupe_key, mob], row_to_frame)
                .optional()
        },
    )
    .map_err(into_boxed)
}

/// The frame with the id `id` among the frames of the mob whose `mob_id`
/// column is `mob` (empty: the default mob). An operation by id reads the
/// mob it names alone: a frame of another mob is not found by it, and so is
/// never read or changed through it.
fn select_frame_by_id(
    conn: &Connection,
    mob: &str,
    id: &str,
) -> ConsoleLogResult<Option<ConsoleFrame>> {
    with_member_sql(
        conn,
        "SELECT cursor_seq, id, dedupe_key, timestamp_ms, runtime_key, identity,
                conversation_id, session_id, kind, status, frame_version, updated_at_ms, payload_json,
                source_kind, source_cursor, source_event_id, interaction_id,
                parent_frame_id, caused_by_frame_id, turn_id, run_id, COALESCE(member_provenance_json,
                    (SELECT payload_json FROM console_member_provenance WHERE id = member_provenance_id)), member_provenance_id, update_marker_shares_provenance, {mob}
         FROM console_frames WHERE id = ?1 AND +{mob} = ?2",
        |sql| conn.query_row(sql, params![id, mob], row_to_frame).optional(),
    )
    .map_err(into_boxed)
}

/// The newest cursor in the range of the rows a read names: of `identity`
/// when the read names one, of the mob whose `mob_id` column is `mob` when
/// the read is by mob (a member's read, or the read of one mob), of
/// `conversation_id` when it names one. The statement names the column by
/// [`with_member_sql`]'s placeholder.
fn latest_matching_cursor(
    conn: &Connection,
    after: i64,
    before: i64,
    identity: Option<&str>,
    mob: Option<&str>,
    conversation_id: Option<&str>,
) -> ConsoleLogResult<Option<ConsoleCursor>> {
    let mut sql = String::from(
        "SELECT cursor_seq FROM console_frames WHERE cursor_seq > ?1 AND cursor_seq < ?2",
    );
    let mut values = vec![
        rusqlite::types::Value::Integer(after),
        rusqlite::types::Value::Integer(before),
    ];
    // A conversation alone is found by its own index: the unary plus keeps
    // the statement on it and checks the found rows' mob.
    let mob_term = if identity.is_none() && conversation_id.is_some() {
        " AND +{mob} = ?"
    } else {
        " AND {mob} = ?"
    };
    for (term, value) in [
        (" AND identity = ?", identity),
        (mob_term, mob),
        (" AND conversation_id = ?", conversation_id),
    ] {
        if let Some(value) = value {
            values.push(rusqlite::types::Value::Text(value.to_string()));
            sql.push_str(term);
            sql.push_str(&values.len().to_string());
        }
    }
    sql.push_str(" ORDER BY cursor_seq DESC LIMIT 1");

    let seq: Option<i64> = with_member_sql(conn, &sql, |sql| {
        conn.query_row(sql, rusqlite::params_from_iter(values.iter()), |row| {
            row.get(0)
        })
        .optional()
    })
    .map_err(into_boxed)?;
    Ok(seq.map(|value| ConsoleCursor::from_seq(value as u64)))
}

fn row_to_frame(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConsoleFrame> {
    let seq: i64 = row.get(0)?;
    let payload_json: String = row.get(12)?;
    let mut payload = serde_json::from_str(&payload_json).unwrap_or(serde_json::Value::Null);
    let source_kind: String = row.get(13)?;
    let provenance_json: Option<String> = row.get(21)?;
    let provenance_id: Option<i64> = row.get(22)?;
    if provenance_id.is_some() && provenance_json.is_none() {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            22,
            rusqlite::types::Type::Integer,
            Box::new(std::io::Error::other(
                "console member provenance reference is missing",
            )),
        ));
    }
    let member_provenance: Option<super::types::ConsoleFrameMemberProvenance> = provenance_json
        .map(|json| {
            serde_json::from_str(&json).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    21,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })
        })
        .transpose()?;
    let kind: String = row.get(8)?;
    let update_marker_shares_provenance: bool = row.get(23)?;
    if update_marker_shares_provenance {
        let invalid_marker = || {
            rusqlite::Error::FromSqlConversionFailure(
                23,
                rusqlite::types::Type::Integer,
                Box::new(std::io::Error::other(
                    "invalid normalized console update marker provenance",
                )),
            )
        };
        if kind != "frame_updated" || provenance_id.is_none() {
            return Err(invalid_marker());
        }
        let provenance = member_provenance.as_ref().ok_or_else(invalid_marker)?;
        let source = payload
            .get_mut("frame")
            .and_then(|frame| frame.get_mut("source"))
            .and_then(serde_json::Value::as_object_mut)
            .ok_or_else(invalid_marker)?;
        if source.contains_key("member_provenance") {
            return Err(invalid_marker());
        }
        source.insert(
            "member_provenance".into(),
            serde_json::to_value(provenance).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    21,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?,
        );
    }
    Ok(ConsoleFrame {
        cursor: ConsoleCursor::from_seq(seq as u64),
        id: row.get(1)?,
        dedupe_key: row.get(2)?,
        timestamp_ms: row.get::<_, i64>(3)? as u64,
        runtime_key: row.get(4)?,
        identity: row.get(5)?,
        mob_id: mob_of_column(row.get(24)?),
        conversation_id: row.get(6)?,
        session_id: row.get(7)?,
        kind,
        status: ConsoleFrameStatus::from_str(row.get::<_, String>(9)?.as_str()),
        frame_version: row.get::<_, i64>(10)? as u64,
        updated_at_ms: row.get::<_, Option<i64>>(11)?.map(|value| value as u64),
        payload,
        source: ConsoleFrameSource {
            member_provenance,
            kind: ConsoleFrameSourceKind::from_str(&source_kind),
            source_cursor: row.get(14)?,
        },
        source_event_id: row.get(15)?,
        interaction_id: row.get(16)?,
        parent_frame_id: row.get(17)?,
        caused_by_frame_id: row.get(18)?,
        turn_id: row.get(19)?,
        run_id: row.get(20)?,
    })
}

fn normalize_limit(limit: usize) -> usize {
    limit.clamp(1, 1000)
}

fn cursor_seq(cursor: &ConsoleCursor) -> ConsoleLogResult<u64> {
    cursor
        .seq()
        .ok_or_else(|| boxed_error(format!("invalid console cursor: {cursor}")))
}

fn cursor_seq_i64(cursor: &ConsoleCursor) -> ConsoleLogResult<i64> {
    let seq = cursor_seq(cursor)?;
    i64::try_from(seq).map_err(|_| boxed_error(format!("console cursor out of range: {cursor}")))
}

/// A `user_input` frame of a console send whose delivery has not settled.
fn is_unsettled_send_frame(frame: &ConsoleFrame) -> bool {
    frame.kind == "user_input"
        && frame.source.kind == ConsoleFrameSourceKind::Send
        && matches!(
            frame.status,
            ConsoleFrameStatus::Accepted
                | ConsoleFrameStatus::Dispatching
                | ConsoleFrameStatus::Submitted
                | ConsoleFrameStatus::Redacted
        )
}

/// Whether `frame` is the send record of the console send whose interaction
/// id is `interaction_id`.
fn is_send_record_of(frame: &ConsoleFrame, interaction_id: &str) -> bool {
    frame.kind == "user_input"
        && frame.source.kind == ConsoleFrameSourceKind::Send
        && frame.interaction_id.as_deref() == Some(interaction_id)
}

pub(crate) fn stable_frame_id(dedupe_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(dedupe_key.as_bytes());
    format!("console-frame-{}", to_hex(&hasher.finalize()))
}

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// The status a send frame moves to under `transition` from the status it
/// records, as the send machine answers; `None` when the machine refuses
/// it from that state, or the frame is no send's.
fn send_frame_transition(
    status: ConsoleFrameStatus,
    transition: SendTransition,
) -> Option<ConsoleFrameStatus> {
    let (next, _effects) = SendState::from_send_frame_status(status)?
        .apply(transition)
        .ok()?;
    next.frame_status()
}

/// Record (`Some`) or clear (`None`) a send frame's payload `field`.
fn set_payload_field(
    payload: &mut serde_json::Value,
    field: &str,
    value: Option<serde_json::Value>,
) -> ConsoleLogResult<()> {
    let Some(fields) = payload.as_object_mut() else {
        return Err(boxed_error("a send frame's payload is not an object"));
    };
    match value {
        Some(value) => {
            fields.insert(field.to_string(), value);
        }
        None => {
            fields.remove(field);
        }
    }
    Ok(())
}

fn boxed_error(message: impl Into<String>) -> ConsoleLogError {
    Box::new(std::io::Error::other(message.into()))
}

fn into_boxed<E>(error: E) -> ConsoleLogError
where
    E: std::error::Error + Send + Sync + 'static,
{
    Box::new(error)
}

fn current_time_ms() -> u64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_millis() as u64,
        Err(_) => 0,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;

    struct LegacyQueryOnlyStore;

    #[async_trait::async_trait]
    impl ConsoleLogStore for LegacyQueryOnlyStore {
        async fn append_if_absent(
            &self,
            _frame: NewConsoleFrame,
        ) -> ConsoleLogResult<AppendOutcome> {
            Err(boxed_error("not implemented for test"))
        }

        async fn update_frame_status(
            &self,
            _frame_id: &str,
            _status: ConsoleFrameStatus,
        ) -> ConsoleLogResult<Option<ConsoleFrame>> {
            Err(boxed_error("not implemented for test"))
        }

        async fn query_frames(
            &self,
            _query: ConsoleTimelineQuery,
        ) -> ConsoleLogResult<ConsoleTimelinePage> {
            Ok(ConsoleTimelinePage {
                frames: Vec::new(),
                next_cursor: None,
            })
        }

        async fn frame_by_dedupe_key(
            &self,
            _dedupe_key: &str,
        ) -> ConsoleLogResult<Option<ConsoleFrame>> {
            Err(boxed_error("not implemented for test"))
        }

        async fn latest_cursor(&self) -> ConsoleLogResult<Option<ConsoleCursor>> {
            Ok(None)
        }

        async fn clear_frames(&self) -> ConsoleLogResult<()> {
            Ok(())
        }

        async fn record_source_watermark(
            &self,
            _runtime_key: &str,
            _source_kind: ConsoleFrameSourceKind,
            _source_cursor: &str,
        ) -> ConsoleLogResult<()> {
            Ok(())
        }

        async fn source_watermark(
            &self,
            _runtime_key: &str,
            _source_kind: ConsoleFrameSourceKind,
        ) -> ConsoleLogResult<Option<String>> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn sqlite_repeated_provenance_is_normalized_and_missing_references_fail_closed() {
        let store = SqliteConsoleLogStore::in_memory().expect("store");
        let witness: crate::ConsoleFrameMemberProvenance = serde_json::from_value(serde_json::json!({
            "identity": {"identity":"private", "display_name":"Private", "runtime_key":"runtime-a",
                "runtime_member_id":"private", "session_id":"session-private", "visibility":"addressable",
                "addressable":true, "health":"ready", "labels":{"private-label":"large private label"}},
            "member": {"agent_identity":"private", "role":"worker", "state":"running",
                "session_id":"session-private", "wired_to":[], "labels":{"private-label":"large private label"}},
            "primary_mob_id":"primary", "source_mob_id":"primary"
        })).expect("witness");
        for index in 0..128 {
            let mut frame = sample_frame(&format!("delta-{index}"), "private");
            frame.source.member_provenance = Some(witness.clone());
            let stored = store.append_if_absent(frame).await.expect("append").frame;
            assert_eq!(stored.source.member_provenance, Some(witness.clone()));
        }
        let original = store
            .frame_by_dedupe_key("delta-0")
            .await
            .expect("lookup")
            .expect("original");
        let mut update = sample_frame("update", "private");
        update.kind = "frame_updated".into();
        update.payload = serde_json::json!({"frame":original});
        update.source.member_provenance = Some(witness.clone());
        let marker = store.append_if_absent(update).await.expect("marker").frame;
        assert_eq!(
            marker.payload["frame"]["source"]["member_provenance"],
            serde_json::json!(witness)
        );
        for (key, source) in [
            ("absent", serde_json::json!({})),
            ("null", serde_json::json!({"member_provenance":null})),
        ] {
            let mut custom = sample_frame(key, "private");
            custom.kind = "frame_updated".into();
            custom.source.member_provenance = Some(witness.clone());
            custom.payload = serde_json::json!({"frame":{"source":source}});
            let expected = custom.payload.clone();
            assert_eq!(
                store
                    .append_if_absent(custom)
                    .await
                    .expect("custom marker")
                    .frame
                    .payload,
                expected,
                "hydration must not invent an intentionally absent nested witness"
            );
        }
        let mut changed = witness.clone();
        changed
            .identity
            .labels
            .insert("generation".into(), "replacement".into());
        let mut collision = sample_frame("delta-0", "private");
        collision.source.member_provenance = Some(changed.clone());
        assert_eq!(
            store
                .append_if_absent(collision)
                .await
                .expect("dedupe collision")
                .frame
                .source
                .member_provenance,
            Some(witness.clone()),
            "duplicate arrival cannot overwrite original provenance"
        );
        let mut failed = sample_frame("failed-unique-id", "private");
        failed.id = Some(original.id.clone());
        failed.source.member_provenance = Some(changed.clone());
        assert!(
            store.append_if_absent(failed).await.is_err(),
            "failed frame insert must roll back its new witness"
        );
        {
            let conn = store.conn.lock().expect("connection");
            let count: i64 = conn
                .query_row(
                    "SELECT count(*) FROM console_member_provenance",
                    [],
                    |row| row.get(0),
                )
                .expect("witness count");
            assert_eq!(count, 1, "one immutable witness for all identical deltas");
            let inline: i64 = conn
                .query_row(
                    "SELECT count(*) FROM console_frames WHERE member_provenance_json IS NOT NULL",
                    [],
                    |row| row.get(0),
                )
                .expect("inline count");
            assert_eq!(inline, 0);
            let marker_payload: String = conn
                .query_row(
                    "SELECT payload_json FROM console_frames WHERE dedupe_key = 'update'",
                    [],
                    |row| row.get(0),
                )
                .expect("marker payload");
            assert!(
                !marker_payload.contains("member_provenance"),
                "nested marker shares outer immutable witness on disk"
            );
        }
        let mut replacement = sample_frame("new-witness", "private");
        replacement.source.member_provenance = Some(changed.clone());
        assert_eq!(
            store
                .append_if_absent(replacement)
                .await
                .expect("new witness")
                .frame
                .source
                .member_provenance,
            Some(changed)
        );
        {
            let conn = store.conn.lock().expect("connection");
            let count: i64 = conn
                .query_row(
                    "SELECT count(*) FROM console_member_provenance",
                    [],
                    |row| row.get(0),
                )
                .expect("witness count");
            assert_eq!(
                count, 2,
                "a changed witness retains its own immutable record"
            );
            conn.execute("DELETE FROM console_member_provenance", [])
                .expect("corrupt missing reference");
        }
        assert!(store.frame_by_dedupe_key("delta-0").await.is_err());
        store.clear_frames().await.expect("clear corrupted store");
        assert!(
            store
                .query_frames(Default::default())
                .await
                .expect("cleared store")
                .frames
                .is_empty()
        );
    }

    #[tokio::test]
    async fn sqlite_normalized_marker_corruption_never_returns_partial_provenance() {
        let store = SqliteConsoleLogStore::in_memory().expect("store");
        let witness: crate::ConsoleFrameMemberProvenance = serde_json::from_value(serde_json::json!({
            "identity": {"identity":"private", "display_name":"Private", "runtime_key":"runtime-a",
                "runtime_member_id":"private", "visibility":"addressable", "addressable":true, "health":"ready"},
            "member": {"agent_identity":"private", "role":"worker", "state":"running", "wired_to":[], "labels":{}},
            "primary_mob_id":"primary", "source_mob_id":"primary"
        })).expect("witness");
        for (index, corruption) in [
            "member_provenance_id = NULL",
            "payload_json = '{}'",
            "payload_json = '{\"frame\":{\"source\":null}}'",
            "payload_json = '{\"frame\":{\"source\":{\"member_provenance\":null}}}'",
            "kind = 'text_delta'",
        ]
        .into_iter()
        .enumerate()
        {
            let key = format!("corrupt-marker-{index}");
            let mut marker = sample_frame(&key, "private");
            marker.kind = "frame_updated".into();
            marker.source.member_provenance = Some(witness.clone());
            marker.payload = serde_json::json!({"frame":{"source":{"member_provenance":witness}}});
            store.append_if_absent(marker).await.expect("valid marker");
            store
                .conn
                .lock()
                .expect("connection")
                .execute(
                    &format!("UPDATE console_frames SET {corruption} WHERE dedupe_key = ?1"),
                    params![key],
                )
                .expect("corrupt normalized marker");
            assert!(
                store.frame_by_dedupe_key(&key).await.is_err(),
                "partial provenance must fail for {corruption}"
            );
        }
    }

    #[tokio::test]
    async fn sqlite_v2_upgrade_preserves_inline_provenance_and_normalizes_new_writes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("console.sqlite");
        let witness: crate::ConsoleFrameMemberProvenance = serde_json::from_value(serde_json::json!({
            "identity": {"identity":"private", "display_name":"Private", "runtime_key":"runtime-a",
                "runtime_member_id":"private", "session_id":"session-private", "visibility":"addressable",
                "addressable":true, "health":"ready", "labels":{}},
            "member": {"agent_identity":"private", "role":"worker", "state":"running",
                "session_id":"session-private", "wired_to":[], "labels":{}},
            "primary_mob_id":"primary", "source_mob_id":"primary"
        })).expect("witness");
        {
            let mut conn = meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::PRIMARY)
                .expect("sqlite");
            let domain = meerkat_sqlite::SchemaDomain {
                migrations: &META_CONSOLE_UNKEYED_DOMAIN.migrations[..2],
                initialize_current: initialize_v2_console_schema,
                allowed_existing_versions: &[2],
                owned_objects: &META_CONSOLE_UNKEYED_DOMAIN.owned_objects[..4],
                released_predecessors: &[],
                ..META_CONSOLE_UNKEYED_DOMAIN
            };
            meerkat_sqlite::apply_domain_migrations(&mut conn, &domain).expect("development v2");
            conn.execute("INSERT INTO console_frames(id,dedupe_key,timestamp_ms,runtime_key,identity,kind,status,payload_json,source_kind,member_provenance_json) VALUES ('v2','v2',1,'runtime-a','private','text_delta','completed','{}','console_event',?1)", params![serde_json::to_string(&witness).expect("JSON")]).expect("v2 row");
        }
        let store = SqliteConsoleLogStore::open(&path).expect("v2 upgrades to v3");
        assert_eq!(
            store
                .frame_by_dedupe_key("v2")
                .await
                .expect("lookup")
                .expect("v2 row")
                .source
                .member_provenance,
            Some(witness.clone())
        );
        let mut new = sample_frame("v3", "private");
        new.source.member_provenance = Some(witness.clone());
        store.append_if_absent(new).await.expect("v3 write");
        drop(store);
        let reopened = SqliteConsoleLogStore::open(&path).expect("v3 reopen");
        for key in ["v2", "v3"] {
            assert_eq!(
                reopened
                    .frame_by_dedupe_key(key)
                    .await
                    .expect("lookup")
                    .expect("row")
                    .source
                    .member_provenance,
                Some(witness.clone())
            );
        }
    }

    #[tokio::test]
    async fn sqlite_v1_upgrade_preserves_legacy_history_and_roundtrips_member_provenance() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("console.sqlite");
        {
            let mut conn = meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::PRIMARY)
                .expect("sqlite");
            let legacy_domain = meerkat_sqlite::SchemaDomain {
                migrations: &[meerkat_sqlite::Migration {
                    version: 1,
                    name: "base-schema",
                    apply: migration_0001_console_schema,
                }],
                initialize_current: migration_0001_console_schema,
                allowed_existing_versions: &[1],
                owned_objects: &META_CONSOLE_UNKEYED_DOMAIN.owned_objects[..4],
                released_predecessors: &[],
                ..META_CONSOLE_UNKEYED_DOMAIN
            };
            meerkat_sqlite::apply_domain_migrations(&mut conn, &legacy_domain)
                .expect("ledgered released v1");
            conn.execute("INSERT INTO console_frames (id, dedupe_key, timestamp_ms, runtime_key, identity, kind, status, payload_json, source_kind) VALUES ('legacy', 'legacy', 1, 'runtime-a', 'retired-readable', 'text_complete', 'completed', '{}', 'session_history')", []).expect("legacy row");
        }
        let store = SqliteConsoleLogStore::open(&path).expect("v1 upgrades");
        let legacy = store
            .query_frames(ConsoleTimelineQuery {
                identity: Some("retired-readable".to_string()),
                ..Default::default()
            })
            .await
            .expect("legacy query");
        assert_eq!(legacy.frames.len(), 1);
        assert!(
            legacy.frames[0].source.member_provenance.is_none(),
            "missing legacy provenance stays absent"
        );
        let mut frame = sample_frame("provenance-roundtrip", "private");
        let provenance = crate::ConsoleFrameMemberProvenance {
            identity: crate::ConsoleIdentityRecord {
                identity: "private".to_string(),
                display_name: "Private".to_string(),
                runtime_key: "runtime-a".to_string(),
                runtime_member_id: "private".to_string(),
                session_id: Some("session-private".to_string()),
                member_id: None,
                visibility: crate::ConsoleVisibility::Addressable,
                addressable: true,
                health: "ready".to_string(),
                topology_peers: vec![],
                labels: Default::default(),
            },
            member: crate::runtime::ConsoleMember {
                agent_identity: "private".to_string(),
                role: "delegate".to_string(),
                state: "running".to_string(),
                model_capabilities: Default::default(),
                runtime_mode: None,
                session_id: Some("session-private".to_string()),
                wired_to: vec![],
                labels: Default::default(),
                progress: None,
            },
            primary_mob_id: "primary".to_string(),
            source_mob_id: "lower".to_string(),
            lineage: Vec::new(),
        };
        frame.source.member_provenance = Some(provenance.clone());
        let appended = store
            .append_if_absent(frame)
            .await
            .expect("append provenance");
        assert_eq!(
            appended.frame.source.member_provenance,
            Some(provenance.clone())
        );
        drop(store);
        let reopened = SqliteConsoleLogStore::open(&path).expect("reopen");
        let read = reopened
            .frame_by_dedupe_key("provenance-roundtrip")
            .await
            .expect("lookup")
            .expect("stored");
        assert_eq!(read.source.member_provenance, Some(provenance));
        {
            let conn = reopened.conn.lock().expect("connection");
            conn.execute("UPDATE console_frames SET member_provenance_json = 'broken' WHERE dedupe_key = 'provenance-roundtrip'", []).expect("corrupt typed source");
        }
        assert!(
            reopened
                .frame_by_dedupe_key("provenance-roundtrip")
                .await
                .is_err(),
            "invalid provenance must fail closed, never become legacy unknown history"
        );
    }

    fn sample_frame(dedupe_key: &str, identity: &str) -> NewConsoleFrame {
        NewConsoleFrame {
            mob_id: None,
            id: None,
            dedupe_key: dedupe_key.to_string(),
            timestamp_ms: 10,
            runtime_key: "runtime-a".to_string(),
            identity: identity.to_string(),
            conversation_id: Some(identity.to_string()),
            session_id: Some("session-1".to_string()),
            kind: "text_delta".to_string(),
            status: ConsoleFrameStatus::Delivered,
            payload: json!({ "delta": "hello" }),
            source: ConsoleFrameSource {
                member_provenance: None,
                kind: ConsoleFrameSourceKind::ConsoleEvent,
                source_cursor: None,
            },
            source_event_id: Some(dedupe_key.to_string()),
            interaction_id: None,
            turn_id: None,
            run_id: None,
            parent_frame_id: None,
            caused_by_frame_id: None,
        }
    }

    fn activity_frame(
        dedupe_key: &str,
        identity: &str,
        timestamp_ms: u64,
        source: ConsoleFrameSourceKind,
    ) -> NewConsoleFrame {
        let mut frame = sample_frame(dedupe_key, identity);
        frame.timestamp_ms = timestamp_ms;
        frame.source.kind = source;
        frame
    }

    fn event_frame(
        dedupe_key: &str,
        identity: &str,
        timestamp_ms: u64,
        kind: &str,
    ) -> NewConsoleFrame {
        let mut frame = activity_frame(
            dedupe_key,
            identity,
            timestamp_ms,
            ConsoleFrameSourceKind::ConsoleEvent,
        );
        frame.kind = kind.to_string();
        frame
    }

    /// The last-activity read takes each identity's newest frame of the
    /// identity acting. Frames rebuilt from session history or synthesized by
    /// the console are stamped when the console observes them, so they do not
    /// count; nor do lifecycle records, interaction failures and memory
    /// events, which record something done to the identity. An identity with
    /// only such frames reads unknown.
    #[tokio::test]
    async fn latest_activity_times_take_each_identitys_newest_acting_frame() {
        let stores: Vec<Box<dyn ConsoleLogStore>> = vec![
            Box::new(InMemoryConsoleLogStore::new()),
            Box::new(SqliteConsoleLogStore::in_memory().expect("sqlite store")),
        ];
        for store in stores {
            for frame in [
                activity_frame(
                    "router-event",
                    "router",
                    100,
                    ConsoleFrameSourceKind::ConsoleEvent,
                ),
                activity_frame(
                    "router-history",
                    "router",
                    300,
                    ConsoleFrameSourceKind::SessionHistory,
                ),
                activity_frame(
                    "delivery-send",
                    "delivery",
                    50,
                    ConsoleFrameSourceKind::Send,
                ),
                activity_frame(
                    "delivery-tool",
                    "delivery",
                    70,
                    ConsoleFrameSourceKind::ToolApplication,
                ),
                activity_frame(
                    "archivist-history",
                    "archivist",
                    400,
                    ConsoleFrameSourceKind::SessionHistory,
                ),
                activity_frame(
                    "notices-synthetic",
                    "notices",
                    500,
                    ConsoleFrameSourceKind::Synthetic,
                ),
                activity_frame(
                    "bystander-event",
                    "bystander",
                    600,
                    ConsoleFrameSourceKind::ConsoleEvent,
                ),
                event_frame("router-retired", "router", 700, "identity_retired"),
                event_frame("router-memory", "router", 710, "memory.record.promoted"),
                event_frame("delivery-failed", "delivery", 720, "interaction_failed"),
                event_frame("retiree-reset", "retiree", 730, "identity_reset"),
                event_frame("retiree-respawned", "retiree", 740, "identity_respawned"),
                event_frame("retiree-deleted", "retiree", 750, "identity_deleted"),
            ] {
                store.append_if_absent(frame).await.expect("append");
            }
            let identities = [
                "router",
                "delivery",
                "archivist",
                "notices",
                "retiree",
                "missing",
            ]
            .map(str::to_string);
            assert_eq!(
                store
                    .latest_activity_times(&identities)
                    .await
                    .expect("activity"),
                HashMap::from([("router".to_string(), 100), ("delivery".to_string(), 70)]),
            );
            assert_eq!(
                store
                    .latest_activity_times(&[])
                    .await
                    .expect("no identities"),
                HashMap::new(),
            );
        }
    }

    /// A departure is a live runtime event that retires or deletes its
    /// identity; a reset, a respawn or a frame rebuilt from history is not.
    /// No departure counts as the identity acting.
    #[tokio::test]
    async fn departure_frames_are_live_retire_and_delete_events_and_never_activity() {
        for kind in DEPARTURE_EVENT_KINDS {
            assert!(
                NON_ACTIVITY_EVENT_KINDS.contains(&kind),
                "{kind} must not count as acting"
            );
        }
        let store = InMemoryConsoleLogStore::new();
        let mut history_retired = event_frame("history-retired", "worker", 5, "identity_retired");
        history_retired.source.kind = ConsoleFrameSourceKind::SessionHistory;
        for (frame, departs) in [
            (
                event_frame("retired", "worker", 1, "identity_retired"),
                true,
            ),
            (
                event_frame("deleted", "worker", 2, "identity_deleted"),
                true,
            ),
            (event_frame("reset", "worker", 3, "identity_reset"), false),
            (
                event_frame("respawned", "worker", 4, "identity_respawned"),
                false,
            ),
            (history_retired, false),
        ] {
            let stored = store.append_if_absent(frame).await.expect("append").frame;
            assert_eq!(
                is_departure_frame(&stored),
                departs,
                "{}",
                stored.dedupe_key
            );
            assert!(!is_activity_frame(&stored), "{}", stored.dedupe_key);
        }
    }

    /// The read has no frame bound: an identity whose one acting frame sits
    /// behind 600 newer frames of it not acting (history and memory events,
    /// as after a backfill) reads that frame's time, in both stores.
    #[tokio::test]
    async fn latest_activity_times_reach_an_acting_frame_behind_any_number_of_newer_frames() {
        let stores: Vec<Box<dyn ConsoleLogStore>> = vec![
            Box::new(InMemoryConsoleLogStore::new()),
            Box::new(SqliteConsoleLogStore::in_memory().expect("sqlite store")),
        ];
        for store in stores {
            store
                .append_if_absent(activity_frame(
                    "archivist-acted",
                    "archivist",
                    100,
                    ConsoleFrameSourceKind::ConsoleEvent,
                ))
                .await
                .expect("append");
            for index in 0..600_u64 {
                let newer = if index % 2 == 0 {
                    activity_frame(
                        &format!("archivist-history-{index}"),
                        "archivist",
                        200 + index,
                        ConsoleFrameSourceKind::SessionHistory,
                    )
                } else {
                    event_frame(
                        &format!("archivist-memory-{index}"),
                        "archivist",
                        200 + index,
                        "memory.record.promoted",
                    )
                };
                store.append_if_absent(newer).await.expect("append");
            }
            assert_eq!(
                store
                    .latest_activity_times(&["archivist".to_string()])
                    .await
                    .expect("activity"),
                HashMap::from([("archivist".to_string(), 100)]),
            );
        }
    }

    /// The walk reads one identity's own frames through the identity/cursor
    /// index, newest first: no sort and no scan of the log.
    #[test]
    fn latest_activity_walk_reads_the_identity_cursor_index_in_order() {
        let store = SqliteConsoleLogStore::in_memory().expect("sqlite store");
        let conn = store.conn.lock().expect("connection");
        assert!(!mob_keyed(&conn), "a fresh log is not at schema version 4");
        let plan = conn
            .prepare(&format!(
                "EXPLAIN QUERY PLAN {}",
                member_sql(&conn, &latest_activity_sql())
            ))
            .expect("prepare query plan")
            .query_map(params!["archivist", ""], |row| row.get::<_, String>(3))
            .expect("run query plan")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect query plan")
            .join("\n")
            .to_lowercase();
        assert!(
            plan.contains("idx_console_frames_identity_cursor"),
            "the walk must seek the identity/cursor index; plan was: {plan}"
        );
        assert!(
            !plan.contains("temp b-tree"),
            "the index order must serve the walk without a sort; plan was: {plan}"
        );
    }

    /// M3: the console store — previously opened with zero PRAGMAs — now
    /// runs under the `Primary` profile (WAL + synchronous=FULL + shared
    /// busy timeout) and stamps the `meta-console` ledger domain, and the
    /// hot paths (append_if_absent, watermarks) still round-trip.
    #[tokio::test]
    async fn fresh_console_store_stamps_domain_gains_wal_and_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("meta_console.sqlite3");
        let store = SqliteConsoleLogStore::open(&path).expect("open");

        let appended = store
            .append_if_absent(sample_frame("dedupe-1", "identity:a"))
            .await
            .expect("append");
        assert_eq!(appended.disposition, AppendDisposition::Inserted);
        let replay = store
            .append_if_absent(sample_frame("dedupe-1", "identity:a"))
            .await
            .expect("replay");
        assert_eq!(replay.disposition, AppendDisposition::Existing);
        assert_eq!(replay.frame.id, appended.frame.id);

        store
            .record_source_watermark("runtime-a", ConsoleFrameSourceKind::ConsoleEvent, "42")
            .await
            .expect("watermark write");
        assert_eq!(
            store
                .source_watermark("runtime-a", ConsoleFrameSourceKind::ConsoleEvent)
                .await
                .expect("watermark read"),
            Some("42".to_string())
        );

        let probe = Connection::open(&path).expect("probe");
        assert_eq!(
            meerkat_sqlite::domain_version(&probe, "meta-console").expect("ledger"),
            Some(3)
        );
        let journal: String = probe
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .expect("journal_mode");
        assert_eq!(journal, "wal", "console store gains WAL for the first time");
    }

    /// A pre-ledger console database (historical DDL, no meerkat_schema) is
    /// refused typed at open with its rows left untouched and no ledger
    /// stamped: pre-ledger corpora are below the mobkit 0.8.8 floor, and the
    /// 0.8.11 reset retired silent pre-floor convergence (this test pinned
    /// that convergence until then).
    #[tokio::test]
    async fn legacy_console_file_is_refused_with_rows_preserved() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("meta_console.sqlite3");
        {
            let conn = Connection::open(&path).expect("legacy create");
            conn.execute_batch(
                "CREATE TABLE console_frames (
                    cursor_seq INTEGER PRIMARY KEY AUTOINCREMENT,
                    id TEXT NOT NULL UNIQUE,
                    dedupe_key TEXT NOT NULL UNIQUE,
                    timestamp_ms INTEGER NOT NULL,
                    runtime_key TEXT NOT NULL,
                    identity TEXT NOT NULL,
                    conversation_id TEXT,
                    session_id TEXT,
                    kind TEXT NOT NULL,
                    status TEXT NOT NULL,
                    frame_version INTEGER NOT NULL DEFAULT 1,
                    updated_at_ms INTEGER,
                    payload_json TEXT NOT NULL,
                    source_kind TEXT NOT NULL,
                    source_cursor TEXT,
                    source_event_id TEXT,
                    interaction_id TEXT,
                    parent_frame_id TEXT,
                    caused_by_frame_id TEXT,
                    turn_id TEXT,
                    run_id TEXT
                );
                CREATE TABLE console_source_watermarks (
                    runtime_key TEXT NOT NULL,
                    source_kind TEXT NOT NULL,
                    source_cursor TEXT NOT NULL,
                    last_ingested_at_ms INTEGER NOT NULL,
                    PRIMARY KEY(runtime_key, source_kind)
                );
                INSERT INTO console_frames (id, dedupe_key, timestamp_ms, runtime_key, \
                 identity, kind, status, payload_json, source_kind)
                    VALUES ('frame-legacy', 'dedupe-legacy', 5, 'runtime-a', 'identity:a', \
                            'text_delta', 'delivered', '{}', 'console_event');
                INSERT INTO console_source_watermarks (runtime_key, source_kind, \
                 source_cursor, last_ingested_at_ms)
                    VALUES ('runtime-a', 'console_event', '7', 5);",
            )
            .expect("legacy rows");
        }

        assert!(
            SqliteConsoleLogStore::open(&path).is_err(),
            "opening a pre-ledger console database must refuse typed: unledgered owned \
             tables are below the mobkit 0.8.8 floor and must never be silently converged"
        );

        let probe = Connection::open(&path).expect("probe");
        let (id, dedupe): (String, String) = probe
            .query_row(
                "SELECT id, dedupe_key FROM console_frames WHERE id = 'frame-legacy'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("legacy frame preserved");
        assert_eq!(
            (id.as_str(), dedupe.as_str()),
            ("frame-legacy", "dedupe-legacy")
        );
        let watermark: String = probe
            .query_row(
                "SELECT source_cursor FROM console_source_watermarks WHERE runtime_key = 'runtime-a'",
                [],
                |row| row.get(0),
            )
            .expect("legacy watermark preserved");
        assert_eq!(watermark, "7");
        assert_eq!(
            meerkat_sqlite::domain_version(&probe, "meta-console").expect("ledger"),
            None,
            "the refusal must not stamp a ledger onto a file it refused to own"
        );
    }

    #[tokio::test]
    async fn legacy_store_default_rejects_v04_window_queries_loudly() {
        let store = LegacyQueryOnlyStore;

        let err = store
            .query_windowed_frames(ConsoleTimelineWindowQuery {
                mode: ConsoleTimelineMode::Recent,
                limit: 10,
                ..ConsoleTimelineWindowQuery::default()
            })
            .await
            .expect_err("legacy stores must implement recent windows explicitly");
        assert!(
            err.to_string()
                .contains("must implement query_windowed_frames")
        );

        let err = store
            .query_windowed_frames(ConsoleTimelineWindowQuery {
                mode: ConsoleTimelineMode::Since,
                before: Some(ConsoleCursor::from_seq(10)),
                limit: 10,
                ..ConsoleTimelineWindowQuery::default()
            })
            .await
            .expect_err("legacy stores must implement before windows explicitly");
        assert!(
            err.to_string()
                .contains("must implement query_windowed_frames")
        );

        let page = store
            .query_windowed_frames(ConsoleTimelineWindowQuery {
                mode: ConsoleTimelineMode::Since,
                limit: 10,
                ..ConsoleTimelineWindowQuery::default()
            })
            .await
            .expect("legacy since-only fallback remains source-compatible");
        assert!(page.frames.is_empty());
    }

    #[tokio::test]
    async fn in_memory_log_assigns_monotonic_cursors_and_dedupes() {
        let store = InMemoryConsoleLogStore::new();
        let first = store
            .append_if_absent(sample_frame("event-1", "agent-a"))
            .await
            .expect("append first");
        let duplicate = store
            .append_if_absent(sample_frame("event-1", "agent-a"))
            .await
            .expect("append duplicate");
        let second = store
            .append_if_absent(sample_frame("event-2", "agent-a"))
            .await
            .expect("append second");

        assert_eq!(first.disposition, AppendDisposition::Inserted);
        assert_eq!(duplicate.disposition, AppendDisposition::Existing);
        assert_eq!(first.frame.cursor.seq(), Some(1));
        assert_eq!(second.frame.cursor.seq(), Some(2));
    }

    #[tokio::test]
    async fn sqlite_log_queries_by_identity_and_cursor() {
        let store = SqliteConsoleLogStore::in_memory().expect("sqlite store");
        let first = store
            .append_if_absent(sample_frame("event-1", "agent-a"))
            .await
            .expect("append first");
        store
            .append_if_absent(sample_frame("event-2", "agent-b"))
            .await
            .expect("append second");
        store
            .append_if_absent(sample_frame("event-3", "agent-a"))
            .await
            .expect("append third");

        let page = store
            .query_windowed_frames(ConsoleTimelineWindowQuery {
                identity: Some("agent-a".to_string()),
                after: Some(first.frame.cursor),
                limit: 10,
                ..ConsoleTimelineWindowQuery::default()
            })
            .await
            .expect("query");
        assert_eq!(page.frames.len(), 1);
        assert_eq!(page.frames[0].dedupe_key, "event-3");
    }

    #[tokio::test]
    async fn in_memory_log_queries_recent_window_in_display_order() {
        let store = InMemoryConsoleLogStore::new();
        for index in 1..=6 {
            store
                .append_if_absent(sample_frame(&format!("event-{index}"), "agent-a"))
                .await
                .expect("append frame");
        }

        let page = store
            .query_windowed_frames(ConsoleTimelineWindowQuery {
                identity: Some("agent-a".to_string()),
                mode: ConsoleTimelineMode::Recent,
                limit: 3,
                ..ConsoleTimelineWindowQuery::default()
            })
            .await
            .expect("query recent");
        assert_eq!(
            page.frames
                .iter()
                .map(|frame| frame.dedupe_key.as_str())
                .collect::<Vec<_>>(),
            vec!["event-4", "event-5", "event-6"]
        );
        assert_eq!(
            page.next_cursor.as_ref().and_then(ConsoleCursor::seq),
            Some(6)
        );
        assert_eq!(
            page.latest_cursor.as_ref().and_then(ConsoleCursor::seq),
            Some(6)
        );
        assert!(!page.exhausted);

        let older = store
            .query_windowed_frames(ConsoleTimelineWindowQuery {
                identity: Some("agent-a".to_string()),
                mode: ConsoleTimelineMode::Recent,
                before: page.frames.first().map(|frame| frame.cursor.clone()),
                limit: 3,
                ..ConsoleTimelineWindowQuery::default()
            })
            .await
            .expect("query older");
        assert_eq!(
            older
                .frames
                .iter()
                .map(|frame| frame.dedupe_key.as_str())
                .collect::<Vec<_>>(),
            vec!["event-1", "event-2", "event-3"]
        );
        assert_eq!(
            older.latest_cursor.as_ref().and_then(ConsoleCursor::seq),
            Some(3)
        );
    }

    #[tokio::test]
    async fn in_memory_log_queries_sparse_identity_recent_window_without_global_tail_scan() {
        let store = InMemoryConsoleLogStore::new();
        store
            .append_if_absent(sample_frame("sparse-event", "sparse-agent"))
            .await
            .expect("append sparse frame");
        for index in 1..=25_000 {
            store
                .append_if_absent(sample_frame(&format!("busy-event-{index}"), "busy-agent"))
                .await
                .expect("append busy frame");
        }

        let page = store
            .query_windowed_frames(ConsoleTimelineWindowQuery {
                identity: Some("sparse-agent".to_string()),
                mode: ConsoleTimelineMode::Recent,
                limit: 10,
                ..ConsoleTimelineWindowQuery::default()
            })
            .await
            .expect("query sparse recent");

        assert_eq!(page.frames.len(), 1);
        assert_eq!(page.frames[0].dedupe_key, "sparse-event");
        assert_eq!(
            page.latest_cursor.as_ref().and_then(ConsoleCursor::seq),
            Some(1)
        );
    }

    #[tokio::test]
    async fn sqlite_log_queries_250k_sparse_identity_recent_window_with_index() {
        let store = SqliteConsoleLogStore::in_memory().expect("sqlite store");
        {
            let mut conn = store.conn.lock().expect("sqlite lock");
            let tx = conn.transaction().expect("begin transaction");
            {
                let mut insert = tx
                    .prepare(
                        "INSERT INTO console_frames (
                            id, dedupe_key, timestamp_ms, runtime_key, identity,
                            conversation_id, session_id, kind, status, frame_version, updated_at_ms,
                            payload_json, source_kind, source_cursor, source_event_id,
                            interaction_id, parent_frame_id, caused_by_frame_id, turn_id, run_id
                        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, NULL, ?10, ?11, NULL, ?12, NULL, NULL, NULL, NULL, NULL)",
                    )
                    .expect("prepare insert");
                insert
                    .execute(rusqlite::params![
                        "sparse-frame",
                        "sparse-event",
                        1_i64,
                        "runtime-a",
                        "sparse-agent",
                        "sparse-agent",
                        "session-sparse",
                        "text_complete",
                        ConsoleFrameStatus::Completed.as_str(),
                        r#"{"text":"still visible"}"#,
                        ConsoleFrameSourceKind::ConsoleEvent.as_str(),
                        "sparse-event",
                    ])
                    .expect("insert sparse frame");
                for index in 2..=250_000_i64 {
                    insert
                        .execute(rusqlite::params![
                            format!("busy-frame-{index}"),
                            format!("busy-event-{index}"),
                            index,
                            "runtime-a",
                            "busy-agent",
                            "busy-agent",
                            "session-busy",
                            "text_delta",
                            ConsoleFrameStatus::Completed.as_str(),
                            format!(r#"{{"delta":{index}}}"#),
                            ConsoleFrameSourceKind::ConsoleEvent.as_str(),
                            format!("busy-event-{index}"),
                        ])
                        .expect("insert busy frame");
                }
            }
            let plan = tx
                .prepare(
                    "EXPLAIN QUERY PLAN
                     SELECT cursor_seq FROM console_frames
                     WHERE cursor_seq > ?1 AND cursor_seq < ?2 AND identity = ?3
                     ORDER BY cursor_seq DESC LIMIT ?4",
                )
                .expect("prepare query plan")
                .query_map(
                    rusqlite::params![0_i64, i64::MAX, "sparse-agent", 11_i64],
                    |row| row.get::<_, String>(3),
                )
                .expect("run query plan")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect query plan")
                .join("\n")
                .to_lowercase();
            assert!(
                plan.contains("idx_console_frames_identity_cursor"),
                "sparse identity recent query should use identity/cursor index; plan was: {plan}"
            );
            tx.commit().expect("commit transaction");
        }

        let page = store
            .query_windowed_frames(ConsoleTimelineWindowQuery {
                identity: Some("sparse-agent".to_string()),
                mode: ConsoleTimelineMode::Recent,
                limit: 10,
                ..ConsoleTimelineWindowQuery::default()
            })
            .await
            .expect("query sparse recent");

        assert_eq!(page.frames.len(), 1);
        assert_eq!(page.frames[0].dedupe_key, "sparse-event");
        assert_eq!(
            page.latest_cursor.as_ref().and_then(ConsoleCursor::seq),
            Some(1)
        );
    }

    #[tokio::test]
    async fn sqlite_log_queries_recent_window_with_before_cursor() {
        let store = SqliteConsoleLogStore::in_memory().expect("sqlite store");
        for index in 1..=6 {
            store
                .append_if_absent(sample_frame(&format!("event-{index}"), "agent-a"))
                .await
                .expect("append frame");
        }

        let page = store
            .query_windowed_frames(ConsoleTimelineWindowQuery {
                identity: Some("agent-a".to_string()),
                mode: ConsoleTimelineMode::Recent,
                limit: 2,
                ..ConsoleTimelineWindowQuery::default()
            })
            .await
            .expect("query recent");
        assert_eq!(
            page.frames
                .iter()
                .map(|frame| frame.dedupe_key.as_str())
                .collect::<Vec<_>>(),
            vec!["event-5", "event-6"]
        );

        let older = store
            .query_windowed_frames(ConsoleTimelineWindowQuery {
                identity: Some("agent-a".to_string()),
                mode: ConsoleTimelineMode::Recent,
                before: page.frames.first().map(|frame| frame.cursor.clone()),
                limit: 2,
                ..ConsoleTimelineWindowQuery::default()
            })
            .await
            .expect("query older");
        assert_eq!(
            older
                .frames
                .iter()
                .map(|frame| frame.dedupe_key.as_str())
                .collect::<Vec<_>>(),
            vec!["event-3", "event-4"]
        );
        assert_eq!(
            older.latest_cursor.as_ref().and_then(ConsoleCursor::seq),
            Some(4)
        );
    }

    #[tokio::test]
    async fn sqlite_log_reports_exhausted_on_exact_size_recent_final_page() {
        let store = SqliteConsoleLogStore::in_memory().expect("sqlite store");
        for index in 1..=400 {
            store
                .append_if_absent(sample_frame(&format!("event-{index}"), "agent-a"))
                .await
                .expect("append frame");
        }

        let first = store
            .query_windowed_frames(ConsoleTimelineWindowQuery {
                identity: Some("agent-a".to_string()),
                mode: ConsoleTimelineMode::Recent,
                limit: 200,
                ..ConsoleTimelineWindowQuery::default()
            })
            .await
            .expect("query recent");
        assert!(!first.exhausted);
        assert_eq!(first.frames[0].dedupe_key, "event-201");

        let older = store
            .query_windowed_frames(ConsoleTimelineWindowQuery {
                identity: Some("agent-a".to_string()),
                mode: ConsoleTimelineMode::Recent,
                before: first.frames.first().map(|frame| frame.cursor.clone()),
                limit: 200,
                ..ConsoleTimelineWindowQuery::default()
            })
            .await
            .expect("query older");
        assert!(older.exhausted);
        assert_eq!(older.frames.len(), 200);
        assert_eq!(older.frames[0].dedupe_key, "event-1");
    }

    #[tokio::test]
    async fn sqlite_log_rejects_out_of_range_console_cursors() {
        let store = SqliteConsoleLogStore::in_memory().expect("sqlite store");
        store
            .append_if_absent(sample_frame("event-1", "agent-a"))
            .await
            .expect("append frame");

        let err = store
            .query_windowed_frames(ConsoleTimelineWindowQuery {
                after: Some(ConsoleCursor::from("console:9223372036854775808")),
                limit: 10,
                ..ConsoleTimelineWindowQuery::default()
            })
            .await
            .expect_err("oversized after cursor should be rejected");
        assert!(err.to_string().contains("out of range"));

        let err = store
            .query_windowed_frames(ConsoleTimelineWindowQuery {
                before: Some(ConsoleCursor::from("console:9223372036854775808")),
                limit: 10,
                ..ConsoleTimelineWindowQuery::default()
            })
            .await
            .expect_err("oversized before cursor should be rejected");
        assert!(err.to_string().contains("out of range"));
    }

    fn send_frame(dedupe_key: &str, status: ConsoleFrameStatus) -> NewConsoleFrame {
        NewConsoleFrame {
            kind: "user_input".to_string(),
            status,
            payload: json!({ "content": "hi" }),
            source: ConsoleFrameSource {
                member_provenance: None,
                kind: ConsoleFrameSourceKind::Send,
                source_cursor: Some("fingerprint".to_string()),
            },
            source_event_id: None,
            ..sample_frame(dedupe_key, "identity:a")
        }
    }

    #[tokio::test]
    async fn unsettled_send_frames_are_bounded_and_exclude_settled_and_other_frames() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sqlite =
            SqliteConsoleLogStore::open(dir.path().join("console.sqlite3")).expect("sqlite store");
        let memory = InMemoryConsoleLogStore::new();
        let stores: [&dyn ConsoleLogStore; 2] = [&sqlite, &memory];
        for store in stores {
            for frame in [
                send_frame("send-old", ConsoleFrameStatus::Accepted),
                send_frame("send-delivered", ConsoleFrameStatus::Delivered),
                send_frame("send-failed", ConsoleFrameStatus::DeliveryFailed),
                sample_frame("not-a-send", "identity:a"),
                send_frame("send-accepted", ConsoleFrameStatus::Accepted),
                send_frame("send-dispatching", ConsoleFrameStatus::Dispatching),
                send_frame("send-redacted", ConsoleFrameStatus::Redacted),
            ] {
                store.append_if_absent(frame).await.expect("append");
            }
            let keys = |frames: Vec<ConsoleFrame>| {
                frames
                    .into_iter()
                    .map(|frame| frame.dedupe_key)
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                keys(store.unsettled_send_frames(100, 100).await.expect("all")),
                [
                    "send-old",
                    "send-accepted",
                    "send-dispatching",
                    "send-redacted"
                ]
            );
            // The window covers only the newest three frames.
            assert_eq!(
                keys(store.unsettled_send_frames(3, 100).await.expect("window")),
                ["send-accepted", "send-dispatching", "send-redacted"]
            );
            assert_eq!(
                keys(store.unsettled_send_frames(100, 2).await.expect("limit")),
                ["send-old", "send-accepted"]
            );
            let settled = store
                .frame_by_dedupe_key("send-old")
                .await
                .expect("lookup")
                .expect("present");
            store
                .update_frame_status(&settled.id, ConsoleFrameStatus::Delivered)
                .await
                .expect("settle");
            assert_eq!(
                keys(store.unsettled_send_frames(100, 100).await.expect("after")),
                ["send-accepted", "send-dispatching", "send-redacted"]
            );
        }
    }

    #[tokio::test]
    async fn sqlite_log_updates_status() {
        let store = SqliteConsoleLogStore::in_memory().expect("sqlite store");
        let first = store
            .append_if_absent(sample_frame("event-1", "agent-a"))
            .await
            .expect("append first");
        let updated = store
            .update_frame_status(&first.frame.id, ConsoleFrameStatus::DeliveryFailed)
            .await
            .expect("update")
            .expect("updated frame");
        assert_eq!(updated.status, ConsoleFrameStatus::DeliveryFailed);
        assert_eq!(updated.frame_version, 2);
        assert!(updated.updated_at_ms.is_some());
    }

    #[tokio::test]
    async fn sqlite_log_records_source_watermarks() {
        let store = SqliteConsoleLogStore::in_memory().expect("sqlite store");
        store
            .record_source_watermark("runtime-a", ConsoleFrameSourceKind::ConsoleEvent, "evt-99")
            .await
            .expect("record watermark");
        let watermark = store
            .source_watermark("runtime-a", ConsoleFrameSourceKind::ConsoleEvent)
            .await
            .expect("read watermark");
        assert_eq!(watermark.as_deref(), Some("evt-99"));
    }

    #[tokio::test]
    async fn sqlite_log_persists_frames_across_handles() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let path = temp_dir.path().join("console.sqlite");
        let store = SqliteConsoleLogStore::open(&path).expect("open first handle");
        store
            .append_if_absent(sample_frame("event-1", "agent-a"))
            .await
            .expect("append frame");
        drop(store);

        let reopened = SqliteConsoleLogStore::open(&path).expect("open second handle");
        let page = reopened
            .query_windowed_frames(ConsoleTimelineWindowQuery {
                identity: Some("agent-a".to_string()),
                limit: 10,
                ..ConsoleTimelineWindowQuery::default()
            })
            .await
            .expect("query frames");
        assert_eq!(page.frames.len(), 1);
        assert_eq!(page.frames[0].dedupe_key, "event-1");
    }

    #[tokio::test]
    async fn send_dedupe_durability_reports_the_store_actually_in_use() {
        assert_eq!(
            InMemoryConsoleLogStore::new().send_dedupe_durability(),
            ConsoleSendDedupeDurability::ProcessLifetime
        );
        assert_eq!(
            SqliteConsoleLogStore::in_memory()
                .expect("store")
                .send_dedupe_durability(),
            ConsoleSendDedupeDurability::ProcessLifetime
        );
        assert_eq!(
            LegacyQueryOnlyStore.send_dedupe_durability(),
            ConsoleSendDedupeDurability::ProcessLifetime,
            "a store that does not vouch for durability never claims it"
        );

        // A file-backed store claims durability, and the claim holds: the
        // dedupe record is still found after the store is reopened.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("console.sqlite");
        {
            let store = SqliteConsoleLogStore::open(&path).expect("store");
            assert_eq!(
                store.send_dedupe_durability(),
                ConsoleSendDedupeDurability::Durable
            );
            store
                .append_if_absent(sample_frame("send:runtime:agent:origin:key", "agent"))
                .await
                .expect("append");
        }
        let reopened = SqliteConsoleLogStore::open(&path).expect("reopen");
        assert_eq!(
            reopened.send_dedupe_durability(),
            ConsoleSendDedupeDurability::Durable
        );
        assert!(
            reopened
                .frame_by_dedupe_key("send:runtime:agent:origin:key")
                .await
                .expect("lookup")
                .is_some(),
            "a durable store must keep the send dedupe record across reopen"
        );
    }

    /// Remove frames from an in-memory store with every index updated, so
    /// the sequence has gaps.
    fn drop_in_memory_frames(store: &InMemoryConsoleLogStore, seqs: &[u64]) {
        let mut state = store.state.lock().expect("state");
        for seq in seqs {
            remove_in_memory_frame(&mut state, *seq);
        }
    }

    /// The scan the range reads replaced: every stored frame in sequence
    /// order, filtered, then windowed by the same helper.
    fn full_scan_window(
        store: &InMemoryConsoleLogStore,
        query: &ConsoleTimelineWindowQuery,
    ) -> ConsoleTimelineWindowPage {
        let state = store.state.lock().expect("state");
        let after = query.after.as_ref().and_then(ConsoleCursor::seq);
        let before = query.before.as_ref().and_then(ConsoleCursor::seq);
        let limit = normalize_limit(query.limit);
        let matches = |seq: u64, frame: &ConsoleFrame| {
            after.is_none_or(|after| seq > after)
                && before.is_none_or(|before| seq < before)
                && query
                    .identity
                    .as_deref()
                    .is_none_or(|identity| frame.identity == identity)
                && query.conversation_id.as_deref().is_none_or(|conversation| {
                    frame.conversation_id.as_deref() == Some(conversation)
                })
        };
        let seqs: Vec<u64> = state.frames.keys().copied().collect();
        in_memory_window_from_seq_iters(
            &state,
            query.mode,
            (limit, limit.saturating_add(1)),
            seqs.iter().copied(),
            seqs.iter().rev().copied(),
            seqs.iter().rev().copied(),
            &matches,
        )
    }

    /// Range reads answer exactly what the full scans they replaced
    /// answered, for every window, mode, limit and filter, with gaps in the
    /// sequence.
    #[tokio::test]
    async fn in_memory_range_reads_answer_what_the_full_scans_answered() {
        let store = InMemoryConsoleLogStore::new();
        for index in 0..40_u64 {
            let identity = if index % 3 == 0 { "alpha" } else { "beta" };
            let conversation = if index % 2 == 0 { "left" } else { "right" };
            store
                .append_if_absent(NewConsoleFrame {
                    conversation_id: Some(conversation.to_string()),
                    ..sample_frame(&format!("frame-{index}"), identity)
                })
                .await
                .expect("append");
        }
        drop_in_memory_frames(&store, &[4, 5, 6, 20, 33]);
        let cursors = [
            None,
            Some(0),
            Some(1),
            Some(5),
            Some(6),
            Some(7),
            Some(19),
            Some(20),
            Some(21),
            Some(39),
            Some(40),
            Some(41),
            Some(100),
        ];
        for after in cursors {
            for before in cursors {
                for mode in [ConsoleTimelineMode::Since, ConsoleTimelineMode::Recent] {
                    for limit in [1, 2, 3, 7, 1000] {
                        for identity in [None, Some("alpha"), Some("beta"), Some("missing")] {
                            for conversation in [None, Some("left"), Some("right")] {
                                let query = ConsoleTimelineWindowQuery {
                                    mob_id: None,
                                    identity: identity.map(str::to_string),
                                    conversation_id: conversation.map(str::to_string),
                                    after: after.map(ConsoleCursor::from_seq),
                                    before: before.map(ConsoleCursor::from_seq),
                                    mode,
                                    limit,
                                };
                                let expected = full_scan_window(&store, &query);
                                let actual = store
                                    .query_windowed_frames(query.clone())
                                    .await
                                    .expect("window read");
                                assert_eq!(actual, expected, "{query:?}");
                            }
                        }
                    }
                }
            }
        }
    }

    /// The send sweep's window is the newest frames of the mob itself, in
    /// both stores alike: the frames that are stored are counted, so rows
    /// removed inside the window leave room for older frames, and a send
    /// among them is in the window. The window stays bounded by frames.
    #[tokio::test]
    async fn the_send_sweep_windows_by_the_mobs_stored_frames_in_both_stores() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sqlite =
            SqliteConsoleLogStore::open(dir.path().join("console.sqlite3")).expect("sqlite store");
        let memory = InMemoryConsoleLogStore::new();
        let stores: [&dyn ConsoleLogStore; 2] = [&sqlite, &memory];
        for store in stores {
            store
                .append_if_absent(send_frame("send-a", ConsoleFrameStatus::Accepted))
                .await
                .expect("append");
            for index in 2..=6 {
                store
                    .append_if_absent(sample_frame(&format!("filler-{index}"), "identity:a"))
                    .await
                    .expect("append");
            }
            store
                .append_if_absent(send_frame("send-b", ConsoleFrameStatus::Accepted))
                .await
                .expect("append");
            for index in 8..=10 {
                store
                    .append_if_absent(sample_frame(&format!("filler-{index}"), "identity:a"))
                    .await
                    .expect("append");
            }
        }
        let removed = [3_u64, 4, 5, 6, 8, 9];
        drop_in_memory_frames(&memory, &removed);
        {
            let conn = sqlite.conn.lock().expect("conn");
            for seq in removed {
                conn.execute(
                    "DELETE FROM console_frames WHERE cursor_seq = ?1",
                    params![i64::try_from(seq).expect("seq")],
                )
                .expect("delete");
            }
        }
        for store in stores {
            let keys: Vec<String> = store
                .unsettled_send_frames(4, 100)
                .await
                .expect("sweep")
                .into_iter()
                .map(|frame| frame.dedupe_key)
                .collect();
            // The stored frames are at cursors 1, 2, 7 and 10: all four are
            // the window, send-a (cursor 1) with them.
            assert_eq!(keys, ["send-a", "send-b"]);
        }
    }

    const BLOCK_MESSAGE: &str = "0199f3c2-9b1e-7e2a-8c41-6d2f0a1b3c4d";
    const OTHER_MESSAGE: &str = "0199f3c2-9b1e-7e2a-8c41-6d2f0a1b3c4e";

    fn block_frame(
        dedupe_key: &str,
        identity: &str,
        kind: &str,
        message: &str,
        sequence: u64,
    ) -> NewConsoleFrame {
        NewConsoleFrame {
            kind: kind.to_string(),
            payload: json!({
                "delta": "chunk ",
                "content": "chunk chunk chunk ",
                "assistant_message_id": message,
                "source_sequence": sequence,
            }),
            ..sample_frame(dedupe_key, identity)
        }
    }

    fn provenance_witness() -> crate::ConsoleFrameMemberProvenance {
        serde_json::from_value(json!({
            "identity": {"identity":"worker", "display_name":"Worker", "runtime_key":"runtime-a",
                "runtime_member_id":"worker", "session_id":"session-1", "visibility":"addressable",
                "addressable":true, "health":"ready", "labels":{}},
            "member": {"agent_identity":"worker", "role":"worker", "state":"running",
                "session_id":"session-1", "wired_to":[], "labels":{}},
            "primary_mob_id":"primary", "source_mob_id":"primary"
        }))
        .expect("witness")
    }

    /// A streamed text block of `worker` among the rows that must outlive
    /// its compaction: the block's first delta, a provenance witness, a send,
    /// the message's reasoning, the same message under another identity, a
    /// delta of the message without a source sequence, another message, a
    /// history row, a snapshot anchor, and a delta after the complete row.
    fn compaction_rows() -> Vec<NewConsoleFrame> {
        let mut witness = sample_frame("run-start", "worker");
        witness.kind = "run_started".into();
        witness.payload = json!({});
        witness.source.member_provenance = Some(provenance_witness());
        let mut history = block_frame("history-text", "worker", "text_delta", BLOCK_MESSAGE, 0);
        history.source.kind = ConsoleFrameSourceKind::SessionHistory;
        let mut snapshot = sample_frame("notice-snapshot", "worker");
        snapshot.kind = "runtime_notice_snapshot".into();
        snapshot.payload = json!({"complete": true});
        snapshot.source.kind = ConsoleFrameSourceKind::SessionHistory;
        let mut no_sequence = block_frame("no-sequence", "worker", "text_delta", BLOCK_MESSAGE, 0);
        no_sequence
            .payload
            .as_object_mut()
            .expect("object payload")
            .remove("source_sequence");
        vec![
            witness,
            send_frame("send", ConsoleFrameStatus::Accepted),
            block_frame("reasoning-1", "worker", "reasoning_delta", BLOCK_MESSAGE, 1),
            block_frame("text-1", "worker", "text_delta", BLOCK_MESSAGE, 2),
            block_frame(
                "other-identity",
                "other-worker",
                "text_delta",
                BLOCK_MESSAGE,
                2,
            ),
            block_frame("text-2", "worker", "text_delta", BLOCK_MESSAGE, 3),
            no_sequence,
            block_frame("other-message", "worker", "text_delta", OTHER_MESSAGE, 4),
            history,
            block_frame("text-3", "worker", "text_delta", BLOCK_MESSAGE, 5),
            snapshot,
            block_frame("complete", "worker", "text_complete", BLOCK_MESSAGE, 6),
            block_frame("after", "worker", "text_delta", BLOCK_MESSAGE, 7),
        ]
    }

    async fn seed(store: &dyn ConsoleLogStore) -> BTreeMap<String, ConsoleFrame> {
        let mut stored = BTreeMap::new();
        for frame in compaction_rows() {
            let key = frame.dedupe_key.clone();
            let outcome = store.append_if_absent(frame).await.expect("append");
            stored.insert(key, outcome.frame);
        }
        stored
    }

    fn block_compaction(
        stored: &BTreeMap<String, ConsoleFrame>,
        first: &str,
        complete: &str,
    ) -> ConsoleDeltaCompaction {
        ConsoleDeltaCompaction::new(
            "runtime-a",
            "worker",
            crate::console_aggregator::ConsoleDeltaKind::Text,
            BLOCK_MESSAGE,
            stored[first].cursor.clone(),
            stored[complete].cursor.clone(),
        )
    }

    async fn all_frames(store: &dyn ConsoleLogStore) -> Vec<ConsoleFrame> {
        store
            .query_windowed_frames(ConsoleTimelineWindowQuery {
                limit: 1_000,
                ..ConsoleTimelineWindowQuery::default()
            })
            .await
            .expect("read")
            .frames
    }

    fn compaction_stores() -> (
        tempfile::TempDir,
        SqliteConsoleLogStore,
        InMemoryConsoleLogStore,
    ) {
        let dir = tempfile::tempdir().expect("tempdir");
        let sqlite =
            SqliteConsoleLogStore::open(dir.path().join("console.sqlite3")).expect("sqlite store");
        (dir, sqlite, InMemoryConsoleLogStore::new())
    }

    /// After the complete row the block's delta rows are gone, its first
    /// delta row aside, and only they: every other row stays, under its own
    /// cursor.
    #[tokio::test]
    async fn compaction_removes_only_the_blocks_delta_rows() {
        let (_dir, sqlite, memory) = compaction_stores();
        let stores: [&dyn ConsoleLogStore; 2] = [&sqlite, &memory];
        for store in stores {
            let stored = seed(store).await;
            let removed = store
                .compact_message_deltas(&block_compaction(&stored, "text-1", "complete"))
                .await
                .expect("compact");
            assert_eq!(removed, 2);
            let remaining = all_frames(store).await;
            let kept: Vec<&str> = remaining
                .iter()
                .map(|frame| frame.dedupe_key.as_str())
                .collect();
            assert_eq!(
                kept,
                [
                    "run-start",
                    "send",
                    "reasoning-1",
                    "text-1",
                    "other-identity",
                    "no-sequence",
                    "other-message",
                    "history-text",
                    "notice-snapshot",
                    "complete",
                    "after",
                ]
            );
            for frame in &remaining {
                assert_eq!(frame, &stored[&frame.dedupe_key], "rows keep their cursors");
            }
            assert_eq!(
                remaining[0].source.member_provenance,
                Some(provenance_witness()),
                "the provenance witness stays"
            );
            for removed in ["text-2", "text-3"] {
                assert!(
                    store
                        .frame_by_dedupe_key(removed)
                        .await
                        .expect("lookup")
                        .is_none()
                );
            }
            // A second removal finds nothing left to remove.
            assert_eq!(
                store
                    .compact_message_deltas(&block_compaction(&stored, "text-1", "complete"))
                    .await
                    .expect("compact again"),
                0
            );
        }
    }

    /// An interrupted block, whose complete row was never stored, keeps its
    /// delta rows, as does a block whose named complete row is another row.
    #[tokio::test]
    async fn compaction_needs_the_blocks_stored_complete_row() {
        let (_dir, sqlite, memory) = compaction_stores();
        let stores: [&dyn ConsoleLogStore; 2] = [&sqlite, &memory];
        for store in stores {
            let stored = seed(store).await;
            let before = all_frames(store).await;
            let refused = [
                // No complete row: the last delta stands in for it.
                block_compaction(&stored, "text-1", "text-3"),
                // The other message's delta, a snapshot, a history row.
                block_compaction(&stored, "text-1", "other-message"),
                block_compaction(&stored, "text-1", "notice-snapshot"),
                block_compaction(&stored, "text-1", "history-text"),
                // An empty range.
                block_compaction(&stored, "complete", "complete"),
                block_compaction(&stored, "after", "complete"),
            ];
            for compaction in refused {
                assert_eq!(
                    store
                        .compact_message_deltas(&compaction)
                        .await
                        .expect("compact"),
                    0,
                    "{compaction:?}"
                );
            }
            // The other message's complete row does not supersede this block.
            let mut wrong_message = block_compaction(&stored, "text-1", "complete");
            wrong_message.assistant_message_id = OTHER_MESSAGE.to_string();
            assert_eq!(
                store
                    .compact_message_deltas(&wrong_message)
                    .await
                    .expect("compact"),
                0
            );
            assert_eq!(all_frames(store).await, before);
        }
    }

    /// A read from a cursor inside the removed range still works: since
    /// returns the complete row next, recent the rows before it, and the
    /// newest cursor and the prefix revision do not move.
    #[tokio::test]
    async fn reads_from_a_cursor_inside_the_removed_range_reach_the_complete_row() {
        let (_dir, sqlite, memory) = compaction_stores();
        let stores: [&dyn ConsoleLogStore; 2] = [&sqlite, &memory];
        for store in stores {
            let stored = seed(store).await;
            let latest = store.latest_cursor().await.expect("latest");
            let revision = store.history_prefix_revision().await.expect("revision");
            store
                .compact_message_deltas(&block_compaction(&stored, "text-1", "complete"))
                .await
                .expect("compact");
            assert_eq!(store.latest_cursor().await.expect("latest"), latest);
            assert_eq!(
                store.history_prefix_revision().await.expect("revision"),
                revision
            );
            let since = store
                .query_windowed_frames(ConsoleTimelineWindowQuery {
                    identity: Some("worker".into()),
                    after: Some(stored["text-2"].cursor.clone()),
                    limit: 1_000,
                    ..ConsoleTimelineWindowQuery::default()
                })
                .await
                .expect("since read");
            let keys: Vec<&str> = since
                .frames
                .iter()
                .map(|frame| frame.dedupe_key.as_str())
                .collect();
            assert_eq!(
                keys,
                [
                    "no-sequence",
                    "other-message",
                    "history-text",
                    "notice-snapshot",
                    "complete",
                    "after"
                ]
            );
            assert_eq!(since.frames[4], stored["complete"]);
            let recent = store
                .query_windowed_frames(ConsoleTimelineWindowQuery {
                    identity: Some("worker".into()),
                    before: Some(stored["text-3"].cursor.clone()),
                    mode: ConsoleTimelineMode::Recent,
                    limit: 2,
                    ..ConsoleTimelineWindowQuery::default()
                })
                .await
                .expect("recent read");
            let keys: Vec<&str> = recent
                .frames
                .iter()
                .map(|frame| frame.dedupe_key.as_str())
                .collect();
            assert_eq!(keys, ["other-message", "history-text"]);
        }
    }

    /// The in-memory store frees what it removes: the frames and every
    /// index entry naming them.
    #[tokio::test]
    async fn the_in_memory_store_shrinks_with_compaction() {
        let store = InMemoryConsoleLogStore::new();
        let stored = seed(&store).await;
        let removed: Vec<u64> = ["text-2", "text-3"]
            .into_iter()
            .map(|key| stored[key].cursor.seq().expect("seq"))
            .collect();
        let before = store.state.lock().expect("state").frames.len();
        store
            .compact_message_deltas(&block_compaction(&stored, "text-1", "complete"))
            .await
            .expect("compact");
        let state = store.state.lock().expect("state");
        assert_eq!(state.frames.len(), before - removed.len());
        for (seq, key) in removed.iter().zip(["text-2", "text-3"]) {
            assert!(!state.frames.contains_key(seq));
            assert!(!state.dedupe_to_seq.contains_key(key));
            assert!(!state.id_to_seq.contains_key(&stored[key].id));
            assert!(!state.identity_to_seqs["worker"].contains(seq));
            assert!(!state.conversation_to_seqs["worker"].contains(seq));
        }
        assert_eq!(state.dedupe_to_seq.len(), state.frames.len());
        assert_eq!(state.id_to_seq.len(), state.frames.len());
    }

    fn member_keyed_frame(dedupe_key: &str, mob_id: Option<&str>, at: u64) -> NewConsoleFrame {
        let mut frame = activity_frame(dedupe_key, "alice", at, ConsoleFrameSourceKind::Send);
        frame.mob_id = mob_id.map(meerkat_mob::MobId::from);
        frame.payload = json!({ "text": dedupe_key });
        frame
    }

    async fn member_keyed_frames_of(
        store: &dyn ConsoleLogStore,
        mob_id: Option<&str>,
    ) -> Vec<String> {
        let page = store
            .query_windowed_frames(ConsoleTimelineWindowQuery {
                identity: Some("alice".to_string()),
                mob_id: mob_id.map(meerkat_mob::MobId::from),
                ..Default::default()
            })
            .await
            .expect("window");
        let frames = store
            .query_frames(ConsoleTimelineQuery {
                identity: Some("alice".to_string()),
                mob_id: mob_id.map(meerkat_mob::MobId::from),
                ..Default::default()
            })
            .await
            .expect("query")
            .frames;
        assert_eq!(page.frames, frames, "both reads name the same member");
        assert_eq!(
            page.latest_cursor,
            frames.last().map(|frame| frame.cursor.clone()),
            "the newest cursor is the member's own"
        );
        frames.into_iter().map(|frame| frame.dedupe_key).collect()
    }

    /// The frame of the mob `mob_id` (none: the default mob) that holds
    /// `key`, read by the read of that one mob. The read by key names no
    /// mob and answers the default mob's frames alone.
    async fn member_keyed_frame_of_mob_by_key(
        store: &dyn ConsoleLogStore,
        mob_id: Option<&str>,
        key: &str,
    ) -> Option<ConsoleFrame> {
        store
            .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
                mob_id: mob_id.map(meerkat_mob::MobId::from),
                limit: 1_000,
                ..Default::default()
            })
            .await
            .expect("the read of one mob")
            .frames
            .into_iter()
            .find(|frame| frame.dedupe_key == key)
    }

    /// Two mobs of one host each have a member named `alice`. Their frames
    /// are stored apart: a read of one member answers none of the other's,
    /// the last activity of one is not the other's, and a frame that names
    /// no mob is the default mob's.
    async fn member_keyed_frames_stay_apart(store: &dyn ConsoleLogStore) {
        for frame in [
            member_keyed_frame("launch-1", None, 100),
            member_keyed_frame("second-1", Some("second"), 200),
            member_keyed_frame("launch-2", None, 300),
            member_keyed_frame("second-2", Some("second"), 400),
        ] {
            store.append_if_absent(frame).await.expect("append");
        }
        assert_eq!(
            member_keyed_frames_of(store, None).await,
            ["launch-1", "launch-2"]
        );
        assert_eq!(
            member_keyed_frames_of(store, Some("second")).await,
            ["second-1", "second-2"]
        );
        assert!(
            member_keyed_frames_of(store, Some("third"))
                .await
                .is_empty()
        );
        // The newest frame of the name is the second mob's; the default
        // mob's alice last acted before it.
        let alice = ["alice".to_string()];
        assert_eq!(
            store.latest_activity_times(&alice).await.expect("times"),
            HashMap::from([("alice".to_string(), 300)])
        );
        assert_eq!(
            store
                .latest_activity_times_in_mob("second", &alice)
                .await
                .expect("times"),
            HashMap::from([("alice".to_string(), 400)])
        );
        assert!(
            store
                .latest_activity_times_in_mob("third", &alice)
                .await
                .expect("times")
                .is_empty()
        );
    }

    /// A stored frame answers the mob it was appended with, and a frame of
    /// the default mob's member is written as it always was.
    async fn member_keyed_frame_answers_its_mob(store: &dyn ConsoleLogStore) {
        let appended = store
            .append_if_absent(member_keyed_frame("second-1", Some("second"), 200))
            .await
            .expect("append")
            .frame;
        assert_eq!(
            appended.mob_id.as_ref().map(meerkat_mob::MobId::as_str),
            Some("second")
        );
        let second = member_keyed_frame_of_mob_by_key(store, Some("second"), "second-1")
            .await
            .expect("frame");
        assert_eq!(
            second.mob_id.as_ref().map(meerkat_mob::MobId::as_str),
            Some("second")
        );
        // A blank mob names none.
        for (key, mob_id) in [("launch-1", None), ("launch-2", Some(""))] {
            store
                .append_if_absent(member_keyed_frame(key, mob_id, 100))
                .await
                .expect("append");
            let launch = store
                .frame_by_dedupe_key(key)
                .await
                .expect("lookup")
                .expect("frame");
            assert_eq!(launch.mob_id, None);
            assert!(
                !serde_json::to_string(&launch)
                    .expect("frame serializes")
                    .contains("mob_id"),
                "a frame of the default mob is written as it always was"
            );
        }
    }

    #[tokio::test]
    async fn member_keyed_frames_a_stored_frame_answers_its_mob_in_sqlite() {
        let store = SqliteConsoleLogStore::in_memory().expect("sqlite");
        member_keyed_frame_answers_its_mob(&store).await;
    }

    #[tokio::test]
    async fn member_keyed_frames_a_stored_frame_answers_its_mob_in_memory() {
        let store = InMemoryConsoleLogStore::new();
        member_keyed_frame_answers_its_mob(&store).await;
    }

    #[tokio::test]
    async fn member_keyed_frames_two_mobs_members_of_one_name_stay_apart_in_sqlite() {
        let store = SqliteConsoleLogStore::in_memory().expect("sqlite");
        member_keyed_frames_stay_apart(&store).await;
    }

    #[tokio::test]
    async fn member_keyed_frames_two_mobs_members_of_one_name_stay_apart_in_memory() {
        let store = InMemoryConsoleLogStore::new();
        member_keyed_frames_stay_apart(&store).await;
    }

    /// The frames the read of one mob answers for `mob_id` (none: the
    /// default mob) when it names no member: both modes answer the same
    /// frames, and the newest cursor is that mob's own.
    async fn member_keyed_frames_of_mob(
        store: &dyn ConsoleLogStore,
        mob_id: Option<&str>,
    ) -> Vec<String> {
        let of_mob = |mode: ConsoleTimelineMode| ConsoleTimelineWindowQuery {
            mob_id: mob_id.map(meerkat_mob::MobId::from),
            mode,
            ..Default::default()
        };
        let since = store
            .query_windowed_frames_of_mob(of_mob(ConsoleTimelineMode::Since))
            .await
            .expect("since");
        let recent = store
            .query_windowed_frames_of_mob(of_mob(ConsoleTimelineMode::Recent))
            .await
            .expect("recent");
        assert_eq!(since.frames, recent.frames, "both modes read the same mob");
        assert_eq!(
            since.latest_cursor,
            since.frames.last().map(|frame| frame.cursor.clone()),
            "the newest cursor is the mob's own"
        );
        since
            .frames
            .into_iter()
            .map(|frame| frame.dedupe_key)
            .collect()
    }

    /// The read of one mob answers that mob's frames, whoever their member,
    /// and none of another mob's: the default mob's for a query that names
    /// no mob, another mob's for a query that names it, none for a mob that
    /// has no frame. With an identity it is the member's read. The plain
    /// read that names no mob answers the default mob's frames alone.
    async fn member_keyed_frames_one_mobs_read_answers_that_mob_alone(store: &dyn ConsoleLogStore) {
        let of = |key: &str, identity: &str, mob_id: Option<&str>, at: u64| {
            let mut frame = activity_frame(key, identity, at, ConsoleFrameSourceKind::Send);
            frame.mob_id = mob_id.map(meerkat_mob::MobId::from);
            frame
        };
        for frame in [
            of("launch-alice-1", "alice", None, 100),
            of("second-alice-1", "alice", Some("second"), 200),
            of("launch-carol-1", "carol", None, 300),
            of("second-bob-1", "bob", Some("second"), 400),
            of("launch-alice-2", "alice", None, 500),
        ] {
            store.append_if_absent(frame).await.expect("append");
        }
        assert_eq!(
            member_keyed_frames_of_mob(store, None).await,
            ["launch-alice-1", "launch-carol-1", "launch-alice-2"],
            "the default mob's frames, and none of another mob's"
        );
        assert_eq!(
            member_keyed_frames_of_mob(store, Some("second")).await,
            ["second-alice-1", "second-bob-1"],
            "the second mob's frames, and none of the default mob's"
        );
        assert!(
            member_keyed_frames_of_mob(store, Some("third"))
                .await
                .is_empty(),
            "a mob that has no frame"
        );
        let of_member = store
            .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
                identity: Some("alice".to_string()),
                mob_id: Some(meerkat_mob::MobId::from("second")),
                ..Default::default()
            })
            .await
            .expect("member");
        assert_eq!(
            of_member
                .frames
                .iter()
                .map(|frame| frame.dedupe_key.as_str())
                .collect::<Vec<_>>(),
            ["second-alice-1"],
            "with an identity it is the member's read"
        );
        let plain = store
            .query_windowed_frames(ConsoleTimelineWindowQuery::default())
            .await
            .expect("plain");
        assert_eq!(
            plain
                .frames
                .iter()
                .map(|frame| frame.dedupe_key.as_str())
                .collect::<Vec<_>>(),
            ["launch-alice-1", "launch-carol-1", "launch-alice-2"],
            "the plain read that names no mob reads the default mob's frames alone"
        );
    }

    #[tokio::test]
    async fn member_keyed_frames_one_mobs_read_answers_that_mob_alone_in_sqlite() {
        let store = SqliteConsoleLogStore::in_memory().expect("sqlite");
        // A log that never stored another mob's frame has no mob column:
        // every frame in it is the default mob's.
        store
            .append_if_absent(member_keyed_frame("before-the-column", None, 50))
            .await
            .expect("append");
        assert_eq!(
            member_keyed_frames_of_mob(&store, None).await,
            ["before-the-column"]
        );
        assert!(
            member_keyed_frames_of_mob(&store, Some("second"))
                .await
                .is_empty()
        );
        let store = SqliteConsoleLogStore::in_memory().expect("sqlite");
        member_keyed_frames_one_mobs_read_answers_that_mob_alone(&store).await;
    }

    #[tokio::test]
    async fn member_keyed_frames_one_mobs_read_answers_that_mob_alone_in_memory() {
        let store = InMemoryConsoleLogStore::new();
        member_keyed_frames_one_mobs_read_answers_that_mob_alone(&store).await;
    }

    /// A store that does not implement the read of one mob refuses it: it
    /// never answers the frames of every mob in its place.
    #[tokio::test]
    async fn member_keyed_frames_a_store_without_the_one_mob_read_refuses_it() {
        let refused = LegacyQueryOnlyStore
            .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery::default())
            .await
            .expect_err("the default refuses");
        assert!(
            refused.to_string().contains("query_windowed_frames_of_mob"),
            "{refused}"
        );
    }

    /// A streamed block's delta rows are removed for the member they belong
    /// to: the same block of the member of that name in another mob stays.
    async fn member_keyed_compaction_keeps_the_other_mobs_rows(store: &dyn ConsoleLogStore) {
        let mut cursors = BTreeMap::new();
        for (key, mob_id, kind) in [
            ("launch-d1", None, "text_delta"),
            ("second-d1", Some("second"), "text_delta"),
            ("launch-d2", None, "text_delta"),
            ("second-d2", Some("second"), "text_delta"),
            ("second-complete", Some("second"), "text_complete"),
            ("launch-complete", None, "text_complete"),
        ] {
            let mut frame = sample_frame(key, "alice");
            frame.mob_id = mob_id.map(meerkat_mob::MobId::from);
            frame.kind = kind.to_string();
            frame.payload = json!({ "assistant_message_id": "message-1", "source_sequence": 1 });
            let stored = store.append_if_absent(frame).await.expect("append").frame;
            cursors.insert(key, stored.cursor);
        }
        let removed = store
            .compact_message_deltas(&ConsoleDeltaCompaction::new(
                "runtime-a",
                "alice",
                crate::console_aggregator::ConsoleDeltaKind::Text,
                "message-1",
                cursors["launch-d1"].clone(),
                cursors["launch-complete"].clone(),
            ))
            .await
            .expect("compaction");
        assert_eq!(removed, 1, "only the default mob's later delta row goes");
        for key in ["second-d1", "second-d2", "second-complete"] {
            assert!(
                member_keyed_frame_of_mob_by_key(store, Some("second"), key)
                    .await
                    .is_some(),
                "{key} stays"
            );
        }
        assert!(
            store
                .frame_by_dedupe_key("launch-d1")
                .await
                .expect("lookup")
                .is_some(),
            "launch-d1 stays"
        );
        assert!(
            store
                .frame_by_dedupe_key("launch-d2")
                .await
                .expect("lookup")
                .is_none()
        );
        // A complete row of the member of that name in another mob
        // supersedes nothing of this member's.
        let removed = store
            .compact_message_deltas(
                &ConsoleDeltaCompaction::new(
                    "runtime-a",
                    "alice",
                    crate::console_aggregator::ConsoleDeltaKind::Text,
                    "message-1",
                    cursors["second-d1"].clone(),
                    cursors["launch-complete"].clone(),
                )
                .in_mob(Some(meerkat_mob::MobId::from("second"))),
            )
            .await
            .expect("compaction");
        assert_eq!(removed, 0, "the default mob's complete row is not hers");
        let removed = store
            .compact_message_deltas(
                &ConsoleDeltaCompaction::new(
                    "runtime-a",
                    "alice",
                    crate::console_aggregator::ConsoleDeltaKind::Text,
                    "message-1",
                    cursors["second-d1"].clone(),
                    cursors["second-complete"].clone(),
                )
                .in_mob(Some(meerkat_mob::MobId::from("second"))),
            )
            .await
            .expect("compaction");
        assert_eq!(removed, 1, "the second mob's own later delta row goes");
    }

    #[tokio::test]
    async fn member_keyed_frames_compaction_keeps_the_other_mobs_rows_in_sqlite() {
        let store = SqliteConsoleLogStore::in_memory().expect("sqlite");
        member_keyed_compaction_keeps_the_other_mobs_rows(&store).await;
    }

    #[tokio::test]
    async fn member_keyed_frames_compaction_keeps_the_other_mobs_rows_in_memory() {
        let store = InMemoryConsoleLogStore::new();
        member_keyed_compaction_keeps_the_other_mobs_rows(&store).await;
    }

    /// The console log's ledger version in the file at `path`.
    fn member_keyed_ledger_version(path: &Path) -> Option<i64> {
        let conn = Connection::open(path).expect("probe");
        meerkat_sqlite::domain_version(&conn, META_CONSOLE_DOMAIN.name).expect("ledger")
    }

    /// Whether the file at `path` has the catalog object, or the frame
    /// column, `name`.
    fn member_keyed_log_has(path: &Path, name: &str) -> bool {
        let conn = Connection::open(path).expect("probe");
        let found: i64 = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM sqlite_master WHERE name = ?1)
                      + (SELECT COUNT(*) FROM pragma_table_info('console_frames') WHERE name = ?1)",
                params![name],
                |row| row.get(0),
            )
            .expect("catalog");
        found > 0
    }

    /// The console log as the build before frames named a mob knows it:
    /// schema versions 1 to 3.
    fn member_keyed_earlier_build() -> meerkat_sqlite::SchemaDomain {
        META_CONSOLE_UNKEYED_DOMAIN
    }

    /// A host that names no mob keeps the console log it had: the file
    /// stays at schema version 3, without the mob column, through appends
    /// and reads, so the build before frames named a mob still opens it.
    #[tokio::test]
    async fn member_keyed_frames_a_host_that_names_no_mob_keeps_a_version_3_log() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("console.sqlite");
        let store = SqliteConsoleLogStore::open(&path).expect("open");
        store
            .append_if_absent(member_keyed_frame("launch-1", None, 100))
            .await
            .expect("append");
        assert_eq!(member_keyed_frames_of(&store, None).await, ["launch-1"]);
        assert!(
            member_keyed_frames_of(&store, Some("second"))
                .await
                .is_empty()
        );
        assert_eq!(
            store
                .latest_activity_times(&["alice".to_string()])
                .await
                .expect("times"),
            HashMap::from([("alice".to_string(), 100)])
        );
        assert!(
            store
                .latest_activity_times_in_mob("second", &["alice".to_string()])
                .await
                .expect("times")
                .is_empty()
        );
        drop(store);
        assert_eq!(member_keyed_ledger_version(&path), Some(3));
        assert!(!member_keyed_log_has(&path, "mob_id"));

        let mut conn = meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::PRIMARY)
            .expect("sqlite");
        let opened =
            meerkat_sqlite::apply_domain_migrations(&mut conn, &member_keyed_earlier_build())
                .expect("the earlier build opens the log");
        assert!(!opened.migrated());
        drop(conn);

        let reopened = SqliteConsoleLogStore::open(&path).expect("reopen");
        assert_eq!(member_keyed_frames_of(&reopened, None).await, ["launch-1"]);
        drop(reopened);
        assert_eq!(member_keyed_ledger_version(&path), Some(3));
    }

    /// The log moves to schema version 4 when it first stores a frame of a
    /// member of another mob, and not before: a read that names another mob
    /// finds nothing and changes nothing. The move adds the column and the
    /// member index, rewrites no row, and from then on the earlier build
    /// refuses the file instead of reading another mob's frames as the
    /// default mob's.
    #[tokio::test]
    async fn member_keyed_frames_the_first_frame_of_another_mob_moves_the_log_to_version_4() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("console.sqlite");
        let store = SqliteConsoleLogStore::open(&path).expect("open");
        store
            .append_if_absent(member_keyed_frame("launch-1", None, 100))
            .await
            .expect("append");
        assert!(
            member_keyed_frames_of(&store, Some("second"))
                .await
                .is_empty()
        );
        assert_eq!(member_keyed_ledger_version(&path), Some(3));

        store
            .append_if_absent(member_keyed_frame("second-1", Some("second"), 200))
            .await
            .expect("append");
        assert_eq!(member_keyed_ledger_version(&path), Some(4));
        assert!(member_keyed_log_has(&path, "mob_id"));
        assert!(member_keyed_log_has(
            &path,
            "idx_console_frames_mob_identity_cursor"
        ));
        assert!(
            member_keyed_log_has(&path, "idx_console_frames_mob_cursor"),
            "a moved log has the index of one mob's frames in cursor order"
        );
        assert!(!member_keyed_log_has(
            &path,
            "idx_console_frames_identity_cursor"
        ));
        assert_eq!(member_keyed_frames_of(&store, None).await, ["launch-1"]);
        assert_eq!(
            member_keyed_frames_of(&store, Some("second")).await,
            ["second-1"]
        );
        drop(store);

        let mut conn = meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::PRIMARY)
            .expect("sqlite");
        let refused =
            meerkat_sqlite::apply_domain_migrations(&mut conn, &member_keyed_earlier_build())
                .expect_err("the earlier build refuses a version 4 log");
        assert!(
            matches!(
                refused,
                meerkat_sqlite::SqliteStoreError::SchemaFromTheFuture {
                    found: 4,
                    supported: 3,
                    ..
                }
            ),
            "{refused}"
        );
        drop(conn);

        let reopened = SqliteConsoleLogStore::open(&path).expect("reopen at version 4");
        assert_eq!(member_keyed_frames_of(&reopened, None).await, ["launch-1"]);
        assert_eq!(
            member_keyed_frames_of(&reopened, Some("second")).await,
            ["second-1"]
        );
    }

    /// A member's timeline page is read through the index on its mob, its
    /// identity and the cursor, in cursor order: the read does not walk the
    /// frames of the members of the same name in other mobs, and does not
    /// sort.
    #[tokio::test]
    async fn member_keyed_frames_a_members_timeline_read_seeks_the_mob_identity_cursor_index() {
        let store = SqliteConsoleLogStore::in_memory().expect("sqlite store");
        store
            .append_if_absent(member_keyed_frame("second-1", Some("second"), 1))
            .await
            .expect("append");
        let conn = store.conn.lock().expect("connection");
        for mode in [ConsoleTimelineMode::Since, ConsoleTimelineMode::Recent] {
            let sql = member_sql(&conn, &timeline_page_sql(true, true, false, &mode));
            assert!(sql.contains("AND mob_id = ?4"), "{sql}");
            let plan = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .expect("prepare query plan")
                .query_map(params![0i64, i64::MAX, "alice", "second", 10i64], |row| {
                    row.get::<_, String>(3)
                })
                .expect("run query plan")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect query plan")
                .join("\n")
                .to_lowercase();
            assert!(
                plan.contains("idx_console_frames_mob_identity_cursor"),
                "the read must seek the member index; plan was: {plan}"
            );
            assert!(
                !plan.contains("temp b-tree"),
                "the index order must serve the read without a sort; plan was: {plan}"
            );
        }
    }

    /// The read of one mob with no identity is read through the index on
    /// the mob and the cursor, in cursor order: it walks that mob's frames
    /// alone, never the frames of every mob, and does not sort. Without
    /// that index the planner takes the member index and sorts every frame
    /// of the mob for one page.
    #[tokio::test]
    async fn member_keyed_frames_the_read_of_one_mob_seeks_the_mob_cursor_index() {
        let store = SqliteConsoleLogStore::in_memory().expect("sqlite store");
        store
            .append_if_absent(member_keyed_frame("second-1", Some("second"), 1))
            .await
            .expect("append");
        let conn = store.conn.lock().expect("connection");
        for mode in [ConsoleTimelineMode::Since, ConsoleTimelineMode::Recent] {
            let sql = member_sql(&conn, &timeline_page_sql(false, true, false, &mode));
            assert!(sql.contains("AND mob_id = ?3"), "{sql}");
            for mob in ["", "second"] {
                let plan = conn
                    .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                    .expect("prepare query plan")
                    .query_map(params![0i64, i64::MAX, mob, 10i64], |row| {
                        row.get::<_, String>(3)
                    })
                    .expect("run query plan")
                    .collect::<Result<Vec<_>, _>>()
                    .expect("collect query plan")
                    .join("\n")
                    .to_lowercase();
                assert!(
                    plan.contains("idx_console_frames_mob_cursor"),
                    "the read of the mob {mob:?} must seek the mob index; plan was: {plan}"
                );
                assert!(
                    !plan.contains("temp b-tree"),
                    "the index order must serve the read without a sort; plan was: {plan}"
                );
            }
        }
    }

    /// The schema domain of version 4 moves no log. Handed a log at version
    /// 3 it refuses it, and the file keeps its version, its tables and its
    /// indexes.
    #[test]
    fn the_version_4_domain_refuses_a_log_at_version_3_and_leaves_it() {
        fn declared(conn: &Connection) -> Vec<String> {
            conn.prepare(
                "SELECT type || ' ' || name || ': ' || COALESCE(sql, '') FROM sqlite_master \
                 ORDER BY type, name",
            )
            .expect("catalog")
            .query_map([], |row| row.get::<_, String>(0))
            .expect("catalog rows")
            .collect::<Result<_, _>>()
            .expect("catalog rows")
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("console.sqlite3");
        drop(SqliteConsoleLogStore::open(&path).expect("a log at version 3"));
        let mut conn = meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::PRIMARY)
            .expect("sqlite");
        let before = declared(&conn);

        let refused = meerkat_sqlite::apply_domain_migrations(&mut conn, &META_CONSOLE_DOMAIN);

        assert!(
            matches!(
                refused,
                Err(
                    meerkat_sqlite::SqliteStoreError::UnsupportedSchemaPredecessor {
                        found: 3,
                        supported: 4,
                        ..
                    }
                )
            ),
            "{refused:?}"
        );
        assert_eq!(
            meerkat_sqlite::domain_version(&conn, "meta-console").expect("ledger"),
            Some(3)
        );
        assert_eq!(declared(&conn), before);
    }

    /// On a log at schema version 4 the default mob's newest cursor is read
    /// from the mob's cursor index: no sort, and no frame of another mob is
    /// read on the way.
    #[test]
    fn the_newest_cursor_of_a_version_4_log_reads_the_mobs_cursor_index() {
        let mut conn = Connection::open_in_memory().expect("sqlite");
        let tx = conn.transaction().expect("transaction");
        initialize_v4_console_schema(&tx).expect("the schema of version 4");
        tx.commit().expect("commit");
        let plan = conn
            .prepare(&format!(
                "EXPLAIN QUERY PLAN {}",
                render_member_sql(NEWEST_OF_THE_DEFAULT_MOB, true)
            ))
            .expect("prepare query plan")
            .query_map([], |row| row.get::<_, String>(3))
            .expect("run query plan")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect query plan")
            .join("\n")
            .to_lowercase();
        assert!(
            plan.contains("idx_console_frames_mob_cursor"),
            "the read must seek the mob's cursor index; plan was: {plan}"
        );
        assert!(
            !plan.contains("temp b-tree"),
            "the index order must serve the read without a sort; plan was: {plan}"
        );
    }

    /// On a log at schema version 4 the last-activity walk reads a member
    /// through the member index, newest first: no sort and no scan of the
    /// log, as on a log that was not moved.
    #[test]
    fn latest_activity_walk_on_a_version_4_log_reads_the_member_index_in_order() {
        let mut conn = Connection::open_in_memory().expect("sqlite");
        let tx = conn.transaction().expect("transaction");
        initialize_v4_console_schema(&tx).expect("the schema of version 4");
        tx.commit().expect("commit");
        let plan = conn
            .prepare(&format!(
                "EXPLAIN QUERY PLAN {}",
                member_sql(&conn, &latest_activity_sql())
            ))
            .expect("prepare query plan")
            .query_map(params!["archivist", ""], |row| row.get::<_, String>(3))
            .expect("run query plan")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect query plan")
            .join("\n")
            .to_lowercase();
        assert!(
            plan.contains("idx_console_frames_mob_identity_cursor"),
            "the walk must seek the member index; plan was: {plan}"
        );
        assert!(
            !plan.contains("temp b-tree"),
            "the index order must serve the walk without a sort; plan was: {plan}"
        );
    }

    /// A console log written before frames named a mob (schema version 3)
    /// opens as it is and its frames read as the default mob's: a read of
    /// that member finds them, a read of the member of the same name in
    /// another mob does not. It gains the column, without a rewritten row,
    /// when a frame of another mob is first stored, and its frames are
    /// still the default mob's.
    #[tokio::test]
    async fn member_keyed_frames_stored_before_the_column_are_the_default_mobs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("console.sqlite");
        {
            let mut conn = meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::PRIMARY)
                .expect("sqlite");
            let released_v3 = member_keyed_earlier_build();
            meerkat_sqlite::apply_domain_migrations(&mut conn, &released_v3)
                .expect("ledgered released v3");
            conn.execute(
                "INSERT INTO console_frames (id, dedupe_key, timestamp_ms, runtime_key, identity, \
                 kind, status, payload_json, source_kind) VALUES ('stored', 'stored', 7, \
                 'runtime-a', 'alice', 'user_input', 'completed', '{}', 'send')",
                [],
            )
            .expect("row of the released schema");
            let columns: Vec<String> = conn
                .prepare("SELECT name FROM pragma_table_info('console_frames')")
                .expect("columns")
                .query_map([], |row| row.get(0))
                .expect("columns")
                .collect::<Result<_, _>>()
                .expect("columns");
            assert!(!columns.contains(&"mob_id".to_string()));
        }
        let store = SqliteConsoleLogStore::open(&path).expect("v3 opens");
        assert_eq!(member_keyed_frames_of(&store, None).await, ["stored"]);
        assert!(
            member_keyed_frames_of(&store, Some("second"))
                .await
                .is_empty()
        );
        let stored = store
            .frame_by_dedupe_key("stored")
            .await
            .expect("lookup")
            .expect("frame");
        assert_eq!(stored.mob_id, None);
        assert_eq!(
            store
                .latest_activity_times(&["alice".to_string()])
                .await
                .expect("times"),
            HashMap::from([("alice".to_string(), 7)])
        );
        store
            .append_if_absent(member_keyed_frame("second-1", Some("second"), 9))
            .await
            .expect("append");
        drop(store);
        let reopened = SqliteConsoleLogStore::open(&path).expect("reopen at v4");
        assert_eq!(member_keyed_frames_of(&reopened, None).await, ["stored"]);
        assert_eq!(
            member_keyed_frames_of(&reopened, Some("second")).await,
            ["second-1"]
        );
    }

    /// A build that knows the console log only up to schema version 3
    /// refuses a log that holds a frame of another mob, by the ledger's own
    /// rule: it never reads that frame as the default mob's.
    #[tokio::test]
    async fn member_keyed_frames_an_earlier_build_refuses_the_upgraded_log() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("console.sqlite");
        let store = SqliteConsoleLogStore::open(&path).expect("open");
        store
            .append_if_absent(member_keyed_frame("second-1", Some("second"), 1))
            .await
            .expect("append");
        drop(store);
        let mut conn = meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::PRIMARY)
            .expect("sqlite");
        let released_v3 = member_keyed_earlier_build();
        let refused = meerkat_sqlite::apply_domain_migrations(&mut conn, &released_v3)
            .expect_err("a version 3 build refuses a version 4 log");
        assert!(
            matches!(
                refused,
                meerkat_sqlite::SqliteStoreError::SchemaFromTheFuture {
                    found: 4,
                    supported: 3,
                    ..
                }
            ),
            "{refused}"
        );
    }

    /// The move to schema version 4 and the frame that asked for it are two
    /// commits. When the frame is not stored after the move (its insert
    /// fails, or the process ends between the two), the log is at version 4
    /// and holds no frame of another mob. That log is the default mob's log
    /// with one more column: the earlier build refuses it without reading
    /// it, this build opens it, every frame in it is the default mob's, the
    /// default mob's appends and reads answer as before, and the frame is
    /// stored when it is sent again.
    #[tokio::test]
    async fn member_keyed_frames_a_move_whose_frame_was_not_stored_leaves_the_default_mobs_log() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("console.sqlite");
        let store = SqliteConsoleLogStore::open(&path).expect("open");
        let launch = store
            .append_if_absent(member_keyed_frame("launch-1", None, 100))
            .await
            .expect("append")
            .frame;
        // The first frame of another mob moves the log, and its own insert
        // fails: it claims the id of a stored frame.
        let mut clashing = member_keyed_frame("second-1", Some("second"), 200);
        clashing.id = Some(launch.id.clone());
        assert!(
            store.append_if_absent(clashing).await.is_err(),
            "the id is taken"
        );
        drop(store);
        assert_eq!(member_keyed_ledger_version(&path), Some(4));

        let mut conn = meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::PRIMARY)
            .expect("sqlite");
        let refused =
            meerkat_sqlite::apply_domain_migrations(&mut conn, &member_keyed_earlier_build())
                .expect_err("the earlier build refuses a version 4 log");
        assert!(
            matches!(
                refused,
                meerkat_sqlite::SqliteStoreError::SchemaFromTheFuture {
                    found: 4,
                    supported: 3,
                    ..
                }
            ),
            "{refused}"
        );
        drop(conn);

        let store = SqliteConsoleLogStore::open(&path).expect("reopen at version 4");
        assert!(
            store
                .frame_by_dedupe_key("second-1")
                .await
                .expect("lookup")
                .is_none()
        );
        assert_eq!(member_keyed_frames_of(&store, None).await, ["launch-1"]);
        assert!(
            member_keyed_frames_of(&store, Some("second"))
                .await
                .is_empty()
        );
        let alice = ["alice".to_string()];
        assert_eq!(
            store.latest_activity_times(&alice).await.expect("times"),
            HashMap::from([("alice".to_string(), 100)])
        );
        assert!(
            store
                .latest_activity_times_in_mob("second", &alice)
                .await
                .expect("times")
                .is_empty()
        );
        let appended = store
            .append_if_absent(member_keyed_frame("launch-2", None, 300))
            .await
            .expect("append")
            .frame;
        assert_eq!(appended.mob_id, None);
        let again = store
            .append_if_absent(member_keyed_frame("second-1", Some("second"), 200))
            .await
            .expect("the frame is stored when it is sent again");
        assert!(matches!(again.disposition, AppendDisposition::Inserted));
        assert_eq!(
            member_keyed_frames_of(&store, None).await,
            ["launch-1", "launch-2"]
        );
        assert_eq!(
            member_keyed_frames_of(&store, Some("second")).await,
            ["second-1"]
        );
    }

    /// A statement names the frame's mob column by a placeholder of its own,
    /// and only that is read as the empty mob on a log without the column.
    /// Anything else in its text that holds the same word (a JSON path,
    /// another column) is the statement's own and stays as written.
    #[tokio::test]
    async fn member_keyed_frames_a_statement_keeps_its_other_uses_of_the_word_on_a_version_3_log() {
        let store = SqliteConsoleLogStore::in_memory().expect("sqlite");
        let mut frame = member_keyed_frame("launch-1", None, 100);
        frame.payload = json!({ "source_mob_id": "lower" });
        store.append_if_absent(frame).await.expect("append");
        let read = |sql: &str| -> (Option<String>, String) {
            let conn = store.conn.lock().expect("connection");
            let sql = member_sql(&conn, sql);
            conn.query_row(&sql, [], |row| Ok((row.get(0)?, row.get(1)?)))
                .expect("the statement runs")
        };
        const OTHER_USE: &str = "SELECT json_extract(payload_json, '$.source_mob_id'), '' \
             FROM console_frames WHERE dedupe_key = 'launch-1'";
        const BOTH: &str = "SELECT json_extract(payload_json, '$.source_mob_id'), {mob} \
             FROM console_frames WHERE dedupe_key = 'launch-1'";
        let expected = (Some("lower".to_string()), String::new());
        assert_eq!(read(OTHER_USE), expected, "a log without the column");
        assert_eq!(read(BOTH), expected, "a log without the column");
        store
            .append_if_absent(member_keyed_frame("second-1", Some("second"), 200))
            .await
            .expect("append");
        assert_eq!(read(OTHER_USE), expected, "a log with the column");
        assert_eq!(read(BOTH), expected, "a log with the column");
    }

    /// On a log without the mob column a member's timeline page seeks the
    /// identity index in cursor order, as it did before frames named a mob:
    /// reading the column as the empty mob costs the read no index and no
    /// sort.
    #[tokio::test]
    async fn member_keyed_frames_a_version_3_log_reads_a_members_page_through_the_identity_index() {
        let store = SqliteConsoleLogStore::in_memory().expect("sqlite store");
        store
            .append_if_absent(member_keyed_frame("launch-1", None, 1))
            .await
            .expect("append");
        let conn = store.conn.lock().expect("connection");
        assert!(!mob_keyed(&conn), "the log has no mob column");
        for mode in [ConsoleTimelineMode::Since, ConsoleTimelineMode::Recent] {
            let sql = member_sql(&conn, &timeline_page_sql(true, true, false, &mode));
            assert!(sql.contains(UNKEYED_MOB), "{sql}");
            let plan = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .expect("prepare query plan")
                .query_map(params![0i64, i64::MAX, "alice", "", 10i64], |row| {
                    row.get::<_, String>(3)
                })
                .expect("run query plan")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect query plan")
                .join("\n")
                .to_lowercase();
            assert!(
                plan.contains("idx_console_frames_identity_cursor"),
                "the read must seek the identity index; plan was: {plan}"
            );
            assert!(
                !plan.contains("temp b-tree"),
                "the index order must serve the read without a sort; plan was: {plan}"
            );
        }
    }

    /// The answers a log gives the default mob's members over the
    /// operations a member's page read does not cover: a send moved through
    /// its states, an update marker that shares its frame's provenance, the
    /// unsettled sends, a changed status, a block's delta compaction, last
    /// activity, and the reads by key, by member and of the whole log. The
    /// time a change was stored at is left out: it is the clock's.
    async fn member_keyed_default_mob_answers(store: &dyn ConsoleLogStore) -> serde_json::Value {
        fn without_clock(value: &mut serde_json::Value) {
            match value {
                serde_json::Value::Object(fields) => {
                    fields.remove("updated_at_ms");
                    fields.values_mut().for_each(without_clock);
                }
                serde_json::Value::Array(items) => items.iter_mut().for_each(without_clock),
                _ => {}
            }
        }
        let stored = seed(store).await;
        let send = &stored["send"];
        let dispatching = store
            .transition_send_frame(
                &send.id,
                SendTransition::StartDispatch,
                Some(("attempt", json!(1))),
            )
            .await
            .expect("dispatch");
        assert!(
            dispatching.is_some(),
            "an accepted send starts its dispatch"
        );
        let delivered = store
            .transition_send_frame(&send.id, SendTransition::MarkDelivered, None)
            .await
            .expect("delivered");
        let again = store
            .transition_send_frame(&send.id, SendTransition::StartDispatch, None)
            .await
            .expect("a settled send");
        store
            .append_if_absent(send_frame("send-waiting", ConsoleFrameStatus::Accepted))
            .await
            .expect("append");
        let mut update = sample_frame("update", "worker");
        update.kind = "frame_updated".to_string();
        update.payload = json!({ "frame": stored["run-start"] });
        update.source.member_provenance = Some(provenance_witness());
        let marker = store.append_if_absent(update).await.expect("marker").frame;
        let redacted = store
            .update_frame_status(&stored["after"].id, ConsoleFrameStatus::Redacted)
            .await
            .expect("status");
        let removed = store
            .compact_message_deltas(&block_compaction(&stored, "text-1", "complete"))
            .await
            .expect("compaction");
        let identities = ["worker", "identity:a", "other-worker"].map(str::to_string);
        let activity: BTreeMap<String, u64> = store
            .latest_activity_times(&identities)
            .await
            .expect("times")
            .into_iter()
            .collect();
        let page = store
            .query_frames(ConsoleTimelineQuery {
                identity: Some("worker".to_string()),
                ..Default::default()
            })
            .await
            .expect("page")
            .frames;
        let recent = store
            .query_windowed_frames(ConsoleTimelineWindowQuery {
                identity: Some("worker".to_string()),
                mode: ConsoleTimelineMode::Recent,
                limit: 4,
                ..Default::default()
            })
            .await
            .expect("window");
        let whole: Vec<ConsoleFrame> = all_frames(store)
            .await
            .into_iter()
            .filter(|frame| frame.identity != "zed")
            .collect();
        let mut answers = json!({
            "dispatching": dispatching,
            "delivered": delivered,
            "again": again,
            "marker": marker,
            "marker_read": store.frame_by_dedupe_key("update").await.expect("lookup"),
            "unsettled": store.unsettled_send_frames(100, 100).await.expect("unsettled"),
            "redacted": redacted,
            "removed": removed,
            "activity": activity,
            "page": page,
            "recent": recent.frames,
            "recent_cursors": format!(
                "{:?} {:?} {}",
                recent.latest_cursor, recent.next_cursor, recent.exhausted
            ),
            "whole": whole,
        });
        without_clock(&mut answers);
        answers
    }

    /// A log that was moved to schema version 4 serves the default mob's
    /// members as a version 3 log does: the same operations in the same
    /// order answer the same on both. One frame of a member the answers do
    /// not read comes first on each; on the moved log it is another mob's,
    /// so that log moves before the default mob stores anything in it.
    #[tokio::test]
    async fn member_keyed_frames_a_moved_log_serves_the_default_mob_as_a_version_3_log_does() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut answers = Vec::new();
        for moved in [false, true] {
            let version = if moved { 4 } else { 3 };
            let path = dir.path().join(format!("console-{version}.sqlite"));
            let store = SqliteConsoleLogStore::open(&path).expect("open");
            let mut first = member_keyed_frame("zed-1", moved.then_some("second"), 50);
            first.identity = "zed".to_string();
            first.conversation_id = Some("zed".to_string());
            store.append_if_absent(first).await.expect("append");
            assert_eq!(member_keyed_ledger_version(&path), Some(version));
            answers.push(member_keyed_default_mob_answers(&store).await);
            assert_eq!(member_keyed_ledger_version(&path), Some(version));
        }
        assert!(
            answers[0]["removed"].as_u64().is_some_and(|rows| rows > 0),
            "the compaction removed rows: {}",
            answers[0]["removed"]
        );
        assert_eq!(answers[0], answers[1]);
    }

    /// A console log file that a second connection moves to schema version
    /// 4 while the first works: the first connection's store, with one frame
    /// of the default mob's `alice`.
    struct MemberKeyedMovingLog {
        _dir: tempfile::TempDir,
        path: PathBuf,
        store: SqliteConsoleLogStore,
        launch: ConsoleFrame,
    }

    impl MemberKeyedMovingLog {
        async fn open() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("console.sqlite");
            let store = SqliteConsoleLogStore::open(&path).expect("open");
            let launch = store
                .append_if_absent(member_keyed_frame("launch-1", None, 100))
                .await
                .expect("append")
                .frame;
            assert_eq!(member_keyed_ledger_version(&path), Some(3));
            Self {
                _dir: dir,
                path,
                store,
                launch,
            }
        }

        /// Right after this store's check number `check` (from zero) of
        /// whether the log has the mob column, a second connection stores
        /// `frame`: a frame of a member of another mob, so it moves the log.
        fn moves_after_check(&self, check: usize, frame: NewConsoleFrame) {
            assert!(frame.mob_id.is_some(), "the frame is another mob's");
            let path = self.path.clone();
            mob_check_probe::arm(check, move || {
                let second = SqliteConsoleLogStore::open(&path).expect("second connection");
                futures::executor::block_on(second.append_if_absent(frame))
                    .expect("the second connection stores its frame");
            });
        }

        /// Whether the second connection moved the log during the last
        /// operation. When it did not, the operation made fewer checks than
        /// the one the move was to follow.
        fn moved(&self) -> bool {
            let moved = !mob_check_probe::disarm();
            assert_eq!(
                member_keyed_ledger_version(&self.path),
                Some(if moved { 4 } else { 3 })
            );
            moved
        }
    }

    /// A member's page read on a log that a second connection moves to
    /// schema version 4 between the read's look at the log and its
    /// statement: whichever look the move follows, the read answers the
    /// frames of the member it names and no frame of the member of that name
    /// in another mob, and its newest cursor is that member's.
    #[tokio::test]
    async fn member_keyed_frames_a_members_page_read_answers_its_member_while_the_log_moves() {
        for check in 0.. {
            let log = MemberKeyedMovingLog::open().await;
            log.moves_after_check(check, member_keyed_frame("second-1", Some("second"), 200));
            let window = log
                .store
                .query_windowed_frames(ConsoleTimelineWindowQuery {
                    identity: Some("alice".to_string()),
                    ..Default::default()
                })
                .await
                .expect("window");
            let moved = log.moved();
            let keys: Vec<&str> = window
                .frames
                .iter()
                .map(|frame| frame.dedupe_key.as_str())
                .collect();
            assert_eq!(
                keys,
                ["launch-1"],
                "the default mob's alice, moved after check {check}: {moved}"
            );
            assert_eq!(window.frames[0].mob_id, None);
            assert_eq!(
                window.latest_cursor.as_ref(),
                Some(&log.launch.cursor),
                "the newest cursor is the member's own, moved after check {check}: {moved}"
            );
            if !moved {
                assert!(check > 0, "the read looks at the log");
                break;
            }
            assert_eq!(
                member_keyed_frames_of(&log.store, Some("second")).await,
                ["second-1"]
            );
        }
    }

    /// A member's last activity on a log that a second connection moves
    /// between the read's look at the log and its statement: the time is the
    /// named member's, never the newer one of the member of that name in
    /// another mob.
    #[tokio::test]
    async fn member_keyed_frames_last_activity_is_its_members_while_the_log_moves() {
        let alice = ["alice".to_string()];
        for check in 0.. {
            let log = MemberKeyedMovingLog::open().await;
            log.moves_after_check(check, member_keyed_frame("second-1", Some("second"), 200));
            let times = log
                .store
                .latest_activity_times(&alice)
                .await
                .expect("times");
            let moved = log.moved();
            assert_eq!(
                times,
                HashMap::from([("alice".to_string(), 100)]),
                "moved after check {check}: {moved}"
            );
            if !moved {
                assert!(check > 0, "the read looks at the log");
                break;
            }
            assert_eq!(
                log.store
                    .latest_activity_times_in_mob("second", &alice)
                    .await
                    .expect("times"),
                HashMap::from([("alice".to_string(), 200)])
            );
        }
    }

    /// The reads of one frame (by its dedupe key, by its send's interaction,
    /// among the unsettled sends, after a status change, and the frame an
    /// append of a stored key answers) on a log that a second connection
    /// moves between the read's look at the log and its statement: a frame
    /// of another mob's member is never answered as the default mob's. A
    /// read that names no mob answers no frame of another mob, and where
    /// one is answered (the append of that mob's own frame answers it) it
    /// is answered as that mob's.
    #[tokio::test]
    async fn member_keyed_frames_a_frame_read_answers_the_frames_own_mob_while_the_log_moves() {
        const INTERACTION: &str = "interaction-of-the-second-send";
        fn second_send() -> NewConsoleFrame {
            let mut frame = send_frame("second-send", ConsoleFrameStatus::Accepted);
            frame.id = Some("second-send-id".to_string());
            frame.identity = "alice".to_string();
            frame.interaction_id = Some(INTERACTION.to_string());
            frame.mob_id = Some(meerkat_mob::MobId::from("second"));
            frame
        }
        let second = Some(meerkat_mob::MobId::from("second"));
        for read in ["by key", "by interaction", "unsettled", "status", "append"] {
            for check in 0.. {
                // An append looks its key up first; only that look is a
                // read. (Two connections that append one key at once after
                // it is another matter: the second insert is refused.)
                if read == "append" && check > 0 {
                    break;
                }
                let log = MemberKeyedMovingLog::open().await;
                log.moves_after_check(check, second_send());
                let answered: Option<ConsoleFrame> = match read {
                    "by key" => log.store.frame_by_dedupe_key("second-send").await,
                    "by interaction" => log.store.send_frame_by_interaction(INTERACTION).await,
                    "unsettled" => log
                        .store
                        .unsettled_send_frames(100, 100)
                        .await
                        .map(|frames| frames.into_iter().next()),
                    "status" => {
                        log.store
                            .update_frame_status("second-send-id", ConsoleFrameStatus::Dispatching)
                            .await
                    }
                    _ => log
                        .store
                        .append_if_absent(second_send())
                        .await
                        .map(|outcome| Some(outcome.frame)),
                }
                .expect("the read answers");
                let moved = log.moved();
                if let Some(frame) = &answered {
                    assert_eq!(frame.dedupe_key, "second-send");
                    assert_eq!(
                        frame.mob_id, second,
                        "{read}, moved after check {check}: {moved}"
                    );
                }
                if read == "append" {
                    assert!(answered.is_some(), "an append answers its frame");
                    assert_eq!(
                        member_keyed_frames_of(&log.store, None).await,
                        ["launch-1"],
                        "{read}, moved after check {check}: {moved}"
                    );
                }
                if !moved {
                    assert!(check > 0, "the read looks at the log: {read}");
                    break;
                }
            }
        }
    }

    /// An append of a frame of the default mob's member on a log that a
    /// second connection moves between the append's look at the log and its
    /// statement: the frame is stored as the default mob's.
    #[tokio::test]
    async fn member_keyed_frames_a_default_mob_append_is_the_default_mobs_while_the_log_moves() {
        for check in 0.. {
            let log = MemberKeyedMovingLog::open().await;
            log.moves_after_check(check, member_keyed_frame("second-1", Some("second"), 200));
            let appended = log
                .store
                .append_if_absent(member_keyed_frame("launch-2", None, 300))
                .await
                .expect("append");
            let moved = log.moved();
            assert!(matches!(appended.disposition, AppendDisposition::Inserted));
            assert_eq!(appended.frame.mob_id, None);
            assert_eq!(
                member_keyed_frames_of(&log.store, None).await,
                ["launch-1", "launch-2"],
                "moved after check {check}: {moved}"
            );
            if !moved {
                assert!(check > 0, "the append looks at the log");
                break;
            }
            assert_eq!(
                member_keyed_frames_of(&log.store, Some("second")).await,
                ["second-1"]
            );
        }
    }

    /// On a log without the mob column, reading the column as the empty mob
    /// reads no frame: the statement of a member's read touches the rows the
    /// identity index leads it to and scans nothing, however long the log.
    #[tokio::test]
    async fn member_keyed_frames_a_version_3_members_read_scans_no_frame_for_the_mob() {
        let store = SqliteConsoleLogStore::in_memory().expect("sqlite store");
        for index in 0..64 {
            let mut frame = member_keyed_frame(&format!("launch-{index}"), None, index);
            if index % 2 == 0 {
                frame.identity = "bob".to_string();
            }
            store.append_if_absent(frame).await.expect("append");
        }
        let conn = store.conn.lock().expect("connection");
        assert!(!mob_keyed(&conn), "the log has no mob column");
        let sql = member_sql(&conn, &latest_activity_sql());
        let mut statement = conn.prepare(&sql).expect("prepare");
        let newest: i64 = statement
            .query_row(params!["alice", ""], |row| row.get(0))
            .expect("alice acted");
        assert_eq!(newest, 63);
        assert_eq!(
            statement.get_status(rusqlite::StatementStatus::FullscanStep),
            0,
            "the read scans no table"
        );
        let other: Option<i64> = statement
            .query_row(params!["alice", "second"], |row| row.get(0))
            .optional()
            .expect("read");
        assert_eq!(other, None, "another mob has no frame in this log");
    }

    fn member_keyed_default_mob() -> DefaultMob {
        DefaultMob::new(meerkat_mob::MobId::from("launch"))
    }

    /// A store that was told the host's default mob reads that mob's id as
    /// no mob: a frame appended under the id is the default mob's, written
    /// as a frame that names none, and a member read, a last-activity read
    /// and a delta compaction that name the default mob by its id are the
    /// default mob's. Another mob's member of the same name stays apart.
    async fn member_keyed_default_mob_named_by_its_id(store: &dyn ConsoleLogStore) {
        store
            .bind_default_mob(&member_keyed_default_mob())
            .expect("the store learns its default mob");
        store
            .append_if_absent(member_keyed_frame("launch-1", None, 100))
            .await
            .expect("append");
        store
            .append_if_absent(member_keyed_frame("second-1", Some("second"), 200))
            .await
            .expect("append");
        let by_id = store
            .append_if_absent(member_keyed_frame("launch-2", Some("launch"), 300))
            .await
            .expect("append")
            .frame;
        assert_eq!(
            by_id.mob_id, None,
            "a frame under the default mob's id is the default mob's"
        );
        assert!(
            !serde_json::to_string(&by_id)
                .expect("frame serializes")
                .contains("mob_id"),
            "a frame of the default mob is written as it always was"
        );
        assert_eq!(
            store
                .frame_by_dedupe_key("launch-2")
                .await
                .expect("lookup")
                .expect("frame")
                .mob_id,
            None
        );
        for default_mob in [None, Some("launch")] {
            assert_eq!(
                member_keyed_frames_of(store, default_mob).await,
                ["launch-1", "launch-2"],
                "the default mob named as {default_mob:?}"
            );
        }
        assert_eq!(
            member_keyed_frames_of(store, Some("second")).await,
            ["second-1"]
        );
        let alice = ["alice".to_string()];
        let newest = HashMap::from([("alice".to_string(), 300)]);
        assert_eq!(
            store.latest_activity_times(&alice).await.expect("times"),
            newest
        );
        assert_eq!(
            store
                .latest_activity_times_in_mob("launch", &alice)
                .await
                .expect("times"),
            newest
        );
        assert_eq!(
            store
                .latest_activity_times_in_mob("second", &alice)
                .await
                .expect("times"),
            HashMap::from([("alice".to_string(), 200)])
        );

        // A block of the default mob's alice, its rows appended under no mob
        // and under the default mob's id alike, compacted by the id.
        let mut cursors = BTreeMap::new();
        for (key, mob_id, kind) in [
            ("block-d1", None, "text_delta"),
            ("block-d2", Some("launch"), "text_delta"),
            ("block-d3", None, "text_delta"),
            ("block-complete", Some("launch"), "text_complete"),
        ] {
            let mut frame = sample_frame(key, "alice");
            frame.mob_id = mob_id.map(meerkat_mob::MobId::from);
            frame.kind = kind.to_string();
            frame.payload = json!({ "assistant_message_id": "message-1", "source_sequence": 1 });
            let stored = store.append_if_absent(frame).await.expect("append").frame;
            cursors.insert(key, stored.cursor);
        }
        let removed = store
            .compact_message_deltas(
                &ConsoleDeltaCompaction::new(
                    "runtime-a",
                    "alice",
                    crate::console_aggregator::ConsoleDeltaKind::Text,
                    "message-1",
                    cursors["block-d1"].clone(),
                    cursors["block-complete"].clone(),
                )
                .in_mob(Some(meerkat_mob::MobId::from("launch"))),
            )
            .await
            .expect("compaction");
        assert_eq!(removed, 2, "the default mob's later delta rows go");
    }

    #[tokio::test]
    async fn member_keyed_frames_the_default_mob_named_by_its_id_is_the_default_mob_in_sqlite() {
        let store = SqliteConsoleLogStore::in_memory().expect("sqlite");
        member_keyed_default_mob_named_by_its_id(&store).await;
    }

    #[tokio::test]
    async fn member_keyed_frames_the_default_mob_named_by_its_id_is_the_default_mob_in_memory() {
        let store = InMemoryConsoleLogStore::new();
        member_keyed_default_mob_named_by_its_id(&store).await;
    }

    /// A frame appended under the default mob's id moves nothing: the log
    /// stays at schema version 3, without the mob column, so the build
    /// before frames named a mob still opens it.
    #[tokio::test]
    async fn member_keyed_frames_a_frame_under_the_default_mobs_id_moves_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("console.sqlite");
        let store = SqliteConsoleLogStore::open(&path).expect("open");
        store
            .bind_default_mob(&member_keyed_default_mob())
            .expect("the store learns its default mob");
        let appended = store
            .append_if_absent(member_keyed_frame("launch-1", Some("launch"), 100))
            .await
            .expect("append")
            .frame;
        assert_eq!(appended.mob_id, None);
        assert_eq!(member_keyed_frames_of(&store, None).await, ["launch-1"]);
        assert_eq!(
            member_keyed_frames_of(&store, Some("launch")).await,
            ["launch-1"]
        );
        drop(store);
        assert_eq!(member_keyed_ledger_version(&path), Some(3));
        assert!(!member_keyed_log_has(&path, "mob_id"));
        let mut conn = meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::PRIMARY)
            .expect("sqlite");
        meerkat_sqlite::apply_domain_migrations(&mut conn, &member_keyed_earlier_build())
            .expect("the earlier build opens the log");
    }

    /// A store learns its default mob once: the same mob again changes
    /// nothing, another mob and a blank id are refused, and the refused mob
    /// is not read as the default mob afterwards.
    async fn member_keyed_store_is_told_its_default_mob_once(
        told: &dyn ConsoleLogStore,
        fresh: &dyn ConsoleLogStore,
    ) {
        let launch = member_keyed_default_mob();
        told.bind_default_mob(&launch).expect("the first telling");
        told.bind_default_mob(&launch).expect("the same mob again");
        assert!(
            told.bind_default_mob(&DefaultMob::new(meerkat_mob::MobId::from("second")))
                .is_err(),
            "a store that was told one default mob refuses another"
        );
        let second = told
            .append_if_absent(member_keyed_frame("second-1", Some("second"), 200))
            .await
            .expect("append")
            .frame;
        assert_eq!(second.mob_id, Some(meerkat_mob::MobId::from("second")));
        assert!(
            fresh
                .bind_default_mob(&DefaultMob::new(meerkat_mob::MobId::from("")))
                .is_err(),
            "a blank id names no mob"
        );
        fresh
            .bind_default_mob(&launch)
            .expect("a refused blank id leaves the store untold");
    }

    #[tokio::test]
    async fn member_keyed_frames_a_store_is_told_its_default_mob_once_in_sqlite() {
        let told = SqliteConsoleLogStore::in_memory().expect("sqlite");
        let fresh = SqliteConsoleLogStore::in_memory().expect("sqlite");
        member_keyed_store_is_told_its_default_mob_once(&told, &fresh).await;
    }

    #[tokio::test]
    async fn member_keyed_frames_a_store_is_told_its_default_mob_once_in_memory() {
        let told = InMemoryConsoleLogStore::new();
        let fresh = InMemoryConsoleLogStore::new();
        member_keyed_store_is_told_its_default_mob_once(&told, &fresh).await;
    }

    /// A store that was never told its default mob keeps a frame under the
    /// mob it names: it cannot know that the id is the default mob's.
    #[tokio::test]
    async fn member_keyed_frames_a_store_that_was_never_told_keeps_the_mob_a_frame_names() {
        let stores: Vec<Box<dyn ConsoleLogStore>> = vec![
            Box::new(SqliteConsoleLogStore::in_memory().expect("sqlite")),
            Box::new(InMemoryConsoleLogStore::new()),
        ];
        for store in stores {
            let stored = store
                .append_if_absent(member_keyed_frame("launch-1", Some("launch"), 100))
                .await
                .expect("append")
                .frame;
            assert_eq!(stored.mob_id, Some(meerkat_mob::MobId::from("launch")));
            assert!(
                member_keyed_frames_of(store.as_ref(), None)
                    .await
                    .is_empty()
            );
        }
    }

    /// A store that keeps the trait's own `bind_default_mob` cannot learn its
    /// default mob, and says so: it never answers that it was told when a
    /// frame under the default mob's id would still get a key of its own.
    #[test]
    fn member_keyed_frames_a_store_that_cannot_learn_its_default_mob_refuses_to_be_told() {
        assert!(
            LegacyQueryOnlyStore
                .bind_default_mob(&member_keyed_default_mob())
                .is_err(),
            "a store without the method was not told"
        );
    }

    /// A read that fails for a reason of its own on a log without the mob
    /// column answers that reason. It is run a second time only when the
    /// log was moved under it: on a log that did not move, the statement a
    /// log with the column takes never runs, and its error never stands in
    /// for the read's own.
    #[tokio::test]
    async fn member_keyed_frames_a_read_that_fails_on_an_unmoved_log_answers_its_own_error() {
        let store = SqliteConsoleLogStore::in_memory().expect("sqlite");
        let mut frame = member_keyed_frame("launch-1", None, 100);
        frame.source.member_provenance = Some(provenance_witness());
        store.append_if_absent(frame).await.expect("append");
        {
            let conn = store.conn.lock().expect("connection");
            assert!(!mob_keyed(&conn), "the log has no mob column");
            // The frame's member record is gone: the frame cannot be read.
            conn.execute("DELETE FROM console_member_provenance", [])
                .expect("remove the member record");
        }
        let by_key = store
            .frame_by_dedupe_key("launch-1")
            .await
            .expect_err("the frame cannot be read by its key")
            .to_string();
        let page = store
            .query_frames(ConsoleTimelineQuery {
                identity: Some("alice".to_string()),
                ..Default::default()
            })
            .await
            .expect_err("the frame cannot be read in its member's page")
            .to_string();
        for error in [by_key, page] {
            assert!(
                error.contains("console member provenance reference is missing"),
                "the read answers its own error: {error}"
            );
            assert!(
                !error.contains("mob_id"),
                "no statement for a log with the mob column ran: {error}"
            );
        }
        let conn = store.conn.lock().expect("connection");
        assert!(!mob_keyed(&conn), "the failed reads moved nothing");
    }

    /// The read of one mob on a log that a second connection moves to
    /// schema version 4 between the read's look at the log and its
    /// statement: whichever look the move follows, the read of the default
    /// mob answers the default mob's frames and no frame of another mob,
    /// with an identity and without one, and its newest cursor is the
    /// default mob's.
    #[tokio::test]
    async fn member_keyed_frames_the_read_of_one_mob_answers_its_mob_while_the_log_moves() {
        for identity in [None, Some("alice")] {
            for check in 0.. {
                let log = MemberKeyedMovingLog::open().await;
                log.moves_after_check(check, member_keyed_frame("second-1", Some("second"), 200));
                let window = log
                    .store
                    .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
                        identity: identity.map(ToString::to_string),
                        ..Default::default()
                    })
                    .await
                    .expect("window");
                let moved = log.moved();
                let keys: Vec<&str> = window
                    .frames
                    .iter()
                    .map(|frame| frame.dedupe_key.as_str())
                    .collect();
                assert_eq!(
                    keys,
                    ["launch-1"],
                    "the default mob's frames, read by {identity:?}, moved after check {check}: \
                     {moved}"
                );
                assert_eq!(
                    window.latest_cursor.as_ref(),
                    Some(&log.launch.cursor),
                    "the newest cursor is the default mob's, read by {identity:?}, moved after \
                     check {check}: {moved}"
                );
                if !moved {
                    assert!(check > 0, "the read looks at the log");
                    break;
                }
                assert_eq!(
                    member_keyed_frames_of_mob(&log.store, Some("second")).await,
                    ["second-1"]
                );
            }
        }
    }

    /// A store that was told the host's default mob reads that mob's id as
    /// no mob in the read of one mob as well: the read that names the
    /// default mob by its id answers the default mob's frames, those stored
    /// under no mob and those appended under the id alike, with an identity
    /// and without one. Another mob's frames stay apart.
    async fn member_keyed_one_mobs_read_takes_the_default_mob_by_its_id(
        store: &dyn ConsoleLogStore,
    ) {
        store
            .bind_default_mob(&member_keyed_default_mob())
            .expect("the store learns its default mob");
        for frame in [
            member_keyed_frame("launch-1", None, 100),
            member_keyed_frame("second-1", Some("second"), 200),
            member_keyed_frame("launch-2", Some("launch"), 300),
        ] {
            store.append_if_absent(frame).await.expect("append");
        }
        for default_mob in [None, Some("launch")] {
            assert_eq!(
                member_keyed_frames_of_mob(store, default_mob).await,
                ["launch-1", "launch-2"],
                "the default mob's frames, the mob named as {default_mob:?}"
            );
            let of_member = store
                .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
                    identity: Some("alice".to_string()),
                    mob_id: default_mob.map(meerkat_mob::MobId::from),
                    ..Default::default()
                })
                .await
                .expect("member");
            assert_eq!(
                of_member
                    .frames
                    .iter()
                    .map(|frame| frame.dedupe_key.as_str())
                    .collect::<Vec<_>>(),
                ["launch-1", "launch-2"],
                "the default mob's member, the mob named as {default_mob:?}"
            );
        }
        assert_eq!(
            member_keyed_frames_of_mob(store, Some("second")).await,
            ["second-1"]
        );
    }

    #[tokio::test]
    async fn member_keyed_frames_the_read_of_one_mob_takes_the_default_mob_by_its_id_in_sqlite() {
        let store = SqliteConsoleLogStore::in_memory().expect("sqlite");
        member_keyed_one_mobs_read_takes_the_default_mob_by_its_id(&store).await;
    }

    #[tokio::test]
    async fn member_keyed_frames_the_read_of_one_mob_takes_the_default_mob_by_its_id_in_memory() {
        let store = InMemoryConsoleLogStore::new();
        member_keyed_one_mobs_read_takes_the_default_mob_by_its_id(&store).await;
    }

    /// The frames of one mob are removed, whoever their member, and nothing
    /// else: the default mob's frames and a third mob's stay, each under the
    /// cursor it had. A second removal finds none. A frame stored afterwards
    /// under the removed mob's id is the only frame a read of that id
    /// answers (a mob created again under the id reads none of the earlier
    /// mob's), a key the earlier mob used is free again, and the new frame's
    /// cursor is above every cursor given out before: the removed frames
    /// were the newest, and their cursors are not given out again. The
    /// default mob's frames are never removed: a blank id and the id of the
    /// default mob the store was told are refused.
    async fn member_keyed_a_removed_mobs_frames_are_gone_and_the_rest_stay(
        store: &dyn ConsoleLogStore,
    ) {
        store
            .bind_default_mob(&member_keyed_default_mob())
            .expect("the store learns its default mob");
        let of = |key: &str, identity: &str, mob_id: Option<&str>, at: u64| {
            let mut frame = activity_frame(key, identity, at, ConsoleFrameSourceKind::Send);
            frame.mob_id = mob_id.map(meerkat_mob::MobId::from);
            frame
        };
        let mut cursors = BTreeMap::new();
        for frame in [
            of("launch-alice-1", "alice", None, 100),
            of("second-alice-1", "alice", Some("second"), 200),
            of("third-alice-1", "alice", Some("third"), 300),
            of("second-bob-1", "bob", Some("second"), 400),
        ] {
            let stored = store.append_if_absent(frame).await.expect("append").frame;
            cursors.insert(stored.dedupe_key.clone(), stored.cursor);
        }
        let second = meerkat_mob::MobId::from("second");

        let removed = store
            .remove_frames_of_mob(&second)
            .await
            .expect("the second mob's frames are removed");
        assert_eq!(
            removed, 2,
            "both frames of the second mob, whoever their member"
        );
        assert!(
            member_keyed_frames_of_mob(store, Some("second"))
                .await
                .is_empty(),
            "the read of the removed mob answers no frame"
        );
        assert!(
            member_keyed_frames_of(store, Some("second"))
                .await
                .is_empty(),
            "the removed mob's member has no frame"
        );
        assert_eq!(
            store
                .frame_by_dedupe_key("second-alice-1")
                .await
                .expect("lookup"),
            None,
            "a removed frame is not found by its key"
        );
        assert!(
            !store
                .latest_activity_times_in_mob("second", &["alice".to_string(), "bob".to_string()])
                .await
                .expect("times")
                .contains_key("alice"),
            "the removed mob's member has no last activity"
        );
        for (mob_id, key) in [(None, "launch-alice-1"), (Some("third"), "third-alice-1")] {
            assert_eq!(
                member_keyed_frames_of_mob(store, mob_id).await,
                [key],
                "the frames of {mob_id:?} stay"
            );
            // Read by the read that names the frame's mob: a read by key
            // names no mob.
            let kept = store
                .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
                    mob_id: mob_id.map(meerkat_mob::MobId::from),
                    ..Default::default()
                })
                .await
                .expect("the mob's frames");
            assert_eq!(
                kept.frames
                    .iter()
                    .map(|frame| Some(frame.cursor.clone()))
                    .collect::<Vec<_>>(),
                [cursors.get(key).cloned()],
                "a frame that stays keeps its cursor"
            );
        }
        assert_eq!(
            store
                .remove_frames_of_mob(&second)
                .await
                .expect("a second removal"),
            0,
            "a mob with no frame has none to remove"
        );

        let again = store
            .append_if_absent(of("second-alice-1", "alice", Some("second"), 500))
            .await
            .expect("append");
        assert_eq!(
            again.disposition,
            AppendDisposition::Inserted,
            "a key the removed mob used is free"
        );
        let newest_before = cursors
            .values()
            .filter_map(ConsoleCursor::seq)
            .max()
            .expect("cursors");
        assert!(
            again
                .frame
                .cursor
                .seq()
                .is_some_and(|seq| seq > newest_before),
            "no cursor given out before is given out again: {:?} after {newest_before}",
            again.frame.cursor
        );
        assert_eq!(
            member_keyed_frames_of_mob(store, Some("second")).await,
            ["second-alice-1"],
            "a mob under the id again reads its own frame alone"
        );

        for default_mob in ["", "launch"] {
            assert!(
                store
                    .remove_frames_of_mob(&meerkat_mob::MobId::from(default_mob))
                    .await
                    .is_err(),
                "the default mob's frames are not removed, the mob named as {default_mob:?}"
            );
        }
        assert_eq!(
            member_keyed_frames_of_mob(store, None).await,
            ["launch-alice-1"],
            "the default mob's frames stay"
        );
    }

    #[tokio::test]
    async fn member_keyed_frames_a_removed_mobs_frames_are_gone_and_the_rest_stay_in_sqlite() {
        let store = SqliteConsoleLogStore::in_memory().expect("sqlite");
        member_keyed_a_removed_mobs_frames_are_gone_and_the_rest_stay(&store).await;
    }

    #[tokio::test]
    async fn member_keyed_frames_a_removed_mobs_frames_are_gone_and_the_rest_stay_in_memory() {
        let store = InMemoryConsoleLogStore::new();
        member_keyed_a_removed_mobs_frames_are_gone_and_the_rest_stay(&store).await;
    }

    /// A removal on a log that never stored a frame of another mob finds
    /// none and moves nothing: the log stays at schema version 3, without
    /// the mob column, and the default mob's frame stays.
    #[tokio::test]
    async fn member_keyed_frames_a_removal_on_a_version_3_log_removes_and_moves_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("console.sqlite");
        let store = SqliteConsoleLogStore::open(&path).expect("open");
        store
            .append_if_absent(member_keyed_frame("launch-1", None, 100))
            .await
            .expect("append");
        let removed = store
            .remove_frames_of_mob(&meerkat_mob::MobId::from("second"))
            .await
            .expect("a removal on a log without another mob's frame");
        assert_eq!(removed, 0);
        assert_eq!(member_keyed_ledger_version(&path), Some(3));
        assert!(!member_keyed_log_has(&path, "mob_id"));
        assert_eq!(member_keyed_frames_of(&store, None).await, ["launch-1"]);
    }

    /// A removal takes the member records that only the removed frames
    /// named, and leaves a member record a frame that stays still names:
    /// the default mob's frame reads with its witness afterwards.
    #[tokio::test]
    async fn member_keyed_frames_a_removal_takes_the_member_records_only_its_frames_named() {
        let store = SqliteConsoleLogStore::in_memory().expect("sqlite");
        let shared = provenance_witness();
        let mut of_the_second_alone = provenance_witness();
        of_the_second_alone.primary_mob_id = "second".to_string();
        for (key, mob_id, witness) in [
            ("launch-1", None, &shared),
            ("second-1", Some("second"), &of_the_second_alone),
            ("second-2", Some("second"), &shared),
        ] {
            let mut frame = member_keyed_frame(key, mob_id, 100);
            frame.source.member_provenance = Some(witness.clone());
            store.append_if_absent(frame).await.expect("append");
        }
        let member_records = || {
            let conn = store.conn.lock().expect("connection");
            conn.query_row(
                "SELECT COUNT(*) FROM console_member_provenance",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("count")
        };
        assert_eq!(member_records(), 2);

        let removed = store
            .remove_frames_of_mob(&meerkat_mob::MobId::from("second"))
            .await
            .expect("the second mob's frames are removed");
        assert_eq!(removed, 2);
        assert_eq!(
            member_records(),
            1,
            "the record only the removed frames named went, the shared one stays"
        );
        let kept = store
            .frame_by_dedupe_key("launch-1")
            .await
            .expect("lookup")
            .expect("the default mob's frame");
        assert_eq!(kept.source.member_provenance, Some(shared));
    }

    /// A frame whose mob is only white space names no mob and is no frame
    /// of the default mob either: it is refused, nothing is stored, and a
    /// SQLite log is not moved.
    #[tokio::test]
    async fn member_keyed_frames_a_frame_whose_mob_is_only_white_space_is_refused() {
        let sqlite = SqliteConsoleLogStore::in_memory().expect("sqlite");
        let in_memory = InMemoryConsoleLogStore::new();
        let stores: [(&str, &dyn ConsoleLogStore); 2] =
            [("sqlite", &sqlite), ("in memory", &in_memory)];
        for (name, store) in stores {
            assert!(
                store
                    .append_if_absent(member_keyed_frame("blank-mob", Some("  "), 100))
                    .await
                    .is_err(),
                "{name}: a frame whose mob is only white space is refused"
            );
            assert_eq!(
                store
                    .frame_by_dedupe_key("blank-mob")
                    .await
                    .expect("lookup"),
                None,
                "{name}: nothing is stored"
            );
        }
        let conn = sqlite.conn.lock().expect("connection");
        assert!(!mob_keyed(&conn), "the refused frame moved nothing");
    }

    /// A frame key and a frame id are one frame's in the whole log, whatever
    /// mob the frame is of. An append for one mob under a key, or under a
    /// frame id, that a frame of another mob holds is refused: nothing is
    /// stored, the frame that holds it stays as it is, and the append is
    /// never answered another mob's frame. The same frame again, for its
    /// own mob, answers the stored frame. Both stores hold it alike, the
    /// default mob's frame first or the other mob's.
    async fn member_keyed_one_key_and_one_frame_id_are_one_frames(store: &dyn ConsoleLogStore) {
        let of = |key: &str, id: &str, mob_id: Option<&str>| {
            let mut frame = member_keyed_frame(key, mob_id, 100);
            frame.id = Some(id.to_string());
            frame
        };
        for (first, other) in [(None, Some("second")), (Some("second"), None)] {
            let key = format!("key-of-{}", first.unwrap_or("the-default-mob"));
            let id = format!("id-of-{}", first.unwrap_or("the-default-mob"));
            let stored = store
                .append_if_absent(of(&key, &id, first))
                .await
                .expect("the first mob's frame");
            assert_eq!(stored.disposition, AppendDisposition::Inserted);
            for (what, frame) in [
                ("the key and the frame id", of(&key, &id, other)),
                ("the key", of(&key, "another-id", other)),
                ("the frame id", of("another-key", &id, other)),
            ] {
                let answer = store.append_if_absent(frame).await;
                assert!(
                    answer.is_err(),
                    "an append for {other:?} under {what} that a frame of {first:?} holds is \
                     refused: {answer:?}"
                );
            }
            assert_eq!(
                member_keyed_frames_of(store, first).await,
                std::slice::from_ref(&key),
                "the frame of {first:?} stays"
            );
            assert!(
                !member_keyed_frames_of(store, other)
                    .await
                    .iter()
                    .any(|kept| *kept == key || kept == "another-key"),
                "nothing of {other:?} is stored"
            );
            let again = store
                .append_if_absent(of(&key, &id, first))
                .await
                .expect("the first mob's frame again");
            assert_eq!(again.disposition, AppendDisposition::Existing);
            assert_eq!(
                (again.frame.id, again.frame.cursor, again.frame.mob_id),
                (stored.frame.id, stored.frame.cursor, stored.frame.mob_id),
                "the same frame again, for its own mob, answers the stored frame"
            );
        }
    }

    #[tokio::test]
    async fn member_keyed_frames_one_key_and_one_frame_id_are_one_frames_in_sqlite() {
        let store = SqliteConsoleLogStore::in_memory().expect("sqlite");
        member_keyed_one_key_and_one_frame_id_are_one_frames(&store).await;
    }

    #[tokio::test]
    async fn member_keyed_frames_one_key_and_one_frame_id_are_one_frames_in_memory() {
        let store = InMemoryConsoleLogStore::new();
        member_keyed_one_key_and_one_frame_id_are_one_frames(&store).await;
    }

    /// An append the in-memory store refuses takes no cursor: the next
    /// stored frame's cursor follows the last stored frame's, after an
    /// append refused for its key and one refused for its frame id.
    #[tokio::test]
    async fn member_keyed_frames_a_refused_append_takes_no_cursor_in_memory() {
        let store = InMemoryConsoleLogStore::new();
        let of = |key: &str, id: &str, mob_id: Option<&str>| {
            let mut frame = member_keyed_frame(key, mob_id, 100);
            frame.id = Some(id.to_string());
            frame
        };
        let first = store
            .append_if_absent(of("key-1", "id-1", None))
            .await
            .expect("the first frame")
            .frame
            .cursor;
        for refused in [
            of("key-1", "id-2", Some("second")),
            of("key-2", "id-1", Some("second")),
            of("key-3", "id-1", None),
        ] {
            assert!(
                store.append_if_absent(refused).await.is_err(),
                "the append is refused"
            );
        }
        let next = store
            .append_if_absent(of("key-4", "id-4", Some("second")))
            .await
            .expect("the next frame")
            .frame
            .cursor;
        assert_eq!(
            next.seq(),
            first.seq().map(|seq| seq + 1),
            "the next stored frame's cursor follows the first frame's: {first} then {next}"
        );
    }
}
