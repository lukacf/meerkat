//! Notes kept under a plain member name are set aside at the first start.
//!
//! At the first start of a composition over a bundled agent memory store,
//! the notes kept under two kinds of plain name are set aside
//! ([`SqliteAgentMemoryStore::set_plain_names_aside`]): each name the
//! event logs show a member private to a person has held, and each name of
//! a temporary council's participant. Set aside means forgotten and moved
//! to the identity scope `set-aside/<name>`: no member's memory tool,
//! recall or injection names that scope, the memory steward keeps out of
//! it, and the memory panel shows it to access administrators only. The
//! records stay in the store as they were written.
//!
//! The event logs are the composition's own mob's and those of the mobs
//! its agent mob tools host ([`HostedMobLogs`]). A name counts when a
//! spawn in one of them seated a private member under it
//! ([`held_by_a_private_member`]), and when it was seated in a hosted mob
//! that a private member created, whatever that spawn's labels say.
//!
//! The notes of a name are set aside whoever stored them: a shared member
//! that used the same name loses its notes under it too. A name no event
//! log holds (a member of a mob that was destroyed) is left as it is, the
//! council participants' names apart. So is a name only the log of a
//! hosted mob that does not run holds (a mob held before its activation,
//! or one that did not restore): the step skips that mob and
//! names it in its answer and in a warning. A private member seated as a
//! durable identity keeps its notes.
//!
//! The step runs once per composition's own mob and store: it writes a
//! marker, and a start that finds the marker reads no log and changes
//! nothing; a mob hosted only after it gets no step. A composition runs it
//! before it hands the store to any reader. A launch that does not speak
//! for its composition runs no step and writes nothing. A step that cannot
//! run (an event log cannot be read, the hosted mobs cannot be read from
//! the agent mob tools, or the store refuses it) writes nothing, and the
//! launch is refused: the next start tries again.

use std::collections::{BTreeMap, BTreeSet};

use crate::memory::sqlite_store::{SetAsideOutcome, SqliteAgentMemoryStore};

/// Events read per page while a mob's event log is read.
const JOURNAL_PAGE: usize = 512;

/// One member spawn a mob's event log holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoggedSpawn {
    /// The roster member id of the spawned member.
    pub(crate) member: String,
    /// The labels the member was spawned with.
    pub(crate) labels: BTreeMap<String, String>,
    /// Whether the member was seated ephemeral (not as a durable identity).
    pub(crate) ephemeral: bool,
}

/// One page of a mob's member spawns and the cursor to read on from.
pub(crate) type LoggedSpawnPage = (Vec<LoggedSpawn>, u64);

/// Why a mob's event log could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum JournalReadError {
    /// The event store failed the read.
    Storage(String),
    /// The read never reached the event store: the mob no longer answers,
    /// or it refused the read to this reader.
    Unavailable(String),
}

impl std::fmt::Display for JournalReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(error) | Self::Unavailable(error) => f.write_str(error),
        }
    }
}

/// A mob's durable event log, read for its member spawns.
#[async_trait::async_trait]
pub(crate) trait MemberSpawnJournal: Send + Sync {
    /// The id of the mob the log belongs to.
    fn mob(&self) -> String;

    /// The member spawns recorded past `cursor`, reading at most `limit`
    /// events, or `None` once the log holds no event past it.
    async fn spawns_after(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Option<LoggedSpawnPage>, JournalReadError>;
}

#[async_trait::async_trait]
impl MemberSpawnJournal for meerkat_mob::MobHandle {
    fn mob(&self) -> String {
        self.mob_id().to_string()
    }

    async fn spawns_after(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Option<LoggedSpawnPage>, JournalReadError> {
        let events = self
            .poll_events(cursor, limit)
            .await
            .map_err(|error| match error {
                meerkat_mob::MobError::StorageError(_) => {
                    JournalReadError::Storage(error.to_string())
                }
                other => JournalReadError::Unavailable(other.to_string()),
            })?;
        let Some(last) = events.last().map(|event| event.cursor) else {
            return Ok(None);
        };
        let spawns = events
            .into_iter()
            .filter_map(|event| match event.kind {
                meerkat_mob::MobEventKind::MemberSpawned(spawned) => Some(LoggedSpawn {
                    member: spawned.agent_identity.to_string(),
                    ephemeral: matches!(
                        spawned.continuity_intent,
                        meerkat_mob::SpawnContinuityIntent::Ephemeral
                    ),
                    labels: spawned.labels,
                }),
                _ => None,
            })
            .collect();
        Ok(Some((spawns, last)))
    }
}

/// One mob the composition's agent mob tools host, as the first start's
/// step takes it: its event log, and the member its creation record names.
pub(crate) struct HostedJournal<'a> {
    /// The mob's durable event log.
    pub(crate) journal: &'a dyn MemberSpawnJournal,
    /// The member that created the mob: the id of the mob that member is
    /// seated in, and its roster member id. `None` for a creation record
    /// that names no member.
    pub(crate) creator: Option<(String, String)>,
}

/// The mobs the composition's agent mob tools host, beside the
/// composition's own mob.
#[derive(Default)]
pub(crate) struct HostedMobs<'a> {
    /// The mobs whose event logs can be read.
    pub(crate) journals: Vec<HostedJournal<'a>>,
    /// The ids of the persisted mobs that do not run (held before their
    /// activation, or not restored): no event log of theirs can be read
    /// here.
    pub(crate) not_restored: Vec<String>,
}

/// The event logs of the mobs a composition's agent mob tools host, read
/// for the first start's step ([`Self::mobs`]).
#[derive(Default)]
pub(crate) struct HostedMobLogs {
    logs: Vec<(meerkat_mob::MobHandle, Option<(String, String)>)>,
    not_restored: Vec<String>,
}

impl HostedMobLogs {
    /// The mobs `tools` hosts, the composition's own mob `own` apart, from
    /// one read of them: each mob that runs with its event log and the
    /// member its creation record names, and by its id each persisted mob
    /// that does not run (held before its activation, or not restored). A
    /// composition without agent mob tools hosts none.
    pub(crate) async fn of(
        tools: Option<&meerkat_mob_mcp::MobMcpState>,
        own: &str,
    ) -> Result<Self, JournalReadError> {
        let Some(tools) = tools else {
            return Ok(Self::default());
        };
        let unavailable =
            |error: meerkat_mob::MobError| JournalReadError::Unavailable(error.to_string());
        let inventory = tools.mob_inventory().await.map_err(unavailable)?;
        let mut logs = Vec::new();
        for (mob_id, _) in inventory.managed {
            if mob_id.as_str() == own {
                continue;
            }
            let handle = match tools.handle_for(&mob_id).await {
                Ok(handle) => handle,
                // A mob destroyed since it was listed: its event log is gone.
                Err(meerkat_mob::MobError::MobNotFound(_)) => continue,
                Err(error) => return Err(unavailable(error)),
            };
            let provenance = handle.provenance().await.map_err(unavailable)?;
            logs.push((
                handle.observer(),
                crate::private_owners::named_creator(&provenance),
            ));
        }
        let not_restored = inventory
            .held
            .into_iter()
            .map(|(mob_id, _)| mob_id.to_string())
            .chain(
                inventory
                    .quarantined
                    .into_iter()
                    .map(|mob| mob.mob_id.to_string()),
            )
            .filter(|mob| mob != own)
            .collect();
        Ok(Self { logs, not_restored })
    }

    /// What the step takes.
    pub(crate) fn mobs(&self) -> HostedMobs<'_> {
        HostedMobs {
            journals: self
                .logs
                .iter()
                .map(|(handle, creator)| HostedJournal {
                    journal: handle,
                    creator: creator.clone(),
                })
                .collect(),
            not_restored: self.not_restored.clone(),
        }
    }
}

/// Whether `spawn` seated a member private to a person that was either
/// ephemeral or a console side chat: a private member that is not a durable
/// identity of its own.
pub(crate) fn held_by_a_private_member(spawn: &LoggedSpawn) -> bool {
    crate::access::private_owner(&spawn.labels).is_some()
        && (spawn.ephemeral || crate::console_spawn::names_side_chat_source(&spawn.labels))
}

/// Why the first start's step did not run to its end. Nothing was written.
#[derive(Debug)]
pub(crate) enum SetAsideError {
    /// The mob's event log could not be read.
    Journal(JournalReadError),
    /// The event log of a mob the agent mob tools host could not be read.
    HostedJournal {
        /// The id of the hosted mob.
        mob: String,
        /// Why its log could not be read.
        error: JournalReadError,
    },
    /// The mobs the agent mob tools host could not be read from them.
    HostedMobs(JournalReadError),
    /// The memory store refused a read or the step's transaction.
    Store(crate::identity_first::agent_memory::AgentMemoryError),
}

impl std::fmt::Display for SetAsideError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Journal(error) => {
                write!(f, "the mob's event log could not be read ({error})")
            }
            Self::HostedJournal { mob, error } => write!(
                f,
                "the event log of the hosted mob '{mob}' could not be read ({error})"
            ),
            Self::HostedMobs(error) => write!(
                f,
                "the mobs the agent mob tools host could not be read ({error})"
            ),
            Self::Store(error) => write!(f, "the agent memory store refused the step ({error})"),
        }
    }
}

impl std::error::Error for SetAsideError {}

impl SetAsideError {
    /// What the one who starts the composition can do about it, as the
    /// refusal says it.
    fn ways_out(&self) -> &'static str {
        match self {
            Self::HostedJournal { .. } => {
                "start again, destroy that mob, or start without agent memory (configure no \
                 `agent_memory`)"
            }
            Self::Journal(_) | Self::HostedMobs(_) | Self::Store(_) => {
                "start again, or start without agent memory (configure no `agent_memory`)"
            }
        }
    }
}

/// What a composition does with its agent memory store once the first
/// start's step has been asked for ([`memory_start`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MemoryStart {
    /// The step ran, had run before, or is not this launch's to run: the
    /// store is handed to its readers.
    Attach,
    /// An event log could not be read (the composition's own mob's or a
    /// hosted mob's: its event store failed, the mob no longer answers, or
    /// it refused the read), the hosted mobs could not be read from the
    /// agent mob tools, or the memory store refused the step: nothing was
    /// written, and the launch is refused with `message`. The next start
    /// tries again.
    Refuse {
        /// The refusal's text.
        message: String,
    },
}

impl MemoryStart {
    fn of(step: Result<Option<FirstStart>, SetAsideError>) -> Self {
        match step {
            Ok(_) => Self::Attach,
            Err(error) => Self::Refuse {
                message: format!(
                    "agent memory could not be prepared: {error}; nothing was changed; {}",
                    error.ways_out()
                ),
            },
        }
    }
}

/// Run the first start's step ([`set_aside_at_first_start`]) and say what
/// the composition does with the store.
async fn memory_start(
    store: &SqliteAgentMemoryStore,
    realm: &str,
    journal: &dyn MemberSpawnJournal,
    hosted: &HostedMobs<'_>,
    speaks_for_composition: bool,
    created_before_ms: u64,
) -> MemoryStart {
    MemoryStart::of(
        set_aside_at_first_start(
            store,
            realm,
            journal,
            hosted,
            speaks_for_composition,
            created_before_ms,
        )
        .await,
    )
}

/// [`memory_start`] for a composition whose agent mob tools answer
/// `hosted`, the mobs they host ([`HostedMobLogs::of`]). They are asked
/// only by a launch that speaks for its composition and finds no marker of
/// the step in the store: every other launch asks the agent mob tools
/// nothing.
async fn memory_start_of<Hosted>(
    store: &SqliteAgentMemoryStore,
    realm: &str,
    journal: &dyn MemberSpawnJournal,
    hosted: Hosted,
    speaks_for_composition: bool,
    created_before_ms: u64,
) -> MemoryStart
where
    Hosted: std::future::Future<Output = Result<HostedMobLogs, JournalReadError>>,
{
    let logs = if speaks_for_composition {
        match store
            .plain_names_were_set_aside(realm, &journal.mob())
            .await
        {
            Ok(true) => HostedMobLogs::default(),
            Ok(false) => match hosted.await {
                Ok(logs) => logs,
                Err(error) => return MemoryStart::of(Err(SetAsideError::HostedMobs(error))),
            },
            Err(error) => return MemoryStart::of(Err(SetAsideError::Store(error))),
        }
    } else {
        HostedMobLogs::default()
    };
    memory_start(
        store,
        realm,
        journal,
        &logs.mobs(),
        speaks_for_composition,
        created_before_ms,
    )
    .await
}

/// [`memory_start_of`] for the composition over the bootstrapped mob
/// runtime `runtime`: its own mob's event log, the mobs its agent mob tools
/// host, and whether this launch speaks for the composition are all taken
/// from that runtime. The gateway and a library build call this.
pub(crate) async fn memory_start_for(
    store: &SqliteAgentMemoryStore,
    realm: &str,
    runtime: &crate::mob_handle_runtime::MobRuntime,
    created_before_ms: u64,
) -> MemoryStart {
    let journal = runtime.handle().observer();
    let own = journal.mob_id().to_string();
    let tools = runtime.agent_mob_mcp_state();
    memory_start_of(
        store,
        realm,
        &journal,
        HostedMobLogs::of(tools.as_deref(), &own),
        runtime.speaks_for_composition(),
        created_before_ms,
    )
    .await
}

/// The step's lower entries, for the rows of other modules. Outside tests
/// [`memory_start_for`] is the one way in.
#[cfg(test)]
pub(crate) mod test_entries {
    use super::{FirstStart, HostedMobs, MemberSpawnJournal, MemoryStart, SetAsideError};
    use crate::memory::sqlite_store::SqliteAgentMemoryStore;

    /// The module's `memory_start`.
    pub(crate) async fn memory_start(
        store: &SqliteAgentMemoryStore,
        realm: &str,
        journal: &dyn MemberSpawnJournal,
        hosted: &HostedMobs<'_>,
        speaks_for_composition: bool,
        created_before_ms: u64,
    ) -> MemoryStart {
        super::memory_start(
            store,
            realm,
            journal,
            hosted,
            speaks_for_composition,
            created_before_ms,
        )
        .await
    }

    /// The module's `set_aside_at_first_start`.
    pub(crate) async fn set_aside_at_first_start(
        store: &SqliteAgentMemoryStore,
        realm: &str,
        journal: &dyn MemberSpawnJournal,
        hosted: &HostedMobs<'_>,
        speaks_for_composition: bool,
        created_before_ms: u64,
    ) -> Result<Option<FirstStart>, SetAsideError> {
        super::set_aside_at_first_start(
            store,
            realm,
            journal,
            hosted,
            speaks_for_composition,
            created_before_ms,
        )
        .await
    }
}

/// The current time, as the store stamps its records.
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// The bound a gateway's start gives the step: nothing reads or writes the
/// store before the step there, so every note the store holds was stored
/// before it, whatever its stamp says (a host's clock can have been set
/// back since a note was stored). The largest bound the store takes.
pub(crate) fn bound_at_a_gateway_start() -> u64 {
    i64::MAX as u64
}

/// What the first start's step did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FirstStart {
    /// What the store's step did.
    pub(crate) outcome: SetAsideOutcome,
    /// The ids of the mobs whose event logs the step read, the
    /// composition's own first. Empty for a step that found its marker in
    /// the store: it reads no log.
    pub(crate) mobs_read: Vec<String>,
    /// The ids of the hosted mobs the step did not read: persisted mobs
    /// that do not run. The names only their event logs hold are left
    /// as they are, also by every later start.
    pub(crate) not_read: Vec<String>,
}

/// What one mob's event log holds for the first start's step.
struct LogRead<'a> {
    /// The id of the mob the log belongs to.
    mob: String,
    /// The member the mob's creation record names: the id of the mob that
    /// member is seated in, and its roster member id.
    creator: Option<&'a (String, String)>,
    /// The roster ids of the members a spawn of which carried a private
    /// owner.
    private_members: BTreeSet<String>,
    /// The roster ids of every member the log seated.
    members: BTreeSet<String>,
    /// The names a private member held by its spawn's own labels
    /// ([`held_by_a_private_member`]).
    held: BTreeSet<String>,
    /// The names of the members seated ephemeral or as a console side
    /// chat, whoever they were private to.
    seated: BTreeSet<String>,
}

impl<'a> LogRead<'a> {
    /// Read `journal`, the event log of a mob whose creation record names
    /// `creator`, to its end.
    async fn of(
        journal: &dyn MemberSpawnJournal,
        creator: Option<&'a (String, String)>,
    ) -> Result<Self, JournalReadError> {
        let mut read = Self {
            mob: journal.mob(),
            creator,
            private_members: BTreeSet::new(),
            members: BTreeSet::new(),
            held: BTreeSet::new(),
            seated: BTreeSet::new(),
        };
        let mut cursor = 0;
        while let Some((spawns, next)) = journal.spawns_after(cursor, JOURNAL_PAGE).await? {
            cursor = next;
            for spawn in spawns {
                let name = crate::member_comms_id::logical_memory_identity(&spawn.member);
                if crate::access::private_owner(&spawn.labels).is_some() {
                    read.private_members.insert(spawn.member.clone());
                }
                if held_by_a_private_member(&spawn) {
                    read.held.insert(name.clone());
                }
                if spawn.ephemeral || crate::console_spawn::names_side_chat_source(&spawn.labels) {
                    read.seated.insert(name);
                }
                read.members.insert(spawn.member);
            }
        }
        Ok(read)
    }
}

/// The names the event logs show a member private to a person held. In
/// every log: the names a private member held by its spawn's own labels.
/// In the log of a mob a private member created: every name seated there,
/// whatever its spawn's labels say (a log an earlier release wrote names
/// no owner for such a member). A mob's creator is private when a spawn of
/// it in the log of the mob it is seated in carried a private owner, or
/// when that mob was itself created by a private member. A creator no log
/// holds, and one seated in a mob whose log is not among `logs`, makes no
/// mob private.
fn names_private_members_held(logs: &[LogRead<'_>]) -> BTreeSet<String> {
    let by_mob: BTreeMap<&str, &LogRead<'_>> =
        logs.iter().map(|log| (log.mob.as_str(), log)).collect();
    let mut created_by_private: BTreeSet<&str> = BTreeSet::new();
    loop {
        let before = created_by_private.len();
        for log in logs {
            let Some((seated_in, member)) = log.creator else {
                continue;
            };
            let Some(creators_log) = by_mob.get(seated_in.as_str()) else {
                continue;
            };
            if creators_log.private_members.contains(member)
                || (created_by_private.contains(seated_in.as_str())
                    && creators_log.members.contains(member))
            {
                created_by_private.insert(log.mob.as_str());
            }
        }
        if created_by_private.len() == before {
            break;
        }
    }
    let mut names = BTreeSet::new();
    for log in logs {
        names.extend(log.held.iter().cloned());
        if created_by_private.contains(log.mob.as_str()) {
            names.extend(log.seated.iter().cloned());
        }
    }
    names
}

/// The first start's step over `store` (see the module docs), for the mob
/// whose event log `journal` is, the mobs `hosted` that the composition's
/// agent mob tools host, and the memory realm `realm`. Records created at
/// or after `created_before_ms` are left where they are.
///
/// `None` for a launch that does not speak for its composition: it reads
/// and writes nothing. A step whose marker is in the store reads no log.
async fn set_aside_at_first_start(
    store: &SqliteAgentMemoryStore,
    realm: &str,
    journal: &dyn MemberSpawnJournal,
    hosted: &HostedMobs<'_>,
    speaks_for_composition: bool,
    created_before_ms: u64,
) -> Result<Option<FirstStart>, SetAsideError> {
    if !speaks_for_composition {
        return Ok(None);
    }
    let mob = journal.mob();
    if store
        .plain_names_were_set_aside(realm, &mob)
        .await
        .map_err(SetAsideError::Store)?
    {
        return Ok(Some(FirstStart {
            outcome: SetAsideOutcome::AlreadyDone,
            mobs_read: Vec::new(),
            not_read: Vec::new(),
        }));
    }
    let mut logs = vec![
        LogRead::of(journal, None)
            .await
            .map_err(SetAsideError::Journal)?,
    ];
    for hosted_mob in &hosted.journals {
        match LogRead::of(hosted_mob.journal, hosted_mob.creator.as_ref()).await {
            Ok(read) => logs.push(read),
            Err(error) => {
                return Err(SetAsideError::HostedJournal {
                    mob: hosted_mob.journal.mob(),
                    error,
                });
            }
        }
    }
    let names = names_private_members_held(&logs);
    let outcome = store
        .set_plain_names_aside(realm, &mob, names, created_before_ms)
        .await
        .map_err(SetAsideError::Store)?;
    let answer = FirstStart {
        outcome,
        mobs_read: logs.into_iter().map(|log| log.mob).collect(),
        not_read: hosted.not_restored.clone(),
    };
    if let SetAsideOutcome::Done { names, records } = answer.outcome {
        tracing::info!(
            mob = %mob,
            realm,
            names,
            records,
            mobs_read = ?answer.mobs_read,
            "agent memory: the notes kept under plain member names were set aside at the first \
             start"
        );
        if !answer.not_read.is_empty() {
            tracing::warn!(
                mob = %mob,
                realm,
                not_read = ?answer.not_read,
                "agent memory: the first start's step did not read the event logs of the hosted \
                 mobs that do not run; the notes under the names only those logs hold are \
                 left as they are"
            );
        }
    }
    Ok(Some(answer))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::identity_first::AgentIdentity;
    use crate::identity_first::agent_memory::{
        AgentMemoryConfig, AgentMemoryProvider, AgentMemoryRecallRequest, AgentMemorySelection,
    };
    use crate::memory::capabilities::{MemoryPanelStore, PendingPromotion, StewardStore};
    use crate::memory::coordinator::RecallCoordinator;
    use crate::memory::records::{MemoryAuthor, MemoryKind, MemoryScope, NewMemoryRecord};
    use crate::memory::sqlite_store::set_aside_scope_key;
    use crate::memory::staged::{
        StagedBatchKind, StagedMemoryStore, StagedMutationBatch, StagedOp,
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// One row of the step's marker: its token, place, kind and detail.
    type MarkerRow = (String, i64, String, String);

    const REALM: &str = "team";
    const MOB: &str = "mob:main";
    /// Two mobs the composition's agent mob tools host.
    const TEAM: &str = "mob:team";
    const DESK: &str = "mob:desk";
    const ALICE: &str = "console-caller:v1:aa";

    /// A mob's event log as a test hands it over: its member spawns in
    /// order, or a log that cannot be read. It counts its reads.
    struct Journal {
        mob: &'static str,
        spawns: Result<Vec<LoggedSpawn>, JournalReadError>,
        reads: AtomicUsize,
    }

    impl Journal {
        fn of(spawns: Vec<LoggedSpawn>) -> Self {
            Self {
                mob: MOB,
                spawns: Ok(spawns),
                reads: AtomicUsize::new(0),
            }
        }

        fn of_mob(mob: &'static str, spawns: Vec<LoggedSpawn>) -> Self {
            Self {
                mob,
                ..Self::of(spawns)
            }
        }

        /// A log whose event store fails the read.
        fn unreadable() -> Self {
            Self {
                spawns: Err(JournalReadError::Storage(
                    "the event store is not readable".to_string(),
                )),
                ..Self::of(Vec::new())
            }
        }

        /// A log whose mob does not answer the read.
        fn unavailable() -> Self {
            Self {
                spawns: Err(JournalReadError::Unavailable(
                    "the mob no longer answers".to_string(),
                )),
                ..Self::of(Vec::new())
            }
        }

        fn reads(&self) -> usize {
            self.reads.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl MemberSpawnJournal for Journal {
        fn mob(&self) -> String {
            self.mob.to_string()
        }

        async fn spawns_after(
            &self,
            cursor: u64,
            _limit: usize,
        ) -> Result<Option<LoggedSpawnPage>, JournalReadError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            let spawns = self.spawns.as_ref().map_err(Clone::clone)?;
            // One spawn a page: the step reads on until the log ends.
            Ok(spawns
                .get(cursor as usize)
                .map(|spawn| (vec![spawn.clone()], cursor + 1)))
        }
    }

    /// A spawn of a member private to a person, seated ephemeral.
    fn private_spawn(member: &str) -> LoggedSpawn {
        LoggedSpawn {
            member: member.to_string(),
            labels: BTreeMap::from([(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                ALICE.to_string(),
            )]),
            ephemeral: true,
        }
    }

    /// A spawn of a member private to no one, seated ephemeral.
    fn shared_spawn(member: &str) -> LoggedSpawn {
        LoggedSpawn {
            member: member.to_string(),
            labels: BTreeMap::new(),
            ephemeral: true,
        }
    }

    fn open() -> (tempfile::TempDir, SqliteAgentMemoryStore) {
        let dir = tempfile::tempdir().expect("memory dir");
        let store = SqliteAgentMemoryStore::open(dir.path()).expect("memory store");
        (dir, store)
    }

    fn scope(name: &str) -> MemoryScope {
        MemoryScope::Identity {
            realm: REALM.to_string(),
            identity: name.to_string(),
        }
    }

    fn agent(name: &str) -> MemoryAuthor {
        MemoryAuthor::Agent {
            identity: name.to_string(),
        }
    }

    fn note(title: &str, body: &str) -> NewMemoryRecord {
        NewMemoryRecord {
            kind: MemoryKind::Fact,
            title: title.to_string(),
            description: String::new(),
            body: body.to_string(),
            tags: Vec::new(),
            evidence: Vec::new(),
            verification: None,
        }
    }

    /// Store a note under the plain name `name`, as that member's memory
    /// tool does: in the name's identity scope, authored by the name.
    async fn remember(
        store: &SqliteAgentMemoryStore,
        name: &str,
        title: &str,
        body: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        Ok(store
            .remember_authored(&scope(name), note(title, body), agent(name))
            .await?
            .memory_id)
    }

    /// The bodies a member named `name` recalls with its memory tool.
    async fn recalled(
        store: &SqliteAgentMemoryStore,
        name: &str,
    ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let mut bodies: Vec<String> = store
            .recall(AgentMemoryRecallRequest {
                identity: AgentIdentity::parse(name)?,
                realm: REALM.to_string(),
                query_text: None,
                query_terms: Vec::new(),
                selection: AgentMemorySelection::Always,
                max_entries: 64,
            })
            .await?
            .into_iter()
            .map(|record| record.body)
            .collect();
        bodies.sort();
        Ok(bodies)
    }

    /// What a member named `name` is given at its build.
    async fn injected(
        store: &SqliteAgentMemoryStore,
        name: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let coordinator = RecallCoordinator::new(
            Arc::new(store.clone()),
            AgentMemoryConfig {
                realm: REALM.to_string(),
                selection: AgentMemorySelection::Always,
                ..AgentMemoryConfig::default()
            },
        );
        Ok(coordinator
            .assemble_build_injection(&AgentIdentity::parse(name)?, None, Vec::new())
            .await?
            .unwrap_or_default())
    }

    /// Every record of the store: its scope key, status, title and body,
    /// in a fixed order. Record ids and times are left out, so two stores
    /// that hold the same notes read the same.
    async fn notes(
        store: &SqliteAgentMemoryStore,
    ) -> Result<Vec<(String, &'static str, String, String)>, Box<dyn std::error::Error>> {
        let page = store
            .records_page(REALM, None, None, None, 1024, None)
            .await?;
        assert!(page.next_cursor.is_none(), "one page holds the store");
        let mut notes: Vec<_> = page
            .records
            .into_iter()
            .map(|record| {
                (
                    record.scope.key().to_string(),
                    record.status.kind_str(),
                    record.title,
                    record.body,
                )
            })
            .collect();
        notes.sort();
        Ok(notes)
    }

    /// Everything a step may change, record ids and times included.
    async fn stored(store: &SqliteAgentMemoryStore) -> Result<String, Box<dyn std::error::Error>> {
        let page = store
            .records_page(REALM, None, None, None, 1024, None)
            .await?;
        let mut records: Vec<String> = page
            .records
            .iter()
            .map(|record| {
                format!(
                    "{} {} {} {} {} {:?}",
                    record.id,
                    record.scope.key(),
                    record.status.kind_str(),
                    record.updated_at_ms,
                    record.title,
                    record.body
                )
            })
            .collect();
        records.sort();
        Ok(format!(
            "records {records:#?}\nprivate {:?}\nproposals {}\npromotions {}",
            store.private_retired_identities(REALM).await?,
            store.pending_proposals(REALM, 64).await?.len(),
            store.pending_promotions(REALM).await?.len(),
        ))
    }

    async fn first_start(
        store: &SqliteAgentMemoryStore,
        journal: &Journal,
    ) -> Result<Option<SetAsideOutcome>, SetAsideError> {
        first_start_with(store, journal, &HostedMobs::default()).await
    }

    /// The first start's step of a composition whose agent mob tools host
    /// `hosted`.
    async fn first_start_with(
        store: &SqliteAgentMemoryStore,
        journal: &Journal,
        hosted: &HostedMobs<'_>,
    ) -> Result<Option<SetAsideOutcome>, SetAsideError> {
        Ok(first_start_answer(store, journal, hosted)
            .await?
            .map(|answer| answer.outcome))
    }

    /// The whole answer of that step: what it did, and the logs it read.
    async fn first_start_answer(
        store: &SqliteAgentMemoryStore,
        journal: &Journal,
        hosted: &HostedMobs<'_>,
    ) -> Result<Option<FirstStart>, SetAsideError> {
        set_aside_at_first_start(store, REALM, journal, hosted, true, now_ms() + 1).await
    }

    /// What a composition whose agent mob tools host `hosted` does with the
    /// store at its start.
    async fn start_with(
        store: &SqliteAgentMemoryStore,
        journal: &Journal,
        hosted: &HostedMobs<'_>,
    ) -> MemoryStart {
        memory_start(store, REALM, journal, hosted, true, now_ms() + 1).await
    }

    /// What a composition that hosts no other mob does with the store at
    /// its start.
    async fn start(store: &SqliteAgentMemoryStore, journal: &Journal) -> MemoryStart {
        start_with(store, journal, &HostedMobs::default()).await
    }

    /// The mobs a composition's agent mob tools host: the event log of
    /// each, with the member its creation record names (the mob that member
    /// is seated in, and its name there).
    fn hosted<'a>(mobs: &[(&'a Journal, Option<(&str, &str)>)]) -> HostedMobs<'a> {
        HostedMobs {
            journals: mobs
                .iter()
                .map(|(journal, creator)| HostedJournal {
                    journal: *journal,
                    creator: creator.map(|(mob, member)| (mob.to_string(), member.to_string())),
                })
                .collect(),
            not_restored: Vec::new(),
        }
    }

    /// The rows of the step's marker in `store`: each row's
    /// token, place, kind and detail, in the order they were written.
    fn marker_rows(
        store: &SqliteAgentMemoryStore,
    ) -> Result<Vec<MarkerRow>, Box<dyn std::error::Error>> {
        let conn = rusqlite::Connection::open(store.path_for_realm(REALM))?;
        let mut stmt = conn.prepare(
            "SELECT stage_token, op_index, op_kind, detail FROM audit \
             WHERE stage_token LIKE 'set-aside-first-start/%' ORDER BY audit_id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The store an earlier release leaves: notes under `ledger`, a name a
    /// member private to a person held (an active note, a note it updated,
    /// a note it forgot), and a note under `helper`, a shared member's name.
    async fn earlier_store(
        store: &SqliteAgentMemoryStore,
    ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let kept = remember(store, "ledger", "Ledger fact", "LEDGER-KEPT").await?;
        let first = remember(store, "ledger", "Ledger draft", "LEDGER-FIRST").await?;
        let updated = store
            .supersede_authored(
                &scope("ledger"),
                &first,
                note("Ledger draft", "LEDGER-UPDATED"),
                agent("ledger"),
            )
            .await?
            .memory_id;
        let forgotten = remember(store, "ledger", "Ledger slip", "LEDGER-FORGOTTEN").await?;
        assert!(
            store
                .forget_authored(&scope("ledger"), &forgotten, agent("ledger"))
                .await?
                .deleted
        );
        remember(store, "helper", "Helper fact", "HELPER-KEPT").await?;
        Ok(vec![kept, first, updated, forgotten])
    }

    fn earlier_journal() -> Journal {
        Journal::of(vec![shared_spawn("helper"), private_spawn("ledger")])
    }

    /// After the first start nothing of the notes under a name a private
    /// member held reaches a member of that name: its recall is empty, its
    /// build is given none of them, and it changes and forgets none of them
    /// by id. The records are all there, forgotten, under the set-aside
    /// scope, with their text. A shared member's notes are as they were.
    #[tokio::test]
    async fn notes_under_a_name_a_private_member_held_are_set_aside_at_the_first_start()
    -> TestResult {
        let (_dir, store) = open();
        let ledger_ids = earlier_store(&store).await?;
        let helper_before = recalled(&store, "helper").await?;
        assert_eq!(
            recalled(&store, "ledger").await?,
            ["LEDGER-KEPT", "LEDGER-UPDATED"],
            "the earlier release recalls them"
        );

        let outcome = first_start(&store, &earlier_journal()).await?;
        assert_eq!(
            outcome,
            Some(SetAsideOutcome::Done {
                names: 1,
                records: 4
            })
        );

        assert!(
            recalled(&store, "ledger").await?.is_empty(),
            "a member of the name recalls none of them"
        );
        let given = injected(&store, "ledger").await?;
        assert!(
            !given.contains("LEDGER-") && !given.contains("Ledger"),
            "a member of the name is given none of them at its build: {given}"
        );
        assert!(
            !store
                .forget_authored(&scope("ledger"), &ledger_ids[0], agent("ledger"))
                .await?
                .deleted,
            "a member of the name forgets none of them by id"
        );
        assert!(
            store
                .supersede_authored(
                    &scope("ledger"),
                    &ledger_ids[0],
                    note("Ledger fact", "LEDGER-REWRITTEN"),
                    agent("ledger"),
                )
                .await
                .is_err(),
            "a member of the name changes none of them by id"
        );
        let key = set_aside_scope_key("ledger");
        assert_eq!(
            notes(&store).await?,
            [
                (
                    "helper".to_string(),
                    "active",
                    "Helper fact".to_string(),
                    "HELPER-KEPT".to_string()
                ),
                (
                    key.clone(),
                    "tombstoned",
                    "Ledger draft".to_string(),
                    "LEDGER-FIRST".to_string()
                ),
                (
                    key.clone(),
                    "tombstoned",
                    "Ledger draft".to_string(),
                    "LEDGER-UPDATED".to_string()
                ),
                (
                    key.clone(),
                    "tombstoned",
                    "Ledger fact".to_string(),
                    "LEDGER-KEPT".to_string()
                ),
                (
                    key.clone(),
                    "tombstoned",
                    "Ledger slip".to_string(),
                    "LEDGER-FORGOTTEN".to_string()
                ),
            ],
            "every record is kept, forgotten, under the set-aside scope"
        );
        assert!(
            store
                .private_retired_identities(REALM)
                .await?
                .contains(&key),
            "the set-aside scope is recorded as private"
        );
        assert_eq!(recalled(&store, "helper").await?, helper_before);
        assert!(injected(&store, "helper").await?.contains("HELPER-KEPT"));
        Ok(())
    }

    /// The step runs once: a second start reads no log and changes nothing.
    #[tokio::test]
    async fn a_second_start_changes_no_record() -> TestResult {
        let (_dir, store) = open();
        earlier_store(&store).await?;
        first_start(&store, &earlier_journal()).await?;
        let after_first = stored(&store).await?;

        let unreadable = Journal::unreadable();
        assert_eq!(
            first_start(&store, &unreadable).await?,
            Some(SetAsideOutcome::AlreadyDone)
        );
        assert_eq!(unreadable.reads(), 0, "a later start reads no log");
        assert_eq!(stored(&store).await?, after_first);
        Ok(())
    }

    /// A step that stops between two names writes nothing, and the next
    /// start ends where a step that never stopped ends.
    #[tokio::test]
    async fn a_step_stopped_part_way_changes_nothing_and_the_next_start_finishes_it() -> TestResult
    {
        let (_dir, store) = open();
        earlier_store(&store).await?;
        remember(&store, "notes", "Notes fact", "NOTES-KEPT").await?;
        let before = stored(&store).await?;
        let names = BTreeSet::from(["ledger".to_string(), "notes".to_string()]);

        let stopped =
            store.set_plain_names_aside_blocking(REALM, MOB, &names, now_ms() + 1, &mut |name| {
                if name == "notes" {
                    return Err(crate::identity_first::agent_memory::AgentMemoryError::Io(
                        "the process stopped".to_string(),
                    ));
                }
                Ok(())
            });
        assert!(stopped.is_err(), "{stopped:?}");
        assert_eq!(
            stored(&store).await?,
            before,
            "a stopped step wrote nothing"
        );
        assert!(!store.plain_names_were_set_aside(REALM, MOB).await?);

        let journal = Journal::of(vec![private_spawn("ledger"), private_spawn("notes")]);
        assert_eq!(
            first_start(&store, &journal).await?,
            Some(SetAsideOutcome::Done {
                names: 2,
                records: 5
            })
        );
        let (_twin_dir, twin) = open();
        earlier_store(&twin).await?;
        remember(&twin, "notes", "Notes fact", "NOTES-KEPT").await?;
        first_start(&twin, &journal).await?;
        assert_eq!(notes(&store).await?, notes(&twin).await?);
        Ok(())
    }

    /// The notes of a name are set aside whoever stored them: a shared
    /// member that held the name after the private one loses its notes
    /// under it too.
    #[tokio::test]
    async fn a_shared_member_that_used_the_name_loses_its_notes_under_it_too() -> TestResult {
        let (_dir, store) = open();
        remember(&store, "ledger", "Private fact", "PRIVATE-MEMBERS").await?;
        remember(&store, "ledger", "Shared fact", "SHARED-MEMBERS").await?;
        let journal = Journal::of(vec![private_spawn("ledger"), shared_spawn("ledger")]);

        first_start(&store, &journal).await?;

        assert!(recalled(&store, "ledger").await?.is_empty());
        let key = set_aside_scope_key("ledger");
        assert!(
            notes(&store)
                .await?
                .iter()
                .all(|(scope_key, status, _, _)| scope_key == &key && *status == "tombstoned"),
            "both members' notes are set aside"
        );
        Ok(())
    }

    /// A name no event log holds is left as it is: the notes under it stay
    /// recalled by a member of that name.
    #[tokio::test]
    async fn a_name_no_event_log_holds_is_left_as_it_is() -> TestResult {
        let (_dir, store) = open();
        remember(&store, "scout", "Scout fact", "SCOUT-KEPT").await?;
        let before = stored(&store).await?;

        first_start(&store, &Journal::of(vec![private_spawn("ledger")])).await?;

        assert_eq!(recalled(&store, "scout").await?, ["SCOUT-KEPT"]);
        assert_eq!(stored(&store).await?, before);
        Ok(())
    }

    /// A private member seated as a durable identity keeps its notes; a
    /// console side chat seated so does not.
    #[tokio::test]
    async fn a_private_member_seated_as_a_durable_identity_keeps_its_notes() -> TestResult {
        let (_dir, store) = open();
        remember(&store, "advisor", "Advisor fact", "ADVISOR-KEPT").await?;
        remember(&store, "aside", "Aside fact", "ASIDE-NOTE").await?;
        let durable = LoggedSpawn {
            ephemeral: false,
            ..private_spawn("advisor")
        };
        let mut side_chat = LoggedSpawn {
            ephemeral: false,
            ..private_spawn("aside")
        };
        side_chat.labels.insert(
            crate::side_chat::SIDE_CHAT_OF_LABEL.to_string(),
            "advisor".to_string(),
        );

        first_start(&store, &Journal::of(vec![durable, side_chat])).await?;

        assert_eq!(recalled(&store, "advisor").await?, ["ADVISOR-KEPT"]);
        assert!(recalled(&store, "aside").await?.is_empty());
        Ok(())
    }

    /// A private member seated after the first start costs a shared member
    /// of its name nothing: the notes under the name, from before the first
    /// start and after it, stay at every later start.
    #[tokio::test]
    async fn a_private_member_seated_after_the_first_start_costs_a_shared_member_nothing()
    -> TestResult {
        let (_dir, store) = open();
        remember(&store, "helper", "Helper fact", "HELPER-EARLIER").await?;
        first_start(&store, &Journal::of(vec![shared_spawn("helper")])).await?;
        remember(&store, "helper", "Helper news", "HELPER-LATER").await?;

        let later = Journal::of(vec![shared_spawn("helper"), private_spawn("helper")]);
        for _ in 0..2 {
            assert_eq!(
                first_start(&store, &later).await?,
                Some(SetAsideOutcome::AlreadyDone)
            );
        }

        assert_eq!(
            recalled(&store, "helper").await?,
            ["HELPER-EARLIER", "HELPER-LATER"]
        );
        Ok(())
    }

    /// On a store with no such name the step changes no record, and it is
    /// still the store's one step.
    #[tokio::test]
    async fn a_store_with_no_such_name_has_no_record_changed() -> TestResult {
        let (_dir, store) = open();
        remember(&store, "helper", "Helper fact", "HELPER-KEPT").await?;
        let before = stored(&store).await?;

        assert_eq!(
            first_start(&store, &Journal::of(vec![shared_spawn("helper")])).await?,
            Some(SetAsideOutcome::Done {
                names: 0,
                records: 0
            })
        );

        assert_eq!(stored(&store).await?, before);
        assert!(store.plain_names_were_set_aside(REALM, MOB).await?);
        Ok(())
    }

    /// The notes under a temporary council participant's name are set aside
    /// whatever the event log holds; a name that only begins alike is left.
    #[tokio::test]
    async fn notes_under_a_council_participants_name_are_set_aside_whatever_the_log_holds()
    -> TestResult {
        let (_dir, store) = open();
        remember(&store, "council-p2", "Council fact", "COUNCIL-NOTE").await?;
        remember(&store, "council-planner", "Planner fact", "PLANNER-KEPT").await?;

        assert_eq!(
            first_start(&store, &Journal::of(Vec::new())).await?,
            Some(SetAsideOutcome::Done {
                names: 1,
                records: 1
            })
        );

        assert!(recalled(&store, "council-p2").await?.is_empty());
        assert_eq!(recalled(&store, "council-planner").await?, ["PLANNER-KEPT"]);
        Ok(())
    }

    /// A gated promotion of a set-aside note commits nothing afterwards:
    /// it is no longer pending and its staged batch is gone.
    #[tokio::test]
    async fn a_gated_promotion_of_a_set_aside_note_commits_nothing() -> TestResult {
        let (_dir, store) = open();
        let id = remember(&store, "ledger", "Ledger fact", "LEDGER-KEPT").await?;
        let token = store
            .stage(StagedMutationBatch {
                realm: REALM.to_string(),
                author: MemoryAuthor::Operator { principal: None },
                kind: StagedBatchKind::ReviewVerdict,
                ops: vec![StagedOp::Tombstone {
                    id: id.clone(),
                    rationale: None,
                }],
            })
            .await?;
        store
            .record_pending_promotion(
                REALM,
                PendingPromotion {
                    pending_id: "gate-1".to_string(),
                    stage_token: token.token.clone(),
                    record_id: id,
                    scope_kind: "identity".to_string(),
                    scope_key: "ledger".to_string(),
                    rationale: None,
                    status: "pending".to_string(),
                    created_at_ms: now_ms(),
                },
            )
            .await?;

        first_start(&store, &Journal::of(vec![private_spawn("ledger")])).await?;

        assert!(store.pending_promotions(REALM).await?.is_empty());
        assert!(
            store.commit(token).await.is_err(),
            "the staged batch is gone"
        );
        Ok(())
    }

    /// A proposal still pending under the name is settled as rejected: the
    /// steward and the panel's proposal list read only pending ones. A
    /// shared member's proposal stays pending.
    #[tokio::test]
    async fn a_proposal_pending_under_the_name_is_settled_as_rejected() -> TestResult {
        let (_dir, store) = open();
        let mob = MemoryScope::Mob {
            realm: REALM.to_string(),
            mob: MOB.to_string(),
        };
        store
            .propose(
                &mob,
                note("Ledger proposal", "LEDGER-PROPOSED"),
                agent("ledger"),
            )
            .await?;
        store
            .propose(
                &mob,
                note("Helper proposal", "HELPER-PROPOSED"),
                agent("helper"),
            )
            .await?;

        first_start(&store, &Journal::of(vec![private_spawn("ledger")])).await?;

        let pending: Vec<String> = store
            .pending_proposals(REALM, 64)
            .await?
            .into_iter()
            .map(|proposal| proposal.record.title)
            .collect();
        assert_eq!(pending, ["Helper proposal"]);
        Ok(())
    }

    /// A log whose event store fails the read sets nothing aside, and the
    /// launch is refused with the reason; the next start whose log reads
    /// sets the notes aside.
    #[tokio::test]
    async fn a_log_whose_store_fails_the_read_refuses_the_launch_and_sets_nothing_aside()
    -> TestResult {
        let (_dir, store) = open();
        earlier_store(&store).await?;
        let before = stored(&store).await?;

        let refused = first_start(&store, &Journal::unreadable()).await;
        assert!(
            matches!(
                refused,
                Err(SetAsideError::Journal(JournalReadError::Storage(_)))
            ),
            "{refused:?}"
        );
        assert_eq!(
            start(&store, &Journal::unreadable()).await,
            MemoryStart::Refuse {
                message: "agent memory could not be prepared: the mob's event log could not be \
                          read (the event store is not readable); nothing was changed; start \
                          again, or start without agent memory (configure no `agent_memory`)"
                    .to_string(),
            }
        );
        assert_eq!(stored(&store).await?, before);
        assert!(!store.plain_names_were_set_aside(REALM, MOB).await?);

        assert_eq!(start(&store, &earlier_journal()).await, MemoryStart::Attach);
        assert!(recalled(&store, "ledger").await?.is_empty());
        Ok(())
    }

    /// A log whose mob does not answer the read refuses the launch and sets
    /// nothing aside: the store is handed to no reader, no marker is
    /// written, and the next start tries again.
    #[tokio::test]
    async fn a_log_whose_mob_does_not_answer_refuses_the_launch_and_sets_nothing_aside()
    -> TestResult {
        let (_dir, store) = open();
        earlier_store(&store).await?;
        let before = stored(&store).await?;

        assert_eq!(
            start(&store, &Journal::unavailable()).await,
            MemoryStart::Refuse {
                message: "agent memory could not be prepared: the mob's event log could not be \
                          read (the mob no longer answers); nothing was changed; start again, \
                          or start without agent memory (configure no `agent_memory`)"
                    .to_string(),
            }
        );
        assert_eq!(stored(&store).await?, before);
        assert!(!store.plain_names_were_set_aside(REALM, MOB).await?);

        assert_eq!(start(&store, &earlier_journal()).await, MemoryStart::Attach);
        assert!(recalled(&store, "ledger").await?.is_empty());
        // Once the step has run, a log that cannot be read refuses nothing.
        for journal in [Journal::unreadable(), Journal::unavailable()] {
            assert_eq!(start(&store, &journal).await, MemoryStart::Attach);
            assert_eq!(journal.reads(), 0);
        }
        Ok(())
    }

    /// A launch that does not speak for its composition runs no step: it
    /// reads no log and writes nothing, and the next launch that does
    /// speak for it runs the step.
    #[tokio::test]
    async fn a_launch_that_does_not_speak_for_its_composition_sets_nothing_aside() -> TestResult {
        let (_dir, store) = open();
        earlier_store(&store).await?;
        let before = stored(&store).await?;
        let journal = earlier_journal();

        let outcome = set_aside_at_first_start(
            &store,
            REALM,
            &journal,
            &HostedMobs::default(),
            false,
            now_ms() + 1,
        )
        .await?;

        assert_eq!(outcome, None);
        assert_eq!(journal.reads(), 0);
        assert_eq!(stored(&store).await?, before);
        assert!(!store.plain_names_were_set_aside(REALM, MOB).await?);
        assert!(first_start(&store, &journal).await?.is_some());
        assert!(recalled(&store, "ledger").await?.is_empty());
        Ok(())
    }

    /// A note stored at or after the moment the step is given stays under
    /// its name: a member built before the step keeps what it stores then.
    #[tokio::test]
    async fn a_note_stored_after_the_given_moment_stays_under_its_name() -> TestResult {
        let (_dir, store) = open();
        remember(&store, "ledger", "Ledger fact", "LEDGER-EARLIER").await?;
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let moment = now_ms();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        remember(&store, "ledger", "Ledger news", "LEDGER-IN-BETWEEN").await?;

        set_aside_at_first_start(
            &store,
            REALM,
            &Journal::of(vec![private_spawn("ledger")]),
            &HostedMobs::default(),
            true,
            moment,
        )
        .await?;

        assert_eq!(recalled(&store, "ledger").await?, ["LEDGER-IN-BETWEEN"]);
        Ok(())
    }

    /// A gateway's start sets aside every note the store holds under such a
    /// name, whatever its stamp: a note stamped later than the start's own
    /// clock (the host's clock was set back since the note was stored) is
    /// set aside like the others, and no member of the name recalls it.
    #[tokio::test]
    async fn a_gateway_start_sets_aside_a_note_stamped_later_than_its_clock() -> TestResult {
        let (_dir, store) = open();
        remember(&store, "ledger", "Ledger fact", "LEDGER-EARLIER").await?;
        let later = now_ms() + 3_600_000;
        let token = store
            .stage(StagedMutationBatch {
                kind: StagedBatchKind::FreshWrite,
                realm: REALM.to_string(),
                author: MemoryAuthor::Application,
                ops: vec![StagedOp::Create {
                    id: None,
                    scope: scope("ledger"),
                    record: note("Ledger news", "LEDGER-STAMPED-LATER"),
                    trust: crate::memory::records::TrustTier::AgentObserved,
                    derived_from: Vec::new(),
                    rationale: None,
                    created_at_ms: Some(later),
                    updated_at_ms: Some(later),
                }],
            })
            .await?;
        store.commit(token).await?;
        assert_eq!(
            recalled(&store, "ledger").await?,
            ["LEDGER-EARLIER", "LEDGER-STAMPED-LATER"]
        );

        set_aside_at_first_start(
            &store,
            REALM,
            &Journal::of(vec![private_spawn("ledger")]),
            &HostedMobs::default(),
            true,
            bound_at_a_gateway_start(),
        )
        .await?;

        assert!(
            recalled(&store, "ledger").await?.is_empty(),
            "no note stays under the name: {:?}",
            recalled(&store, "ledger").await?
        );
        Ok(())
    }

    /// Each mob that shares a store sets aside the names its own log holds,
    /// once.
    #[tokio::test]
    async fn each_mob_of_a_store_sets_its_own_names_aside_once() -> TestResult {
        let (_dir, store) = open();
        remember(&store, "ledger", "Ledger fact", "LEDGER-KEPT").await?;
        remember(&store, "journal", "Journal fact", "JOURNAL-KEPT").await?;
        let first = Journal::of_mob("mob:first", vec![private_spawn("ledger")]);
        let second = Journal::of_mob("mob:second", vec![private_spawn("journal")]);

        first_start(&store, &first).await?;
        assert_eq!(recalled(&store, "journal").await?, ["JOURNAL-KEPT"]);
        assert_eq!(
            first_start(&store, &second).await?,
            Some(SetAsideOutcome::Done {
                names: 1,
                records: 1
            })
        );
        assert!(recalled(&store, "journal").await?.is_empty());
        for journal in [&first, &second] {
            assert_eq!(
                first_start(&store, journal).await?,
                Some(SetAsideOutcome::AlreadyDone)
            );
        }
        Ok(())
    }

    /// The first start reads the event log of every mob the agent mob tools
    /// host as well as the composition's own, each to its end, and writes
    /// its one marker, the composition's own mob's. A later start reads
    /// none of them.
    #[tokio::test]
    async fn the_first_start_reads_every_hosted_mobs_log_as_well_as_its_own() -> TestResult {
        let (_dir, store) = open();
        let own = Journal::of(vec![shared_spawn("helper")]);
        let team = Journal::of_mob(TEAM, vec![shared_spawn("scout"), shared_spawn("scribe")]);
        let desk = Journal::of_mob(DESK, Vec::new());

        assert_eq!(
            first_start_answer(&store, &own, &hosted(&[(&team, None), (&desk, None)])).await?,
            Some(FirstStart {
                outcome: SetAsideOutcome::Done {
                    names: 0,
                    records: 0
                },
                mobs_read: vec![MOB.to_string(), TEAM.to_string(), DESK.to_string()],
                not_read: Vec::new(),
            }),
            "the answer names the logs read, the composition's own first"
        );

        // One spawn a page, and the read that finds the log's end.
        assert_eq!(
            [own.reads(), team.reads(), desk.reads()],
            [2, 3, 1],
            "each log is read to its end"
        );
        assert!(store.plain_names_were_set_aside(REALM, MOB).await?);
        for mob in [TEAM, DESK] {
            assert!(
                !store.plain_names_were_set_aside(REALM, mob).await?,
                "a hosted mob has no marker of its own: {mob}"
            );
        }

        assert_eq!(
            first_start_answer(&store, &own, &hosted(&[(&team, None), (&desk, None)])).await?,
            Some(FirstStart {
                outcome: SetAsideOutcome::AlreadyDone,
                mobs_read: Vec::new(),
                not_read: Vec::new(),
            })
        );
        assert_eq!(
            [own.reads(), team.reads(), desk.reads()],
            [2, 3, 1],
            "a later start reads no log"
        );
        Ok(())
    }

    /// The notes under a name a member private to a person held in a mob
    /// the agent mob tools host are set aside with the composition's own
    /// step; a shared member's notes are as they were.
    #[tokio::test]
    async fn notes_under_a_name_a_private_member_of_a_hosted_mob_held_are_set_aside() -> TestResult
    {
        let (_dir, store) = open();
        let scribe_id = remember(&store, "scribe", "Scribe fact", "SCRIBE-KEPT").await?;
        remember(&store, "helper", "Helper fact", "HELPER-KEPT").await?;
        let own = Journal::of(vec![shared_spawn("helper")]);
        let team = Journal::of_mob(TEAM, vec![shared_spawn("scout"), private_spawn("scribe")]);

        assert_eq!(
            first_start_with(&store, &own, &hosted(&[(&team, None)])).await?,
            Some(SetAsideOutcome::Done {
                names: 1,
                records: 1
            })
        );

        assert!(
            recalled(&store, "scribe").await?.is_empty(),
            "a member of the name recalls none of them"
        );
        let given = injected(&store, "scribe").await?;
        assert!(
            !given.contains("SCRIBE-") && !given.contains("Scribe"),
            "a member of the name is given none of them at its build: {given}"
        );
        assert!(
            !store
                .forget_authored(&scope("scribe"), &scribe_id, agent("scribe"))
                .await?
                .deleted,
            "a member of the name forgets none of them by id"
        );
        let key = set_aside_scope_key("scribe");
        assert_eq!(
            notes(&store).await?,
            [
                (
                    "helper".to_string(),
                    "active",
                    "Helper fact".to_string(),
                    "HELPER-KEPT".to_string()
                ),
                (
                    key.clone(),
                    "tombstoned",
                    "Scribe fact".to_string(),
                    "SCRIBE-KEPT".to_string()
                ),
            ],
            "the record is kept, forgotten, under the set-aside scope"
        );
        assert!(
            store
                .private_retired_identities(REALM)
                .await?
                .contains(&key),
            "the set-aside scope is recorded as private"
        );
        assert!(store.plain_names_were_set_aside(REALM, MOB).await?);
        assert!(!store.plain_names_were_set_aside(REALM, TEAM).await?);
        Ok(())
    }

    /// A hosted mob whose own event log names no owner for its members (an
    /// earlier release wrote it), created by a member private to a person:
    /// the names its members held are set aside too.
    #[tokio::test]
    async fn a_name_held_in_a_mob_a_private_member_created_is_set_aside_without_an_owner_label()
    -> TestResult {
        let (_dir, store) = open();
        remember(&store, "scribe", "Scribe fact", "SCRIBE-KEPT").await?;
        remember(&store, "helper", "Helper fact", "HELPER-KEPT").await?;
        let own = Journal::of(vec![shared_spawn("helper"), private_spawn("ledger")]);
        let team = Journal::of_mob(TEAM, vec![shared_spawn("scribe")]);

        assert_eq!(
            first_start_with(&store, &own, &hosted(&[(&team, Some((MOB, "ledger")))])).await?,
            Some(SetAsideOutcome::Done {
                names: 1,
                records: 1
            })
        );

        assert!(recalled(&store, "scribe").await?.is_empty());
        assert_eq!(recalled(&store, "helper").await?, ["HELPER-KEPT"]);
        assert_eq!(
            notes(&store).await?,
            [
                (
                    "helper".to_string(),
                    "active",
                    "Helper fact".to_string(),
                    "HELPER-KEPT".to_string()
                ),
                (
                    set_aside_scope_key("scribe"),
                    "tombstoned",
                    "Scribe fact".to_string(),
                    "SCRIBE-KEPT".to_string()
                ),
            ]
        );
        Ok(())
    }

    /// A mob created by a member of a mob that a private member created is
    /// private in the same way, in whatever order the hosted mobs are
    /// handed over: the names its members held are set aside.
    #[tokio::test]
    async fn a_name_held_in_a_mob_created_inside_a_private_members_mob_is_set_aside() -> TestResult
    {
        let (_dir, store) = open();
        remember(&store, "clerk", "Clerk fact", "CLERK-KEPT").await?;
        remember(&store, "scribe", "Scribe fact", "SCRIBE-KEPT").await?;
        remember(&store, "helper", "Helper fact", "HELPER-KEPT").await?;
        let own = Journal::of(vec![shared_spawn("helper"), private_spawn("ledger")]);
        let team = Journal::of_mob(TEAM, vec![shared_spawn("scribe")]);
        let desk = Journal::of_mob(DESK, vec![shared_spawn("clerk")]);
        let mobs = hosted(&[
            (&desk, Some((TEAM, "scribe"))),
            (&team, Some((MOB, "ledger"))),
        ]);

        assert_eq!(
            first_start_with(&store, &own, &mobs).await?,
            Some(SetAsideOutcome::Done {
                names: 2,
                records: 2
            })
        );

        assert!(recalled(&store, "clerk").await?.is_empty());
        assert!(recalled(&store, "scribe").await?.is_empty());
        assert_eq!(recalled(&store, "helper").await?, ["HELPER-KEPT"]);
        Ok(())
    }

    /// A mob the agent mob tools host only after the first start gets no
    /// step: a private member seated in it later costs a shared member of
    /// its name nothing, and its log is not read.
    #[tokio::test]
    async fn a_mob_hosted_after_the_first_start_costs_a_shared_member_nothing() -> TestResult {
        let (_dir, store) = open();
        remember(&store, "scribe", "Scribe fact", "SCRIBE-EARLIER").await?;
        let own = Journal::of(vec![shared_spawn("scribe")]);
        first_start(&store, &own).await?;
        remember(&store, "scribe", "Scribe news", "SCRIBE-LATER").await?;

        let team = Journal::of_mob(TEAM, vec![private_spawn("scribe")]);
        for _ in 0..2 {
            assert_eq!(
                first_start_with(&store, &own, &hosted(&[(&team, Some((MOB, "scribe")))])).await?,
                Some(SetAsideOutcome::AlreadyDone)
            );
        }

        assert_eq!(team.reads(), 0);
        assert!(!store.plain_names_were_set_aside(REALM, TEAM).await?);
        assert_eq!(
            recalled(&store, "scribe").await?,
            ["SCRIBE-EARLIER", "SCRIBE-LATER"]
        );
        Ok(())
    }

    /// A name only shared members held in the hosted mobs is left as it is:
    /// in a mob a shared member created, in a mob whose creation record
    /// names no member, in a mob whose creator no log holds, and in a mob
    /// whose creator sat in a mob that is not hosted.
    #[tokio::test]
    async fn a_name_shared_members_held_in_the_hosted_mobs_is_left_as_it_is() -> TestResult {
        let (_dir, store) = open();
        for (name, body) in [
            ("helper", "HELPER-KEPT"),
            ("scribe", "SCRIBE-KEPT"),
            ("clerk", "CLERK-KEPT"),
            ("runner", "RUNNER-KEPT"),
            ("porter", "PORTER-KEPT"),
        ] {
            remember(&store, name, "A fact", body).await?;
        }
        let before = stored(&store).await?;
        let own = Journal::of(vec![shared_spawn("helper"), private_spawn("ledger")]);
        let team = Journal::of_mob(TEAM, vec![shared_spawn("scribe")]);
        let desk = Journal::of_mob(DESK, vec![shared_spawn("clerk")]);
        let yard = Journal::of_mob("mob:yard", vec![shared_spawn("runner")]);
        let dock = Journal::of_mob("mob:dock", vec![shared_spawn("porter")]);
        let mobs = hosted(&[
            (&team, Some((MOB, "helper"))),
            (&desk, None),
            (&yard, Some((MOB, "nobody"))),
            (&dock, Some(("mob:gone", "ledger"))),
        ]);

        assert_eq!(
            first_start_with(&store, &own, &mobs).await?,
            Some(SetAsideOutcome::Done {
                names: 0,
                records: 0
            })
        );

        assert_eq!(stored(&store).await?, before);
        Ok(())
    }

    /// The marker of a start that read hosted mobs is the marker a start
    /// without them writes: the same token (the composition's own mob's),
    /// the same rows, the same fields in each.
    #[tokio::test]
    async fn a_start_that_read_hosted_mobs_writes_the_marker_a_start_without_them_writes()
    -> TestResult {
        let (_plain_dir, plain) = open();
        let (_hosting_dir, hosting) = open();
        for store in [&plain, &hosting] {
            remember(store, "ledger", "Ledger fact", "LEDGER-KEPT").await?;
            remember(store, "helper", "Helper fact", "HELPER-KEPT").await?;
        }

        first_start(
            &plain,
            &Journal::of(vec![shared_spawn("helper"), private_spawn("ledger")]),
        )
        .await?;
        let team = Journal::of_mob(TEAM, vec![private_spawn("ledger")]);
        first_start_with(
            &hosting,
            &Journal::of(vec![shared_spawn("helper")]),
            &hosted(&[(&team, None)]),
        )
        .await?;

        let rows = marker_rows(&plain)?;
        assert_eq!(
            rows.iter()
                .map(|(token, place, kind, _)| (token.as_str(), *place, kind.as_str()))
                .collect::<Vec<_>>(),
            [
                ("set-aside-first-start/mob:main", 1, "set_aside"),
                ("set-aside-first-start/mob:main", 0, "set_aside_summary"),
            ]
        );
        assert_eq!(marker_rows(&hosting)?, rows);
        // And each row's fields are the ones a start has always written,
        // by name.
        for ((_, _, kind, detail), fields) in rows.iter().zip([
            &[
                "author",
                "name",
                "op",
                "proposals",
                "rationale",
                "records",
                "scope_key",
            ][..],
            &["author", "mob", "names", "op", "records"][..],
        ]) {
            let detail: serde_json::Value = serde_json::from_str(detail)?;
            let mut found: Vec<&str> = detail
                .as_object()
                .ok_or("a marker row's detail is an object")?
                .keys()
                .map(String::as_str)
                .collect();
            found.sort_unstable();
            assert_eq!(found, fields, "the fields of the {kind} row");
        }
        Ok(())
    }

    /// A hosted mob whose event log cannot be read refuses the launch like
    /// the composition's own, and the refusal names the ways out: nothing is
    /// set aside, no marker is written, and the next start whose logs read
    /// sets the notes aside.
    #[tokio::test]
    async fn a_hosted_mob_whose_log_cannot_be_read_refuses_the_launch_and_sets_nothing_aside()
    -> TestResult {
        let (_dir, store) = open();
        earlier_store(&store).await?;
        remember(&store, "scribe", "Scribe fact", "SCRIBE-KEPT").await?;
        let before = stored(&store).await?;
        let own = earlier_journal();

        for (team, reason) in [
            (
                Journal {
                    mob: TEAM,
                    ..Journal::unreadable()
                },
                "the event store is not readable",
            ),
            (
                Journal {
                    mob: TEAM,
                    ..Journal::unavailable()
                },
                "the mob no longer answers",
            ),
        ] {
            assert_eq!(
                start_with(&store, &own, &hosted(&[(&team, None)])).await,
                MemoryStart::Refuse {
                    message: format!(
                        "agent memory could not be prepared: the event log of the hosted mob \
                         '{TEAM}' could not be read ({reason}); nothing was changed; start \
                         again, destroy that mob, or start without agent memory (configure no \
                         `agent_memory`)"
                    ),
                }
            );
            assert_eq!(stored(&store).await?, before);
            assert!(!store.plain_names_were_set_aside(REALM, MOB).await?);
        }

        let team = Journal::of_mob(TEAM, vec![private_spawn("scribe")]);
        assert_eq!(
            start_with(&store, &own, &hosted(&[(&team, None)])).await,
            MemoryStart::Attach
        );
        assert!(recalled(&store, "ledger").await?.is_empty());
        assert!(recalled(&store, "scribe").await?.is_empty());
        Ok(())
    }

    /// A persisted hosted mob that did not restore has no event log to
    /// read: the step skips it and names it in its answer, sets aside what
    /// the logs it can read hold, and writes the marker a start without
    /// such a mob writes. The names only that mob's log holds are left as
    /// they are, also once the mob restores: no later start reads its log.
    #[tokio::test]
    async fn a_hosted_mob_that_did_not_restore_is_skipped_and_named_in_the_answer() -> TestResult {
        let (_dir, store) = open();
        earlier_store(&store).await?;
        remember(&store, "clerk", "Clerk fact", "CLERK-KEPT").await?;
        let own = earlier_journal();
        let team = Journal::of_mob(TEAM, Vec::new());
        let mut mobs = hosted(&[(&team, None)]);
        mobs.not_restored = vec![DESK.to_string()];

        assert_eq!(
            first_start_answer(&store, &own, &mobs).await?,
            Some(FirstStart {
                outcome: SetAsideOutcome::Done {
                    names: 1,
                    records: 4
                },
                mobs_read: vec![MOB.to_string(), TEAM.to_string()],
                not_read: vec![DESK.to_string()],
            })
        );

        assert!(recalled(&store, "ledger").await?.is_empty());
        assert_eq!(
            recalled(&store, "clerk").await?,
            ["CLERK-KEPT"],
            "a name only the skipped mob's log could hold stays as it is"
        );
        let rows = marker_rows(&store)?;
        let summary: serde_json::Value = serde_json::from_str(
            &rows
                .iter()
                .find(|(_, _, kind, _)| kind == "set_aside_summary")
                .ok_or("the summary row")?
                .3,
        )?;
        let mut keys: Vec<&str> = summary
            .as_object()
            .ok_or("the summary is an object")?
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["author", "mob", "names", "op", "records"],
            "the stored summary names no skipped mob"
        );

        // The mob restores later, with a private member of the name in its
        // log: the step has run, and reads nothing.
        let desk = Journal::of_mob(DESK, vec![private_spawn("clerk")]);
        assert_eq!(
            first_start_with(&store, &own, &hosted(&[(&team, None), (&desk, None)])).await?,
            Some(SetAsideOutcome::AlreadyDone)
        );
        assert_eq!(desk.reads(), 0);
        assert_eq!(recalled(&store, "clerk").await?, ["CLERK-KEPT"]);
        Ok(())
    }

    /// The mobs the agent mob tools host cannot be read from them: the
    /// launch is refused with the ways out before any event log is read,
    /// nothing is set aside and no marker is written. The next start that
    /// reads them sets the notes aside.
    #[tokio::test]
    async fn hosted_mobs_that_cannot_be_read_from_the_agent_mob_tools_refuse_the_launch()
    -> TestResult {
        let (_dir, store) = open();
        earlier_store(&store).await?;
        let before = stored(&store).await?;
        let own = earlier_journal();

        assert_eq!(
            memory_start_of(
                &store,
                REALM,
                &own,
                async {
                    Err(JournalReadError::Unavailable(
                        "the agent mob tools do not answer".to_string(),
                    ))
                },
                true,
                now_ms() + 1,
            )
            .await,
            MemoryStart::Refuse {
                message: "agent memory could not be prepared: the mobs the agent mob tools host \
                          could not be read (the agent mob tools do not answer); nothing was \
                          changed; start again, or start without agent memory (configure no \
                          `agent_memory`)"
                    .to_string(),
            }
        );
        assert_eq!(own.reads(), 0, "no event log is read");
        assert_eq!(stored(&store).await?, before);
        assert!(!store.plain_names_were_set_aside(REALM, MOB).await?);

        assert_eq!(
            memory_start_of(
                &store,
                REALM,
                &own,
                async { Ok(HostedMobLogs::default()) },
                true,
                now_ms() + 1,
            )
            .await,
            MemoryStart::Attach
        );
        assert!(recalled(&store, "ledger").await?.is_empty());
        Ok(())
    }

    /// The agent mob tools are asked for the mobs they host by the start
    /// that runs the step, once, and by no other: not by a launch that does
    /// not speak for its composition, and not by a start that finds the
    /// step's marker.
    #[tokio::test]
    async fn only_the_start_that_runs_the_step_asks_the_agent_mob_tools_for_their_mobs()
    -> TestResult {
        let (_dir, store) = open();
        earlier_store(&store).await?;
        let own = earlier_journal();
        let asked = AtomicUsize::new(0);
        let count = &asked;
        let hosted = || async move {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(HostedMobLogs::default())
        };

        assert_eq!(
            memory_start_of(&store, REALM, &own, hosted(), false, now_ms() + 1).await,
            MemoryStart::Attach
        );
        assert_eq!(
            asked.load(Ordering::SeqCst),
            0,
            "a launch that does not speak for its composition asks nothing"
        );
        assert!(!store.plain_names_were_set_aside(REALM, MOB).await?);

        assert_eq!(
            memory_start_of(&store, REALM, &own, hosted(), true, now_ms() + 1).await,
            MemoryStart::Attach
        );
        assert_eq!(
            asked.load(Ordering::SeqCst),
            1,
            "the start that runs the step asks once"
        );
        assert!(recalled(&store, "ledger").await?.is_empty());

        assert_eq!(
            memory_start_of(&store, REALM, &own, hosted(), true, now_ms() + 1).await,
            MemoryStart::Attach
        );
        assert_eq!(
            asked.load(Ordering::SeqCst),
            1,
            "a start that finds the marker asks nothing"
        );
        Ok(())
    }

    /// The spawns that count: a private owner, and either an ephemeral seat
    /// or the side chat label.
    #[test]
    fn a_spawn_counts_when_it_is_private_and_ephemeral_or_a_side_chat() {
        assert!(held_by_a_private_member(&private_spawn("ledger")));
        assert!(!held_by_a_private_member(&shared_spawn("ledger")));
        let durable = LoggedSpawn {
            ephemeral: false,
            ..private_spawn("ledger")
        };
        assert!(!held_by_a_private_member(&durable));
        let mut shared_side_chat = shared_spawn("ledger");
        shared_side_chat.labels.insert(
            crate::side_chat::SIDE_CHAT_OF_LABEL.to_string(),
            "lead".to_string(),
        );
        assert!(!held_by_a_private_member(&shared_side_chat));
        let blank_owner = LoggedSpawn {
            labels: BTreeMap::from([(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                "  ".to_string(),
            )]),
            ..shared_spawn("ledger")
        };
        assert!(!held_by_a_private_member(&blank_owner));
    }
}
